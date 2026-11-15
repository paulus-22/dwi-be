//! Open/allowlist self-registration: picking a ticket type claims one
//! pre-generated row from its pool (or creates a fresh one, if unlimited).
//!
//! Split from `routes::public`'s token-based RSVP flow on purpose — that one
//! assumes a guest already exists (closed, admin-pre-seeded identity,
//! matching the wedding model). This one is for a registrant with no prior
//! identity in the system at all.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{AdminUser, Role};
use crate::error::{ApiError, ApiResult};
use crate::routes::public::published_event;
use crate::routes::public_site::payload_for_stored_code;
use crate::state::AppState;
use crate::whatsapp;

// --------------------------------------------------------- admin: mode

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationModeInput {
    mode: String,
}

pub async fn set_mode(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
    Json(body): Json<RegistrationModeInput>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let mode = body.mode.trim().to_uppercase();
    if mode != "OPEN" && mode != "ALLOWLIST" {
        return Err(ApiError::bad_request("invalid_mode", "Mode registrasi harus OPEN atau ALLOWLIST."));
    }
    sqlx::query(r#"UPDATE "event" SET "registrationMode" = $2::"RegistrationMode" WHERE "id" = $1"#)
        .bind(event_id)
        .bind(&mode)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "registrationMode": mode })))
}

// --------------------------------------------------- admin: allowlist

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct AllowlistEntry {
    id: Uuid,
    phone: Option<String>,
    email: Option<String>,
    used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_allowlist(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, event_id, Role::Viewer).await?;
    let entries: Vec<AllowlistEntry> = sqlx::query_as(
        "SELECT id, phone, email, used_at FROM registration_allowlist_entry
         WHERE event_id = $1 ORDER BY created_at",
    )
    .bind(event_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "entries": entries })))
}

#[derive(Deserialize)]
pub struct AllowlistItem {
    phone: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
pub struct AllowlistAddInput {
    entries: Vec<AllowlistItem>,
}

/// Bulk add — the admin pastes/uploads a phone or email per line.
pub async fn add_allowlist(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
    Json(body): Json<AllowlistAddInput>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, event_id, Role::Organizer).await?;
    if body.entries.len() > 5000 {
        return Err(ApiError::bad_request("too_many_entries", "Maksimal 5.000 entri sekali unggah."));
    }
    let mut added = 0;
    for item in &body.entries {
        let phone = item.phone.as_deref().map(str::trim).filter(|p| !p.is_empty());
        let email = item.email.as_deref().map(|e| e.trim().to_lowercase());
        let email = email.as_deref().filter(|e| !e.is_empty());
        if phone.is_none() && email.is_none() {
            continue;
        }
        sqlx::query("INSERT INTO registration_allowlist_entry (event_id, phone, email) VALUES ($1, $2, $3)")
            .bind(event_id)
            .bind(phone)
            .bind(email)
            .execute(&state.db)
            .await?;
        added += 1;
    }
    Ok(Json(json!({ "added": added })))
}

pub async fn remove_allowlist_entry(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let event_id: Uuid =
        sqlx::query_scalar("SELECT event_id FROM registration_allowlist_entry WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found("Entri tidak ditemukan."))?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    sqlx::query("DELETE FROM registration_allowlist_entry WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------- public: types

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct PublicTicketType {
    id: Uuid,
    name: String,
    description: Option<String>,
}

/// Only ticket types that can still be claimed — unlimited ones always, and
/// quota'd ones with an unclaimed row left. A sold-out type just isn't here;
/// the form never shows it (no error, per spec).
pub async fn ticket_types(State(state): State<AppState>, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    let types: Vec<PublicTicketType> = sqlx::query_as(
        "SELECT t.id, t.name, t.description
         FROM ticket_type t
         WHERE t.event_id = $1
           AND (t.quota IS NULL
                OR EXISTS (SELECT 1 FROM \"guest\" g
                           WHERE g.ticket_type_id = t.id AND g.ticket_claimed_at IS NULL))
         ORDER BY t.sort_order, t.created_at",
    )
    .bind(event_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "ticketTypes": types })))
}

// ------------------------------------------------------- public: register

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInput {
    ticket_type_id: Uuid,
    name: String,
    phone: Option<String>,
    email: Option<String>,
    #[serde(default)]
    responses: Vec<super::public::ResponseInput>,
}

async fn allowlisted(
    state: &AppState,
    event_id: Uuid,
    phone: Option<&str>,
    email: Option<&str>,
) -> ApiResult<Uuid> {
    let email = email.map(str::to_lowercase);
    let row: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM registration_allowlist_entry
         WHERE event_id = $1 AND used_at IS NULL
           AND ((phone IS NOT NULL AND phone = $2) OR (email IS NOT NULL AND email = $3))
         LIMIT 1",
    )
    .bind(event_id)
    .bind(phone)
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;
    row.ok_or_else(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "not_allowlisted",
            "Nomor/email ini belum terdaftar di daftar undangan acara ini, atau sudah pernah dipakai.",
        )
    })
}

