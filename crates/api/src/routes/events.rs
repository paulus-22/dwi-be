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
        })
        .collect();
    Ok(Json(json!({ "events": events })))
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
    Ok(Json(json!({
        "event": EventListItem { info: (&e).into(), status: e.status.clone(), code_prefix: e.code_prefix.clone(), role }
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
}

pub const GUEST_SELECT: &str = r#"
    SELECT g."id", g."token" AS code, g."householdName" AS household_name, g."category",
           g."plusGuestAllowance" AS plus_allowance, r."attending", r."guestCount" AS rsvp_count
    FROM "guest" g LEFT JOIN "rsvp" r ON r."guestId" = g."id"
    WHERE g."eventId" = $1
    ORDER BY g."sequence", g."householdName""#;

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
                "checkedInAt": check_in.map(|c| c.occurred_at),
                "checkedInHeadcount": check_in.map(|c| c.headcount),
            })
        })
        .collect();
    Ok(Json(json!({ "guests": guests })))
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
}

/// One row per guest: their earliest check-in from any device.
pub async fn first_check_ins(state: &AppState, event_id: Uuid) -> ApiResult<Vec<FirstCheckIn>> {
    Ok(sqlx::query_as::<_, FirstCheckIn>(
        r#"SELECT DISTINCT ON (s.guest_code)
                  s.id, s.guest_code AS code, COALESCE(s.headcount, 1) AS headcount, s.occurred_at,
                  d.name AS device_name, g."householdName" AS household_name
           FROM scanner_event s
           JOIN scanner_device d ON d.id = s.device_id
           LEFT JOIN "guest" g ON g."id" = s.guest_id
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
    // Collisions are astronomically unlikely, but retry rather than 500.
    for _ in 0..5 {
        let code = new_pairing_code();
        let row: Option<(Uuid, DateTime<Utc>)> = sqlx::query_as(
            "INSERT INTO scanner_pairing_code (event_id, code, label, created_by, expires_at)
             VALUES ($1, $2, $3, $4, now() + make_interval(hours => $5::int))
             ON CONFLICT (code) DO NOTHING
             RETURNING id, expires_at",
        )
        .bind(id)
        .bind(&code)
        .bind(&label)
        .bind(admin.id)
        .bind(hours as i32)
        .fetch_optional(&state.db)
        .await?;
        if let Some((pid, expires_at)) = row {
            return Ok(Json(json!({ "id": pid, "code": code, "label": label, "expiresAt": expires_at })));
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
    let rows: Vec<(Uuid, String, Option<String>, DateTime<Utc>, i32, Option<DateTime<Utc>>)> =
        sqlx::query_as(
            "SELECT id, code, label, expires_at, uses, last_used_at FROM scanner_pairing_code
         WHERE event_id = $1 AND revoked_at IS NULL AND expires_at > now()
         ORDER BY created_at DESC",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let codes: Vec<Value> = rows
        .into_iter()
        .map(|(id, code, label, expires_at, uses, last_used_at)| {
            json!({ "id": id, "code": code, "label": label, "expiresAt": expires_at, "uses": uses, "lastUsedAt": last_used_at })
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
