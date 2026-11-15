//! Redis: a short-lived cache, login rate limiting, and the live-update bus.
//!
//! Redis is optional. Every helper degrades to "no cache / no limit" when it
//! is not configured or briefly unreachable — a Redis outage must never stop
//! a guest from checking in.

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct Redis {
    pub client: redis::Client,
    pub conn: ConnectionManager,
    /// Prefixed to every key so one Redis can serve several environments.
    pub prefix: String,
}

impl Redis {
    pub async fn connect(url: &str, prefix: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = client.get_connection_manager().await?;
        Ok(Self { client, conn, prefix: prefix.to_string() })
    }

    pub fn key(&self, k: &str) -> String {
        format!("{}:{k}", self.prefix)
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let mut conn = self.conn.clone();
        match conn.get::<_, Option<String>>(self.key(key)).await {
            Ok(Some(s)) => serde_json::from_str(&s).ok(),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = %e, "redis get failed");
                None
            }
        }
    }

    pub async fn set_json<T: Serialize>(&self, key: &str, value: &T, ttl_secs: u64) {
        let Ok(s) = serde_json::to_string(value) else { return };
        let mut conn = self.conn.clone();
        if let Err(e) = conn.set_ex::<_, _, ()>(self.key(key), s, ttl_secs).await {
            tracing::warn!(error = %e, "redis set failed");
        }
    }

    pub async fn del(&self, key: &str) {
        let mut conn = self.conn.clone();
        let _: Result<(), _> = conn.del(self.key(key)).await;
    }

    /// Fixed-window counter. Returns the count within the window, or `None`
    /// if Redis could not be reached (callers then allow the request).
    pub async fn hit(&self, key: &str, window_secs: i64) -> Option<i64> {
        let mut conn = self.conn.clone();
        let key = self.key(key);
        let n: i64 = match conn.incr(&key, 1).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "redis incr failed");
                return None;
            }
        };
        if n == 1 {
            let _: Result<bool, _> = conn.expire(&key, window_secs).await;
        }
        Some(n)
    }

    pub async fn publish(&self, channel: &str, payload: &str) -> redis::RedisResult<()> {
        let mut conn = self.conn.clone();
        conn.publish::<_, _, ()>(self.key(channel), payload).await
    }
}