pub async fn register(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<RegisterInput>,
) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad_request("invalid_input", "Nama wajib diisi (maks. 120 karakter)."));
    }
    let phone = body.phone.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let email = body.email.as_deref().map(str::trim).filter(|e| !e.is_empty());

    let (mode, prefix): (String, String) =
        sqlx::query_as(r#"SELECT "registrationMode"::text, "codePrefix" FROM "event" WHERE "id" = $1"#)
            .bind(event_id)
            .fetch_one(&state.db)
            .await?;

    let mut tx = state.db.begin().await?;
    let mut allowlist_id = None;
    if mode == "ALLOWLIST" {
        allowlist_id = Some(allowlisted(&state, event_id, phone, email).await?);
    }

    // Claim a pre-generated row for this ticket type...
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE \"guest\" SET \"householdName\" = $3, phone = $4, ticket_claimed_at = now(), \"updatedAt\" = now()
         WHERE id = (
             SELECT id FROM \"guest\"
             WHERE \"eventId\" = $1 AND ticket_type_id = $2 AND ticket_claimed_at IS NULL
             ORDER BY \"sequence\" LIMIT 1 FOR UPDATE SKIP LOCKED
         )
         RETURNING id",
    )
    .bind(event_id)
    .bind(body.ticket_type_id)
    .bind(&name)
    .bind(phone)
    .fetch_optional(&mut *tx)
    .await?;

    let guest_id = match claimed {
        Some(id) => id,
        None => {
            // ...or, for an unlimited ticket type, mint a fresh one. A quota'd
            // type with nothing left to claim is sold out.
            let unlimited: bool =
                sqlx::query_scalar("SELECT quota IS NULL FROM ticket_type WHERE id = $1 AND event_id = $2")
                    .bind(body.ticket_type_id)
                    .bind(event_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| ApiError::not_found("Jenis tiket tidak ditemukan."))?;
            if !unlimited {
                return Err(ApiError::new(StatusCode::CONFLICT, "sold_out", "Jenis tiket ini sudah penuh."));
            }
            let next_seq: i32 = sqlx::query_scalar(
                r#"SELECT COALESCE(MAX("sequence"), 0) + 1 FROM "guest" WHERE "eventId" = $1"#,
            )
            .bind(event_id)
            .fetch_one(&mut *tx)
            .await?;
            let token =
                dwi_domain::build_invite_code(&prefix, dwi_domain::CategoryCode::Gen, next_seq as u32);
            sqlx::query_scalar(
                r#"INSERT INTO "guest"
                       ("id", "eventId", "householdName", "token", "sequence", "category",
                        ticket_type_id, phone, ticket_claimed_at, "codeLockedAt", "updatedAt")
                   VALUES (gen_random_uuid(), $1, $2, $3, $4, 'umum', $5, $6, now(), now(), now())
                   RETURNING id"#,
            )
            .bind(event_id)
            .bind(&name)
            .bind(&token)
            .bind(next_seq)
            .bind(body.ticket_type_id)
            .bind(phone)
            .fetch_one(&mut *tx)
            .await?
        }
    };

    sqlx::query(
        r#"INSERT INTO "rsvp" ("id", "guestId", "attending", "guestCount", "respondentName", "updatedAt")
           VALUES (gen_random_uuid(), $1, true, 1, $2, now())
           ON CONFLICT ("guestId") DO UPDATE SET "attending" = true, "respondentName" = EXCLUDED."respondentName", "updatedAt" = now()"#,
    )
    .bind(guest_id)
    .bind(&name)
    .execute(&mut *tx)
    .await?;

    let fields: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, field_key FROM registration_form_field WHERE event_id = $1")
            .bind(event_id)
            .fetch_all(&mut *tx)
            .await?;
    for r in &body.responses {
        let key = r.field_key.trim();
        let Some((field_id, _)) = fields.iter().find(|(_, k)| k == key) else { continue };
        sqlx::query(
            "INSERT INTO guest_response (guest_id, field_id, value) VALUES ($1, $2, $3)
             ON CONFLICT (guest_id, field_id) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(guest_id)
        .bind(field_id)
        .bind(&r.value)
        .execute(&mut *tx)
        .await?;
    }

    if let Some(allow_id) = allowlist_id {
        sqlx::query("UPDATE registration_allowlist_entry SET used_at = now() WHERE id = $1")
            .bind(allow_id)
            .execute(&mut *tx)
            .await?;
    }

    let token: String = sqlx::query_scalar(r#"SELECT token FROM "guest" WHERE id = $1"#)
        .bind(guest_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    // Best effort: a WhatsApp hiccup must never fail the registration itself.
    whatsapp::spawn_rsvp_confirmation(state.clone(), guest_id);

    let qr_payload = payload_for_stored_code(&state.invite_secret, &token, 1);
    Ok(Json(json!({ "token": token, "qrPayload": qr_payload })))
}
