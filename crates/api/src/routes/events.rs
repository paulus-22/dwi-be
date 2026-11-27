//! Admin views of an event: list, guests, attendance, door devices, pairing codes.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{AdminUser, Role};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::views::{EVENT_SELECT, EventInfo, EventRow, guest_admits, guest_category};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventListItem {
    #[serde(flatten)]
    info: EventInfo,
    status: String,
    code_prefix: String,
    role: Role,
    registration_mode: String,
}

pub async fn list(State(state): State<AppState>, AdminUser(admin): AdminUser) -> ApiResult<Json<Value>> {
    let rows: Vec<(EventRow, Role)> = if admin.is_super_admin {
        sqlx::query_as::<_, EventRow>(sqlx::AssertSqlSafe(format!(
            r#"{EVENT_SELECT} ORDER BY e."headlineDate" DESC"#
        )))
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|e| (e, Role::Owner))
        .collect()
    } else {
        let events = sqlx::query_as::<_, EventRow>(sqlx::AssertSqlSafe(format!(
            r#"{EVENT_SELECT} JOIN "event_membership" m ON m."eventId" = e."id"
               WHERE m."adminId" = $1 ORDER BY e."headlineDate" DESC"#
        )))
        .bind(admin.id)
        .fetch_all(&state.db)
        .await?;
        let mut out = Vec::with_capacity(events.len());
        for e in events {
            let role = admin.require_event(&state, e.id, Role::Viewer).await?;
            out.push((e, role));
        }
        out
    };
    let events: Vec<EventListItem> = rows
        .iter()
        .map(|(e, role)| EventListItem {
            info: e.into(),
            status: e.status.clone(),
            code_prefix: e.code_prefix.clone(),
            role: *role,
            registration_mode: e.registration_mode.clone(),
        })
        .collect();
    Ok(Json(json!({ "events": events })))
}

/// `POST /v1/events` — creates a WEDDING or GENERIC event. The creator
/// becomes its OWNER; every other admin can be invited later.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventFields {
    kind: Option<String>,
    slug: Option<String>,
    status: Option<String>,
    headline_date: Option<DateTime<Utc>>,
    timezone: Option<String>,
    // GENERIC
    title: Option<String>,
    subtitle: Option<String>,
    description: Option<String>,
    // WEDDING
    bride_short_name: Option<String>,
    bride_full_name: Option<String>,
    bride_parents: Option<String>,
    bride_instagram: Option<String>,
    groom_short_name: Option<String>,
    groom_full_name: Option<String>,
    groom_parents: Option<String>,
    groom_instagram: Option<String>,
    default_locale: Option<String>,
    bilingual: Option<bool>,
    theme: Option<String>,
    hashtag: Option<String>,
    quote_text_id: Option<String>,
    quote_text_en: Option<String>,
    quote_author: Option<String>,
    story_intro_id: Option<String>,
    story_intro_en: Option<String>,
    closing_note_id: Option<String>,
    closing_note_en: Option<String>,
    dress_code_note_id: Option<String>,
    dress_code_note_en: Option<String>,
    dress_code_colors: Option<Vec<String>>,
    music_url: Option<String>,
    cover_photo_url: Option<String>,
    cover_background_url: Option<String>,
    closing_photo_url: Option<String>,
    dress_code_image_url: Option<String>,
    interlude_photos: Option<Vec<String>>,
    gallery_photos: Option<Vec<String>>,
    favicon_url: Option<String>,
    /// 'OPEN' (no password) or 'PHONE_6' (guest's last-6-phone-digits).
    invite_gate: Option<String>,
}

/// Lowercase letters, digits and hyphens, no leading/trailing/double hyphen —
/// the same shape the public `/e/[slug]` route expects in a URL.
fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Trims, and turns an empty string into `None`.
fn nz(s: Option<String>) -> Option<String> {
    s.and_then(|s| {
        let t = s.trim().to_string();
        (!t.is_empty()).then_some(t)
    })
}

