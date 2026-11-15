//! Admin: the section builder behind a GENERIC event's public page.

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{AdminUser, Role};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Mirrors the CHECK constraint on `event_page_block.block_type`.
pub const BLOCK_TYPES: &[&str] = &[
    "COVER",
    "COUNTDOWN",
    "SCHEDULE",
    "MAP",
    "GALLERY",
    "RSVP_FORM",
    "REGISTRATION_FORM",
    "CUSTOM_HTML",
    "SPONSOR",
    "CONTACT",
];

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PageBlock {
    pub id: Uuid,
    pub block_type: String,
    pub config: Value,
    pub sort_order: i32,
    pub is_visible: bool,
}

const BLOCK_COLUMNS: &str = "id, block_type, config, sort_order, is_visible";

/// Body of both create and update: PUT replaces the whole block.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockInput {
    block_type: String,
    config: Option<Value>,
    #[serde(default)]
    sort_order: i32,
    is_visible: Option<bool>,
}

fn validate(body: BlockInput) -> ApiResult<(String, Value, i32, bool)> {
    let block_type = body.block_type.trim().to_uppercase();
    if !BLOCK_TYPES.contains(&block_type.as_str()) {
        return Err(ApiError::bad_request(
            "invalid_block_type",
            format!("Jenis blok tidak dikenal. Pilihan: {}.", BLOCK_TYPES.join(", ")),
        ));
    }
    let config = body.config.unwrap_or_else(|| json!({}));
    if !config.is_object() {
        return Err(ApiError::bad_request("invalid_config", "Konfigurasi blok harus berupa objek."));
    }
    Ok((block_type, config, body.sort_order, body.is_visible.unwrap_or(true)))
}

async fn block_event(state: &AppState, block_id: Uuid) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT event_id FROM event_page_block WHERE id = $1")
        .bind(block_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Blok halaman tidak ditemukan."))
}

pub async fn list(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Viewer).await?;
    let blocks = sqlx::query_as::<_, PageBlock>(sqlx::AssertSqlSafe(format!(
        "SELECT {BLOCK_COLUMNS} FROM event_page_block WHERE event_id = $1 ORDER BY sort_order, created_at"
    )))
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "blocks": blocks })))
}

pub async fn create(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<BlockInput>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Organizer).await?;
    let (block_type, config, sort_order, is_visible) = validate(body)?;
    let block = sqlx::query_as::<_, PageBlock>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO event_page_block (event_id, block_type, config, sort_order, is_visible)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING {BLOCK_COLUMNS}"
    )))
    .bind(id)
    .bind(&block_type)
    .bind(&config)
    .bind(sort_order)
    .bind(is_visible)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({ "block": block })))
}

pub async fn update(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(block_id): Path<Uuid>,
    Json(body): Json<BlockInput>,
) -> ApiResult<Json<Value>> {
    let event_id = block_event(&state, block_id).await?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let (block_type, config, sort_order, is_visible) = validate(body)?;
    let block = sqlx::query_as::<_, PageBlock>(sqlx::AssertSqlSafe(format!(
        "UPDATE event_page_block
         SET block_type = $2, config = $3, sort_order = $4, is_visible = $5, updated_at = now()
         WHERE id = $1
         RETURNING {BLOCK_COLUMNS}"
    )))
    .bind(block_id)
    .bind(&block_type)
    .bind(&config)
    .bind(sort_order)
    .bind(is_visible)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("Blok halaman tidak ditemukan."))?;
    Ok(Json(json!({ "block": block })))
}

pub async fn delete(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(block_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let event_id = block_event(&state, block_id).await?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    sqlx::query("DELETE FROM event_page_block WHERE id = $1").bind(block_id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}
