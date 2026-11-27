//! Doorprize (undian): eligible pool from scanned/checked-in guests + persisted winners.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{AdminUser, Role, Viewer};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(sqlx::FromRow)]
struct ParticipantRow {
    check_in_id: Uuid,
    code: String,
    winner_name: Option<String>,
    household_name: Option<String>,
    category: Option<String>,
    device_name: String,
    occurred_at: DateTime<Utc>,
    photo_key: Option<String>,
}

#[derive(sqlx::FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct WinnerRow {
    id: Uuid,
    check_in_id: Option<Uuid>,
    code: String,
    name: String,
    household_name: Option<String>,
    prize: Option<String>,
    drawn_at: DateTime<Utc>,
}

/// `GET /v1/events/{id}/doorprize` — lists every distinct checked-in guest
/// (eligible for the wheel) plus previously saved winners.
///
/// Deduplication rules:
/// - Normal / DIRECT guests: one entry per `guest_code` (re-scans don't double odds).
/// - GROUP guests with usher-typed `memberName`: one entry per distinct
///   `(guest_code, lower(trim(memberName)))`, so each arriving group member has a fair slot.
pub async fn state(
    State(state): State<AppState>,
    viewer: Viewer,
    Path(event_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    viewer.require_event(&state, event_id).await?;

    let rows = sqlx::query_as::<_, ParticipantRow>(
        r#"
        SELECT DISTINCT ON (s.guest_code, COALESCE(NULLIF(lower(trim(ci."memberName")), ''), ''))
               s.id AS check_in_id,
               s.guest_code AS code,
               COALESCE(NULLIF(trim(ci."memberName"), ''), g."householdName") AS winner_name,
               g."householdName" AS household_name,
               g."category" AS category,
               d.name AS device_name,
               s.occurred_at,
               p.object_key AS photo_key
        FROM scanner_event s
        JOIN scanner_device d ON d.id = s.device_id
        LEFT JOIN "guest" g ON g."id" = s.guest_id
        LEFT JOIN "check_in" ci ON ci."id" = s.id
        LEFT JOIN LATERAL (
            SELECT ph.object_key
            FROM scanner_photo ph
            WHERE ph.event_id = s.event_id
              AND ph.hidden_at IS NULL
              AND (ph.check_in_id = s.id OR ph.guest_code = s.guest_code)
            ORDER BY (ph.check_in_id = s.id) DESC, ph.occurred_at DESC
            LIMIT 1
        ) p ON true
        WHERE s.event_id = $1 AND s.kind = 'CHECK_IN'
        ORDER BY s.guest_code,
                 COALESCE(NULLIF(lower(trim(ci."memberName")), ''), ''),
                 s.occurred_at,
                 s.seq
        "#,
    )
    .bind(event_id)
    .fetch_all(&state.db)
    .await?;

    let winners = sqlx::query_as::<_, WinnerRow>(
        r#"
        SELECT id, check_in_id, guest_code AS code, winner_name AS name,
               household_name, prize, drawn_at
        FROM doorprize_winner
        WHERE event_id = $1
        ORDER BY drawn_at DESC
        "#,
    )
    .bind(event_id)
    .fetch_all(&state.db)
    .await?;

    let selfie_url = |key: Option<String>| -> Option<String> {
        match (state.storage.as_ref(), key) {
            (Some(st), Some(k)) if !k.is_empty() => Some(st.read_url(&k)),
            _ => None,
        }
    };

    let mut participants: Vec<Value> = rows
        .into_iter()
        .filter_map(|r| {
            let name = r.winner_name.as_deref().unwrap_or(&r.code).trim().to_string();
            if name.is_empty() {
                return None;
            }
            let norm_member = if r.household_name.as_deref().map(str::trim) == Some(name.as_str()) {
                ""
            } else {
                name.as_str()
            };
            let entry_key = format!("{}::{}", r.code, norm_member.to_lowercase());
            Some(json!({
                "entryKey": entry_key,
                "checkInId": r.check_in_id,
                "code": r.code,
                "name": name,
                "householdName": r.household_name,
                "category": r.category,
                "deviceName": r.device_name,
                "occurredAt": r.occurred_at,
                "selfie": selfie_url(r.photo_key),
            }))
        })
        .collect();

    participants.sort_by(|a, b| {
        let an = a["name"].as_str().unwrap_or("").to_lowercase();
        let bn = b["name"].as_str().unwrap_or("").to_lowercase();
        an.cmp(&bn)
    });

    Ok(Json(json!({
        "participants": participants,
        "winners": winners,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordWinnerBody {
    check_in_id: Option<Uuid>,
    code: String,
    name: String,
    household_name: Option<String>,
    prize: Option<String>,
}

/// `POST /v1/events/{id}/doorprize/winners` — persists a confirmed doorprize winner.
pub async fn record_winner(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
    Json(body): Json<RecordWinnerBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    admin.require_event(&state, event_id, Role::Organizer).await?;

    let code = body.code.trim().to_string();
    let name = body.name.trim().to_string();
    if code.is_empty() || name.is_empty() || name.chars().count() > 160 {
        return Err(ApiError::bad_request("invalid_winner", "Data pemenang tidak valid."));
    }
    let household_name = body
        .household_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(160).collect::<String>());
    let prize = body
        .prize
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(120).collect::<String>());

    let winner = sqlx::query_as::<_, WinnerRow>(
        r#"
        INSERT INTO doorprize_winner (event_id, check_in_id, guest_code, winner_name, household_name, prize, drawn_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, check_in_id, guest_code AS code, winner_name AS name,
                  household_name, prize, drawn_at
        "#,
    )
    .bind(event_id)
    .bind(body.check_in_id)
    .bind(&code)
    .bind(&name)
    .bind(&household_name)
    .bind(&prize)
    .bind(admin.id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(json!({ "winner": winner }))))
}

/// `DELETE /v1/events/{id}/doorprize/winners/{winnerId}` — removes one saved winner.
pub async fn remove_winner(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path((event_id, winner_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let res = sqlx::query("DELETE FROM doorprize_winner WHERE id = $1 AND event_id = $2")
        .bind(winner_id)
        .bind(event_id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("Data pemenang tidak ditemukan."));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v1/events/{id}/doorprize/winners` — clears all winners for an event.
pub async fn reset_winners(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    admin.require_event(&state, event_id, Role::Organizer).await?;
    sqlx::query("DELETE FROM doorprize_winner WHERE event_id = $1").bind(event_id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