const VALID_STATUSES: &[&str] = &["DRAFT", "PUBLISHED", "ARCHIVED"];
const VALID_INVITE_GATES: &[&str] = &["OPEN", "PHONE_6"];
/// Defaults an absent or invalid gate to OPEN; maps unknown values safely.
fn normalize_invite_gate(raw: Option<&str>) -> &'static str {
    match raw.map(str::trim).unwrap_or("") {
        "PHONE_6" => "PHONE_6",
        _ => "OPEN",
    }
}
/// GENERIC events pick from a small fixed set, deliberately separate from
/// the wedding product's much larger theme registry (different visual
/// language entirely — see dwi-fe's `GENERIC_THEMES`).
const VALID_GENERIC_THEMES: &[&str] = &["corporate-classic", "corporate-soft", "editorial"];

pub async fn create(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(body): Json<EventFields>,
) -> ApiResult<Json<Value>> {
    let kind = body.kind.as_deref().unwrap_or("");
    if kind != "WEDDING" && kind != "GENERIC" {
        return Err(ApiError::bad_request("invalid_kind", "Jenis acara harus WEDDING atau GENERIC."));
    }
    let slug = body.slug.as_deref().unwrap_or("").trim().to_lowercase();
    if !valid_slug(&slug) {
        return Err(ApiError::bad_request(
            "invalid_slug",
            "Slug hanya boleh huruf kecil, angka, dan tanda hubung (tanpa hubung ganda/di ujung).",
        ));
    }
    let Some(headline_date) = body.headline_date else {
        return Err(ApiError::bad_request("invalid_headline_date", "Tanggal & waktu acara wajib diisi."));
    };
    let timezone = body.timezone.clone().unwrap_or_else(|| "Asia/Jakarta".to_string());
    let status = body.status.as_deref().unwrap_or("DRAFT");
    if !VALID_STATUSES.contains(&status) {
        return Err(ApiError::bad_request("invalid_status", "Status tidak dikenal."));
    }

    let exists: bool = sqlx::query_scalar(r#"SELECT EXISTS (SELECT 1 FROM "event" WHERE "slug" = $1)"#)
        .bind(&slug)
        .fetch_one(&state.db)
        .await?;
    if exists {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "slug_taken",
            "Slug ini sudah dipakai acara lain.",
        ));
    }

    let id = Uuid::new_v4();
    if kind == "GENERIC" {
        let Some(title) = nz(body.title.clone()) else {
            return Err(ApiError::bad_request("invalid_title", "Judul acara wajib diisi."));
        };
        let theme = nz(body.theme.clone()).unwrap_or_else(|| "corporate-classic".to_string());
        if !VALID_GENERIC_THEMES.contains(&theme.as_str()) {
            return Err(ApiError::bad_request(
                "invalid_theme",
                format!("Tema tidak dikenal. Pilihan: {}.", VALID_GENERIC_THEMES.join(", ")),
            ));
        }
        sqlx::query(
            r#"INSERT INTO "event"
                ("id", "slug", "status", "kind", "title", "subtitle", "description", "headlineDate", "timezone", "theme", "faviconUrl", "updatedAt")
               VALUES ($1, $2, $3::"EventStatus", 'GENERIC', $4, $5, $6, $7, $8, $9, $10, now())"#,
        )
        .bind(id)
        .bind(&slug)
        .bind(status)
        .bind(&title)
        .bind(nz(body.subtitle.clone()))
        .bind(nz(body.description.clone()))
        .bind(headline_date)
        .bind(&timezone)
        .bind(&theme)
        .bind(nz(body.favicon_url.clone()))
        .execute(&state.db)
        .await?;
    } else {
        let (Some(bride_short), Some(bride_full), Some(groom_short), Some(groom_full)) = (
            nz(body.bride_short_name.clone()),
            nz(body.bride_full_name.clone()),
            nz(body.groom_short_name.clone()),
            nz(body.groom_full_name.clone()),
        ) else {
            return Err(ApiError::bad_request(
                "missing_names",
                "Nama panggilan dan nama lengkap mempelai wajib diisi.",
            ));
        };
        sqlx::query(
            r#"INSERT INTO "event"
                ("id", "slug", "status", "kind",
                 "brideShortName", "brideFullName", "brideParents", "brideInstagram",
                 "groomShortName", "groomFullName", "groomParents", "groomInstagram",
                 "headlineDate", "timezone", "defaultLocale", "bilingual", "theme", "hashtag",
                 "quoteTextId", "quoteTextEn", "quoteAuthor",
                 "storyIntroId", "storyIntroEn", "closingNoteId", "closingNoteEn",
                 "dressCodeNoteId", "dressCodeNoteEn", "dressCodeColors",
                 "musicUrl", "coverPhotoUrl", "coverBackgroundUrl", "closingPhotoUrl", "dressCodeImageUrl",
                 "interludePhotos", "galleryPhotos", "faviconUrl", "updatedAt")
               VALUES ($1, $2, $3::"EventStatus", 'WEDDING', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                       $14::"Locale", $15, $16, $17,
                       $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, now())"#,
        )
        .bind(id)
        .bind(&slug)
        .bind(status)
        .bind(&bride_short)
        .bind(&bride_full)
        .bind(nz(body.bride_parents.clone()))
        .bind(nz(body.bride_instagram.clone()))
        .bind(&groom_short)
        .bind(&groom_full)
        .bind(nz(body.groom_parents.clone()))
        .bind(nz(body.groom_instagram.clone()))
        .bind(headline_date)
        .bind(&timezone)
        .bind(body.default_locale.clone().filter(|l| l == "ID" || l == "EN").unwrap_or_else(|| "ID".into()))
        .bind(body.bilingual.unwrap_or(true))
        .bind(body.theme.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "sage-peach".into()))
        .bind(nz(body.hashtag.clone()))
        .bind(nz(body.quote_text_id.clone()))
        .bind(nz(body.quote_text_en.clone()))
        .bind(nz(body.quote_author.clone()))
        .bind(nz(body.story_intro_id.clone()))
        .bind(nz(body.story_intro_en.clone()))
        .bind(nz(body.closing_note_id.clone()))
        .bind(nz(body.closing_note_en.clone()))
        .bind(nz(body.dress_code_note_id.clone()))
        .bind(nz(body.dress_code_note_en.clone()))
        .bind(body.dress_code_colors.clone().unwrap_or_default())
        .bind(nz(body.music_url.clone()))
        .bind(nz(body.cover_photo_url.clone()))
        .bind(nz(body.cover_background_url.clone()))
        .bind(nz(body.closing_photo_url.clone()))
        .bind(nz(body.dress_code_image_url.clone()))
        .bind(body.interlude_photos.clone().unwrap_or_default())
        .bind(body.gallery_photos.clone().unwrap_or_default())
        .bind(nz(body.favicon_url.clone()))
        .execute(&state.db)
        .await?;
    }

    // The creator is the owner — every other admin can be invited later.
    sqlx::query(
        r#"INSERT INTO "event_membership" ("id", "adminId", "eventId", "role")
           VALUES (gen_random_uuid(), $1, $2, 'OWNER')"#,
    )
    .bind(admin.id)
    .bind(id)
    .execute(&state.db)
    .await?;

    let e = load_event(&state, id).await?;
    Ok(Json(json!({
        "event": EventListItem { info: (&e).into(), status: e.status.clone(), code_prefix: e.code_prefix.clone(), role: Role::Owner, registration_mode: e.registration_mode.clone() }
    })))
}

