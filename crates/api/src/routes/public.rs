//! Public guest side: no admin session. Only PUBLISHED events are visible.
//!
//! TODO(wire up to real guest auth once public guest endpoints migrate here):
//! the guest is identified by their invite `token` in the path, exactly like
//! the old app's `/api/rsvp` (token-only, no session). The old app's per-IP
//! rate limit is not ported yet.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::routes::forms::load_fields;
use crate::state::AppState;

pub(crate) async fn published_event(state: &AppState, slug: &str) -> ApiResult<Uuid> {
    sqlx::query_scalar(r#"SELECT "id" FROM "event" WHERE "slug" = $1 AND "status" = 'PUBLISHED'"#)
        .bind(slug)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Acara tidak ditemukan."))
}

pub async fn form_fields(State(state): State<AppState>, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    Ok(Json(json!({ "fields": load_fields(&state, event_id).await? })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RsvpInput {
    name: String,
    attending: bool,
    guest_count: i32,
    message: Option<String>,
    /// Answers to the event's dynamic form. Unknown keys are ignored.
    #[serde(default)]
    responses: Vec<ResponseInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseInput {
    pub(crate) field_key: String,
    pub(crate) value: Value,
}

/// Same rules as the old app's `/api/rsvp`: the typed name goes on the RSVP
/// (never over `householdName`), and the invitation's allowance caps the count.
pub async fn rsvp(
    State(state): State<AppState>,
    Path((slug, token)): Path<(String, String)>,
    Json(body): Json<RsvpInput>,
) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    let (guest_id, plus_allowance): (Uuid, i32) = sqlx::query_as(
        r#"SELECT "id", "plusGuestAllowance" FROM "guest" WHERE "eventId" = $1 AND "token" = $2"#,
    )
    .bind(event_id)
    .bind(token.trim().to_uppercase())
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "unknown_token", "Undangan tidak ditemukan."))?;

    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad_request("invalid_input", "Nama wajib diisi (maks. 120 karakter)."));
    }
    if !(1..=20).contains(&body.guest_count) {
        return Err(ApiError::bad_request("invalid_input", "Jumlah tamu harus antara 1 dan 20."));
    }
    let message = body.message.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    if message.as_ref().is_some_and(|m| m.chars().count() > 600) {
        return Err(ApiError::bad_request("invalid_input", "Pesan maksimal 600 karakter."));
    }
    let max_guests = plus_allowance.max(0) + 1;
    if body.attending && body.guest_count > max_guests {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "over_allowance",
            format!("Undangan ini berlaku untuk maksimal {max_guests} orang."),
        ));
    }
    let guest_count = if body.attending { body.guest_count } else { 0 };

    let fields: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, field_key FROM registration_form_field WHERE event_id = $1")
            .bind(event_id)
            .fetch_all(&state.db)
            .await?;

    let mut tx = state.db.begin().await?;
    let (attending, saved_count, respondent_name, saved_message, updated_at): (
        bool,
        i32,
        Option<String>,
        Option<String>,
        DateTime<Utc>,
    ) = sqlx::query_as(
        r#"INSERT INTO "rsvp" ("id", "guestId", "attending", "guestCount", "respondentName", "message", "updatedAt")
           VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, now())
           ON CONFLICT ("guestId") DO UPDATE SET
               "attending" = EXCLUDED."attending", "guestCount" = EXCLUDED."guestCount",
               "respondentName" = EXCLUDED."respondentName", "message" = EXCLUDED."message",
               "updatedAt" = now()
           RETURNING "attending", "guestCount", "respondentName", "message",
                     "updatedAt" AT TIME ZONE 'UTC'"#,
    )
    .bind(guest_id)
    .bind(body.attending)
    .bind(guest_count)
    .bind(&name)
    .bind(&message)
    .fetch_one(&mut *tx)
    .await?;

    let mut saved = Vec::new();
    for r in body.responses {
        let key = r.field_key.trim();
        let Some((field_id, field_key)) = fields.iter().find(|(_, k)| k == key) else { continue };
        sqlx::query(
            "INSERT INTO guest_response (guest_id, field_id, value) VALUES ($1, $2, $3)
             ON CONFLICT (guest_id, field_id) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(guest_id)
        .bind(field_id)
        .bind(&r.value)
        .execute(&mut *tx)
        .await?;
        saved.retain(|s: &Value| s["fieldKey"] != field_key.as_str());
        saved.push(json!({ "fieldKey": field_key, "value": r.value }));
    }
    tx.commit().await?;

    Ok(Json(json!({
        "rsvp": {
            "attending": attending,
            "guestCount": saved_count,
            "respondentName": respondent_name,
            "message": saved_message,
            "updatedAt": updated_at,
        },
        "responses": saved,
    })))
}
