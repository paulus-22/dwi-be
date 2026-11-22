//! Public read side of the invitation site (ported from the old Next.js
//! app's `lib/event.ts`, `lib/voucher.ts` and `api/wishes/route.ts`).
//! No session: the invite token, when relevant, comes in the query string.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use dwi_domain::{QrFields, build_qr_payload, parse_invite_code};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::routes::page_blocks::load_blocks;
use crate::routes::public::published_event;
use crate::state::AppState;

const WISH_LIMIT: i64 = 3;
const WISH_WINDOW_SECS: i64 = 60;
const WISHES_DEFAULT_LIMIT: i64 = 10;
const WISHES_MAX_LIMIT: i64 = 50;

fn normalize_token(raw: &str) -> String {
    raw.trim().to_uppercase()
}

/// Best-effort client address, same rule as the old app: the left-most
/// `x-forwarded-for` entry, then `x-real-ip`, else one shared (strict) bucket.
fn client_key(headers: &HeaderMap) -> String {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    get("x-forwarded-for")
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .or_else(|| get("x-real-ip").map(str::trim).filter(|v| !v.is_empty()))
        .unwrap_or("unknown")
        .to_string()
}

// ------------------------------------------------------------------ event

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
struct WeddingEvent {
    id: Uuid,
    slug: String,
    kind: String,
    status: String,
    bride_short_name: Option<String>,
    bride_full_name: Option<String>,
    bride_parents: Option<String>,
    bride_instagram: Option<String>,
    groom_short_name: Option<String>,
    groom_full_name: Option<String>,
    groom_parents: Option<String>,
    groom_instagram: Option<String>,
    headline_date: DateTime<Utc>,
    timezone: String,
    default_locale: String,
    theme: String,
    theme_tokens: Option<Value>,
    bilingual: bool,
    hashtag: Option<String>,
    quote_author: Option<String>,
    quote_text_id: Option<String>,
    quote_text_en: Option<String>,
    story_intro_id: Option<String>,
    story_intro_en: Option<String>,
    closing_note_id: Option<String>,
    closing_note_en: Option<String>,
    dress_code_note_id: Option<String>,
    dress_code_note_en: Option<String>,
    dress_code_colors: Vec<String>,
    music_url: Option<String>,
    cover_photo_url: Option<String>,
    closing_photo_url: Option<String>,
    dress_code_image_url: Option<String>,
    gift_image_url: Option<String>,
    cover_background_url: Option<String>,
    interlude_photos: Vec<String>,
    gift_address: Option<String>,
    favicon_url: Option<String>,
    invite_gate: String,
}

