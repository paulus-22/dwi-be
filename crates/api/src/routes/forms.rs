//! Admin: the dynamic registration form of an event (one row per question).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{AdminUser, Role};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Mirrors the CHECK constraint on `registration_form_field.field_type`.
pub const FIELD_TYPES: &[&str] = &["TEXT", "TEXTAREA", "NUMBER", "SELECT", "RADIO", "CHECKBOX", "DATE"];

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    pub id: Uuid,
    pub field_key: String,
    pub label_id: String,
    pub label_en: Option<String>,
    pub field_type: String,
    pub options: Value,
    pub required: bool,
    pub sort_order: i32,
}

const FIELD_COLUMNS: &str = "id, field_key, label_id, label_en, field_type, options, required, sort_order";

/// The form of one event, in display order. Shared with the public route.
pub async fn load_fields(state: &AppState, event_id: Uuid) -> ApiResult<Vec<FormField>> {
    Ok(sqlx::query_as::<_, FormField>(sqlx::AssertSqlSafe(format!(
        "SELECT {FIELD_COLUMNS} FROM registration_form_field WHERE event_id = $1 ORDER BY sort_order, created_at"
    )))
    .bind(event_id)
    .fetch_all(&state.db)
    .await?)
}

/// Body of both create and update: PUT replaces the whole field.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldInput {
    field_key: String,
    label_id: String,
    label_en: Option<String>,
    field_type: String,
    options: Option<Value>,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    sort_order: i32,
}

struct ValidField {
    field_key: String,
    label_id: String,
    label_en: Option<String>,
    field_type: String,
    options: Value,
    required: bool,
    sort_order: i32,
}

fn validate(body: FieldInput) -> ApiResult<ValidField> {
    let field_key = body.field_key.trim().to_string();
    if field_key.is_empty() || field_key.len() > 64 {
        return Err(ApiError::bad_request(
            "invalid_field_key",
            "Kunci kolom wajib diisi (maks. 64 karakter).",
        ));
    }
    let label_id = body.label_id.trim().to_string();
    if label_id.is_empty() {
        return Err(ApiError::bad_request("invalid_label", "Label kolom wajib diisi."));
    }
    let field_type = body.field_type.trim().to_uppercase();
    if !FIELD_TYPES.contains(&field_type.as_str()) {
        return Err(ApiError::bad_request(
            "invalid_field_type",
            format!("Jenis kolom tidak dikenal. Pilihan: {}.", FIELD_TYPES.join(", ")),
        ));
    }
    let options = body.options.unwrap_or_else(|| json!([]));
    if !options.is_array() {
        return Err(ApiError::bad_request("invalid_options", "Pilihan kolom harus berupa daftar."));
    }
    Ok(ValidField {
        field_key,
        label_id,
        label_en: body.label_en.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()),
        field_type,
        options,
        required: body.required,
        sort_order: body.sort_order,
    })
}

fn duplicate_key(e: sqlx::Error) -> ApiError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::new(
            StatusCode::CONFLICT,
            "duplicate_field_key",
            "Kunci kolom sudah dipakai di acara ini.",
        ),
        _ => e.into(),
    }
}

async fn field_event(state: &AppState, field_id: Uuid) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT event_id FROM registration_form_field WHERE id = $1")
        .bind(field_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Kolom formulir tidak ditemukan."))
}

pub async fn list(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Viewer).await?;
    Ok(Json(json!({ "fields": load_fields(&state, id).await? })))
}

pub async fn create(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<FieldInput>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Organizer).await?;
    let f = validate(body)?;
    let field = sqlx::query_as::<_, FormField>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO registration_form_field
             (event_id, field_key, label_id, label_en, field_type, options, required, sort_order)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {FIELD_COLUMNS}"
    )))
    .bind(id)
    .bind(&f.field_key)
    .bind(&f.label_id)
    .bind(&f.label_en)
    .bind(&f.field_type)
    .bind(&f.options)
    .bind(f.required)
    .bind(f.sort_order)
    .fetch_one(&state.db)
    .await
    .map_err(duplicate_key)?;
    Ok(Json(json!({ "field": field })))
}

pub async fn update(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(field_id): Path<Uuid>,
    Json(body): Json<FieldInput>,
) -> ApiResult<Json<Value>> {
    let event_id = field_event(&state, field_id).await?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let f = validate(body)?;
    let field = sqlx::query_as::<_, FormField>(sqlx::AssertSqlSafe(format!(
        "UPDATE registration_form_field
         SET field_key = $2, label_id = $3, label_en = $4, field_type = $5, options = $6,
             required = $7, sort_order = $8, updated_at = now()
         WHERE id = $1
         RETURNING {FIELD_COLUMNS}"
    )))
    .bind(field_id)
    .bind(&f.field_key)
    .bind(&f.label_id)
    .bind(&f.label_en)
    .bind(&f.field_type)
    .bind(&f.options)
    .bind(f.required)
    .bind(f.sort_order)
    .fetch_optional(&state.db)
    .await
    .map_err(duplicate_key)?
    .ok_or_else(|| ApiError::not_found("Kolom formulir tidak ditemukan."))?;
    Ok(Json(json!({ "field": field })))
}

pub async fn delete(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(field_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let event_id = field_event(&state, field_id).await?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    // guest_response rows go with it (ON DELETE CASCADE).
    sqlx::query("DELETE FROM registration_form_field WHERE id = $1")
        .bind(field_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
