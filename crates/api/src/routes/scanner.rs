//! Door devices: pair → snapshot → sync (push + pull), repeated.
//!
//! The protocol is built for bad venue wifi:
//! * every row carries a device-generated UUID, so resending a batch after a
//!   timeout is harmless (`ON CONFLICT DO NOTHING`), and the response lists
//!   every id it now holds — including ones it already had;
//! * the server assigns `seq`; a device pulls "everything after my cursor",
//!   so it never misses a check-in from another door, however late it syncs;
//! * the device's clock is not trusted blindly: each sync measures the offset
//!   and corrects `occurredAt` by it.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Duration, Utc};
use dwi_domain::normalize_invite_code;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{Device, Viewer, new_device_token};
use crate::error::{ApiError, ApiResult};
use crate::routes::events::{GUEST_SELECT, GuestRow, load_event};
use crate::state::AppState;
use crate::views::{EventInfo, guest_admits, guest_category};

const MAX_BATCH: usize = 500;
const PULL_LIMIT: i64 = 1000;
const SNAPSHOT_CACHE_SECS: u64 = 15;

// -------------------------------------------------------------------- pair

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairBody {
    code: String,
    device_name: String,
}

pub async fn pair(State(state): State<AppState>, Json(body): Json<PairBody>) -> ApiResult<Json<Value>> {
    let code = body.code.trim().to_uppercase().replace(' ', "");
    let found: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "UPDATE scanner_pairing_code SET uses = uses + 1, last_used_at = now()
         WHERE code = $1 AND revoked_at IS NULL AND expires_at > now()
         RETURNING id, event_id, gate",
    )
    .bind(&code)
    .fetch_optional(&state.db)
    .await?;
    let Some((code_id, event_id, gate)) = found else {
        return Err(ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "invalid_pairing_code",
            "Kode pairing salah atau sudah kedaluwarsa. Minta kode baru ke admin acara.",
        ));
    };

    // The gate is set from the dashboard on the pairing code; it wins over the
    // device-local door pick so check-ins are credited to the right entrance.
    let device_name = if gate.trim().is_empty() {
        body.device_name.trim().to_string()
    } else {
        gate.trim().chars().take(60).collect()
    };
    if device_name.is_empty() || device_name.chars().count() > 60 {
        return Err(ApiError::bad_request(
            "invalid_device_name",
            "Nama perangkat wajib diisi (maks. 60 karakter).",
        ));
    }

    let (token, hash) = new_device_token();
    let device_id: Uuid = sqlx::query_scalar(
        "INSERT INTO scanner_device (event_id, pairing_code_id, name, token_hash, last_seen_at)
         VALUES ($1, $2, $3, $4, now()) RETURNING id",
    )
    .bind(event_id)
    .bind(code_id)
    .bind(&device_name)
    .bind(hash)
    .fetch_one(&state.db)
    .await?;

    let event = load_event(&state, event_id).await?;
    tracing::info!(%event_id, %device_id, device = device_name, "device paired");
    Ok(Json(json!({
        "event": EventInfo::from(&event),
        "deviceToken": token,
        "deviceId": device_id,
        "deviceName": device_name,
    })))
}