const WEDDING_SELECT: &str = r#"
    SELECT "id", "slug", "kind"::text AS "kind", "status"::text AS "status",
           "brideShortName", "brideFullName", "brideParents", "brideInstagram",
           "groomShortName", "groomFullName", "groomParents", "groomInstagram",
           "headlineDate" AT TIME ZONE 'UTC' AS "headlineDate", "timezone",
           "defaultLocale"::text AS "defaultLocale", "theme", "themeTokens", "bilingual", "hashtag",
           "quoteAuthor", "quoteTextId", "quoteTextEn",
           "storyIntroId", "storyIntroEn", "closingNoteId", "closingNoteEn",
           "dressCodeNoteId", "dressCodeNoteEn",
           COALESCE("dressCodeColors", ARRAY[]::text[]) AS "dressCodeColors",
           "musicUrl", "coverPhotoUrl", "closingPhotoUrl", "dressCodeImageUrl", "giftImageUrl",
           "coverBackgroundUrl", COALESCE("interludePhotos", ARRAY[]::text[]) AS "interludePhotos",
           "giftAddress", "faviconUrl", "inviteGate"::text AS "inviteGate"
    FROM "event" WHERE "id" = $1"#;

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
struct Ceremony {
    id: Uuid,
    name_id: String,
    name_en: String,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    venue_name: String,
    venue_address: Option<String>,
    maps_url: Option<String>,
    maps_embed_url: Option<String>,
    sort_order: i32,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
struct Milestone {
    id: Uuid,
    title_id: String,
    title_en: String,
    body_id: String,
    body_en: String,
    period_label: Option<String>,
    photo_url: Option<String>,
    sort_order: i32,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
struct RundownItem {
    id: Uuid,
    time_label: String,
    title_id: String,
    title_en: String,
    sort_order: i32,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
struct GiftAccount {
    id: Uuid,
    kind: String,
    institution: String,
    account_number: String,
    account_holder: String,
    qr_image_url: Option<String>,
    sort_order: i32,
}

/// `GET /v1/public/events/{slug}`
///
/// WEDDING → `{ event, ceremonies, milestones, rundown, giftAccounts }`.
/// GENERIC → `{ event: {id, slug, kind, title, subtitle, description,
/// headlineDate, timezone, status}, blocks }` (visible blocks only).
pub async fn event(State(state): State<AppState>, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let (id, kind, status): (Uuid, String, String) =
        sqlx::query_as(r#"SELECT "id", "kind"::text, "status"::text FROM "event" WHERE "slug" = $1"#)
            .bind(&slug)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found("Acara tidak ditemukan."))?;

    // ARCHIVED is gone for everyone. DRAFT is also 404 here.
    // TODO(allow admin preview of drafts once dwi-fe and dwi-be share a
    // session): the old app let a signed-in admin/member see their own draft.
    if status != "PUBLISHED" {
        return Err(ApiError::not_found("Acara tidak ditemukan."));
    }

    if kind == "GENERIC" {
        let (title, subtitle, description, headline_date, timezone, registration_mode, theme, favicon_url, invite_gate): (
            Option<String>,
            Option<String>,
            Option<String>,
            DateTime<Utc>,
            String,
            String,
            String,
            Option<String>,
            String,
        ) = sqlx::query_as(
            r#"SELECT "title", "subtitle", "description", "headlineDate" AT TIME ZONE 'UTC', "timezone",
                      "registrationMode"::text, "theme", "faviconUrl", "inviteGate"::text
               FROM "event" WHERE "id" = $1"#,
        )
        .bind(id)
        .fetch_one(&state.db)
        .await?;
        let blocks = load_blocks(&state.db, id, true).await?;
        return Ok(Json(json!({
            "event": {
                "id": id, "slug": slug, "kind": kind, "title": title, "subtitle": subtitle,
                "description": description, "headlineDate": headline_date, "timezone": timezone,
                "status": status, "registrationMode": registration_mode, "theme": theme,
                "faviconUrl": favicon_url, "inviteGate": invite_gate,
            },
            "blocks": blocks,
        })));
    }

    let event = sqlx::query_as::<_, WeddingEvent>(WEDDING_SELECT).bind(id).fetch_one(&state.db).await?;
    let ceremonies = sqlx::query_as::<_, Ceremony>(
        r#"SELECT "id", "nameId", "nameEn", "startsAt" AT TIME ZONE 'UTC' AS "startsAt",
                  "endsAt" AT TIME ZONE 'UTC' AS "endsAt", "venueName", "venueAddress", "mapsUrl",
                  "mapsEmbedUrl", "sortOrder"
           FROM "ceremony" WHERE "eventId" = $1 ORDER BY "sortOrder", "startsAt""#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let milestones = sqlx::query_as::<_, Milestone>(
        r#"SELECT "id", "titleId", "titleEn", "bodyId", "bodyEn", "periodLabel", "photoUrl", "sortOrder"
           FROM "story_milestone" WHERE "eventId" = $1 ORDER BY "sortOrder", "id""#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let rundown = sqlx::query_as::<_, RundownItem>(
        r#"SELECT "id", "timeLabel", "titleId", "titleEn", "sortOrder"
           FROM "rundown_item" WHERE "eventId" = $1 ORDER BY "sortOrder", "id""#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let gift_accounts = sqlx::query_as::<_, GiftAccount>(
        r#"SELECT "id", "kind"::text AS "kind", "institution", "accountNumber", "accountHolder",
                  "qrImageUrl", "sortOrder"
           FROM "gift_account" WHERE "eventId" = $1 ORDER BY "sortOrder", "id""#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({
        "event": event,
        "ceremonies": ceremonies,
        "milestones": milestones,
        "rundown": rundown,
        "giftAccounts": gift_accounts,
    })))
}

