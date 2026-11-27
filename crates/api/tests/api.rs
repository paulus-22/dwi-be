//! End-to-end tests against real Postgres (a fresh database per test, created
//! by `sqlx::test` from DATABASE_URL) and, where noted, real Redis.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use dwi_api::config::{Config, StorageConfig};
use dwi_api::redis_ext::Redis;
use dwi_api::{AppState, db, routes, seed};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "demo-password-123";

fn config(storage: bool) -> Config {
    Config {
        database_url: String::new(),
        redis_url: None,
        jwt_secret: "test-jwt-secret-that-is-long-enough-123".into(),
        invite_signing_secret: "golden-vector-secret".into(),
        bind_addr: "127.0.0.1:0".into(),
        cors_origins: "http://localhost:3000".into(),
        session_hours: 12,
        public_site_url: "http://localhost:3000".into(),
        storage: StorageConfig {
            s3_endpoint: storage.then(|| "https://acct.r2.cloudflarestorage.com".into()),
            s3_bucket: storage.then(|| "dwi-photos".into()),
            s3_region: "auto".into(),
            s3_access_key_id: storage.then(|| "AKIDEXAMPLE".into()),
            s3_secret_access_key: storage.then(|| "secret".into()),
            s3_public_base_url: None,
        },
    }
}

struct App {
    router: Router,
    state: AppState,
    demo: seed::Demo,
}

async fn app_with(pool: PgPool, storage: bool, redis: Option<Redis>) -> App {
    db::migrate(&pool).await.unwrap();
    let demo = seed::seed_demo(&pool, "admin@demo.local", PASSWORD).await.unwrap();
    let state = AppState::new(&config(storage), pool, redis).unwrap();
    state.live.spawn_redis_relay();
    App { router: routes::router(state.clone(), &["http://localhost:3000".into()]), state, demo }
}

async fn app(pool: PgPool) -> App {
    app_with(pool, false, None).await
}

