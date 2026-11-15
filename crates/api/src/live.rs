//! Live updates: every accepted scan is pushed to dashboards and door devices.
//!
//! Inside one process this is a `tokio::broadcast` channel per event. With
//! Redis configured, publishing goes through Redis pub/sub instead and each
//! instance relays what it hears into its local channels — so a check-in on
//! instance A reaches a dashboard connected to instance B.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::redis_ext::Redis;

const CHANNEL_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct LiveHub {
    channels: Arc<Mutex<HashMap<Uuid, broadcast::Sender<Arc<str>>>>>,
    redis: Option<Redis>,
}

impl LiveHub {
    pub fn new(redis: Option<Redis>) -> Self {
        Self { channels: Arc::default(), redis }
    }

    fn sender(&self, event_id: Uuid) -> broadcast::Sender<Arc<str>> {
        let mut map = self.channels.lock().expect("live hub lock");
        map.entry(event_id).or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0).clone()
    }

    pub fn subscribe(&self, event_id: Uuid) -> broadcast::Receiver<Arc<str>> {
        self.sender(event_id).subscribe()
    }

    /// Delivers locally only. Used by the Redis relay and when Redis is off.
    fn deliver(&self, event_id: Uuid, payload: Arc<str>) {
        let tx = self.sender(event_id);
        // No receivers is normal (nobody watching); ignore.
        let _ = tx.send(payload);
        // Drop channels nobody listens to any more.
        let mut map = self.channels.lock().expect("live hub lock");
        map.retain(|_, s| s.receiver_count() > 0);
    }

    pub async fn publish(&self, event_id: Uuid, payload: String) {
        match &self.redis {
            Some(r) => {
                if let Err(e) = r.publish(&format!("live:{event_id}"), &payload).await {
                    // Redis down: at least the clients on this instance get it.
                    tracing::warn!(error = %e, "redis publish failed, delivering locally");
                    self.deliver(event_id, payload.into());
                }
            }
            None => self.deliver(event_id, payload.into()),
        }
    }

    /// Relays Redis pub/sub into local channels. Reconnects forever.
    pub fn spawn_redis_relay(&self) {
        let Some(redis) = self.redis.clone() else { return };
        let hub = self.clone();
        tokio::spawn(async move {
            let pattern = redis.key("live:*");
            let strip = redis.key("live:");
            loop {
                match redis.client.get_async_pubsub().await {
                    Ok(mut ps) => {
                        if let Err(e) = ps.psubscribe(&pattern).await {
                            tracing::warn!(error = %e, "redis psubscribe failed");
                        } else {
                            tracing::info!("live relay subscribed to {pattern}");
                            let mut stream = ps.into_on_message();
                            while let Some(msg) = stream.next().await {
                                let Some(id) = msg.get_channel_name().strip_prefix(&strip) else { continue };
                                let (Ok(id), Ok(payload)) = (id.parse::<Uuid>(), msg.get_payload::<String>())
                                else {
                                    continue;
                                };
                                hub.deliver(id, payload.into());
                            }
                            tracing::warn!("live relay stream ended, reconnecting");
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "redis pubsub connect failed"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }
}
