//! Real-time POS push (WebSocket + SSE fallback).
//!
//! Per-store broadcast channels fan out events to any connected POS/MPOS/supervisor
//! client. Every message carries a monotonic `seq` so clients detect drops and ask for
//! a resnapshot.
//!
//! Wire-level message shape:
//! ```json
//! {
//!   "version": "1.0.0",
//!   "store_id": "...",
//!   "seq": 42,
//!   "kind": "cart_updated",
//!   "payload": { ... }
//! }
//! ```

use apex_edge_metrics::{STREAM_CONNECTIONS, STREAM_MESSAGES_TOTAL};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    CartUpdated,
    ApprovalRequested,
    ApprovalDecided,
    DocumentReady,
    SyncProgress,
    PriceChanged,
    ReturnUpdated,
    ShiftUpdated,
    Heartbeat,
}

impl StreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CartUpdated => "cart_updated",
            Self::ApprovalRequested => "approval_requested",
            Self::ApprovalDecided => "approval_decided",
            Self::DocumentReady => "document_ready",
            Self::SyncProgress => "sync_progress",
            Self::PriceChanged => "price_changed",
            Self::ReturnUpdated => "return_updated",
            Self::ShiftUpdated => "shift_updated",
            Self::Heartbeat => "heartbeat",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEnvelope {
    pub version: String,
    pub store_id: Uuid,
    pub seq: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// Per-store broadcast hub. Held in `AppState` and shared across handlers.
#[derive(Clone, Default)]
pub struct StreamHub {
    inner: Arc<Mutex<HashMap<Uuid, Arc<StoreChannel>>>>,
}

struct StoreChannel {
    seq: AtomicU64,
    tx: broadcast::Sender<StreamEnvelope>,
}

impl StreamHub {
    pub fn new() -> Self {
        Self::default()
    }

    fn channel(&self, store_id: Uuid) -> Arc<StoreChannel> {
        let mut guard = self.inner.lock().expect("stream hub poisoned");
        guard
            .entry(store_id)
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(256);
                Arc::new(StoreChannel {
                    seq: AtomicU64::new(0),
                    tx,
                })
            })
            .clone()
    }

    pub fn publish(&self, store_id: Uuid, kind: StreamKind, payload: serde_json::Value) -> u64 {
        let ch = self.channel(store_id);
        let seq = ch.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let envelope = StreamEnvelope {
            version: "1.0.0".into(),
            store_id,
            seq,
            kind: kind.as_str().into(),
            payload,
        };
        let _ = ch.tx.send(envelope);
        metrics::counter!(STREAM_MESSAGES_TOTAL, 1u64, "kind" => kind.as_str());
        seq
    }

    pub fn subscribe(&self, store_id: Uuid) -> broadcast::Receiver<StreamEnvelope> {
        self.channel(store_id).tx.subscribe()
    }

    pub fn current_seq(&self, store_id: Uuid) -> u64 {
        self.channel(store_id).seq.load(Ordering::SeqCst)
    }
}

/// Convenience: publish from handlers when an `AppState` is in scope.
pub async fn stream_broadcast(
    state: &AppState,
    store_id: Uuid,
    kind: StreamKind,
    payload: serde_json::Value,
) -> u64 {
    state.stream.publish(store_id, kind, payload)
}

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub store_id: Option<Uuid>,
    /// Not yet used for replay (we don't persist history in v0.6.0); clients receive new
    /// messages after subscription.
    #[serde(default)]
    pub since: Option<u64>,
}

/// GET /pos/stream — WebSocket real-time feed.
pub async fn pos_stream_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> impl IntoResponse {
    let store_id = q.store_id.unwrap_or(state.store_id);
    ws.on_upgrade(move |socket| handle_ws(socket, state, store_id))
}

async fn handle_ws(socket: WebSocket, state: AppState, store_id: Uuid) {
    metrics::increment_gauge!(STREAM_CONNECTIONS, 1.0);
    let mut rx = state.stream.subscribe(store_id);
    let (mut sender, mut receiver) = socket.split();

    // Send an initial hello/heartbeat so clients know the connection is live.
    let hello = StreamEnvelope {
        version: "1.0.0".into(),
        store_id,
        seq: state.stream.current_seq(store_id),
        kind: StreamKind::Heartbeat.as_str().into(),
        payload: serde_json::json!({"connected": true}),
    };
    if sender
        .send(Message::Text(
            serde_json::to_string(&hello).unwrap_or_else(|_| "{}".into()),
        ))
        .await
        .is_err()
    {
        metrics::decrement_gauge!(STREAM_CONNECTIONS, 1.0);
        return;
    }

    let forward = tokio::spawn(async move {
        while let Ok(env) = rx.recv().await {
            let text = match serde_json::to_string(&env) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if sender.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    // Drain incoming messages (we don't process client-to-server commands here; they
    // continue to flow through /pos/command). Close when the socket closes.
    while let Some(Ok(msg)) = receiver.next().await {
        if matches!(msg, Message::Close(_)) {
            break;
        }
    }

    forward.abort();
    metrics::decrement_gauge!(STREAM_CONNECTIONS, 1.0);
}

/// GET /pos/events?store_id=...&since=N — SSE fallback for environments without WS.
pub async fn pos_stream_sse(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let store_id = q.store_id.unwrap_or(state.store_id);
    let rx = state.stream.subscribe(store_id);
    metrics::increment_gauge!(STREAM_CONNECTIONS, 1.0);

    let stream = BroadcastStream::new(rx).map(move |item| {
        let env = match item {
            Ok(e) => e,
            Err(_) => StreamEnvelope {
                version: "1.0.0".into(),
                store_id,
                seq: 0,
                kind: StreamKind::Heartbeat.as_str().into(),
                payload: serde_json::json!({"lagged": true}),
            },
        };
        let data = serde_json::to_string(&env).unwrap_or_else(|_| "{}".into());
        Ok(Event::default().event(env.kind.clone()).data(data))
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_and_subscribe_delivers_in_order() {
        let hub = StreamHub::new();
        let store = Uuid::new_v4();
        let mut rx = hub.subscribe(store);

        let seq1 = hub.publish(store, StreamKind::CartUpdated, serde_json::json!({"n": 1}));
        let seq2 = hub.publish(store, StreamKind::CartUpdated, serde_json::json!({"n": 2}));

        let msg1 = rx.recv().await.unwrap();
        let msg2 = rx.recv().await.unwrap();
        assert_eq!(msg1.seq, seq1);
        assert_eq!(msg2.seq, seq2);
        assert_eq!(msg1.payload["n"], 1);
        assert_eq!(msg2.payload["n"], 2);
        assert!(seq2 > seq1);
    }

    #[tokio::test]
    async fn publishes_are_scoped_per_store() {
        let hub = StreamHub::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut rx_a = hub.subscribe(a);
        let mut rx_b = hub.subscribe(b);

        hub.publish(a, StreamKind::Heartbeat, serde_json::json!({"s": "A"}));
        hub.publish(b, StreamKind::Heartbeat, serde_json::json!({"s": "B"}));

        let first_a = rx_a.recv().await.unwrap();
        let first_b = rx_b.recv().await.unwrap();
        assert_eq!(first_a.payload["s"], "A");
        assert_eq!(first_b.payload["s"], "B");
        // No cross-talk.
        assert!(rx_a.try_recv().is_err());
        assert!(rx_b.try_recv().is_err());
    }
}
