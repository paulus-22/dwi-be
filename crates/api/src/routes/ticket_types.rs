//! Admin: ticket types with pre-generated, quota'd QR pools.
//!
//! A ticket type with a quota pre-generates that many placeholder `guest`
//! rows up front (unclaimed: `ticket_claimed_at IS NULL`). Registering
//! (`routes::register`) atomically claims one — the QR was already fully
//! formed and signable before anyone registered. A ticket type with no
//! quota (`quota IS NULL`) is unlimited: no pool, a guest row is created
//! fresh on each registration instead.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dwi_domain::{CategoryCode, build_invite_code};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{AdminUser, Role};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TicketType {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub quota: Option<i32>,
    pub sort_order: i32,
    /// Pre-generated rows already handed to a registrant.
    pub claimed: i64,
    /// `None` when `quota` is `None` (unlimited — there is no pool to exhaust).
    pub remaining: Option<i64>,
}

const SELECT: &str = r#"
    SELECT t.id, t.name, t.description, t.quota, t.sort_order,
           COUNT(g.id) FILTER (WHERE g.ticket_claimed_at IS NOT NULL) AS claimed,
           CASE WHEN t.quota IS NULL THEN NULL
                ELSE t.quota - COUNT(g.id) FILTER (WHERE g.ticket_claimed_at IS NOT NULL)
           END AS remaining
    FROM "ticket_type" t
    LEFT JOIN "guest" g ON g.ticket_type_id = t.id
"#;

pub async fn list(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, event_id, Role::Viewer).await?;
    let types: Vec<TicketType> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.event_id = $1 GROUP BY t.id ORDER BY t.sort_order, t.created_at"
    )))
    .bind(event_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "ticketTypes": types })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketTypeInput {
    name: String,
    description: Option<String>,
    /// Omitted or `null` = unlimited.
    #[serde(default)]
    quota: Option<i32>,
    #[serde(default)]
    sort_order: i32,
}

fn validate_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim().to_string();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Nama jenis tiket wajib diisi (maks. 80 karakter).",
        ));
    }
    Ok(name)
}

/// Bulk-inserts `count` unclaimed placeholder guests for a ticket type,
/// each with a real, already-signable invite code — the pre-generated pool.
async fn generate_pool(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: Uuid,
    ticket_type_id: Uuid,
    count: i32,
) -> ApiResult<()> {
    if count <= 0 {
        return Ok(());
    }
    // Scoped by codePrefix across ALL events, not just this one: `token`
    // (prefix-category-sequence-check) is globally unique, but `codePrefix`
    // itself isn't enforced unique between events (every event defaults to
    // "INV" unless the admin sets a custom one) — a per-event-only sequence
    // would collide the moment two events share a prefix.
    let (prefix, next_seq): (String, i32) = sqlx::query_as(
        r#"SELECT e."codePrefix",
                  COALESCE((SELECT MAX(g2."sequence") FROM "guest" g2 JOIN "event" e2 ON e2."id" = g2."eventId"
                            WHERE e2."codePrefix" = e."codePrefix"), 0) + 1
           FROM "event" e WHERE e."id" = $1"#,
    )
    .bind(event_id)
    .fetch_one(&mut **tx)
    .await?;
    for i in 0..count {
        let seq = next_seq + i;
        let token = build_invite_code(&prefix, CategoryCode::Gen, seq as u32);
        sqlx::query(
            r#"INSERT INTO "guest"
                   ("id", "eventId", "householdName", "token", "sequence", "category",
                    ticket_type_id, "codeLockedAt", "updatedAt")
               VALUES (gen_random_uuid(), $1, '(belum diklaim)', $2, $3, 'umum', $4, now(), now())"#,
        )
        .bind(event_id)
        .bind(&token)
        .bind(seq)
        .bind(ticket_type_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn create(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
    Json(body): Json<TicketTypeInput>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let name = validate_name(&body.name)?;
    let description = body.description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    if let Some(q) = body.quota
        && q < 0
    {
        return Err(ApiError::bad_request("invalid_quota", "Kuota tidak boleh negatif."));
    }

    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO "ticket_type" (event_id, name, description, quota, sort_order)
           VALUES ($1, $2, $3, $4, $5) RETURNING id"#,
    )
    .bind(event_id)
    .bind(&name)
    .bind(&description)
    .bind(body.quota)
    .bind(body.sort_order)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(q) = body.quota {
        generate_pool(&mut tx, event_id, id, q).await?;
    }
    tx.commit().await?;

    let ticket_type: TicketType =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1 GROUP BY t.id")))
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(json!({ "ticketType": ticket_type })))
}

async fn type_event(state: &AppState, id: Uuid) -> ApiResult<Uuid> {
    sqlx::query_scalar(r#"SELECT event_id FROM "ticket_type" WHERE id = $1"#)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Jenis tiket tidak ditemukan."))
}

/// Name/description/order only; quota is managed separately via
/// `POST /ticket-types/{id}/expand` because shrinking it safely needs its
/// own rules (never touch a claimed row).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketTypeUpdate {
    name: String,
    description: Option<String>,
    #[serde(default)]
    sort_order: i32,
}

pub async fn update(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<TicketTypeUpdate>,
) -> ApiResult<Json<Value>> {
    let event_id = type_event(&state, id).await?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let name = validate_name(&body.name)?;
    let description = body.description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    sqlx::query(
        r#"UPDATE "ticket_type" SET name = $2, description = $3, sort_order = $4, updated_at = now() WHERE id = $1"#,
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(body.sort_order)
    .execute(&state.db)
    .await?;
    let ticket_type: TicketType =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1 GROUP BY t.id")))
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(json!({ "ticketType": ticket_type })))
}

/// Adds `count` more pre-generated, unclaimed slots to an existing ticket
/// type's pool (increasing its quota by the same amount). A `NULL` quota
/// (unlimited) has no pool to expand — rejected.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandInput {
    count: i32,
}

pub async fn expand(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ExpandInput>,
) -> ApiResult<Json<Value>> {
    let event_id = type_event(&state, id).await?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    if body.count <= 0 || body.count > 10_000 {
        return Err(ApiError::bad_request("invalid_count", "Jumlah tambahan harus antara 1 dan 10.000."));
    }
    let quota: Option<i32> = sqlx::query_scalar(r#"SELECT quota FROM "ticket_type" WHERE id = $1"#)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let Some(quota) = quota else {
        return Err(ApiError::bad_request(
            "unlimited_ticket_type",
            "Jenis tiket ini tidak berkuota (tidak terbatas), tidak ada pool untuk ditambah.",
        ));
    };

    let mut tx = state.db.begin().await?;
    generate_pool(&mut tx, event_id, id, body.count).await?;
    sqlx::query(r#"UPDATE "ticket_type" SET quota = $2, updated_at = now() WHERE id = $1"#)
        .bind(id)
        .bind(quota + body.count)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let ticket_type: TicketType =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1 GROUP BY t.id")))
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(json!({ "ticketType": ticket_type })))
}

pub async fn delete(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let event_id = type_event(&state, id).await?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let claimed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM \"guest\" WHERE ticket_type_id = $1 AND ticket_claimed_at IS NOT NULL",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if claimed > 0 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "ticket_type_has_registrants",
            format!("Jenis tiket ini sudah punya {claimed} pendaftar; tidak bisa dihapus."),
        ));
    }
    // Unclaimed placeholder guests go with it; nothing real is lost.
    sqlx::query("DELETE FROM \"guest\" WHERE ticket_type_id = $1").bind(id).execute(&state.db).await?;
    sqlx::query(r#"DELETE FROM "ticket_type" WHERE id = $1"#).bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}
