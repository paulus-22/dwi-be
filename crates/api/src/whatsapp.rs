//! Outbound WhatsApp: invitations and automatic RSVP confirmations.
//!
//! Every attempt is logged in `whatsapp_message` (PENDING, then SENT/FAILED).
//! Sending is always best effort: callers use the `spawn_*` helpers, which
//! run in the background and only log failures, so a provider outage can
//! never fail a guest's RSVP.

use std::future::Future;
use std::time::Duration;

use anyhow::Context;
use chrono::NaiveDateTime;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::AppState;
use crate::views::{couple_line, date_label_id};

pub trait WhatsappProvider: Send + Sync {
    /// Stored in `whatsapp_message.provider`.
    fn name(&self) -> &'static str;
    /// Sends one text message; returns the provider's message id.
    fn send(&self, phone: &str, text: &str) -> impl Future<Output = anyhow::Result<String>> + Send;
}

const FONNTE_URL: &str = "https://api.fonnte.com/send";

/// https://fonnte.com/ — form POST with the device token as `Authorization`.
/// Without a token it sends nothing and every attempt is logged as FAILED.
pub struct FonnteProvider {
    token: Option<String>,
    http: reqwest::Client,
}

impl FonnteProvider {
    pub fn new(token: Option<String>) -> anyhow::Result<Self> {
        let token = token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        if token.is_none() {
            tracing::warn!("FONNTE_TOKEN not set; WhatsApp messages will be logged as FAILED, not sent");
        }
        let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
        Ok(Self { token, http })
    }
}

impl WhatsappProvider for FonnteProvider {
    fn name(&self) -> &'static str {
        "fonnte"
    }

    async fn send(&self, phone: &str, text: &str) -> anyhow::Result<String> {
        let Some(token) = &self.token else {
            tracing::warn!("WhatsApp message not sent: FONNTE_TOKEN is not set");
            anyhow::bail!("FONNTE_TOKEN is not set");
        };
        let res = self
            .http
            .post(FONNTE_URL)
            .header(reqwest::header::AUTHORIZATION, token)
            .form(&[("target", phone), ("message", text)])
            .send()
            .await
            .context("fonnte request failed")?;
        let status = res.status();
        let raw = res.text().await.context("fonnte response unreadable")?;
        let body: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        if !status.is_success() || body["status"] != Value::Bool(true) {
            let reason = body["reason"].as_str().map(str::to_string).unwrap_or(raw);
            anyhow::bail!("fonnte rejected the message ({status}): {reason}");
        }
        // `id` is an array of ids (one per target); a single target gives one.
        let id = match &body["id"] {
            Value::Array(ids) => ids.first().map(id_string),
            Value::Null => None,
            v => Some(id_string(v)),
        };
        Ok(id.unwrap_or_default())
    }
}

fn id_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------------ messages

#[derive(sqlx::FromRow)]
struct GuestContext {
    event_id: Uuid,
    household_name: String,
    token: String,
    phone: Option<String>,
    slug: String,
    kind: String,
    title: Option<String>,
    bride_short_name: Option<String>,
    groom_short_name: Option<String>,
    headline_date: NaiveDateTime,
    timezone: String,
    attending: Option<bool>,
    guest_count: Option<i32>,
    respondent_name: Option<String>,
}

impl GuestContext {
    fn event_name(&self) -> String {
        match self.kind.as_str() {
            "GENERIC" => self.title.clone().unwrap_or_default(),
            _ => couple_line(
                self.bride_short_name.as_deref().unwrap_or_default(),
                self.groom_short_name.as_deref().unwrap_or_default(),
            ),
        }
    }
}

async fn load_guest(state: &AppState, guest_id: Uuid) -> anyhow::Result<GuestContext> {
    sqlx::query_as::<_, GuestContext>(
        r#"SELECT g."eventId" AS event_id, g."householdName" AS household_name, g."token", g."phone",
                  e."slug", e."kind"::text AS kind, e."title",
                  e."brideShortName" AS bride_short_name, e."groomShortName" AS groom_short_name,
                  e."headlineDate" AS headline_date, e."timezone",
                  r."attending", r."guestCount" AS guest_count, r."respondentName" AS respondent_name
           FROM "guest" g
           JOIN "event" e ON e."id" = g."eventId"
           LEFT JOIN "rsvp" r ON r."guestId" = g."id"
           WHERE g."id" = $1"#,
    )
    .bind(guest_id)
    .fetch_optional(&state.db)
    .await?
    .with_context(|| format!("guest {guest_id} not found"))
}

fn usable_phone(g: &GuestContext) -> Option<String> {
    g.phone.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_string)
}