impl App {
    async fn call(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn login(&self) -> String {
        let (s, b) = self
            .call(
                "POST",
                "/v1/auth/login",
                None,
                Some(json!({ "email": "Admin@Demo.local", "password": PASSWORD })),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        b["token"].as_str().unwrap().to_string()
    }

    async fn pair(&self, admin: &str, name: &str) -> String {
        let ev = self.demo.event_id;
        let (s, b) =
            self.call("POST", &format!("/v1/events/{ev}/pairing-codes"), Some(admin), Some(json!({}))).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let code = b["code"].as_str().unwrap().to_lowercase(); // typed sloppily at the door
        let (s, b) = self
            .call("POST", "/v1/devices/pair", None, Some(json!({ "code": code, "deviceName": name })))
            .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(b["event"]["coupleLine"], "Ajeng & Alex");
        assert_eq!(b["event"]["dateLabel"], "Sabtu, 15 Mei 2027");
        b["deviceToken"].as_str().unwrap().to_string()
    }
}

fn check_in(code: &str, headcount: i32, at: &str) -> Value {
    json!({ "id": Uuid::new_v4(), "code": code, "headcount": headcount, "occurredAt": at,
            "deviceName": "ignored", "override": false, "synced": false })
}

// --------------------------------------------------------------- creation

#[sqlx::test(migrations = false)]
async fn creates_edits_and_archives_events(pool: PgPool) {
    let app = app(pool.clone()).await;
    let token = app.login().await;

    let (s, b) = app
        .call(
            "POST",
            "/v1/events",
            Some(&token),
            Some(json!({
                "kind": "GENERIC", "slug": "conf-2027", "title": "Konferensi 2027",
                "subtitle": "Dibuat lewat dashboard", "headlineDate": "2027-03-10T09:00:00Z",
                "timezone": "Asia/Jakarta",
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["event"]["slug"], "conf-2027");
    assert_eq!(b["event"]["kind"], "GENERIC");
    assert_eq!(b["event"]["status"], "DRAFT");
    assert_eq!(b["event"]["role"], "OWNER");
    assert_eq!(b["event"]["displayName"], "Konferensi 2027");

    // It shows up in the list with the creator as OWNER.
    let (s, b) = app.call("GET", "/v1/events", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["events"].as_array().unwrap().iter().any(|e| e["slug"] == "conf-2027" && e["role"] == "OWNER"));

    // Duplicate slug -> 409.
    let (s, b) = app
        .call(
            "POST",
            "/v1/events",
            Some(&token),
            Some(json!({ "kind": "GENERIC", "slug": "conf-2027", "title": "Lagi", "headlineDate": "2027-03-10T09:00:00Z" })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert_eq!(b["error"]["code"], "slug_taken");

    // Bad slug -> 400.
    let (s, b) = app
        .call(
            "POST",
            "/v1/events",
            Some(&token),
            Some(json!({ "kind": "GENERIC", "slug": "Bad Slug", "title": "x", "headlineDate": "2027-03-10T09:00:00Z" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["error"]["code"], "invalid_slug");

    // WEDDING is supported, but requires the couple's names.
    let (s, b) = app
        .call(
            "POST",
            "/v1/events",
            Some(&token),
            Some(json!({ "kind": "WEDDING", "slug": "should-fail", "headlineDate": "2027-03-10T09:00:00Z" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["error"]["code"], "missing_names");

    // A full WEDDING creation succeeds and round-trips the names.
    let (s, b) = app
        .call(
            "POST",
            "/v1/events",
            Some(&token),
            Some(json!({
                "kind": "WEDDING", "slug": "browser-wedding-2027",
                "brideShortName": "Ani", "brideFullName": "Ani Suryani",
                "groomShortName": "Budi", "groomFullName": "Budi Hartono",
                "headlineDate": "2027-06-01T09:00:00Z", "defaultLocale": "ID", "bilingual": true,
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["event"]["kind"], "WEDDING");
    assert_eq!(b["event"]["coupleLine"], "Ani & Budi");
    let wedding_id = b["event"]["id"].as_str().unwrap().to_string();

    // PUT edits top-level fields; GET reflects the change.
    let (s, b) = app
        .call(
            "PUT",
            &format!("/v1/events/{wedding_id}"),
            Some(&token),
            Some(json!({
                "slug": "browser-wedding-2027", "status": "PUBLISHED",
                "brideShortName": "Ani", "brideFullName": "Ani Suryani",
                "groomShortName": "Budi Edited", "groomFullName": "Budi Hartono",
                "headlineDate": "2027-06-01T09:00:00Z",
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["event"]["coupleLine"], "Ani & Budi Edited");
    assert_eq!(b["event"]["status"], "PUBLISHED");

    // DELETE archives it rather than removing the row.
    let (s, b) = app.call("DELETE", &format!("/v1/events/{wedding_id}"), Some(&token), None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (s, b) = app.call("GET", &format!("/v1/events/{wedding_id}"), Some(&token), None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["event"]["status"], "ARCHIVED");
}

// ------------------------------------------------------------------ auth

#[sqlx::test(migrations = false)]
async fn login_and_roles(pool: PgPool) {
    let app = app(pool.clone()).await;
    let (s, b) = app
        .call(
            "POST",
            "/v1/auth/login",
            None,
            Some(json!({ "email": "admin@demo.local", "password": "nope" })),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b["error"]["code"], "invalid_credentials");
    let (s, _) = app
        .call("POST", "/v1/auth/login", None, Some(json!({ "email": "ghost@demo.local", "password": "x" })))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let token = app.login().await;
    let (s, b) = app.call("GET", "/v1/me", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["admin"]["isSuperAdmin"], true);

    let (s, b) = app.call("GET", "/v1/events", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["events"][0]["slug"], "alex-ajeng");
    assert_eq!(b["events"][0]["role"], "OWNER");

    // A VIEWER member: can read attendance, cannot create pairing codes,
    // cannot see an event they are not a member of.
    let viewer_hash = dwi_api::routes::auth::hash_password("viewer-password-1").unwrap();
    let viewer: Uuid = sqlx::query_scalar(
        r#"INSERT INTO "admin_user" ("id","email","name","passwordHash","updatedAt")
           VALUES (gen_random_uuid(),'viewer@demo.local','Viewer',$1,now()) RETURNING "id""#,
    )
    .bind(viewer_hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(r#"INSERT INTO "event_membership" ("id","adminId","eventId","role") VALUES (gen_random_uuid(),$1,$2,'VIEWER')"#)
        .bind(viewer)
        .bind(app.demo.event_id)
        .execute(&pool)
        .await
        .unwrap();
    let (_, b) = app
        .call(
            "POST",
            "/v1/auth/login",
            None,
            Some(json!({ "email": "viewer@demo.local", "password": "viewer-password-1" })),
        )
        .await;
    let vt = b["token"].as_str().unwrap().to_string();
    let ev = app.demo.event_id;
    assert_eq!(
        app.call("GET", &format!("/v1/events/{ev}/attendance"), Some(&vt), None).await.0,
        StatusCode::OK
    );
    let (s, b) =
        app.call("POST", &format!("/v1/events/{ev}/pairing-codes"), Some(&vt), Some(json!({}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
    let other = Uuid::new_v4();
    assert_eq!(
        app.call("GET", &format!("/v1/events/{other}"), Some(&vt), None).await.0,
        StatusCode::NOT_FOUND
    );

    // Disabling the account kills the live session immediately.
    sqlx::query(r#"UPDATE "admin_user" SET "isActive" = false WHERE "id" = $1"#)
        .bind(viewer)
        .execute(&pool)
        .await
        .unwrap();
    let (s, b) = app.call("GET", "/v1/me", Some(&vt), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b["error"]["code"], "account_disabled");

    // Garbage and missing tokens.
    assert_eq!(app.call("GET", "/v1/me", Some("not.a.jwt"), None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(app.call("GET", "/v1/me", None, None).await.0, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = false)]
async fn temporary_password_must_be_changed_first(pool: PgPool) {
    let app = app(pool.clone()).await;
    sqlx::query(r#"UPDATE "admin_user" SET "mustChangePassword" = true"#).execute(&pool).await.unwrap();
    let token = app.login().await;
    let (s, b) = app.call("GET", "/v1/events", Some(&token), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(b["error"]["code"], "must_change_password");
    let (s, _) = app
        .call(
            "POST",
            "/v1/auth/password",
            Some(&token),
            Some(json!({ "current": PASSWORD, "next": "short" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, b) = app
        .call(
            "POST",
            "/v1/auth/password",
            Some(&token),
            Some(json!({ "current": PASSWORD, "next": "a-much-better-password" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(app.call("GET", "/v1/events", Some(&token), None).await.0, StatusCode::OK);
}

// --------------------------------------------------------------- scanner

#[sqlx::test(migrations = false)]
async fn two_doors_sync_idempotently_and_see_each_other(pool: PgPool) {
    let app = app(pool.clone()).await;
    let admin = app.login().await;
    let ev = app.demo.event_id;
    let codes = &app.demo.codes;
    let door_a = app.pair(&admin, "Pintu Utara").await;
    let door_b = app.pair(&admin, "Pintu Selatan").await;

    // Device tokens are not admin tokens.
    let (s, _) = app.call("GET", "/v1/events", Some(&door_a), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, snap) = app.call("GET", &format!("/v1/events/{ev}/snapshot"), Some(&door_a), None).await;
    assert_eq!(s, StatusCode::OK, "{snap}");
    let guests = snap["guests"].as_array().unwrap();
    assert_eq!(guests.len(), 8);
    assert_eq!(guests[0]["code"], codes[0]);
    assert_eq!(guests[0]["admits"], 4); // RSVP'd 4
    assert_eq!(guests[3]["attending"], Value::Null);
    assert_eq!(guests[3]["admits"], 1); // no RSVP, no plus-ones
    assert_eq!(guests[5]["vip"], true);
    assert_eq!(snap["cursor"], 0);

    // Live listener for the event (what a dashboard would get).
    let mut live = app.state.live.subscribe(ev);

    // Door A checks in guest 0 plus a souvenir record. Its clock is 2 minutes behind.
    let now = chrono::Utc::now();
    let device_now = (now - chrono::Duration::minutes(2)).to_rfc3339();
    let at = (now - chrono::Duration::minutes(3)).to_rfc3339(); // = 1 min ago in server time
    let ci = check_in(&codes[0], 4, &at);
    let rec = json!({ "id": Uuid::new_v4(), "checkInId": ci["id"], "code": codes[0], "type": "SOUVENIR",
                      "occurredAt": at, "data": { "given": true }, "synced": false });
    let batch = json!({ "deviceNow": device_now, "cursor": 0, "checkIns": [ci], "records": [rec] });
    let (s, b) = app.call("POST", "/v1/sync", Some(&door_a), Some(batch.clone())).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["acked"].as_array().unwrap().len(), 2);
    assert_eq!(b["remoteCheckIns"].as_array().unwrap().len(), 0, "own scans are not echoed back");

    let msg: Value = serde_json::from_str(&live.recv().await.unwrap()).unwrap();
    assert_eq!(msg["activity"], "CHECK_IN");
    assert_eq!(msg["householdName"], "Keluarga Bpk. Hartono");
    assert_eq!(msg["deviceName"], "Pintu Utara");
    // Clock correction: stored time is ~1 minute ago, not ~3.
    let stored: chrono::DateTime<chrono::Utc> = msg["occurredAt"].as_str().unwrap().parse().unwrap();
    let skew = (now - stored).num_seconds();
    assert!((50..=70).contains(&skew), "clock-corrected occurredAt should be ~60s ago, was {skew}s");
    let _souvenir = live.recv().await.unwrap();

    // Network dropped the response; the device resends the same batch.
    let (s, b) = app.call("POST", "/v1/sync", Some(&door_a), Some(batch)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["acked"].as_array().unwrap().len(), 2, "replay is acknowledged");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM scanner_event").fetch_one(&pool).await.unwrap();
    assert_eq!(rows, 2, "replay is not stored twice");
    assert!(live.try_recv().is_err(), "replay is not re-broadcast");

    // Door B pulls and learns guest 0 is already in.
    let (_, b) = app.call("POST", "/v1/sync", Some(&door_b), Some(json!({ "cursor": 0 }))).await;
    let remote = b["remoteCheckIns"].as_array().unwrap();
    assert_eq!(remote.len(), 1);
    assert_eq!(remote[0]["code"], codes[0]);
    assert_eq!(remote[0]["deviceName"], "Pintu Utara");
    let cursor_b = b["cursor"].as_i64().unwrap();
    assert!(cursor_b > 0);
    // Pulling again from the new cursor returns nothing.
    let (_, b) = app.call("POST", "/v1/sync", Some(&door_b), Some(json!({ "cursor": cursor_b }))).await;
    assert_eq!(b["remoteCheckIns"].as_array().unwrap().len(), 0);

    // Door B was offline earlier and scanned guest 0 *before* door A did,
    // plus guest 2. Both rows are kept; the earliest one counts.
    let earlier = (now - chrono::Duration::minutes(10)).to_rfc3339();
    let (_, b) = app
        .call(
            "POST",
            "/v1/sync",
            Some(&door_b),
            Some(json!({
                "deviceNow": now.to_rfc3339(), "cursor": cursor_b,
                "checkIns": [check_in(&codes[0], 3, &earlier), check_in(&codes[2], 2, &earlier)],
            })),
        )
        .await;
    assert_eq!(b["acked"].as_array().unwrap().len(), 2);

    let (s, att) = app.call("GET", &format!("/v1/events/{ev}/attendance"), Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK, "{att}");
    assert_eq!(att["households"], 8);
    assert_eq!(att["attendingHouseholds"], 5);
    assert_eq!(att["expectedPeople"], 16);
    assert_eq!(att["checkedInHouseholds"], 2);
    assert_eq!(
        att["checkedInPeople"],
        3 + 2,
        "guest 0 counted once, from the earliest scan (door B, 3 people)"
    );
    assert_eq!(att["repeatScans"], 1);

    let (_, g) = app.call("GET", &format!("/v1/events/{ev}/guests"), Some(&admin), None).await;
    assert_eq!(g["guests"][0]["checkedInHeadcount"], 3);
    assert_eq!(g["guests"][1]["checkedInAt"], Value::Null);

    // Mirrored into the old app's table for the transition period.
    let legacy: i64 =
        sqlx::query_scalar(r#"SELECT count(*) FROM "check_in""#).fetch_one(&pool).await.unwrap();
    assert_eq!(legacy, 3);

    // Invalid rows are rejected (and acknowledged as such), the rest still land.
    let (_, b) = app
        .call("POST", "/v1/sync", Some(&door_b), Some(json!({
            "checkIns": [check_in("", 1, &at), check_in(&codes[4], 99, &at), check_in(&codes[4], 1, &at)],
            "records": [{ "id": Uuid::new_v4(), "checkInId": Uuid::new_v4(), "code": codes[4], "type": "DANCE", "occurredAt": at }],
        })))
        .await;
    assert_eq!(b["acked"].as_array().unwrap().len(), 1);
    assert_eq!(b["rejected"].as_array().unwrap().len(), 3);

    // Revoking door B: its token stops working at once.
    let (_, devs) = app.call("GET", &format!("/v1/events/{ev}/devices"), Some(&admin), None).await;
    let b_id =
        devs["devices"].as_array().unwrap().iter().find(|d| d["name"] == "Pintu Selatan").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
    assert_eq!(devs["devices"][0]["checkIns"], 1);
    assert_eq!(
        app.call("DELETE", &format!("/v1/devices/{b_id}"), Some(&admin), None).await.0,
        StatusCode::OK
    );
    let (s, b) = app.call("POST", "/v1/sync", Some(&door_b), Some(json!({}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b["error"]["code"], "device_revoked");

    // A device cannot read another event's snapshot.
    let other = Uuid::new_v4();
    assert_eq!(
        app.call("GET", &format!("/v1/events/{other}/snapshot"), Some(&door_a), None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = false)]
async fn pairing_codes_expire_and_can_be_revoked(pool: PgPool) {
    let app = app(pool.clone()).await;
    let admin = app.login().await;
    let ev = app.demo.event_id;

    let (s, b) = app
        .call("POST", "/v1/devices/pair", None, Some(json!({ "code": "ZZZZ-ZZZZ", "deviceName": "x" })))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["error"]["code"], "invalid_pairing_code");

    let (_, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/pairing-codes"),
            Some(&admin),
            Some(json!({ "label": "Hari H" })),
        )
        .await;
    let (code, id) = (b["code"].as_str().unwrap().to_string(), b["id"].as_str().unwrap().to_string());
    assert!(code.len() == 9 && code.as_bytes()[4] == b'-');

    let (s, _) =
        app.call("POST", "/v1/devices/pair", None, Some(json!({ "code": code, "deviceName": "  " }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (_, list) = app.call("GET", &format!("/v1/events/{ev}/pairing-codes"), Some(&admin), None).await;
    assert_eq!(list["codes"][0]["label"], "Hari H");

    assert_eq!(
        app.call("DELETE", &format!("/v1/pairing-codes/{id}"), Some(&admin), None).await.0,
        StatusCode::OK
    );
    let (s, _) =
        app.call("POST", "/v1/devices/pair", None, Some(json!({ "code": code, "deviceName": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (_, b) =
        app.call("POST", &format!("/v1/events/{ev}/pairing-codes"), Some(&admin), Some(json!({}))).await;
    sqlx::query("UPDATE scanner_pairing_code SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let (s, _) = app
        .call("POST", "/v1/devices/pair", None, Some(json!({ "code": b["code"], "deviceName": "x" })))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- photos

#[sqlx::test(migrations = false)]
async fn photo_upload_flow(pool: PgPool) {
    let app = app_with(pool, true, None).await;
    let admin = app.login().await;
    let door = app.pair(&admin, "Photobooth").await;
    let ev = app.demo.event_id;
    let photo_id = Uuid::new_v4();

    let (s, _) = app
        .call(
            "POST",
            "/v1/uploads/presign",
            Some(&door),
            Some(json!({ "photoId": photo_id, "contentType": "image/gif" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, p) = app
        .call(
            "POST",
            "/v1/uploads/presign",
            Some(&door),
            Some(json!({ "photoId": photo_id, "contentType": "image/jpeg" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let key = p["key"].as_str().unwrap();
    assert_eq!(key, format!("events/{ev}/photos/{photo_id}.jpg"));
    let url = p["url"].as_str().unwrap();
    assert!(url.starts_with("https://acct.r2.cloudflarestorage.com/dwi-photos/events/"), "{url}");
    assert!(url.contains("X-Amz-Signature="));

    // A device may not register a key outside its own event.
    let (s, _) = app
        .call(
            "POST",
            "/v1/photos",
            Some(&door),
            Some(json!({
            "id": photo_id, "code": app.demo.codes[1], "key": "events/other/photos/x.jpg",
            "contentType": "image/jpeg", "occurredAt": chrono::Utc::now() })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let body = json!({ "id": photo_id, "code": app.demo.codes[1], "key": key, "contentType": "image/jpeg",
                       "occurredAt": chrono::Utc::now() });
    assert_eq!(app.call("POST", "/v1/photos", Some(&door), Some(body.clone())).await.0, StatusCode::OK);
    assert_eq!(app.call("POST", "/v1/photos", Some(&door), Some(body)).await.0, StatusCode::OK, "idempotent");

    for token in [&admin, &door] {
        let (s, list) = app.call("GET", &format!("/v1/events/{ev}/photos"), Some(token), None).await;
        assert_eq!(s, StatusCode::OK, "{list}");
        let photos = list["photos"].as_array().unwrap();
        assert_eq!(photos.len(), 1);
        assert_eq!(photos[0]["householdName"], "Ibu Ratna & Keluarga");
        assert!(photos[0]["url"].as_str().unwrap().contains("X-Amz-Signature="));
    }
}

#[sqlx::test(migrations = false)]
async fn photos_answer_503_without_storage(pool: PgPool) {
    let app = app(pool).await;
    let admin = app.login().await;
    let door = app.pair(&admin, "x").await;
    let (s, b) = app
        .call(
            "POST",
            "/v1/uploads/presign",
            Some(&door),
            Some(json!({ "photoId": Uuid::new_v4(), "contentType": "image/jpeg" })),
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(b["error"]["code"], "storage_not_configured");
}

// ------------------------------------------------------------ migrations

/// Production already has the Prisma schema. Migrating it must adopt the
/// baseline (not re-run it) and apply only the new scanner tables.
#[sqlx::test(migrations = false)]
async fn adopts_existing_prisma_database(pool: PgPool) {
    let baseline = include_str!("../../../migrations/0001_baseline.sql");
    sqlx::raw_sql(sqlx::AssertSqlSafe(baseline)).execute(&pool).await.unwrap();
    sqlx::query(r#"INSERT INTO "admin_user" ("id","email","name","passwordHash","updatedAt") VALUES (gen_random_uuid(),'keep@me','Keep','x',now())"#)
        .execute(&pool)
        .await
        .unwrap();

    db::migrate(&pool).await.unwrap();
    db::migrate(&pool).await.unwrap(); // and again: a no-op

    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&pool)
        .await
        .unwrap();
    let expected: Vec<i64> = db::MIGRATOR.iter().map(|m| m.version).collect();
    assert_eq!(versions, expected);
    assert_eq!(versions[..2], [1, 2]);
    let kept: i64 =
        sqlx::query_scalar(r#"SELECT count(*) FROM "admin_user""#).fetch_one(&pool).await.unwrap();
    assert_eq!(kept, 1, "existing data untouched");
    let has_scanner: bool = sqlx::query_scalar("SELECT to_regclass('public.scanner_event') IS NOT NULL")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(has_scanner);
}

#[sqlx::test(migrations = false)]
async fn refuses_to_adopt_an_outdated_prisma_database(pool: PgPool) {
    let baseline = include_str!("../../../migrations/0001_baseline.sql");
    sqlx::raw_sql(sqlx::AssertSqlSafe(baseline)).execute(&pool).await.unwrap();
    sqlx::query(r#"ALTER TABLE "event" DROP COLUMN "giftAddress""#).execute(&pool).await.unwrap();
    let err = db::migrate(&pool).await.unwrap_err().to_string();
    assert!(err.contains("prisma migrate deploy"), "{err}");
}

// ----------------------------------------------------------------- redis

fn redis_url() -> Option<String> {
    std::env::var("TEST_REDIS_URL").ok()
}

/// Two API instances share one Redis: a scan synced through instance A
/// reaches a live listener on instance B.
#[sqlx::test(migrations = false)]
async fn live_updates_cross_instances_through_redis(pool: PgPool) {
    let Some(url) = redis_url() else { return eprintln!("skipped: TEST_REDIS_URL not set") };
    let prefix = format!("test-{}", Uuid::new_v4());
    let a = app_with(pool.clone(), false, Some(Redis::connect(&url, &prefix).await.unwrap())).await;
    let b_state =
        AppState::new(&config(false), pool, Some(Redis::connect(&url, &prefix).await.unwrap())).unwrap();
    b_state.live.spawn_redis_relay();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await; // let both relays subscribe

    let mut on_b = b_state.live.subscribe(a.demo.event_id);
    let admin = a.login().await;
    let door = a.pair(&admin, "Pintu A").await;
    let (s, _) = a
        .call(
            "POST",
            "/v1/sync",
            Some(&door),
            Some(json!({ "checkIns": [check_in(&a.demo.codes[0], 2, &chrono::Utc::now().to_rfc3339())] })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let msg =
        tokio::time::timeout(std::time::Duration::from_secs(3), on_b.recv()).await.expect("relayed").unwrap();
    let msg: Value = serde_json::from_str(&msg).unwrap();
    assert_eq!(msg["deviceName"], "Pintu A");
}

#[sqlx::test(migrations = false)]
async fn login_is_rate_limited_with_redis(pool: PgPool) {
    let Some(url) = redis_url() else { return eprintln!("skipped: TEST_REDIS_URL not set") };
    let redis = Redis::connect(&url, &format!("test-{}", Uuid::new_v4())).await.unwrap();
    let app = app_with(pool, false, Some(redis)).await;
    let wrong = json!({ "email": "admin@demo.local", "password": "wrong" });
    for _ in 0..10 {
        assert_eq!(
            app.call("POST", "/v1/auth/login", None, Some(wrong.clone())).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (s, b) = app.call("POST", "/v1/auth/login", None, Some(wrong)).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(b["error"]["code"], "rate_limited");
}

#[sqlx::test(migrations = false)]
async fn snapshot_is_cached_in_redis(pool: PgPool) {
    let Some(url) = redis_url() else { return eprintln!("skipped: TEST_REDIS_URL not set") };
    let redis = Redis::connect(&url, &format!("test-{}", Uuid::new_v4())).await.unwrap();
    let app = app_with(pool.clone(), false, Some(redis.clone())).await;
    let admin = app.login().await;
    let door = app.pair(&admin, "x").await;
    let uri = format!("/v1/events/{}/snapshot", app.demo.event_id);
    let (_, first) = app.call("GET", &uri, Some(&door), None).await;
    assert!(redis.get_json::<Value>(&format!("snapshot:{}", app.demo.event_id)).await.is_some());
    // A guest renamed in the DB is not visible until the cache expires (15s).
    sqlx::query(r#"UPDATE "guest" SET "householdName" = 'Renamed'"#).execute(&pool).await.unwrap();
    let (_, second) = app.call("GET", &uri, Some(&door), None).await;
    assert_eq!(first["guests"], second["guests"]);
}

// ------------------------------------------------- forms, blocks, public RSVP

#[sqlx::test(migrations = false)]
async fn form_field_crud(pool: PgPool) {
    let app = app(pool).await;
    let t = app.login().await;
    let ev = app.demo.event_id;

    let (s, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/form-fields"),
            Some(&t),
            Some(json!({ "fieldKey": "dietary", "labelId": "Pantangan makanan", "labelEn": "Dietary needs",
                         "fieldType": "select", "options": [{ "value": "veg", "labelId": "Vegetarian" }],
                         "required": true, "sortOrder": 2 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["field"]["fieldType"], "SELECT");
    assert_eq!(b["field"]["options"][0]["value"], "veg");
    let dietary = b["field"]["id"].as_str().unwrap().to_string();

    let (s, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/form-fields"),
            Some(&t),
            Some(json!({ "fieldKey": "company", "labelId": "Perusahaan", "fieldType": "TEXT", "sortOrder": 1 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["field"]["required"], false);
    assert_eq!(b["field"]["options"], json!([]));

    // Duplicate key in the same event is a clean 409.
    let (s, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/form-fields"),
            Some(&t),
            Some(json!({ "fieldKey": "company", "labelId": "Lagi", "fieldType": "TEXT" })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert_eq!(b["error"]["code"], "duplicate_field_key");

    let (s, b) = app.call("GET", &format!("/v1/events/{ev}/form-fields"), Some(&t), None).await;
    assert_eq!(s, StatusCode::OK);
    let keys: Vec<&str> =
        b["fields"].as_array().unwrap().iter().map(|f| f["fieldKey"].as_str().unwrap()).collect();
    assert_eq!(keys, ["company", "dietary"]);

    let (s, b) = app
        .call(
            "PUT",
            &format!("/v1/form-fields/{dietary}"),
            Some(&t),
            Some(json!({ "fieldKey": "dietary", "labelId": "Alergi", "fieldType": "TEXTAREA", "sortOrder": 0 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["field"]["labelId"], "Alergi");
    assert_eq!(b["field"]["labelEn"], Value::Null);
    assert_eq!(b["field"]["fieldType"], "TEXTAREA");

    let (s, _) = app.call("DELETE", &format!("/v1/form-fields/{dietary}"), Some(&t), None).await;
    assert_eq!(s, StatusCode::OK);
    let (_, b) = app.call("GET", &format!("/v1/events/{ev}/form-fields"), Some(&t), None).await;
    assert_eq!(b["fields"].as_array().unwrap().len(), 1);
    let (s, _) = app.call("DELETE", &format!("/v1/form-fields/{dietary}"), Some(&t), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // No session, no form.
    let (s, _) = app.call("GET", &format!("/v1/events/{ev}/form-fields"), None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = false)]
async fn invalid_types_are_rejected(pool: PgPool) {
    let app = app(pool).await;
    let t = app.login().await;
    let ev = app.demo.event_id;
    let (s, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/form-fields"),
            Some(&t),
            Some(json!({ "fieldKey": "x", "labelId": "X", "fieldType": "SIGNATURE" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["error"]["code"], "invalid_field_type");

    let (s, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/page-blocks"),
            Some(&t),
            Some(json!({ "blockType": "CAROUSEL" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["error"]["code"], "invalid_block_type");
}

#[sqlx::test(migrations = false)]
async fn page_block_crud(pool: PgPool) {
    let app = app(pool).await;
    let t = app.login().await;
    let ev = app.demo.event_id;

    let (s, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/page-blocks"),
            Some(&t),
            Some(json!({ "blockType": "MAP", "config": { "lat": -6.3 }, "sortOrder": 1 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["block"]["isVisible"], true);
    let map = b["block"]["id"].as_str().unwrap().to_string();
    let (s, b) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/page-blocks"),
            Some(&t),
            Some(json!({ "blockType": "cover" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["block"]["blockType"], "COVER");
    assert_eq!(b["block"]["config"], json!({}));

    let (_, b) = app.call("GET", &format!("/v1/events/{ev}/page-blocks"), Some(&t), None).await;
    let types: Vec<&str> =
        b["blocks"].as_array().unwrap().iter().map(|x| x["blockType"].as_str().unwrap()).collect();
    assert_eq!(types, ["COVER", "MAP"]);

    let (s, b) = app
        .call(
            "PUT",
            &format!("/v1/page-blocks/{map}"),
            Some(&t),
            Some(
                json!({ "blockType": "MAP", "config": { "lat": -6.2 }, "sortOrder": 0, "isVisible": false }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["block"]["isVisible"], false);
    assert_eq!(b["block"]["config"]["lat"], -6.2);

    let (s, _) = app.call("DELETE", &format!("/v1/page-blocks/{map}"), Some(&t), None).await;
    assert_eq!(s, StatusCode::OK);
    let (_, b) = app.call("GET", &format!("/v1/events/{ev}/page-blocks"), Some(&t), None).await;
    assert_eq!(b["blocks"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = false)]
async fn public_rsvp_saves_dynamic_responses(pool: PgPool) {
    let app = app(pool.clone()).await;
    let t = app.login().await;
    let ev = app.demo.event_id;
    for (key, ty) in [("dietary", "TEXT"), ("shirt", "SELECT")] {
        let (s, b) = app
            .call(
                "POST",
                &format!("/v1/events/{ev}/form-fields"),
                Some(&t),
                Some(json!({ "fieldKey": key, "labelId": key, "fieldType": ty })),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{b}");
    }

    let (s, b) = app.call("GET", "/v1/public/events/alex-ajeng/form-fields", None, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["fields"].as_array().unwrap().len(), 2);
    assert_eq!(
        app.call("GET", "/v1/public/events/nope/form-fields", None, None).await.0,
        StatusCode::NOT_FOUND
    );

    // "Sinta Maharani": no RSVP yet, no plus-ones.
    let code = &app.demo.codes[3];
    let guest_id: Uuid = sqlx::query_scalar(
        r#"UPDATE "guest" SET "phone" = '081234567890' WHERE "token" = $1 RETURNING "id""#,
    )
    .bind(code)
    .fetch_one(&pool)
    .await
    .unwrap();
    let uri = format!("/v1/public/events/alex-ajeng/guests/{}/rsvp", code.to_lowercase());

    let (s, b) = app
        .call("POST", &uri, None, Some(json!({ "name": "Sinta", "attending": true, "guestCount": 2 })))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
    assert_eq!(b["error"]["code"], "over_allowance");

    let (s, b) = app
        .call(
            "POST",
            &uri,
            None,
            Some(json!({ "name": " Sinta M ", "attending": true, "guestCount": 1, "message": "",
                         "responses": [
                             { "fieldKey": "dietary", "value": "vegetarian" },
                             { "fieldKey": "shirt", "value": ["M", "L"] },
                             { "fieldKey": "unknown", "value": 1 }
                         ] })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["rsvp"]["respondentName"], "Sinta M");
    assert_eq!(b["rsvp"]["guestCount"], 1);
    assert_eq!(b["rsvp"]["message"], Value::Null);
    assert_eq!(b["responses"].as_array().unwrap().len(), 2);

    // Re-submitting updates in place.
    let (s, b) = app
        .call(
            "POST",
            &uri,
            None,
            Some(json!({ "name": "Sinta", "attending": false, "guestCount": 1,
                         "responses": [{ "fieldKey": "dietary", "value": "none" }] })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["rsvp"]["guestCount"], 0);

    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT f.field_key, r.value FROM guest_response r JOIN registration_form_field f ON f.id = r.field_id
         WHERE r.guest_id = $1 ORDER BY f.field_key",
    )
    .bind(guest_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![("dietary".into(), json!("none")), ("shirt".into(), json!(["M", "L"]))]);
    let (attending,): (bool,) = sqlx::query_as(r#"SELECT "attending" FROM "rsvp" WHERE "guestId" = $1"#)
        .bind(guest_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!attending);

    // Unknown token, and a draft event is invisible.
    let (s, b) = app
        .call(
            "POST",
            "/v1/public/events/alex-ajeng/guests/AJ27-XXX-9999-9/rsvp",
            None,
            Some(json!({ "name": "X", "attending": false, "guestCount": 1 })),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["error"]["code"], "unknown_token");
    sqlx::query(r#"UPDATE "event" SET "status" = 'DRAFT' WHERE "id" = $1"#)
        .bind(ev)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", "/v1/public/events/alex-ajeng/form-fields", None, None).await.0,
        StatusCode::NOT_FOUND
    );
}

// ------------------------------------------------------- public site (read)

#[sqlx::test(migrations = false)]
async fn public_wedding_event_payload(pool: PgPool) {
    let app = app(pool.clone()).await;
    let ev = app.demo.event_id;
    // Ceremonies sort by sortOrder; add the other child tables.
    sqlx::query(
        r#"INSERT INTO "ceremony" ("id","eventId","nameId","nameEn","startsAt","venueName","sortOrder")
           VALUES (gen_random_uuid(), $1, 'Resepsi', 'Reception', '2027-05-15 06:00', 'Ballroom', 1)"#,
    )
    .bind(ev)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO "story_milestone" ("id","eventId","titleId","titleEn","bodyId","bodyEn","sortOrder")
           VALUES (gen_random_uuid(), $1, 'Bertemu', 'Met', 'a', 'b', 0)"#,
    )
    .bind(ev)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(r#"INSERT INTO "rundown_item" ("id","eventId","timeLabel","titleId","titleEn","sortOrder") VALUES (gen_random_uuid(), $1, '10.00', 'Akad', 'Ceremony', 0)"#)
        .bind(ev)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO "gift_account" ("id","eventId","institution","accountNumber","accountHolder","sortOrder")
           VALUES (gen_random_uuid(), $1, 'BCA', '123', 'Ajeng', 0)"#,
    )
    .bind(ev)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(r#"UPDATE "event" SET "giftAddress" = 'Jl. Mawar 1', "interludePhotos" = ARRAY['a.jpg'] WHERE "id" = $1"#)
        .bind(ev)
        .execute(&pool)
        .await
        .unwrap();

    let (s, b) = app.call("GET", "/v1/public/events/alex-ajeng", None, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let e = &b["event"];
    assert_eq!(e["kind"], "WEDDING");
    assert_eq!(e["brideShortName"], "Ajeng");
    assert_eq!(e["groomFullName"], "Alexander Wiratama");
    assert_eq!(e["defaultLocale"], "ID");
    assert_eq!(e["theme"], "sage-peach");
    assert_eq!(e["bilingual"], true);
    assert_eq!(e["hashtag"], "#AlexAjengForever");
    assert_eq!(e["headlineDate"], "2027-05-15T03:00:00Z");
    assert_eq!(e["giftAddress"], "Jl. Mawar 1");
    assert_eq!(e["interludePhotos"], json!(["a.jpg"]));
    assert_eq!(e["dressCodeColors"], json!([]));
    assert!(e.get("coverPhotoUrl").is_some() && e["coverBackgroundUrl"].is_null());
    assert!(e.get("codePrefix").is_none(), "internal field must not leak");
    let names: Vec<_> = b["ceremonies"].as_array().unwrap().iter().map(|c| c["nameId"].clone()).collect();
    assert_eq!(names, vec![json!("Akad Nikah"), json!("Resepsi")]);
    assert_eq!(b["milestones"][0]["titleEn"], "Met");
    assert_eq!(b["rundown"][0]["timeLabel"], "10.00");
    assert_eq!(b["giftAccounts"][0]["kind"], "BANK");

    // DRAFT, ARCHIVED and unknown slugs are all 404.
    assert_eq!(app.call("GET", "/v1/public/events/nope", None, None).await.0, StatusCode::NOT_FOUND);
    for status in ["DRAFT", "ARCHIVED"] {
        sqlx::query(r#"UPDATE "event" SET "status" = $2::"EventStatus" WHERE "id" = $1"#)
            .bind(ev)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            app.call("GET", "/v1/public/events/alex-ajeng", None, None).await.0,
            StatusCode::NOT_FOUND
        );
    }
}

#[sqlx::test(migrations = false)]
async fn public_generic_event_has_title_and_visible_blocks(pool: PgPool) {
    let app = app(pool.clone()).await;
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO "event" ("id","slug","status","kind","title","subtitle","description","headlineDate","updatedAt")
           VALUES (gen_random_uuid(),'tech-day','PUBLISHED','GENERIC','Tech Day','Annual','About it','2027-01-01 02:00',now())
           RETURNING "id""#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for (ty, order, visible) in [("MAP", 2, true), ("COVER", 1, true), ("GALLERY", 3, false)] {
        sqlx::query("INSERT INTO event_page_block (event_id, block_type, sort_order, is_visible) VALUES ($1,$2,$3,$4)")
            .bind(id)
            .bind(ty)
            .bind(order)
            .bind(visible)
            .execute(&pool)
            .await
            .unwrap();
    }
    let (s, b) = app.call("GET", "/v1/public/events/tech-day", None, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["event"]["title"], "Tech Day");
    assert_eq!(b["event"]["subtitle"], "Annual");
    assert_eq!(b["event"]["description"], "About it");
    assert_eq!(b["event"]["kind"], "GENERIC");
    assert_eq!(b["event"]["status"], "PUBLISHED");
    assert_eq!(b["event"]["headlineDate"], "2027-01-01T02:00:00Z");
    assert!(b.get("ceremonies").is_none());
    let types: Vec<_> = b["blocks"].as_array().unwrap().iter().map(|x| x["blockType"].clone()).collect();
    assert_eq!(types, vec![json!("COVER"), json!("MAP")], "ordered, hidden block omitted");
}

#[sqlx::test(migrations = false)]
async fn public_guest_lookup(pool: PgPool) {
    let app = app(pool.clone()).await;
    let base = "/v1/public/events/alex-ajeng/guest";

    let (s, b) =
        app.call("GET", &format!("{base}?token={}", app.demo.codes[0].to_lowercase()), None, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["guest"]["token"], app.demo.codes[0]);
    assert_eq!(b["guest"]["householdName"], "Keluarga Bpk. Hartono");
    assert_eq!(b["guest"]["plusGuestAllowance"], 3);
    assert_eq!(b["guest"]["rsvp"]["attending"], true);
    assert_eq!(b["guest"]["rsvp"]["guestCount"], 4);

    // No RSVP yet: rsvp is null.
    let (_, b) = app.call("GET", &format!("{base}?token={}", app.demo.codes[3]), None, None).await;
    assert!(b["guest"]["id"].is_string() && b["guest"]["rsvp"].is_null());

    // Missing, unknown, and foreign-event tokens are all `guest: null`, not 404.
    sqlx::query(
        r#"INSERT INTO "event" ("id","slug","status","brideShortName","brideFullName","groomShortName","groomFullName","headlineDate","updatedAt")
           VALUES (gen_random_uuid(),'other','PUBLISHED','A','A','B','B','2027-01-01',now())"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    for uri in [
        base.to_string(),
        format!("{base}?token="),
        format!("{base}?token=AJ27-XXX-9999-9"),
        "/v1/public/events/other/guest?token=".to_string() + &app.demo.codes[0],
    ] {
        let (s, b) = app.call("GET", &uri, None, None).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        assert!(b["guest"].is_null(), "{uri}: {b}");
    }
    assert_eq!(
        app.call("GET", &format!("{base}?token=x").replace("alex-ajeng", "nope"), None, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = false)]
async fn public_voucher_three_states(pool: PgPool) {
    let app = app(pool.clone()).await;
    let uri = |t: &str| format!("/v1/public/events/alex-ajeng/voucher?token={t}");
    let secret = b"golden-vector-secret";

    // unknown: no token / bad token
    for u in ["/v1/public/events/alex-ajeng/voucher".to_string(), uri("AJ27-XXX-9999-9")] {
        let (s, b) = app.call("GET", &u, None, None).await;
        assert_eq!((s, b), (StatusCode::OK, json!({ "status": "unknown" })));
    }

    // pending (no RSVP) and pending + declined
    let (_, b) = app.call("GET", &uri(&app.demo.codes[3]), None, None).await;
    assert_eq!(b["status"], "pending");
    assert_eq!(b["declined"], false);
    assert_eq!(b["guest"]["displayName"], "Sinta Maharani");
    let (_, b) = app.call("GET", &uri(&app.demo.codes[4]), None, None).await;
    assert_eq!((b["status"].clone(), b["declined"].clone()), (json!("pending"), json!(true)));

    // issued: signed payload that the scanner-side verifier accepts
    let (_, b) = app.call("GET", &uri(&app.demo.codes[1].to_lowercase()), None, None).await;
    assert_eq!(b["status"], "issued");
    assert_eq!(b["admits"], 2);
    let fields =
        dwi_domain::verify_qr_payload(secret, b["qrPayload"].as_str().unwrap()).expect("valid signature");
    assert_eq!(fields.code(), app.demo.codes[1]);
    assert_eq!(fields.admits, 2);

    // admits is capped by the allowance (3 + 1) and floored at 1.
    sqlx::query(r#"UPDATE "rsvp" SET "guestCount" = 9 WHERE "guestId" = (SELECT "id" FROM "guest" WHERE "token" = $1)"#)
        .bind(&app.demo.codes[0])
        .execute(&pool)
        .await
        .unwrap();
    let (_, b) = app.call("GET", &uri(&app.demo.codes[0]), None, None).await;
    assert_eq!(b["admits"], 4);
    sqlx::query(r#"UPDATE "rsvp" SET "guestCount" = 0 WHERE "guestId" = (SELECT "id" FROM "guest" WHERE "token" = $1)"#)
        .bind(&app.demo.codes[0])
        .execute(&pool)
        .await
        .unwrap();
    let (_, b) = app.call("GET", &uri(&app.demo.codes[0]), None, None).await;
    assert_eq!(b["admits"], 1);

    // An unparseable stored code falls back to the bare token.
    sqlx::query(r#"UPDATE "guest" SET "token" = 'LEGACY-1' WHERE "token" = $1"#)
        .bind(&app.demo.codes[1])
        .execute(&pool)
        .await
        .unwrap();
    let (_, b) = app.call("GET", &uri("legacy-1"), None, None).await;
    assert_eq!((b["status"].clone(), b["qrPayload"].clone()), (json!("issued"), json!("LEGACY-1")));
}

async fn seed_wishes(pool: &PgPool, event: Uuid, n: i32) {
    sqlx::query(
        r#"INSERT INTO "wish" ("id","eventId","name","message","createdAt")
           SELECT gen_random_uuid(), $1, 'Tamu ' || i, 'Pesan ' || i,
                  (now() AT TIME ZONE 'UTC') - (i * interval '1 minute')
           FROM generate_series(1, $2) AS i"#,
    )
    .bind(event)
    .bind(n)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = false)]
async fn public_wishes_paginate_by_cursor(pool: PgPool) {
    let app = app(pool.clone()).await;
    seed_wishes(&pool, app.demo.event_id, 12).await;
    sqlx::query(r#"UPDATE "wish" SET "isHidden" = true WHERE "name" = 'Tamu 1'"#)
        .execute(&pool)
        .await
        .unwrap();
    let base = "/v1/public/events/alex-ajeng/wishes";

    // 11 visible; default page is 10, newest first, hidden skipped.
    let (s, p1) = app.call("GET", base, None, None).await;
    assert_eq!(s, StatusCode::OK, "{p1}");
    let w1 = p1["wishes"].as_array().unwrap();
    assert_eq!(w1.len(), 10);
    assert_eq!(w1[0]["name"], "Tamu 2");
    assert!(w1[0]["createdAt"].is_string());
    let cursor = p1["nextCursor"].as_str().unwrap();
    assert_eq!(cursor, w1[9]["id"]);

    let (_, p2) = app.call("GET", &format!("{base}?cursor={cursor}"), None, None).await;
    assert_eq!(p2["wishes"].as_array().unwrap().len(), 1);
    assert_eq!(p2["wishes"][0]["name"], "Tamu 12");
    assert!(p2["nextCursor"].is_null());

    let (_, b) = app.call("GET", &format!("{base}?limit=4"), None, None).await;
    assert_eq!(b["wishes"].as_array().unwrap().len(), 4);
    assert!(b["nextCursor"].is_string());

    assert_eq!(app.call("GET", &format!("{base}?cursor=zzz"), None, None).await.0, StatusCode::BAD_REQUEST);
    let (_, b) = app.call("GET", &format!("{base}?cursor={}", Uuid::new_v4()), None, None).await;
    assert_eq!(b["wishes"], json!([]));
}

#[sqlx::test(migrations = false)]
async fn public_wish_create_and_duplicate_suppression(pool: PgPool) {
    let app = app(pool.clone()).await;
    let uri = "/v1/public/events/alex-ajeng/wishes";

    let (s, b) = app
        .call("POST", uri, None, Some(json!({ "token": app.demo.codes[0].to_lowercase(), "name": " Dimas ", "message": " Selamat! " })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["wish"]["name"], "Dimas");
    assert_eq!(b["wish"]["message"], "Selamat!");
    let id = b["wish"]["id"].clone();
    let attributed: Option<Uuid> =
        sqlx::query_scalar(r#"SELECT "guestId" FROM "wish" WHERE "id" = $1::uuid"#)
            .bind(id.as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(attributed.is_some(), "token resolves to the guest");

    // Same text again: existing row, 200, no second insert.
    let (s, b) = app.call("POST", uri, None, Some(json!({ "name": "Dimas", "message": "Selamat!" }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["wish"]["id"], id);

    // Outside the 5-minute window it is a new wish; a bad token stays anonymous.
    sqlx::query(r#"UPDATE "wish" SET "createdAt" = "createdAt" - interval '6 minutes'"#)
        .execute(&pool)
        .await
        .unwrap();
    let (s, b) = app
        .call(
            "POST",
            uri,
            None,
            Some(json!({ "token": "AJ27-XXX-9999-9", "name": "Dimas", "message": "Selamat!" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_ne!(b["wish"]["id"], id);
    let count: i64 = sqlx::query_scalar(r#"SELECT count(*) FROM "wish""#).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 2);

    // Validation, and non-PUBLISHED events refuse wishes.
    for bad in [
        json!({ "name": "", "message": "x" }),
        json!({ "name": "x", "message": "  " }),
        json!({ "name": "x", "message": "m".repeat(601) }),
    ] {
        let (s, b) = app.call("POST", uri, None, Some(bad)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(b["error"]["code"], "invalid_input");
    }
    assert_eq!(
        app.call("POST", "/v1/public/events/nope/wishes", None, Some(json!({ "name": "x", "message": "y" })))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    sqlx::query(r#"UPDATE "event" SET "status" = 'DRAFT' WHERE "id" = $1"#)
        .bind(app.demo.event_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("POST", uri, None, Some(json!({ "name": "x", "message": "y" }))).await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = false)]
async fn public_wishes_are_rate_limited_with_redis(pool: PgPool) {
    let Some(url) = redis_url() else { return eprintln!("skipped: TEST_REDIS_URL not set") };
    let redis = Redis::connect(&url, &format!("test-{}", Uuid::new_v4())).await.unwrap();
    let app = app_with(pool, false, Some(redis)).await;
    let uri = "/v1/public/events/alex-ajeng/wishes";
    for i in 0..3 {
        let (s, b) =
            app.call("POST", uri, None, Some(json!({ "name": "A", "message": format!("m{i}") }))).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
    let (s, b) = app.call("POST", uri, None, Some(json!({ "name": "A", "message": "m3" }))).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(b["error"]["code"], "rate_limited");
}

#[sqlx::test(migrations = false)]
async fn greetings_only_emit_after_photo_is_confirmed(pool: PgPool) {
    let app = app_with(pool, true, None).await;
    let admin = app.login().await;
    let door = app.pair(&admin, "Pintu Utama").await;
    let ev = app.demo.event_id;
    let check_in_id = Uuid::new_v4();

    // 1. Check-in happens BEFORE photo confirmation — greetings queue must remain empty.
    let (s, b) = app
        .call(
            "POST",
            "/v1/sync",
            Some(&door),
            Some(json!({
                "checkIns": [{
                    "id": check_in_id,
                    "code": app.demo.codes[1],
                    "headcount": 2,
                    "occurredAt": chrono::Utc::now(),
                    "memberName": "Pak Hendra",
                }],
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    let (s, g) = app.call("GET", &format!("/v1/events/{ev}/greetings?after=0"), Some(&door), None).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["greetings"].as_array().unwrap().len(), 0, "must not greet before photo is confirmed");

    // 2. Guest takes a photo and taps "Pakai foto" → presign + POST /v1/photos + GREET record.
    let photo_id = Uuid::new_v4();
    let (s, p) = app
        .call(
            "POST",
            "/v1/uploads/presign",
            Some(&door),
            Some(json!({ "photoId": photo_id, "contentType": "image/jpeg" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let key = p["key"].as_str().unwrap();

    let (s, b) = app
        .call(
            "POST",
            "/v1/photos",
            Some(&door),
            Some(json!({
                "id": photo_id,
                "checkInId": check_in_id,
                "code": app.demo.codes[1],
                "key": key,
                "contentType": "image/jpeg",
                "occurredAt": chrono::Utc::now(),
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    // Still not greeted until the explicit GREET signal arrives after confirmation.
    let (s, g_mid) = app.call("GET", &format!("/v1/events/{ev}/greetings?after=0"), Some(&door), None).await;
    assert_eq!(s, StatusCode::OK, "{g_mid}");
    assert_eq!(g_mid["greetings"].as_array().unwrap().len(), 0);

    let (s, b) = app
        .call(
            "POST",
            "/v1/sync",
            Some(&door),
            Some(json!({
                "records": [{
                    "id": Uuid::new_v4(),
                    "checkInId": check_in_id,
                    "code": app.demo.codes[1],
                    "type": "GREET",
                    "occurredAt": chrono::Utc::now(),
                    "data": { "photoId": photo_id },
                }],
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    // 3. Now greetings queue delivers the confirmed entry with memberName + optional selfie URL.
    let (s, g) = app.call("GET", &format!("/v1/events/{ev}/greetings?after=0"), Some(&door), None).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    let items = g["greetings"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Pak Hendra");
    assert_eq!(items[0]["code"], app.demo.codes[1]);
    assert!(items[0]["selfie"].as_str().unwrap().contains("X-Amz-Signature="));
    assert!(items[0]["occurredAt"].is_string());

    // 4. Skipping photo also greets cleanly by name (selfie = null).
    let skip_ci = Uuid::new_v4();
    let cursor = g["cursor"].as_i64().unwrap();
    let (s, b) = app
        .call(
            "POST",
            "/v1/sync",
            Some(&door),
            Some(json!({
                "checkIns": [{
                    "id": skip_ci,
                    "code": app.demo.codes[0],
                    "headcount": 1,
                    "occurredAt": chrono::Utc::now(),
                }],
                "records": [{
                    "id": Uuid::new_v4(),
                    "checkInId": skip_ci,
                    "code": app.demo.codes[0],
                    "type": "GREET",
                    "occurredAt": chrono::Utc::now(),
                    "data": { "skippedPhoto": true },
                }],
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    let (s, g2) =
        app.call("GET", &format!("/v1/events/{ev}/greetings?after={cursor}"), Some(&door), None).await;
    assert_eq!(s, StatusCode::OK, "{g2}");
    let items2 = g2["greetings"].as_array().unwrap();
    assert_eq!(items2.len(), 1);
    assert!(items2[0]["selfie"].is_null());
}

#[sqlx::test(migrations = false)]
async fn doorprize_lists_scanned_guests_and_persists_winners(pool: PgPool) {
    let app = app_with(pool, true, None).await;
    let admin = app.login().await;
    let door = app.pair(&admin, "Pintu Utama").await;
    let ev = app.demo.event_id;

    // Empty before any scans.
    let (s, d0) = app.call("GET", &format!("/v1/events/{ev}/doorprize"), Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK, "{d0}");
    assert_eq!(d0["participants"].as_array().unwrap().len(), 0);
    assert_eq!(d0["winners"].as_array().unwrap().len(), 0);

    // Check in guest 0 twice (repeat scan must deduplicate) + group guest 1 with 2 distinct members.
    let c0a = Uuid::new_v4();
    let c0b = Uuid::new_v4();
    let c1a = Uuid::new_v4();
    let c1b = Uuid::new_v4();
    let (s, b) = app
        .call(
            "POST",
            "/v1/sync",
            Some(&door),
            Some(json!({
                "checkIns": [
                    { "id": c0a, "code": app.demo.codes[0], "headcount": 1, "occurredAt": chrono::Utc::now() },
                    { "id": c0b, "code": app.demo.codes[0], "headcount": 1, "occurredAt": chrono::Utc::now() },
                    { "id": c1a, "code": app.demo.codes[1], "headcount": 1, "occurredAt": chrono::Utc::now(), "memberName": "Andi" },
                    { "id": c1b, "code": app.demo.codes[1], "headcount": 1, "occurredAt": chrono::Utc::now(), "memberName": "Budi" },
                ],
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    let (s, d1) = app.call("GET", &format!("/v1/events/{ev}/doorprize"), Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK, "{d1}");
    let participants = d1["participants"].as_array().unwrap();
    assert_eq!(participants.len(), 3, "guest 0 deduped + 2 group members: {participants:?}");

    // Save a winner.
    let (s, w) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/doorprize/winners"),
            Some(&admin),
            Some(json!({
                "checkInId": c1a,
                "code": app.demo.codes[1],
                "name": "Andi",
                "householdName": "Ibu Ratna & Keluarga",
                "prize": "Grand Prize — Emas 1g",
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{w}");
    let winner_id = w["winner"]["id"].as_str().unwrap().to_string();
    assert_eq!(w["winner"]["name"], "Andi");
    assert_eq!(w["winner"]["prize"], "Grand Prize — Emas 1g");

    // Winner appears in state.
    let (s, d2) = app.call("GET", &format!("/v1/events/{ev}/doorprize"), Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK, "{d2}");
    assert_eq!(d2["winners"].as_array().unwrap().len(), 1);

    // Delete single winner.
    let (s, _) = app
        .call("DELETE", &format!("/v1/events/{ev}/doorprize/winners/{winner_id}"), Some(&admin), None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Re-add then reset all.
    let (s, _) = app
        .call(
            "POST",
            &format!("/v1/events/{ev}/doorprize/winners"),
            Some(&admin),
            Some(json!({ "code": app.demo.codes[0], "name": "Bapak Budi" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = app.call("DELETE", &format!("/v1/events/{ev}/doorprize/winners"), Some(&admin), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (_, d3) = app.call("GET", &format!("/v1/events/{ev}/doorprize"), Some(&admin), None).await;
    assert_eq!(d3["winners"].as_array().unwrap().len(), 0);
}