// ------------------------------------------------------------------ guest

#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

#[derive(sqlx::FromRow)]
struct GuestRow {
    id: Uuid,
    token: String,
    household_name: String,
    plus_guest_allowance: i32,
    phone: Option<String>,
    pass_mode: String, // 'NORMAL' | 'DIRECT' | 'GROUP'
    passcode: Option<String>,
    password_required: bool,
    rsvp_attending: Option<bool>,
    rsvp_guest_count: Option<i32>,
    rsvp_respondent_name: Option<String>,
    rsvp_message: Option<String>,
}

impl GuestRow {
    fn has_rsvp(&self) -> bool {
        self.rsvp_attending.is_some()
    }
    fn direct_pass(&self) -> bool {
        self.pass_mode == "DIRECT"
    }
    /// A GROUP guest is admitted as an open group, not via a scanned QR pass.
    fn open_group(&self) -> bool {
        self.pass_mode == "GROUP"
    }
}

/// Qualified guess for the invite-gate code. An explicit guest passcode wins;
/// otherwise derives from the phone (last 6 digits). `None` = no gate (can't
/// lock a guest out). This happens when a guest is explicitly exempted from
/// the password (`PasswordRequired = false`), is a GROUP invite (no per-person
/// pass), or has neither a phone nor a passcode.
fn gate_code(g: &GuestRow) -> Option<String> {
    if !g.password_required || g.open_group() {
        return None;
    }
    if let Some(c) = g.passcode.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        return Some(c.to_string());
    }
    let digits: String = g
        .phone
        .as_deref()
        .map(|p| p.chars().filter(|c| c.is_ascii_digit()).collect())
        .unwrap_or_default();
    (!digits.is_empty() && digits.len() >= 6).then(|| digits[digits.len() - 6..].to_string())
}

/// A bad, missing or foreign token is "no guest", never an error.
async fn find_guest(state: &AppState, event_id: Uuid, token: Option<&str>) -> ApiResult<Option<GuestRow>> {
    let Some(token) = token.map(normalize_token).filter(|t| !t.is_empty()) else { return Ok(None) };
    Ok(sqlx::query_as::<_, GuestRow>(
        r#"SELECT g."id", g."token", g."householdName" AS household_name,
                  g."plusGuestAllowance" AS plus_guest_allowance, g.phone,
                  g."passMode"::text AS pass_mode, g."passcode",
                  g."passwordRequired" AS password_required,
                  r."attending" AS rsvp_attending, r."guestCount" AS rsvp_guest_count,
                  r."respondentName" AS rsvp_respondent_name, r."message" AS rsvp_message
           FROM "guest" g LEFT JOIN "rsvp" r ON r."guestId" = g."id"
           WHERE g."token" = $1 AND g."eventId" = $2"#,
    )
    .bind(token)
    .bind(event_id)
    .fetch_optional(&state.db)
    .await?)
}

/// `GET /v1/public/events/{slug}/guest?token=`
pub async fn guest(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(q): Query<TokenQuery>,
) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    let Some(g) = find_guest(&state, event_id, q.token.as_deref()).await? else {
        return Ok(Json(json!({ "guest": null })));
    };
    let rsvp = g.has_rsvp().then(|| {
        json!({
            "attending": g.rsvp_attending,
            "guestCount": g.rsvp_guest_count,
            "respondentName": g.rsvp_respondent_name,
            "message": g.rsvp_message,
        })
    });
    Ok(Json(json!({
        "guest": {
            "id": g.id, "token": g.token, "householdName": g.household_name,
            "plusGuestAllowance": g.plus_guest_allowance, "passMode": g.pass_mode,
            "passwordRequired": g.password_required, "rsvp": rsvp,
        }
    })))
}