/// `PUT /v1/events/{id}` — replaces the top-level fields (OWNER only, same
/// rule as the old app: a slug/name change re-points every link already
/// sent, so it's gated above day-to-day guest management). `kind` itself is
/// immutable and ignored if present in the body.
pub async fn update(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<EventFields>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Owner).await?;
    let current = load_event(&state, id).await?;

    let slug = body.slug.as_deref().unwrap_or("").trim().to_lowercase();
    if !valid_slug(&slug) {
        return Err(ApiError::bad_request(
            "invalid_slug",
            "Slug hanya boleh huruf kecil, angka, dan tanda hubung (tanpa hubung ganda/di ujung).",
        ));
    }
    let Some(headline_date) = body.headline_date else {
        return Err(ApiError::bad_request("invalid_headline_date", "Tanggal & waktu acara wajib diisi."));
    };
    let timezone = body.timezone.clone().unwrap_or_else(|| "Asia/Jakarta".to_string());
    let status = body.status.as_deref().unwrap_or("DRAFT");
    if !VALID_STATUSES.contains(&status) {
        return Err(ApiError::bad_request("invalid_status", "Status tidak dikenal."));
    }

    let clash: bool =
        sqlx::query_scalar(r#"SELECT EXISTS (SELECT 1 FROM "event" WHERE "slug" = $1 AND "id" != $2)"#)
            .bind(&slug)
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if clash {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "slug_taken",
            "Slug ini sudah dipakai acara lain.",
        ));
    }

    if current.kind == "GENERIC" {
        let Some(title) = nz(body.title.clone()) else {
            return Err(ApiError::bad_request("invalid_title", "Judul acara wajib diisi."));
        };
        let theme = nz(body.theme.clone()).unwrap_or_else(|| "corporate-classic".to_string());
        if !VALID_GENERIC_THEMES.contains(&theme.as_str()) {
            return Err(ApiError::bad_request(
                "invalid_theme",
                format!("Tema tidak dikenal. Pilihan: {}.", VALID_GENERIC_THEMES.join(", ")),
            ));
        }
        sqlx::query(
            r#"UPDATE "event" SET "slug" = $2, "status" = $3::"EventStatus", "title" = $4, "subtitle" = $5,
                "description" = $6, "headlineDate" = $7, "timezone" = $8, "theme" = $9, "faviconUrl" = $10,
                "inviteGate" = $11, "updatedAt" = now()
               WHERE "id" = $1"#,
        )
        .bind(id)
        .bind(&slug)
        .bind(status)
        .bind(&title)
        .bind(nz(body.subtitle.clone()))
        .bind(nz(body.description.clone()))
        .bind(headline_date)
        .bind(&timezone)
        .bind(&theme)
        .bind(nz(body.favicon_url.clone()))
        .bind(normalize_invite_gate(body.invite_gate.as_deref()))
        .execute(&state.db)
        .await?;
    } else {
        let (Some(bride_short), Some(bride_full), Some(groom_short), Some(groom_full)) = (
            nz(body.bride_short_name.clone()),
            nz(body.bride_full_name.clone()),
            nz(body.groom_short_name.clone()),
            nz(body.groom_full_name.clone()),
        ) else {
            return Err(ApiError::bad_request(
                "missing_names",
                "Nama panggilan dan nama lengkap mempelai wajib diisi.",
            ));
        };
        sqlx::query(
            r#"UPDATE "event" SET
                "slug" = $2, "status" = $3::"EventStatus",
                "brideShortName" = $4, "brideFullName" = $5, "brideParents" = $6, "brideInstagram" = $7,
                "groomShortName" = $8, "groomFullName" = $9, "groomParents" = $10, "groomInstagram" = $11,
                "headlineDate" = $12, "timezone" = $13, "defaultLocale" = $14::"Locale", "bilingual" = $15,
                "theme" = $16, "hashtag" = $17,
                "quoteTextId" = $18, "quoteTextEn" = $19, "quoteAuthor" = $20,
                "storyIntroId" = $21, "storyIntroEn" = $22, "closingNoteId" = $23, "closingNoteEn" = $24,
                "dressCodeNoteId" = $25, "dressCodeNoteEn" = $26, "dressCodeColors" = $27,
                "musicUrl" = $28, "coverPhotoUrl" = $29, "coverBackgroundUrl" = $30,
                "closingPhotoUrl" = $31, "dressCodeImageUrl" = $32, "interludePhotos" = $33,
                "galleryPhotos" = $34, "faviconUrl" = $35, "inviteGate" = $36,
                "updatedAt" = now()
               WHERE "id" = $1"#,
        )
        .bind(id)
        .bind(&slug)
        .bind(status)
        .bind(&bride_short)
        .bind(&bride_full)
        .bind(nz(body.bride_parents.clone()))
        .bind(nz(body.bride_instagram.clone()))
        .bind(&groom_short)
        .bind(&groom_full)
        .bind(nz(body.groom_parents.clone()))
        .bind(nz(body.groom_instagram.clone()))
        .bind(headline_date)
        .bind(&timezone)
        .bind(body.default_locale.clone().filter(|l| l == "ID" || l == "EN").unwrap_or_else(|| "ID".into()))
        .bind(body.bilingual.unwrap_or(true))
        .bind(body.theme.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "sage-peach".into()))
        .bind(nz(body.hashtag.clone()))
        .bind(nz(body.quote_text_id.clone()))
        .bind(nz(body.quote_text_en.clone()))
        .bind(nz(body.quote_author.clone()))
        .bind(nz(body.story_intro_id.clone()))
        .bind(nz(body.story_intro_en.clone()))
        .bind(nz(body.closing_note_id.clone()))
        .bind(nz(body.closing_note_en.clone()))
        .bind(nz(body.dress_code_note_id.clone()))
        .bind(nz(body.dress_code_note_en.clone()))
        .bind(body.dress_code_colors.clone().unwrap_or_default())
        .bind(nz(body.music_url.clone()))
        .bind(nz(body.cover_photo_url.clone()))
        .bind(nz(body.cover_background_url.clone()))
        .bind(nz(body.closing_photo_url.clone()))
        .bind(nz(body.dress_code_image_url.clone()))
        .bind(body.interlude_photos.clone().unwrap_or_default())
        .bind(body.gallery_photos.clone().unwrap_or_default())
        .bind(nz(body.favicon_url.clone()))
        .bind(normalize_invite_gate(body.invite_gate.as_deref()))
        .execute(&state.db)
        .await?;
    }

    let e = load_event(&state, id).await?;
    Ok(Json(json!({
        "event": EventListItem { info: (&e).into(), status: e.status.clone(), code_prefix: e.code_prefix.clone(), role: Role::Owner, registration_mode: e.registration_mode.clone() }
    })))
}

