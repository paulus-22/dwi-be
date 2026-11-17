use std::sync::Arc;

use jsonwebtoken::{DecodingKey, EncodingKey};
use sqlx::PgPool;

use crate::config::Config;
use crate::live::LiveHub;
use crate::redis_ext::Redis;
use crate::storage::Storage;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub db: PgPool,
    pub redis: Option<Redis>,
    pub live: LiveHub,
    pub storage: Option<Storage>,
    pub jwt_encoding: EncodingKey,
    pub jwt_decoding: DecodingKey,
    pub invite_secret: Vec<u8>,
    pub session_hours: i64,
    /// No trailing slash.
    pub public_site_url: String,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    pub fn new(config: &Config, db: PgPool, redis: Option<Redis>) -> anyhow::Result<Self> {
        let live = LiveHub::new(redis.clone());
        Ok(Self(Arc::new(Inner {
            db,
            live,
            redis,
            storage: Storage::from_config(&config.storage)?,
            jwt_encoding: EncodingKey::from_secret(config.jwt_secret.as_bytes()),
            jwt_decoding: DecodingKey::from_secret(config.jwt_secret.as_bytes()),
            invite_secret: config.invite_signing_secret.as_bytes().to_vec(),
            session_hours: config.session_hours,
            public_site_url: config.public_site_url.trim_end_matches('/').to_string(),
        })))
    }
}
