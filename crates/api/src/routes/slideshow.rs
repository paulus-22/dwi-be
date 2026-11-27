//! Scanner slideshow: images the organiser uploads from the dashboard, shown
//! cycling on every scanner device.
//!
//! Flow (same presigned pattern as selfies, so image bytes never pass through
//! the API):
//!   dashboard → POST /events/{id}/slideshow/presign → 15-min presigned PUT URL
//!   dashboard → PUT straight to R2 (WebP only) 
//!   dashboard → POST /events/{id}/slideshow        → register the object
//!   scanner   → GET  /events/{id}/slideshow        → list of image URLs

use axum::Json;
use axum::extract::{Path, State};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{AdminUser, Role, Viewer};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::{UPLOAD_TTL, Storage};

/// Slideshow uploads are always WebP (the dashboard converts before upload) so
/// both storage and scanner decoding stay predictable.
const SLIDESHOW_CONTENT_TYPE: &str = "image/webp";

/// `events/<event>/slideshow/<id>.webp` — separate prefix from selfies
/// (`photos/`), so a wedding's slides can be replaced/exported independently.
pub fn slideshow_key(event_id: Uuid, id: Uuid) -> String {
    format!("events/{event_id}/slideshow/{id}.webp")
}

fn storage(state: &AppState) -> ApiResult<&Storage> {
    state.storage.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "storage_not_configured",
            "Penyimpanan foto belum diatur di server (R2).",
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlideshowPresignBody {
    photo_id: Uuid,
    content_type: String,
}

/// Admin: hand a dashboard a short-lived URL to PUT a slide into R2.
pub async fn presign(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SlideshowPresignBody>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Organizer).await?;
    if body.content_type != SLIDESHOW_CONTENT_TYPE {
        return Err(ApiError::bad_request(
            "unsupported_type",
            "Slide harus dalam format WebP (dikonversi otomatis di dashboard).",
        ));
    }
    let storage = storage(&state)?;
    let key = slideshow_key(id, body.photo_id);
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
pub struct SlideshowCompleteBody {
    id: Uuid,
    key: String,
    content_type: String,
}

/// Admin: register a slide whose bytes are now in R2.
pub async fn complete(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SlideshowCompleteBody>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Organizer).await?;
    // The key must be one this organiser was allowed to write for this event.
    if body.key != slideshow_key(id, body.id) || body.content_type != SLIDESHOW_CONTENT_TYPE {
        return Err(ApiError::bad_request("invalid_key", "Kunci slide tidak cocok."));
    }
    sqlx::query(
        "INSERT INTO scanner_slideshow (id, event_id, object_key, content_type, created_by)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(body.id)
    .bind(id)
    .bind(&body.key)
    .bind(&body.content_type)
    .bind(admin.id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "id": body.id })))
}

/// Admin: remove a slide from the slideshow (soft; the R2 object stays so a
/// re-add is cheap and nothing is ever half-visible).
pub async fn remove(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(slide_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let event_id: Uuid = sqlx::query_scalar("SELECT event_id FROM scanner_slideshow WHERE id = $1")
        .bind(slide_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Slide tidak ditemukan."))?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    sqlx::query("UPDATE scanner_slideshow SET hidden_at = now() WHERE id = $1 AND hidden_at IS NULL")
        .bind(slide_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "id": slide_id, "hidden": true })))
}

/// Both the dashboard (manage) and any door device (show) read the slideshow.
pub async fn list(
    State(state): State<AppState>,
    viewer: Viewer,
    Path(event_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    viewer.require_event(&state, event_id).await?;
    let storage = storage(&state)?;
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, object_key, content_type FROM scanner_slideshow
         WHERE event_id = $1 AND hidden_at IS NULL ORDER BY seq",
    )
    .bind(event_id)
    .fetch_all(&state.db)
    .await?;
    let images: Vec<Value> = rows
        .into_iter()
        .map(|(id, key, content_type)| {
            json!({ "id": id, "url": storage.read_url(&key), "contentType": content_type })
        })
        .collect();
    Ok(Json(json!({ "images": images })))
}