/// `POST /v1/public/events/{slug}/gate` — unlock a PHONE_6 invite with the
/// guest's secret code (last 6 digits of their phone). Body: { token, code }.
/// Nonexistent guest / wrong code → 403; correct code → 200.
#[derive(Deserialize)]
pub struct GateCheckInput {
    token: String,
    code: String,
}

pub async fn check_gate(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<GateCheckInput>,
) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    // A guest with neither a phone nor a custom passcode can't be locked out.
    // Otherwise the gate equals the event gate for an enabled PHONE_6 event.
    let Some(g) = find_guest(&state, event_id, Some(&body.token)).await? else {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "gate_denied", "Kode tidak valid."));
    };

    let gate: String = sqlx::query_scalar(r#"SELECT "inviteGate" FROM "event" WHERE "id" = $1"#)
        .bind(event_id)
        .fetch_one(&state.db)
        .await?;

    let expected = gate_code(&g);
    // A guest whose gate code is None (explicitly password-free, a GROUP
    // invite, or no phone/passcode at all) can't be locked out: auto-pass.
    let ok = gate != "PHONE_6"
        || expected.is_none()
        || expected.as_deref().is_some_and(|expected| expected == body.code.trim());

    if ok {
        Ok(Json(json!({ "ok": true, "guestId": g.id })))
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "gate_denied", "Kode tidak valid."))
    }
}

// ---------------------------------------------------------------- voucher

/// Signed payload from the guest's **stored** code (never their current
/// category or the event's current prefix), so printed cards keep matching.
/// An unparseable code falls back to the bare token, like the old app.
pub(crate) fn payload_for_stored_code(secret: &[u8], token: &str, admits: u32) -> String {
    match parse_invite_code(token) {
        Some(p) => build_qr_payload(
            secret,
            &QrFields { prefix: p.prefix, category: p.category, sequence: p.sequence, admits },
        ),
        None => token.to_string(),
    }
}

/// `GET /v1/public/events/{slug}/voucher?token=` — `unknown` / `pending` / `issued`.
pub async fn voucher(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(q): Query<TokenQuery>,
) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    let Some(g) = find_guest(&state, event_id, q.token.as_deref()).await? else {
        return Ok(Json(json!({ "status": "unknown" })));
    };

    // Prefer what the guest called themselves when confirming.
    let display_name = g
        .rsvp_respondent_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&g.household_name)
        .to_string();
    let guest = json!({
        "id": g.id, "householdName": g.household_name, "token": g.token,
        "plusGuestAllowance": g.plus_guest_allowance, "displayName": display_name,
    });

    // A GROUP guest admits as an open group via ONE shareable QR: any member
    // can scan it (the scanner then records which member arrived + headcount).
    // It is always issuable, like a DIRECT pass — no per-person RSVP needed.
    // A DIRECT guest never needs to confirm either: their pass admits the
    // invitation's own allowance (there's no submitted guest count).
    if g.open_group() || g.direct_pass() {
        let admits = (g.plus_guest_allowance + 1).max(1);
        let qr_payload = payload_for_stored_code(&state.invite_secret, &g.token, admits as u32);
        return Ok(Json(json!({ "status": "issued", "guest": guest, "admits": admits, "qrPayload": qr_payload })));
    }

    if g.rsvp_attending != Some(true) {
        return Ok(Json(json!({ "status": "pending", "guest": guest, "declined": g.has_rsvp() })));
    }

    // The pass admits the confirmed headcount, not the invitation's allowance.
    let confirmed = g.rsvp_guest_count.unwrap_or(1);
    let admits = confirmed.min(g.plus_guest_allowance + 1).max(1);
    let qr_payload = payload_for_stored_code(&state.invite_secret, &g.token, admits as u32);
    Ok(Json(json!({ "status": "issued", "guest": guest, "admits": admits, "qrPayload": qr_payload })))
}

// ----------------------------------------------------------------- wishes

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Wish {
    id: Uuid,
    name: String,
    message: String,
    created_at: DateTime<Utc>,
}

const WISH_COLUMNS: &str = r#""id", "name", "message", "createdAt" AT TIME ZONE 'UTC' AS created_at"#;

#[derive(Deserialize)]
pub struct WishesQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