/// `DELETE /v1/events/{id}` — soft delete: archives the event (OWNER only).
/// The old app never hard-deletes an event either — `status = ARCHIVED` is
/// the only "gone" state, and it's reversible by an OWNER through `update`.
pub async fn archive(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Owner).await?;
    sqlx::query(r#"UPDATE "event" SET "status" = 'ARCHIVED', "updatedAt" = now() WHERE "id" = $1"#)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn load_event(state: &AppState, id: Uuid) -> ApiResult<EventRow> {
    sqlx::query_as::<_, EventRow>(sqlx::AssertSqlSafe(format!(r#"{EVENT_SELECT} WHERE e."id" = $1"#)))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Acara tidak ditemukan."))
}

pub async fn get(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let role = admin.require_event(&state, id, Role::Viewer).await?;
    let e = load_event(&state, id).await?;
    // The summary (`EventInfo`) is enough for lists and headers; an edit form
    // needs the full raw field set too, so it rides alongside as `detail`.
    let detail: Value = sqlx::query_scalar(
        r#"SELECT to_jsonb(x) FROM (
             SELECT "slug", "description",
                    "brideFullName", "groomFullName",
                    "brideParents", "brideInstagram", "groomParents", "groomInstagram",
                    "headlineDate" AT TIME ZONE 'UTC' AS "headlineDate", "timezone",
                    "defaultLocale"::text AS "defaultLocale", "bilingual", "theme",
                    "quoteTextId", "quoteTextEn", "quoteAuthor",
                    "storyIntroId", "storyIntroEn", "closingNoteId", "closingNoteEn",
                    "dressCodeNoteId", "dressCodeNoteEn",
                    COALESCE("dressCodeColors", ARRAY[]::text[]) AS "dressCodeColors",
                    "musicUrl", "coverPhotoUrl", "coverBackgroundUrl", "closingPhotoUrl", "dressCodeImageUrl",
                    COALESCE("interludePhotos", ARRAY[]::text[]) AS "interludePhotos",
                    COALESCE("galleryPhotos", ARRAY[]::text[]) AS "galleryPhotos",
                    "faviconUrl", "inviteGate"::text AS "inviteGate"
             FROM "event" WHERE "id" = $1
           ) x"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({
        "event": EventListItem { info: (&e).into(), status: e.status.clone(), code_prefix: e.code_prefix.clone(), role, registration_mode: e.registration_mode.clone() },
        "detail": detail,
    })))
}

