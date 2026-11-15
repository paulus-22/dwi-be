//! Selfies: presign → device PUTs to R2 → device reports completion.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use dwi_domain::normalize_invite_code;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{Device, Viewer};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::{UPLOAD_TTL, photo_key};

const ALLOWED_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];

fn storage(state: &AppState) -> ApiResult<&crate::storage::Storage> {
    state.storage.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "storage_not_configured",
            "Penyimpanan foto belum diatur di server. Foto tetap aman di perangkat.",
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresignBody {
    photo_id: Uuid,
    content_type: String,
}

pub async fn presign(
    State(state): State<AppState>,
    device: Device,
    Json(body): Json<PresignBody>,
) -> ApiResult<Json<Value>> {
    if !ALLOWED_TYPES.contains(&body.content_type.as_str()) {
        return Err(ApiError::bad_request("unsupported_type", "Format foto tidak didukung."));
    }
    let storage = storage(&state)?;
    let key = photo_key(device.event_id, body.photo_id, &body.content_type);
    let url = storage.presign_put(&key, &body.content_type);
    Ok(Json(json!({
        "method": "PUT",
        "url": url.as_str(),
        "key": key,
        "headers": { "content-type": body.content_type },
        "expiresAt": Utc::now() + UPLOAD_TTL,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteBody {
    id: Uuid,
    check_in_id: Option<Uuid>,
    code: String,
    key: String,
    content_type: String,
    occurred_at: DateTime<Utc>,
}

pub async fn complete(
    State(state): State<AppState>,
    device: Device,
    Json(body): Json<CompleteBody>,
) -> ApiResult<Json<Value>> {
    let storage = storage(&state)?;
    // The key must be the one this device was allowed to write.
    if body.key != photo_key(device.event_id, body.id, &body.content_type) {
        return Err(ApiError::bad_request("invalid_key", "Kunci foto tidak cocok."));
    }
    let code = normalize_invite_code(&body.code);
    let guest: Option<(Uuid, String)> =
        sqlx::query_as(r#"SELECT "id", "householdName" FROM "guest" WHERE "eventId" = $1 AND "token" = $2"#)
            .bind(device.event_id)
            .bind(&code)
            .fetch_optional(&state.db)
            .await?;
    let occurred = body.occurred_at.min(Utc::now());
    let seq: Option<i64> = sqlx::query_scalar(
        "INSERT INTO scanner_photo (id, event_id, device_id, check_in_id, guest_code, guest_id, object_key, content_type, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (id) DO NOTHING RETURNING seq",
    )
    .bind(body.id)
    .bind(device.event_id)
    .bind(device.id)
    .bind(body.check_in_id)
    .bind(&code)
    .bind(guest.as_ref().map(|g| g.0))
    .bind(&body.key)
    .bind(&body.content_type)
    .bind(occurred)
    .fetch_optional(&state.db)
    .await?;

    let url = storage.read_url(&body.key);
    if let Some(seq) = seq {
        let payload = json!({
            "type": "photo", "id": body.id, "seq": seq, "code": code,
            "householdName": guest.map(|g| g.1), "url": url, "occurredAt": occurred,
        });
        state.live.publish(device.event_id, payload.to_string()).await;
    }
    Ok(Json(json!({ "id": body.id, "url": url })))
}

#[derive(Deserialize)]
pub struct PhotoQuery {
    #[serde(default)]
    after: i64,
    limit: Option<i64>,
}

#[derive(sqlx::FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoRow {
    id: Uuid,
    seq: i64,
    code: String,
    household_name: Option<String>,
    #[serde(skip)]
    object_key: String,
    occurred_at: DateTime<Utc>,
    device_name: String,
}

/// Newest-last page of photos after `after` (a `seq`). Admins and the
/// event's own devices (the photo wall) may read it.
pub async fn list(
    State(state): State<AppState>,
    viewer: Viewer,
    Path(event_id): Path<Uuid>,
    Query(q): Query<PhotoQuery>,
) -> ApiResult<Json<Value>> {
    viewer.require_event(&state, event_id).await?;
    let storage = storage(&state)?;
    let limit = q.limit.unwrap_or(60).clamp(1, 200);
    let rows = sqlx::query_as::<_, PhotoRow>(
        r#"SELECT p.id, p.seq, p.guest_code AS code, g."householdName" AS household_name, p.object_key,
                  p.occurred_at, d.name AS device_name
           FROM scanner_photo p
           JOIN scanner_device d ON d.id = p.device_id
           LEFT JOIN "guest" g ON g."id" = p.guest_id
           WHERE p.event_id = $1 AND p.seq > $2 AND p.hidden_at IS NULL
           ORDER BY p.seq LIMIT $3"#,
    )
    .bind(event_id)
    .bind(q.after)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    let cursor = rows.last().map_or(q.after, |r| r.seq);
    let photos: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or_default();
            v["url"] = Value::String(storage.read_url(&r.object_key));
            v
        })
        .collect();
    Ok(Json(json!({ "photos": photos, "cursor": cursor })))
}