// ---------------------------------------------------------------- snapshot

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SnapshotGuest {
    code: String,
    household_name: String,
    category: String,
    attending: Option<bool>,
    admits: i32,
    table: Option<String>,
    vip: bool,
    /// Guest added at the scanner door (not on the invite list).
    #[serde(default)]
    is_walk_in: bool,
    /// Group (kelompok) invitation — no per-person pass; usher records a member name.
    #[serde(default)]
    group: bool,
    /// This group's full headcount cap (so the door knows it's N people).
    #[serde(default)]
    group_cap: i32,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct RemoteCheckIn {
    id: Uuid,
    code: String,
    headcount: i32,
    occurred_at: DateTime<Utc>,
    device_name: String,
    #[serde(rename = "override")]
    is_override: bool,
    #[serde(skip)]
    seq: i64,
    #[serde(skip)]
    device_id: Uuid,
}

const REMOTE_CHECK_INS: &str = "
    SELECT s.id, s.guest_code AS code, COALESCE(s.headcount, 1) AS headcount, s.occurred_at,
           d.name AS device_name, s.override AS is_override, s.seq, s.device_id
    FROM scanner_event s JOIN scanner_device d ON d.id = s.device_id
    WHERE s.event_id = $1 AND s.kind = 'CHECK_IN' AND s.seq > $2
    ORDER BY s.seq LIMIT $3";

async fn snapshot_guests(state: &AppState, event_id: Uuid) -> ApiResult<Vec<SnapshotGuest>> {
    let key = format!("snapshot:{event_id}");
    if let Some(r) = &state.redis
        && let Some(cached) = r.get_json::<Vec<SnapshotGuest>>(&key).await
    {
        return Ok(cached);
    }
    let rows = sqlx::query_as::<_, GuestRow>(GUEST_SELECT).bind(event_id).fetch_all(&state.db).await?;
    let guests: Vec<SnapshotGuest> = rows
        .into_iter()
        .map(|g| {
            let category = guest_category(&g.code, g.category.as_deref());
            SnapshotGuest {
                admits: guest_admits(g.attending, g.rsvp_count, g.plus_allowance),
                category: category.as_str().to_string(),
                vip: category == dwi_domain::CategoryCode::Vip,
                code: g.code,
                household_name: g.household_name,
                attending: g.attending,
                table: None,
                group: g.pass_mode == "GROUP",
                group_cap: g.plus_allowance + 1,
                is_walk_in: g.is_walk_in,
            }
        })
        .collect();
    if let Some(r) = &state.redis {
        r.set_json(&key, &guests, SNAPSHOT_CACHE_SECS).await;
    }
    Ok(guests)
}

pub async fn snapshot(
    State(state): State<AppState>,
    device: Device,
    Path(event_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    device.require_event(event_id)?;
    let guests = snapshot_guests(&state, event_id).await?;
    let remote = sqlx::query_as::<_, RemoteCheckIn>(REMOTE_CHECK_INS)
        .bind(event_id)
        .bind(0_i64)
        .bind(i64::MAX)
        .fetch_all(&state.db)
        .await?;
    let cursor = remote.last().map_or(0, |r| r.seq);
    Ok(Json(json!({ "guests": guests, "remoteCheckIns": remote, "cursor": cursor, "at": Utc::now() })))
}

// -------------------------------------------------------------------- sync

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncBody {
    /// The device's clock when it sent this request.
    device_now: Option<DateTime<Utc>>,
    /// Highest `seq` this device has already seen.
    #[serde(default)]
    cursor: i64,
    #[serde(default)]
    check_ins: Vec<CheckInIn>,
    #[serde(default)]
    records: Vec<RecordIn>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckInIn {
    id: Uuid,
    code: String,
    headcount: i32,
    occurred_at: DateTime<Utc>,
    #[serde(default, rename = "override")]
    is_override: bool,
    /// For GROUP invitations — which member arrived (usher-typed).
    #[serde(default, rename = "memberName")]
    member_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordIn {
    id: Uuid,
    check_in_id: Uuid,
    code: String,
    #[serde(rename = "type")]
    kind: String,
    occurred_at: DateTime<Utc>,
    #[serde(default)]
    data: Value,
}

/// What dashboards and other doors receive on the live stream.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveScan {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: Uuid,
    pub seq: i64,
    pub activity: String,
    pub code: String,
    pub household_name: Option<String>,
    pub headcount: Option<i32>,
    pub occurred_at: DateTime<Utc>,
    pub device_name: String,
    #[serde(rename = "override")]
    pub is_override: bool,
    /// GROUP check-in: which member arrived (usher-typed name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_name: Option<String>,
    /// The guest's DB category (e.g. "vip") — the TV shows a badge when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

pub async fn sync(
    State(state): State<AppState>,
    device: Device,
    Json(body): Json<SyncBody>,
) -> ApiResult<Json<Value>> {
    if body.check_ins.len() + body.records.len() > MAX_BATCH {
        return Err(ApiError::bad_request(
            "batch_too_large",
            format!("Maksimal {MAX_BATCH} data per sinkronisasi."),
        ));
    }
    let now = Utc::now();
    // Positive when the device clock is behind. Only trust it if the request
    // carried a timestamp; network latency makes it approximate (± seconds).
    let offset = body.device_now.map(|d| now - d).unwrap_or_else(Duration::zero);
    let fix = |t: DateTime<Utc>| (t + offset).min(now);

    let mut acked: Vec<Uuid> = Vec::new();
    let mut rejected: Vec<Value> = Vec::new();
    let mut fresh: Vec<LiveScan> = Vec::new();

    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE scanner_device SET clock_offset_ms = $1 WHERE id = $2")
        .bind(offset.num_milliseconds())
        .bind(device.id)
        .execute(&mut *tx)
        .await?;

    for c in &body.check_ins {
        let code = normalize_invite_code(&c.code);
        if code.is_empty() || !(1..=50).contains(&c.headcount) {
            rejected.push(json!({ "id": c.id, "reason": "invalid" }));
            continue;
        }
        let guest: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
            r#"SELECT "id", "householdName", "category" FROM "guest" WHERE "eventId" = $1 AND "token" = $2"#,
        )
        .bind(device.event_id)
        .bind(&code)
        .fetch_optional(&mut *tx)
        .await?;
        let occurred = fix(c.occurred_at);
        let name = c.member_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO scanner_event (id, event_id, device_id, kind, guest_code, guest_id, headcount, override, occurred_at)
             VALUES ($1, $2, $3, 'CHECK_IN', $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO NOTHING RETURNING seq",
        )
        .bind(c.id)
        .bind(device.event_id)
        .bind(device.id)
        .bind(&code)
        .bind(guest.as_ref().map(|g| g.0))
        .bind(c.headcount)
        .bind(c.is_override)
        .bind(occurred)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(seq) = inserted {
            // Mirror into the old app's table so its admin keeps showing
            // check-ins during the transition. Group member name rides along.
            if let Some((guest_id, _, _)) = &guest {
                sqlx::query(
                    r#"INSERT INTO "check_in" ("id", "guestId", "scannedAt", "scannedBy", "guestCount", "memberName")
                       VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT ("id") DO NOTHING"#,
                )
                .bind(c.id)
                .bind(guest_id)
                .bind(occurred.naive_utc())
                .bind(&device.name)
                .bind(c.headcount)
                .bind(&name)
                .execute(&mut *tx)
                .await?;
            }
            fresh.push(LiveScan {
                kind: "scan",
                id: c.id,
                seq,
                activity: "CHECK_IN".into(),
                code: code.clone(),
                household_name: guest.as_ref().map(|g| g.1.clone()),
                headcount: Some(c.headcount),
                occurred_at: occurred,
                device_name: device.name.clone(),
                is_override: c.is_override,
                member_name: name.clone(),
                category: guest.as_ref().and_then(|g| g.2.clone()),
            });
        }
        acked.push(c.id);
    }

    for r in &body.records {
        let kind = match r.kind.as_str() {
            "PHOTO" | "GUESTBOOK" | "SOUVENIR" | "GREET" => r.kind.as_str(),
            _ => {
                rejected.push(json!({ "id": r.id, "reason": "unknown_type" }));
                continue;
            }
        };
        let code = normalize_invite_code(&r.code);
        let guest: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
            r#"SELECT "id", "householdName", "category" FROM "guest" WHERE "eventId" = $1 AND "token" = $2"#,
        )
        .bind(device.event_id)
        .bind(&code)
        .fetch_optional(&mut *tx)
        .await?;
        let member_name: Option<String> = if kind == "GREET" {
            sqlx::query_scalar(r#"SELECT NULLIF(trim("memberName"), '') FROM "check_in" WHERE "id" = $1"#)
                .bind(r.check_in_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten()
        } else {
            None
        };
        let occurred = fix(r.occurred_at);
        let data = if r.data.is_object() { r.data.clone() } else { json!({}) };
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO scanner_event (id, event_id, device_id, kind, guest_code, guest_id, check_in_id, data, occurred_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO NOTHING RETURNING seq",
        )
        .bind(r.id)
        .bind(device.event_id)
        .bind(device.id)
        .bind(kind)
        .bind(&code)
        .bind(guest.as_ref().map(|g| g.0))
        .bind(r.check_in_id)
        .bind(&data)
        .bind(occurred)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(seq) = inserted {
            fresh.push(LiveScan {
                kind: "scan",
                id: r.id,
                seq,
                activity: kind.to_string(),
                code,
                household_name: guest.as_ref().map(|g| g.1.clone()),
                headcount: None,
                occurred_at: occurred,
                device_name: device.name.clone(),
                is_override: false,
                member_name,
                category: guest.as_ref().and_then(|g| g.2.clone()),
            });
        }
        acked.push(r.id);
    }
    tx.commit().await?;

    // Only after commit: listeners must never see a scan that rolled back.
    for scan in &fresh {
        if let Ok(payload) = serde_json::to_string(scan) {
            state.live.publish(device.event_id, payload).await;
        }
    }

    // Pull: check-ins from other doors this device has not seen yet.
    let pulled = sqlx::query_as::<_, RemoteCheckIn>(REMOTE_CHECK_INS)
        .bind(device.event_id)
        .bind(body.cursor)
        .bind(PULL_LIMIT)
        .fetch_all(&state.db)
        .await?;
    let has_more = pulled.len() as i64 == PULL_LIMIT;
    let cursor = pulled.last().map_or(body.cursor, |r| r.seq);
    let remote: Vec<&RemoteCheckIn> = pulled.iter().filter(|r| r.device_id != device.id).collect();

    Ok(Json(json!({
        "acked": acked,
        "rejected": rejected,
        "remoteCheckIns": remote,
        "cursor": cursor,
        "hasMore": has_more,
        "serverNow": now,
    })))
}

// ----------------------------------------------------------------- greetings

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GreetingsQuery {
    #[serde(default)]
    after: i64,
    limit: Option<i64>,
}

/// Durable greeting queue for a TV greeter: every `GREET` event since `after`
/// (a `scanner_event.seq`), oldest first. The scanner emits `GREET` strictly
/// after the guest finishes the photo step (taps "Pakai foto" to confirm their
/// picture, or skips when photo isn't taken), so the TV never greets while the
/// guest is still posing or reviewing. By default the TV renders name-only;
/// `selfie` rides along when a photo exists in case photo display is enabled.
pub async fn greetings(
    State(state): State<AppState>,
    viewer: Viewer,
    Path(event_id): Path<Uuid>,
    Query(q): Query<GreetingsQuery>,
) -> ApiResult<Json<Value>> {
    viewer.require_event(&state, event_id).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // Member name (usher-typed on a group QR) wins over the household name.
    let rows: Vec<(i64, Option<String>, Option<String>, Option<String>, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT s.seq,
               s.guest_code AS code,
               COALESCE(NULLIF(trim(ci."memberName"), ''), g."householdName") AS name,
               p.object_key AS photo_key,
               s.occurred_at,
               g."category" AS category
        FROM scanner_event s
        LEFT JOIN "guest" g ON g."id" = s.guest_id
        LEFT JOIN "check_in" ci ON ci."id" = s.check_in_id
        LEFT JOIN LATERAL (
            SELECT ph.object_key
            FROM scanner_photo ph
            WHERE ph.event_id = s.event_id
              AND ph.hidden_at IS NULL
              AND ph.check_in_id = s.check_in_id
            ORDER BY ph.occurred_at DESC
            LIMIT 1
        ) p ON true
        WHERE s.event_id = $1 AND s.kind = 'GREET' AND s.seq > $2
        ORDER BY s.seq
        LIMIT $3
        "#,
    )
    .bind(event_id)
    .bind(q.after)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let selfie_url = |key: Option<String>| -> Option<String> {
        match (state.storage.as_ref(), key) {
            (Some(st), Some(k)) if !k.is_empty() => Some(st.read_url(&k)),
            _ => None,
        }
    };

    let greetings: Vec<Value> = rows
        .into_iter()
        .filter_map(|(seq, code, name, photo, occurred_at, category)| {
            let name = name?.trim().to_string();
            if name.is_empty() || seq <= q.after {
                return None;
            }
            Some(json!({
                "seq": seq,
                "code": code,
                "name": name,
                "selfie": selfie_url(photo),
                "occurredAt": occurred_at,
                "category": category,
            }))
        })
        .collect();
    let cursor = greetings.last().and_then(|g| g["seq"].as_i64()).unwrap_or(q.after);
    Ok(Json(json!({ "greetings": greetings, "cursor": cursor })))
}

// ------------------------------------------------------------- walk-ins

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkInInput {
    household_name: String,
    /// Optional guest category ("VIP" / "GEN" ...). Defaults to the generic one.
    category: Option<String>,
    /// How many people this walk-in brings. Default 1.
    #[serde(default = "default_headcount")]
    headcount: i32,
}

fn default_headcount() -> i32 {
    1
}

/// Add a guest right at the scanner door (walk-in) and make them check-in-able.
/// Flagged `isWalkIn` so the team can tell newly-added guests from invited ones;
/// the category (e.g. VIP) can be chosen at the door.
pub async fn add_walkin_guest(
    State(state): State<AppState>,
    device: Device,
    Path(event_id): Path<Uuid>,
    Json(body): Json<WalkInInput>,
) -> ApiResult<Json<Value>> {
    device.require_event(event_id)?;
    let name = body.household_name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("name_required", "Nama wajib diisi."));
    }
    let headcount = body.headcount.clamp(1, 50);
    let cat = dwi_domain::to_category_code(body.category.as_deref());

    let next_seq: i32 = sqlx::query_scalar(
        r#"SELECT COALESCE(MAX("sequence"), 0) + 1 FROM "guest" WHERE "eventId" = $1"#,
    )
    .bind(event_id)
    .fetch_one(&state.db)
    .await?;
    let prefix: String = sqlx::query_scalar(r#"SELECT "codePrefix" FROM "event" WHERE "id" = $1"#)
        .bind(event_id)
        .fetch_one(&state.db)
        .await?;
    let token = dwi_domain::build_invite_code(&prefix, cat, next_seq as u32);
    let guest_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO "guest" ("id", "eventId", "householdName", "token", "plusGuestAllowance",
            "category", "sequence", "isWalkIn", "createdAt", "updatedAt")
           VALUES ($1, $2, $3, $4, 0, $5, $6, true, now(), now())"#,
    )
    .bind(guest_id)
    .bind(event_id)
    .bind(&name)
    .bind(&token)
    .bind(cat.label())
    .bind(next_seq)
    .execute(&state.db)
    .await?;

    sqlx::query(
        r#"INSERT INTO "rsvp" ("id", "guestId", "attending", "guestCount", "submittedAt", "updatedAt")
           VALUES ($1, $2, true, $3, now(), now())"#,
    )
    .bind(Uuid::new_v4())
    .bind(guest_id)
    .bind(headcount)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({
        "id": guest_id,
        "code": token,
        "householdName": name,
        "category": cat.as_str(),
        "isWalkIn": true,
    })))
}
