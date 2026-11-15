pub mod auth;
pub mod events;
pub mod forms;
pub mod live;
pub mod page_blocks;
pub mod photos;
pub mod public;
pub mod public_site;
pub mod registration;
pub mod scanner;
pub mod ticket_types;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, Method, header};
use axum::routing::{delete, get, post, put};
use serde_json::{Value, json};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState, cors_origins: &[String]) -> Router {
    let origins: Vec<HeaderValue> = cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .max_age(std::time::Duration::from_secs(3600));

    let v1 = Router::new()
        // admin session
        .route("/auth/login", post(auth::login))
        .route("/auth/password", post(auth::change_password))
        .route("/me", get(auth::me))
        // admin: events
        .route("/events", get(events::list).post(events::create))
        .route("/events/{id}", get(events::get).put(events::update).delete(events::archive))
        .route("/events/{id}/guests", get(events::guests).post(events::create_guest))
        .route("/events/{id}/attendance", get(events::attendance))
        .route(
            "/events/{id}/pairing-codes",
            get(events::list_pairing_codes).post(events::create_pairing_code),
        )
        .route("/pairing-codes/{id}", delete(events::revoke_pairing_code))
        .route("/events/{id}/devices", get(events::devices))
        .route("/devices/{id}", delete(events::revoke_device))
        // admin: registration form + page builder
        .route("/events/{id}/form-fields", get(forms::list).post(forms::create))
        .route("/form-fields/{id}", put(forms::update).delete(forms::delete))
        .route("/events/{id}/page-blocks", get(page_blocks::list).post(page_blocks::create))
        .route("/page-blocks/{id}", put(page_blocks::update).delete(page_blocks::delete))
        // admin: ticket types + open/allowlist registration
        .route("/events/{id}/ticket-types", get(ticket_types::list).post(ticket_types::create))
        .route("/ticket-types/{id}", put(ticket_types::update).delete(ticket_types::delete))
        .route("/ticket-types/{id}/expand", post(ticket_types::expand))
        .route("/events/{id}/registration-mode", put(registration::set_mode))
        .route("/events/{id}/allowlist", get(registration::list_allowlist).post(registration::add_allowlist))
        .route("/allowlist/{id}", delete(registration::remove_allowlist_entry))
        // public guest side (no session)
        .route("/public/events/{slug}", get(public_site::event))
        .route("/public/events/{slug}/guest", get(public_site::guest))
        .route("/public/events/{slug}/voucher", get(public_site::voucher))
        .route("/public/events/{slug}/wishes", get(public_site::wishes).post(public_site::create_wish))
        .route("/public/events/{slug}/form-fields", get(public::form_fields))
        .route("/public/events/{slug}/guests/{token}/rsvp", post(public::rsvp))
        .route("/public/events/{slug}/ticket-types", get(registration::ticket_types))
        .route("/public/events/{slug}/register", post(registration::register))
        // door devices
        .route("/devices/pair", post(scanner::pair))
        .route("/events/{id}/snapshot", get(scanner::snapshot))
        .route("/sync", post(scanner::sync))
        .route("/uploads/presign", post(photos::presign))
        .route("/photos", post(photos::complete))
        // either
        .route("/events/{id}/photos", get(photos::list))
        .route("/events/{id}/live", get(live::stream));

    Router::new()
        .route("/healthz", get(health))
        .nest("/v1", v1)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let db = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.db).await.is_ok();
    let redis = match &state.redis {
        None => "off",
        Some(r) => {
            let mut c = r.conn.clone();
            if redis::cmd("PING").query_async::<String>(&mut c).await.is_ok() { "ok" } else { "down" }
        }
    };
    Json(
        json!({ "ok": db, "db": db, "redis": redis, "storage": state.storage.is_some(), "version": env!("CARGO_PKG_VERSION") }),
    )
}