// ------------------------------------------------------------------ guests

#[derive(sqlx::FromRow)]
pub struct GuestRow {
    pub id: Uuid,
    pub code: String,
    pub household_name: String,
    pub category: Option<String>,
    pub plus_allowance: i32,
    pub attending: Option<bool>,
    pub rsvp_count: Option<i32>,
    pub pass_mode: String,
    pub passcode: Option<String>,
    pub password_required: bool,
}

pub const GUEST_SELECT: &str = r#"
    SELECT g."id", g."token" AS code, g."householdName" AS household_name, g."category",
           g."plusGuestAllowance" AS plus_allowance, r."attending", r."guestCount" AS rsvp_count,
           g."passMode"::text AS pass_mode, g."passcode", g."passwordRequired" AS password_required
    FROM "guest" g LEFT JOIN "rsvp" r ON r."guestId" = g."id"
    WHERE g."eventId" = $1
    ORDER BY g."sequence", g."householdName""#;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestQrInput {
    pass_mode: Option<String>,
    passcode: Option<Option<String>>,
    password_required: Option<bool>,
}

/// `PUT /v1/events/{id}/guests/{guestId}` — set a single guest's pass mode
/// (NORMAL / DIRECT / GROUP) and optional custom passcode.
pub async fn set_guest_qr(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path((event_id, guest_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<GuestQrInput>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let mode = body.pass_mode.as_deref().unwrap_or("NORMAL").trim();
    if !matches!(mode, "NORMAL" | "DIRECT" | "GROUP") {
        return Err(ApiError::bad_request("invalid_pass_mode", "Mode tidak dikenal."));
    }
    let mode = mode.to_string();
    // passcode: Some(Some(val)) sets it, Some(None) clears, None leaves it.
    // passwordRequired keeps its current value when omitted (COALESCE).
    let upd = if let Some(passcode) = &body.passcode {
        sqlx::query(
            r#"UPDATE "guest" SET "passMode" = $1, "passcode" = $2,
               "passwordRequired" = COALESCE($3, "passwordRequired"), "updatedAt" = now()
               WHERE "id" = $4 AND "eventId" = $5"#,
        )
        .bind(&mode)
        .bind(passcode.as_deref())
        .bind(body.password_required)
        .bind(guest_id)
        .bind(event_id)
        .execute(&state.db)
        .await?
    } else {
        sqlx::query(
            r#"UPDATE "guest" SET "passMode" = $1,
               "passwordRequired" = COALESCE($2, "passwordRequired"), "updatedAt" = now()
               WHERE "id" = $3 AND "eventId" = $4"#,
        )
        .bind(&mode)
        .bind(body.password_required)
        .bind(guest_id)
        .bind(event_id)
        .execute(&state.db)
        .await?
    };
    if upd.rows_affected() == 0 {
        return Err(ApiError::not_found("Tamu tidak ditemukan."));
    }
    Ok(Json(json!({ "ok": true, "passMode": mode })))
}

pub async fn guests(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Viewer).await?;
    let rows = sqlx::query_as::<_, GuestRow>(GUEST_SELECT).bind(id).fetch_all(&state.db).await?;
    let first = first_check_ins(&state, id).await?;
    let guests: Vec<Value> = rows
        .iter()
        .map(|g| {
            let check_in = first.iter().find(|c| c.code == g.code);
            json!({
                "id": g.id,
                "code": g.code,
                "householdName": g.household_name,
                "category": guest_category(&g.code, g.category.as_deref()).as_str(),
                "attending": g.attending,
                "admits": guest_admits(g.attending, g.rsvp_count, g.plus_allowance),
                "passMode": g.pass_mode,
                "passcode": g.passcode,
                "passwordRequired": g.password_required,
                "checkedInAt": check_in.map(|c| c.occurred_at),
                "checkedInHeadcount": check_in.map(|c| c.headcount),
            })
        })
        .collect();
    Ok(Json(json!({ "guests": guests })))
}

/// `POST /v1/events/{id}/guests` — adds one guest to a closed, admin-managed
/// list and mints their unique invite code on the spot (same model the
/// wedding product already uses: the admin decides who's invited, each gets
/// their own link, no quota/pool involved — unlike `ticket_types`, which is
/// for open self-registration instead).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestInput {
    household_name: String,
    phone: Option<String>,
    #[serde(default)]
    plus_guest_allowance: i32,
    category: Option<String>,
}

