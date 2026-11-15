//! `GET /v1/events/:id/live` — Server-Sent Events of every new scan and photo.
//!
//! SSE rather than WebSockets: one-way is all a dashboard needs, it survives
//! proxies and Cloudflare, and browsers reconnect on their own. Clients should
//! refetch `/attendance` on (re)connect and on a `resync` event, then apply
//! `scan` / `photo` events incrementally.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::auth::Viewer;
use crate::error::ApiResult;
use crate::state::AppState;

pub async fn stream(
    State(state): State<AppState>,
    viewer: Viewer,
    Path(event_id): Path<Uuid>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    viewer.require_event(&state, event_id).await?;
    let mut rx = state.live.subscribe(event_id);

    let stream = async_stream(move |yielder| async move {
        yielder.send(Event::default().event("ready").data("{}")).await;
        loop {
            match rx.recv().await {
                Ok(payload) => {
                    let kind = if payload.contains(r#""type":"photo""#) { "photo" } else { "scan" };
                    yielder.send(Event::default().event(kind).data(payload.as_ref())).await;
                }
                // Too slow to keep up: tell the client to refetch instead of
                // silently dropping scans.
                Err(RecvError::Lagged(_)) => yielder.send(Event::default().event("resync").data("{}")).await,
                Err(RecvError::Closed) => break,
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping")))
}

/// Tiny generator: run `f` and turn what it sends into a stream, without
/// pulling in another dependency.
struct Yielder(tokio::sync::mpsc::Sender<Event>);

impl Yielder {
    async fn send(&self, e: Event) {
        let _ = self.0.send(e).await;
    }
}

fn async_stream<F, Fut>(f: F) -> impl Stream<Item = Result<Event, Infallible>>
where
    F: FnOnce(Yielder) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let task = tokio::spawn(f(Yielder(tx)));
    // Abort the producer when the client disconnects (stream dropped).
    let guard = AbortOnDrop(task);
    futures::stream::unfold((rx, guard), |(mut rx, guard)| async move {
        rx.recv().await.map(|e| (Ok(e), (rx, guard)))
    })
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