/// `GET /v1/public/events/{slug}/wishes?cursor=&limit=`
pub async fn wishes(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(q): Query<WishesQuery>,
) -> ApiResult<Json<Value>> {
    let event_id = published_event(&state, &slug).await?;
    let limit = q.limit.unwrap_or(WISHES_DEFAULT_LIMIT).clamp(1, WISHES_MAX_LIMIT);
    let cursor = match q.cursor.as_deref().filter(|c| !c.is_empty()) {
        None => None,
        Some(c) => Some(
            c.parse::<Uuid>().map_err(|_| ApiError::bad_request("invalid_cursor", "Kursor tidak valid."))?,
        ),
    };

    // Newest first; id breaks createdAt ties so pages never overlap. An
    // unknown cursor yields an empty page (what Prisma does too).
    let mut rows = sqlx::query_as::<_, Wish>(sqlx::AssertSqlSafe(format!(
        r#"SELECT {WISH_COLUMNS} FROM "wish"
           WHERE "eventId" = $1 AND NOT "isHidden"
             AND ($2::uuid IS NULL OR ("createdAt", "id") < (
                   SELECT w."createdAt", w."id" FROM "wish" w WHERE w."id" = $2 AND w."eventId" = $1))
           ORDER BY "createdAt" DESC, "id" DESC
           LIMIT $3"#
    )))
    .bind(event_id)
    .bind(cursor)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;

    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if has_more { rows.last().map(|w| w.id) } else { None };
    Ok(Json(json!({ "wishes": rows, "nextCursor": next_cursor })))
}

#[derive(Deserialize)]
pub struct WishInput {
    token: Option<String>,
    name: String,
    message: String,
}

/// `POST /v1/public/events/{slug}/wishes` — 201 with the wish; a repeat of the
/// same name+message within 5 minutes returns the existing row with 200.
pub async fn create_wish(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(body): Json<WishInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    // The one open write path: 3 a minute per client. No Redis, no limit.
    if let Some(r) = &state.redis
        && r.hit(&format!("wish:{}", client_key(&headers)), WISH_WINDOW_SECS).await.unwrap_or(0) > WISH_LIMIT
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Terlalu banyak ucapan. Coba lagi sebentar lagi.",
        ));
    }

    let name = body.name.trim().to_string();
    let message = body.message.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad_request("invalid_input", "Nama wajib diisi (maks. 120 karakter)."));
    }
    if message.is_empty() || message.chars().count() > 600 {
        return Err(ApiError::bad_request("invalid_input", "Ucapan wajib diisi (maks. 600 karakter)."));
    }

    let event_id = published_event(&state, &slug).await?;
    // A wish from an invite link is attributed to that guest; otherwise anonymous.
    let guest_id = find_guest(&state, event_id, body.token.as_deref()).await?.map(|g| g.id);

    let duplicate = sqlx::query_as::<_, Wish>(sqlx::AssertSqlSafe(format!(
        r#"SELECT {WISH_COLUMNS} FROM "wish"
           WHERE "eventId" = $1 AND "name" = $2 AND "message" = $3
             AND "createdAt" >= (now() AT TIME ZONE 'UTC') - interval '5 minutes'
           ORDER BY "createdAt" DESC LIMIT 1"#
    )))
    .bind(event_id)
    .bind(&name)
    .bind(&message)
    .fetch_optional(&state.db)
    .await?;
    if let Some(w) = duplicate {
        return Ok((StatusCode::OK, Json(json!({ "wish": w }))));
    }

    // Prisma-era table: id has no DB default, and timestamps are UTC.
    let wish = sqlx::query_as::<_, Wish>(sqlx::AssertSqlSafe(format!(
        r#"INSERT INTO "wish" ("id", "eventId", "guestId", "name", "message", "createdAt")
           VALUES (gen_random_uuid(), $1, $2, $3, $4, now() AT TIME ZONE 'UTC')
           RETURNING {WISH_COLUMNS}"#
    )))
    .bind(event_id)
    .bind(guest_id)
    .bind(&name)
    .bind(&message)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "wish": wish }))))
}