pub async fn create_guest(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(event_id): Path<Uuid>,
    Json(body): Json<GuestInput>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, event_id, Role::Organizer).await?;
    let name = body.household_name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Nama tamu/peserta wajib diisi (maks. 120 karakter).",
        ));
    }
    if !(0..=20).contains(&body.plus_guest_allowance) {
        return Err(ApiError::bad_request("invalid_allowance", "Jumlah tambahan harus antara 0 dan 20."));
    }
    let phone = body.phone.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let category_code = dwi_domain::to_category_code(body.category.as_deref());
    let category_label = category_code.label();

    // Same global-prefix-scoped sequence as ticket_types::generate_pool and
    // registration::register — `token` is globally unique, `codePrefix`
    // isn't guaranteed unique between events.
    let mut tx = state.db.begin().await?;
    let prefix: String = sqlx::query_scalar(r#"SELECT "codePrefix" FROM "event" WHERE "id" = $1"#)
        .bind(event_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::not_found("Acara tidak ditemukan."))?;
    let next_seq: i32 = sqlx::query_scalar(
        r#"SELECT COALESCE(MAX(g."sequence"), 0) + 1 FROM "guest" g JOIN "event" e ON e."id" = g."eventId"
           WHERE e."codePrefix" = $1"#,
    )
    .bind(&prefix)
    .fetch_one(&mut *tx)
    .await?;
    let token = dwi_domain::build_invite_code(&prefix, category_code, next_seq as u32);

    let guest_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO "guest"
               ("id", "eventId", "householdName", "token", "sequence", "plusGuestAllowance", "category",
                "phone", "codeLockedAt", "updatedAt")
           VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, now(), now())
           RETURNING "id""#,
    )
    .bind(event_id)
    .bind(&name)
    .bind(&token)
    .bind(next_seq)
    .bind(body.plus_guest_allowance)
    .bind(category_label)
    .bind(phone)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "guest": { "id": guest_id, "code": token, "householdName": name, "category": category_label },
    })))
}

