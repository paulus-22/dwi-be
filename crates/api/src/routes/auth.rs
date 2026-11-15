use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{SignedIn, issue_admin_token};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Same cost the old app uses, so either app can verify the other's hashes.
const BCRYPT_COST: u32 = 12;
const MIN_PASSWORD: usize = 10;
const LOGIN_LIMIT: i64 = 10;
const LOGIN_WINDOW_SECS: i64 = 15 * 60;

/// Verified against when the email is unknown, so both paths take as long.
static DUMMY_HASH: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| bcrypt::hash("not-a-real-password", BCRYPT_COST).expect("bcrypt hash"));

#[derive(Deserialize)]
pub struct LoginBody {
    email: String,
    password: String,
}

pub async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> ApiResult<Json<Value>> {
    let email = body.email.trim().to_lowercase();
    if let Some(r) = &state.redis
        && r.hit(&format!("login:{email}"), LOGIN_WINDOW_SECS).await.unwrap_or(0) > LOGIN_LIMIT
    {
        return Err(ApiError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Terlalu banyak percobaan. Coba lagi dalam 15 menit.",
        ));
    }

    let row = sqlx::query_as::<_, (uuid::Uuid, String, bool)>(
        r#"SELECT "id", "passwordHash", "isActive" FROM "admin_user" WHERE lower("email") = $1"#,
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;

    let hash = row.as_ref().map(|r| r.1.clone()).unwrap_or_else(|| DUMMY_HASH.to_string());
    let password = body.password;
    let ok = tokio::task::spawn_blocking(move || bcrypt::verify(password, &hash).unwrap_or(false))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;

    let Some((id, _, active)) = row.filter(|_| ok) else {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Email atau kata sandi salah.",
        ));
    };
    if !active {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "account_disabled",
            "Akun ini dinonaktifkan.",
        ));
    }
    if let Some(r) = &state.redis {
        r.del(&format!("login:{email}")).await;
    }

    let (token, expires_at) = issue_admin_token(&state, id)?;
    let admin = me_json(&state, id).await?;
    Ok(Json(json!({ "token": token, "expiresAt": expires_at, "admin": admin })))
}

async fn me_json(state: &AppState, id: uuid::Uuid) -> ApiResult<Value> {
    let (email, name, is_super, must_change): (String, String, bool, bool) = sqlx::query_as(
        r#"SELECT "email", "name", "isSuperAdmin", "mustChangePassword" FROM "admin_user" WHERE "id" = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(
        json!({ "id": id, "email": email, "name": name, "isSuperAdmin": is_super, "mustChangePassword": must_change }),
    )
}

pub async fn me(SignedIn(admin): SignedIn) -> Json<Value> {
    Json(json!({ "admin": admin }))
}

#[derive(Deserialize)]
pub struct PasswordBody {
    current: String,
    next: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    SignedIn(admin): SignedIn,
    Json(body): Json<PasswordBody>,
) -> ApiResult<Json<Value>> {
    if body.next.chars().count() < MIN_PASSWORD {
        return Err(ApiError::bad_request(
            "password_too_short",
            format!("Kata sandi baru minimal {MIN_PASSWORD} karakter."),
        ));
    }
    if body.next == body.current {
        return Err(ApiError::bad_request("password_unchanged", "Kata sandi baru harus berbeda."));
    }
    let hash: String = sqlx::query_scalar(r#"SELECT "passwordHash" FROM "admin_user" WHERE "id" = $1"#)
        .bind(admin.id)
        .fetch_one(&state.db)
        .await?;
    let (current, next) = (body.current, body.next);
    let new_hash = tokio::task::spawn_blocking(move || -> Option<String> {
        bcrypt::verify(current, &hash).ok().filter(|ok| *ok)?;
        bcrypt::hash(next, BCRYPT_COST).ok()
    })
    .await
    .map_err(|e| ApiError::Internal(e.into()))?
    .ok_or_else(|| ApiError::bad_request("invalid_credentials", "Kata sandi saat ini salah."))?;

    sqlx::query(
        r#"UPDATE "admin_user" SET "passwordHash" = $1, "mustChangePassword" = false, "updatedAt" = now()
           WHERE "id" = $2"#,
    )
    .bind(new_hash)
    .bind(admin.id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    Ok(bcrypt::hash(password, BCRYPT_COST)?)
}
