//! Two kinds of caller, two kinds of token.
//!
//! * **Admins** sign in with email + password (the same bcrypt hashes the old
//!   app wrote) and get a short-lived JWT. The account is re-read on every
//!   request so disabling someone takes effect immediately.
//! * **Door devices** pair with an event code and get an opaque random token.
//!   Only its SHA-256 is stored; revoking the device row kills the token.
//!   A device token is scoped to exactly one event.
//!
//! Both are sent as `Authorization: Bearer …`. EventSource cannot set headers,
//! so the live stream also accepts `?access_token=…`.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const DEVICE_TOKEN_PREFIX: &str = "dwd_";

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: Uuid,
    iat: i64,
    exp: i64,
}

pub fn issue_admin_token(state: &AppState, admin_id: Uuid) -> ApiResult<(String, DateTime<Utc>)> {
    let now = Utc::now();
    let exp = now + chrono::Duration::hours(state.session_hours);
    let claims = Claims { sub: admin_id, iat: now.timestamp(), exp: exp.timestamp() };
    let token = jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &state.jwt_encoding)
        .map_err(|e| ApiError::Internal(e.into()))?;
    Ok((token, exp))
}

fn decode_admin_token(state: &AppState, token: &str) -> Option<Uuid> {
    let mut v = Validation::new(Algorithm::HS256);
    v.leeway = 30;
    jsonwebtoken::decode::<Claims>(token, &state.jwt_decoding, &v).ok().map(|d| d.claims.sub)
}

pub fn new_device_token() -> (String, Vec<u8>) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("{DEVICE_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let hash = hash_token(&token);
    (token, hash)
}

pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Bearer header first, then `?access_token=` (live stream only needs it).
fn bearer(parts: &Parts) -> Option<String> {
    if let Some(h) = parts.headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        && let Some(t) = h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))
    {
        return Some(t.trim().to_string());
    }
    parts.uri.query()?.split('&').find_map(|kv| {
        kv.strip_prefix("access_token=").map(|v| {
            url::form_urlencoded::parse(format!("v={v}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        })
    })
}

// ------------------------------------------------------------------ admin

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "\"AdminRole\"", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum Role {
    // Declaration order is rank order: Viewer < Organizer < Owner.
    Viewer,
    Organizer,
    Owner,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Admin {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub is_super_admin: bool,
    pub must_change_password: bool,
}

/// A signed-in admin whose account is active. Rejects accounts that still
/// have to replace a temporary password — use [`SignedIn`] to allow those.
pub struct AdminUser(pub Admin);

/// Any signed-in, active admin, including one with a pending password change.
pub struct SignedIn(pub Admin);

async fn load_admin(parts: &Parts, state: &AppState) -> ApiResult<Admin> {
    let token = bearer(parts).ok_or_else(|| ApiError::unauthorized("Silakan masuk terlebih dahulu."))?;
    if token.starts_with(DEVICE_TOKEN_PREFIX) {
        return Err(ApiError::forbidden("Perangkat scanner tidak bisa membuka halaman admin."));
    }
    let id = decode_admin_token(state, &token)
        .ok_or_else(|| ApiError::unauthorized("Sesi berakhir. Silakan masuk lagi."))?;
    let row = sqlx::query_as::<_, (Uuid, String, String, bool, bool, bool)>(
        r#"SELECT "id", "email", "name", "isSuperAdmin", "isActive", "mustChangePassword"
           FROM "admin_user" WHERE "id" = $1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    match row {
        Some((id, email, name, is_super_admin, true, must_change_password)) => {
            Ok(Admin { id, email, name, is_super_admin, must_change_password })
        }
        _ => Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "account_disabled",
            "Akun ini dinonaktifkan.",
        )),
    }
}

impl FromRequestParts<AppState> for SignedIn {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        load_admin(parts, state).await.map(SignedIn)
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let admin = load_admin(parts, state).await?;
        if admin.must_change_password {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "must_change_password",
                "Ganti kata sandi sementara terlebih dahulu.",
            ));
        }
        Ok(AdminUser(admin))
    }
}

impl Admin {
    /// The admin's role on an event, `Owner` for the super admin, or 403/404.
    /// Unknown and inaccessible events look the same, so ids cannot be probed.
    pub async fn require_event(&self, state: &AppState, event_id: Uuid, min: Role) -> ApiResult<Role> {
        let role = if self.is_super_admin {
            let exists: bool = sqlx::query_scalar(r#"SELECT EXISTS (SELECT 1 FROM "event" WHERE "id" = $1)"#)
                .bind(event_id)
                .fetch_one(&state.db)
                .await?;
            exists.then_some(Role::Owner)
        } else {
            sqlx::query_scalar::<_, Role>(
                r#"SELECT "role" FROM "event_membership" WHERE "adminId" = $1 AND "eventId" = $2"#,
            )
            .bind(self.id)
            .bind(event_id)
            .fetch_optional(&state.db)
            .await?
        };
        match role {
            None => Err(ApiError::not_found("Acara tidak ditemukan.")),
            Some(r) if r < min => Err(ApiError::forbidden("Peran Anda tidak mengizinkan tindakan ini.")),
            Some(r) => Ok(r),
        }
    }
}

// ----------------------------------------------------------------- device

#[derive(Debug, Clone)]
pub struct Device {
    pub id: Uuid,
    pub event_id: Uuid,
    pub name: String,
}

impl FromRequestParts<AppState> for Device {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let token = bearer(parts)
            .filter(|t| t.starts_with(DEVICE_TOKEN_PREFIX))
            .ok_or_else(|| ApiError::unauthorized("Perangkat belum dipasangkan."))?;
        let row = sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "UPDATE scanner_device SET last_seen_at = now()
             WHERE token_hash = $1 AND revoked_at IS NULL
             RETURNING id, event_id, name",
        )
        .bind(hash_token(&token))
        .fetch_optional(&state.db)
        .await?;
        let (id, event_id, name) = row.ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "device_revoked",
                "Perangkat ini sudah dilepas dari acara. Pasangkan ulang.",
            )
        })?;
        Ok(Device { id, event_id, name })
    }
}

impl Device {
    pub fn require_event(&self, event_id: Uuid) -> ApiResult<()> {
        if self.event_id == event_id { Ok(()) } else { Err(ApiError::not_found("Acara tidak ditemukan.")) }
    }
}

/// Either kind of caller, for read-only views both may open (the live stream).
pub enum Viewer {
    Admin(Admin),
    Device(Device),
}

impl FromRequestParts<AppState> for Viewer {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        match bearer(parts) {
            Some(t) if t.starts_with(DEVICE_TOKEN_PREFIX) => {
                Device::from_request_parts(parts, state).await.map(Viewer::Device)
            }
            _ => AdminUser::from_request_parts(parts, state).await.map(|a| Viewer::Admin(a.0)),
        }
    }
}

impl Viewer {
    pub async fn require_event(&self, state: &AppState, event_id: Uuid) -> ApiResult<()> {
        match self {
            Viewer::Admin(a) => a.require_event(state, event_id, Role::Viewer).await.map(|_| ()),
            Viewer::Device(d) => d.require_event(event_id),
        }
    }
}