// -------------------------------------------------------------- attendance

#[derive(sqlx::FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstCheckIn {
    pub id: Uuid,
    pub code: String,
    pub headcount: i32,
    pub occurred_at: DateTime<Utc>,
    pub device_name: String,
    pub household_name: Option<String>,
    /// Group check-in: which member arrived (usher-typed), on the FIRST scan.
    pub member_name: Option<String>,
}

/// One row per guest: their earliest check-in from any device.
pub async fn first_check_ins(state: &AppState, event_id: Uuid) -> ApiResult<Vec<FirstCheckIn>> {
    Ok(sqlx::query_as::<_, FirstCheckIn>(
        r#"SELECT DISTINCT ON (s.guest_code)
                  s.id, s.guest_code AS code, COALESCE(s.headcount, 1) AS headcount, s.occurred_at,
                  d.name AS device_name, g."householdName" AS household_name,
                  ci."memberName" AS member_name
           FROM scanner_event s
           JOIN scanner_device d ON d.id = s.device_id
           LEFT JOIN "guest" g ON g."id" = s.guest_id
           LEFT JOIN "check_in" ci ON ci."id" = s.id
           WHERE s.event_id = $1 AND s.kind = 'CHECK_IN'
           ORDER BY s.guest_code, s.occurred_at, s.seq"#,
    )
    .bind(event_id)
    .fetch_all(&state.db)
    .await?)
}

pub async fn attendance(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Viewer).await?;

    let (households, attending_households, expected_people): (i64, i64, i64) = sqlx::query_as(
        r#"SELECT count(*),
                  count(*) FILTER (WHERE r."attending"),
                  COALESCE(sum(r."guestCount") FILTER (WHERE r."attending"), 0)
           FROM "guest" g LEFT JOIN "rsvp" r ON r."guestId" = g."id"
           WHERE g."eventId" = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    let mut first = first_check_ins(&state, id).await?;
    let checked_in_people: i64 = first.iter().map(|c| i64::from(c.headcount)).sum();
    let checked_in_households = first.len();

    let mut by_device: Vec<(String, i64)> = Vec::new();
    for c in &first {
        match by_device.iter_mut().find(|(n, _)| *n == c.device_name) {
            Some((_, n)) => *n += 1,
            None => by_device.push((c.device_name.clone(), 1)),
        }
    }
    by_device.sort_by_key(|d| std::cmp::Reverse(d.1));

    let duplicates: i64 =
        sqlx::query_scalar("SELECT count(*) FROM scanner_event WHERE event_id = $1 AND kind = 'CHECK_IN'")
            .bind(id)
            .fetch_one(&state.db)
            .await?;

    first.sort_by_key(|c| std::cmp::Reverse(c.occurred_at));
    first.truncate(20);

    Ok(Json(json!({
        "households": households,
        "attendingHouseholds": attending_households,
        "expectedPeople": expected_people,
        "checkedInHouseholds": checked_in_households,
        "checkedInPeople": checked_in_people,
        "totalScans": duplicates,
        "repeatScans": duplicates - checked_in_households as i64,
        "byDevice": by_device.into_iter().map(|(name, n)| json!({ "deviceName": name, "checkIns": n })).collect::<Vec<_>>(),
        "recent": first,
        "at": Utc::now(),
    })))
}

// --------------------------------------------------------- pairing codes