pub async fn send_invite(state: &AppState, guest_id: Uuid) -> anyhow::Result<()> {
    let g = load_guest(state, guest_id).await?;
    let Some(phone) = usable_phone(&g) else {
        tracing::debug!(%guest_id, "no phone number; WhatsApp invite skipped");
        return Ok(());
    };
    let link = format!("{}/e/{}?to={}", state.public_site_url, g.slug, g.token);
    let date = date_label_id(g.headline_date, &g.timezone);
    let event = g.event_name();
    let text = format!(
        "Halo {},\n\nDengan hormat, kami mengundang Anda ke {event} pada {date}.\n\n\
         Buka undangan Anda di:\n{link}\n\nKode undangan: {}\n\nTerima kasih.",
        g.household_name, g.token
    );
    let payload = json!({ "text": text, "link": link, "event": event, "date": date });
    deliver(state, &g, guest_id, "INVITE", &phone, "invite_v1", payload, &text).await
}

pub async fn send_rsvp_confirmation(state: &AppState, guest_id: Uuid) -> anyhow::Result<()> {
    let g = load_guest(state, guest_id).await?;
    let Some(phone) = usable_phone(&g) else {
        tracing::debug!(%guest_id, "no phone number; WhatsApp RSVP confirmation skipped");
        return Ok(());
    };
    let attending = g.attending.with_context(|| format!("guest {guest_id} has no RSVP"))?;
    let name = g.respondent_name.clone().unwrap_or_else(|| g.household_name.clone());
    let event = g.event_name();
    let date = date_label_id(g.headline_date, &g.timezone);
    let count = g.guest_count.unwrap_or(0);
    let voucher_link = format!("{}/v/{}?to={}", state.public_site_url, g.slug, g.token);
    let text = if attending {
        format!(
            "Halo {name}, terima kasih atas konfirmasi kehadiran Anda di {event} ({date}).\n\n\
             Kami mencatat Anda akan hadir: {count} orang.\n\n\
             Berikut tautan QR masuk Anda — tunjukkan saat registrasi di lokasi:\n{voucher_link}\n\n\
             Sampai jumpa!"
        )
    } else {
        format!(
            "Halo {name}, terima kasih sudah mengabari.\n\n\
             Kami mencatat Anda berhalangan hadir di {event} ({date}). Semoga kita bisa bertemu di lain kesempatan."
        )
    };
    let payload = json!({
        "text": text, "attending": attending, "guestCount": count, "event": event,
        "voucherLink": attending.then(|| voucher_link.clone()),
    });
    deliver(state, &g, guest_id, "RSVP_CONFIRMATION", &phone, "rsvp_confirmation_v1", payload, &text).await
}

#[allow(clippy::too_many_arguments)]
async fn deliver(
    state: &AppState,
    g: &GuestContext,
    guest_id: Uuid,
    kind: &str,
    phone: &str,
    template_key: &str,
    payload: Value,
    text: &str,
) -> anyhow::Result<()> {
    let provider = &state.whatsapp;
    let message_id: Uuid = sqlx::query_scalar(
        "INSERT INTO whatsapp_message (event_id, guest_id, kind, phone, template_key, payload, provider, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'PENDING')
         RETURNING id",
    )
    .bind(g.event_id)
    .bind(guest_id)
    .bind(kind)
    .bind(phone)
    .bind(template_key)
    .bind(&payload)
    .bind(provider.name())
    .fetch_one(&state.db)
    .await?;

    let result = provider.send(phone, text).await;
    match &result {
        Ok(provider_id) => {
            sqlx::query(
                "UPDATE whatsapp_message
                 SET status = 'SENT', provider_message_id = NULLIF($2, ''), error = NULL, sent_at = now()
                 WHERE id = $1",
            )
            .bind(message_id)
            .bind(provider_id)
            .execute(&state.db)
            .await?;
        }
        Err(e) => {
            sqlx::query("UPDATE whatsapp_message SET status = 'FAILED', error = $2 WHERE id = $1")
                .bind(message_id)
                .bind(format!("{e:#}"))
                .execute(&state.db)
                .await?;
        }
    }
    result.map(|_| ())
}

/// Fire-and-forget [`send_invite`]; failures are logged, never returned.
pub fn spawn_invite(state: AppState, guest_id: Uuid) {
    tokio::spawn(async move {
        if let Err(e) = send_invite(&state, guest_id).await {
            tracing::warn!(%guest_id, error = %format!("{e:#}"), "WhatsApp invite failed");
        }
    });
}

/// Fire-and-forget [`send_rsvp_confirmation`]; failures are logged, never returned.
pub fn spawn_rsvp_confirmation(state: AppState, guest_id: Uuid) {
    tokio::spawn(async move {
        if let Err(e) = send_rsvp_confirmation(&state, guest_id).await {
            tracing::warn!(%guest_id, error = %format!("{e:#}"), "WhatsApp RSVP confirmation failed");
        }
    });
}