/// Same unambiguous alphabet as invite codes: easy to read out at a venue.
const PAIR_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRTUVWXYZ";

fn new_pairing_code() -> String {
    let mut rng = rand::rng();
    let mut pick = || PAIR_ALPHABET[rng.random_range(0..PAIR_ALPHABET.len())] as char;
    let a: String = (0..4).map(|_| pick()).collect();
    let b: String = (0..4).map(|_| pick()).collect();
    format!("{a}-{b}")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPairingCode {
    label: Option<String>,
    /// The gate (entrance) this code is bound to, e.g. "Pintu Utama". Shown to
    /// the device as its name after pairing. Defaults to "Pintu Utama".
    gate: Option<String>,
    /// Defaults to 24; at most a week.
    valid_hours: Option<i64>,
}

pub async fn create_pairing_code(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<NewPairingCode>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Organizer).await?;
    let hours = body.valid_hours.unwrap_or(24).clamp(1, 24 * 7);
    let label = body.label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
    let gate = body
        .gate
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| "Pintu Utama".to_string());
    // Collisions are astronomically unlikely, but retry rather than 500.
    for _ in 0..5 {
        let code = new_pairing_code();
        let row: Option<(Uuid, DateTime<Utc>)> = sqlx::query_as(
            "INSERT INTO scanner_pairing_code (event_id, code, label, gate, created_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, now() + make_interval(hours => $6::int))
             ON CONFLICT (code) DO NOTHING
             RETURNING id, expires_at",
        )
        .bind(id)
        .bind(&code)
        .bind(&label)
        .bind(&gate)
        .bind(admin.id)
        .bind(hours as i32)
        .fetch_optional(&state.db)
        .await?;
        if let Some((pid, expires_at)) = row {
            return Ok(Json(json!({ "id": pid, "code": code, "label": label, "gate": gate, "expiresAt": expires_at })));
        }
    }
    Err(ApiError::Internal(anyhow::anyhow!("could not allocate a pairing code")))
}

pub async fn list_pairing_codes(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Organizer).await?;
    let rows: Vec<(Uuid, String, Option<String>, String, DateTime<Utc>, i32, Option<DateTime<Utc>>)> =
        sqlx::query_as(
            "SELECT id, code, label, gate, expires_at, uses, last_used_at FROM scanner_pairing_code
        WHERE event_id = $1 AND revoked_at IS NULL AND expires_at > now()
        ORDER BY created_at DESC",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let codes: Vec<Value> = rows
        .into_iter()
        .map(|(id, code, label, gate, expires_at, uses, last_used_at)| {
            json!({ "id": id, "code": code, "label": label, "gate": gate, "expiresAt": expires_at, "uses": uses, "lastUsedAt": last_used_at })
        })
        .collect();
    Ok(Json(json!({ "codes": codes })))
}

pub async fn revoke_pairing_code(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(code_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let event_id: Uuid = sqlx::query_scalar("SELECT event_id FROM scanner_pairing_code WHERE id = $1")
        .bind(code_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Kode tidak ditemukan."))?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    sqlx::query("UPDATE scanner_pairing_code SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(code_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

// ----------------------------------------------------------------- devices

pub async fn devices(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    admin.require_event(&state, id, Role::Viewer).await?;
    type Row = (Uuid, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, i64, i64);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT d.id, d.name, d.paired_at, d.last_seen_at, d.revoked_at, d.clock_offset_ms,
                (SELECT count(*) FROM scanner_event s WHERE s.device_id = d.id AND s.kind = 'CHECK_IN')
         FROM scanner_device d WHERE d.event_id = $1 ORDER BY d.paired_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let devices: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, paired_at, last_seen_at, revoked_at, offset, check_ins)| {
            json!({
                "id": id, "name": name, "pairedAt": paired_at, "lastSeenAt": last_seen_at,
                "revokedAt": revoked_at, "clockOffsetMs": offset, "checkIns": check_ins,
            })
        })
        .collect();
    Ok(Json(json!({ "devices": devices })))
}

pub async fn revoke_device(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(device_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let event_id: Uuid = sqlx::query_scalar("SELECT event_id FROM scanner_device WHERE id = $1")
        .bind(device_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Perangkat tidak ditemukan."))?;
    admin.require_event(&state, event_id, Role::Organizer).await?;
    sqlx::query("UPDATE scanner_device SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(device_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
