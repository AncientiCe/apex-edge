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

use apex_edge_metrics::{REGISTER_PRESENCE, STREAM_CONNECTIONS, STREAM_MESSAGES_TOTAL};
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
    collections::{HashMap, VecDeque},
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
    StockChanged,
    RegisterPresence,
    CartHandoff,
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
            Self::StockChanged => "stock_changed",
            Self::RegisterPresence => "register_presence",
            Self::CartHandoff => "cart_handoff",
            Self::Heartbeat => "heartbeat",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// store_id -> (register_id -> active connection count). A register is "present"
    /// while it holds at least one open stream connection.
    presence: Arc<Mutex<HashMap<Uuid, HashMap<Uuid, u32>>>>,
}

/// Bounded number of recent events retained per store for `since` replay on reconnect.
const HISTORY_CAPACITY: usize = 256;

struct StoreChannel {
    seq: AtomicU64,
    tx: broadcast::Sender<StreamEnvelope>,
    /// Recent events (bounded ring) for replay to reconnecting clients.
    history: Mutex<VecDeque<StreamEnvelope>>,
}

/// Result of a `since`-based replay request.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplayResult {
    /// The requested events are still retained; replay these (may be empty if caller is current).
    Replay(Vec<StreamEnvelope>),
    /// The requested `since` is older than retained history; the client must resnapshot.
    ResnapshotRequired,
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
                    history: Mutex::new(VecDeque::with_capacity(HISTORY_CAPACITY)),
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
        {
            let mut hist = ch.history.lock().expect("history poisoned");
            if hist.len() == HISTORY_CAPACITY {
                hist.pop_front();
            }
            hist.push_back(envelope.clone());
        }
        let _ = ch.tx.send(envelope);
        metrics::counter!(STREAM_MESSAGES_TOTAL, 1u64, "kind" => kind.as_str());
        seq
    }

    /// Return events with `seq > since` for replay, or signal that a resnapshot is required
    /// when `since` predates the retained ring buffer (a gap we can no longer fill).
    pub fn events_since(&self, store_id: Uuid, since: u64) -> ReplayResult {
        let ch = self.channel(store_id);
        let current = ch.seq.load(Ordering::SeqCst);
        // Caller is already current (or ahead, e.g. after a hub restart): nothing to replay.
        if since >= current {
            return ReplayResult::Replay(Vec::new());
        }
        let hist = ch.history.lock().expect("history poisoned");
        match hist.front() {
            // We retain events starting at `earliest.seq`; the client needs everything after
            // `since`, so the event at `since + 1` must still be present.
            Some(earliest) if earliest.seq <= since + 1 => {
                ReplayResult::Replay(hist.iter().filter(|e| e.seq > since).cloned().collect())
            }
            // Either history is empty (but we know events were lost) or the oldest retained
            // event is newer than what the client needs: the gap is unrecoverable.
            _ => ReplayResult::ResnapshotRequired,
        }
    }

    pub fn subscribe(&self, store_id: Uuid) -> broadcast::Receiver<StreamEnvelope> {
        self.channel(store_id).tx.subscribe()
    }

    pub fn current_seq(&self, store_id: Uuid) -> u64 {
        self.channel(store_id).seq.load(Ordering::SeqCst)
    }

    /// Mark a register as connected (present). Returns true if this was the register's
    /// first active connection (i.e. it just became present).
    pub fn connect_register(&self, store_id: Uuid, register_id: Uuid) -> bool {
        let mut guard = self.presence.lock().expect("presence poisoned");
        let store = guard.entry(store_id).or_default();
        let count = store.entry(register_id).or_insert(0);
        *count += 1;
        let is_new = *count == 1;
        Self::refresh_presence_gauge(&guard);
        is_new
    }

    /// Mark a register connection as closed. Returns true if the register became absent
    /// (no remaining connections).
    pub fn disconnect_register(&self, store_id: Uuid, register_id: Uuid) -> bool {
        let mut guard = self.presence.lock().expect("presence poisoned");
        let mut became_absent = false;
        if let Some(store) = guard.get_mut(&store_id) {
            if let Some(count) = store.get_mut(&register_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    store.remove(&register_id);
                    became_absent = true;
                }
            }
            if store.is_empty() {
                guard.remove(&store_id);
            }
        }
        Self::refresh_presence_gauge(&guard);
        became_absent
    }

    /// List the registers currently present in a store.
    pub fn present_registers(&self, store_id: Uuid) -> Vec<Uuid> {
        let guard = self.presence.lock().expect("presence poisoned");
        guard
            .get(&store_id)
            .map(|s| s.keys().copied().collect())
            .unwrap_or_default()
    }

    fn refresh_presence_gauge(guard: &HashMap<Uuid, HashMap<Uuid, u32>>) {
        let total: usize = guard.values().map(|s| s.len()).sum();
        metrics::gauge!(REGISTER_PRESENCE, total as f64);
    }
}

/// RAII guard that marks a register present for the lifetime of a stream connection,
/// broadcasting `RegisterPresence` on connect and (on drop) disconnect. Robust to SSE
/// drops where there is no explicit close handler.
pub struct PresenceGuard {
    hub: StreamHub,
    store_id: Uuid,
    register_id: Option<Uuid>,
}

impl PresenceGuard {
    pub fn new(hub: &StreamHub, store_id: Uuid, register_id: Option<Uuid>) -> Self {
        if let Some(register_id) = register_id {
            if hub.connect_register(store_id, register_id) {
                hub.publish(
                    store_id,
                    StreamKind::RegisterPresence,
                    serde_json::json!({
                        "event": "connected",
                        "register_id": register_id.to_string(),
                        "present": hub.present_registers(store_id)
                            .iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                    }),
                );
            }
        }
        Self {
            hub: hub.clone(),
            store_id,
            register_id,
        }
    }
}

impl Drop for PresenceGuard {
    fn drop(&mut self) {
        if let Some(register_id) = self.register_id {
            if self.hub.disconnect_register(self.store_id, register_id) {
                self.hub.publish(
                    self.store_id,
                    StreamKind::RegisterPresence,
                    serde_json::json!({
                        "event": "disconnected",
                        "register_id": register_id.to_string(),
                        "present": self.hub.present_registers(self.store_id)
                            .iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                    }),
                );
            }
        }
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
    /// Identifies the connecting register for live presence tracking.
    #[serde(default)]
    pub register_id: Option<Uuid>,
    /// Not yet used for replay (we don't persist history in v0.6.0); clients receive new
    /// messages after subscription.
    #[serde(default)]
    pub since: Option<u64>,
}

/// Build the replay batch for a reconnecting client. Returns the retained events to
/// resend and, when the gap is unrecoverable, a synthetic `resnapshot_required` envelope
/// instructing the client to refetch full state from `/pos/snapshot`.
fn build_replay(
    state: &AppState,
    store_id: Uuid,
    since: u64,
) -> (Vec<StreamEnvelope>, Option<StreamEnvelope>) {
    match state.stream.events_since(store_id, since) {
        ReplayResult::Replay(events) => (events, None),
        ReplayResult::ResnapshotRequired => (
            Vec::new(),
            Some(StreamEnvelope {
                version: "1.0.0".into(),
                store_id,
                seq: state.stream.current_seq(store_id),
                kind: StreamKind::Heartbeat.as_str().into(),
                payload: serde_json::json!({
                    "resnapshot_required": true,
                    "reason": "requested seq predates retained history",
                }),
            }),
        ),
    }
}

/// GET /pos/stream — WebSocket real-time feed.
pub async fn pos_stream_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> impl IntoResponse {
    let store_id = q.store_id.unwrap_or(state.store_id);
    let register_id = q.register_id;
    let since = q.since;
    ws.on_upgrade(move |socket| handle_ws(socket, state, store_id, register_id, since))
}

async fn handle_ws(
    socket: WebSocket,
    state: AppState,
    store_id: Uuid,
    register_id: Option<Uuid>,
    since: Option<u64>,
) {
    metrics::increment_gauge!(STREAM_CONNECTIONS, 1.0);
    let _presence = PresenceGuard::new(&state.stream, store_id, register_id);
    // Subscribe before computing replay so no event published in between is lost.
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

    // Replay missed events (or tell the client to resnapshot) when `since` is supplied.
    let mut sent_through: u64 = 0;
    if let Some(since) = since {
        let (replay_msgs, resnapshot) = build_replay(&state, store_id, since);
        for env in &replay_msgs {
            sent_through = sent_through.max(env.seq);
            let text = serde_json::to_string(env).unwrap_or_else(|_| "{}".into());
            if sender.send(Message::Text(text)).await.is_err() {
                metrics::decrement_gauge!(STREAM_CONNECTIONS, 1.0);
                return;
            }
        }
        if let Some(env) = resnapshot {
            sent_through = sent_through.max(env.seq);
            let text = serde_json::to_string(&env).unwrap_or_else(|_| "{}".into());
            let _ = sender.send(Message::Text(text)).await;
        }
    }

    let forward = tokio::spawn(async move {
        while let Ok(env) = rx.recv().await {
            // Skip anything already covered by the replay batch to avoid duplicates.
            if env.seq != 0 && env.seq <= sent_through {
                continue;
            }
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
    // Subscribe before computing replay so no event published in between is lost.
    let rx = state.stream.subscribe(store_id);
    metrics::increment_gauge!(STREAM_CONNECTIONS, 1.0);
    // Held by the stream closure so presence is released when the SSE connection drops.
    let presence = PresenceGuard::new(&state.stream, store_id, q.register_id);

    // Replay batch (or resnapshot signal) prepended to the live stream when `since` is set.
    let mut replay: Vec<StreamEnvelope> = Vec::new();
    let mut sent_through: u64 = 0;
    if let Some(since) = q.since {
        let (msgs, resnapshot) = build_replay(&state, store_id, since);
        replay = msgs;
        if let Some(env) = resnapshot {
            replay.push(env);
        }
        sent_through = replay.iter().map(|e| e.seq).max().unwrap_or(0);
    }
    let replay_stream = tokio_stream::iter(replay);

    let live = BroadcastStream::new(rx).map(move |item| match item {
        Ok(e) => e,
        Err(_) => StreamEnvelope {
            version: "1.0.0".into(),
            store_id,
            seq: 0,
            kind: StreamKind::Heartbeat.as_str().into(),
            payload: serde_json::json!({"lagged": true}),
        },
    });

    // Unified envelope stream (replay then live), deduped against the replay batch, then
    // rendered to SSE events. `presence` is held by the closure so it drops with the stream.
    let stream = replay_stream.chain(live).filter_map(move |env| {
        let _ = &presence;
        let keep = env.seq == 0 || env.seq > sent_through;
        std::future::ready(keep.then(|| {
            let data = serde_json::to_string(&env).unwrap_or_else(|_| "{}".into());
            Ok::<_, Infallible>(Event::default().event(env.kind.clone()).data(data))
        }))
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// GET /pos/snapshot?store_id=... — full live state for clients that reconnected after a
/// gap (`resnapshot_required`). Returns current stock availability, present registers,
/// open parked carts, and the latest `seq` to resume `since` from.
pub async fn pos_snapshot(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let store_id = q.store_id.unwrap_or(state.store_id);

    let stock = apex_edge_storage::list_available_to_sell(&state.pool, store_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let items: Vec<serde_json::Value> = stock
        .into_iter()
        .map(|(item_id, available)| {
            serde_json::json!({
                "item_id": item_id.to_string(),
                "available_to_sell": available,
            })
        })
        .collect();

    let parked = apex_edge_storage::list_parked_carts(&state.pool, store_id, None)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let registers: Vec<String> = state
        .stream
        .present_registers(store_id)
        .into_iter()
        .map(|r| r.to_string())
        .collect();

    Ok(axum::Json(serde_json::json!({
        "store_id": store_id.to_string(),
        "seq": state.stream.current_seq(store_id),
        "stock": items,
        "registers": registers,
        "parked_carts": parked,
    })))
}

/// GET /pos/registers?store_id=... — list registers currently present (live connections).
pub async fn list_registers(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> axum::Json<serde_json::Value> {
    let store_id = q.store_id.unwrap_or(state.store_id);
    let registers: Vec<String> = state
        .stream
        .present_registers(store_id)
        .into_iter()
        .map(|r| r.to_string())
        .collect();
    axum::Json(serde_json::json!({
        "store_id": store_id.to_string(),
        "registers": registers,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn presence_tracks_connect_and_disconnect() {
        let hub = StreamHub::new();
        let store = Uuid::new_v4();
        let reg_a = Uuid::new_v4();
        let reg_b = Uuid::new_v4();

        assert!(
            hub.connect_register(store, reg_a),
            "first connect is present"
        );
        assert!(
            !hub.connect_register(store, reg_a),
            "second connection not a new presence"
        );
        assert!(hub.connect_register(store, reg_b));
        let mut present = hub.present_registers(store);
        present.sort();
        let mut expected = vec![reg_a, reg_b];
        expected.sort();
        assert_eq!(present, expected);

        // One of reg_a's two connections closes — still present.
        assert!(!hub.disconnect_register(store, reg_a));
        assert!(hub.present_registers(store).contains(&reg_a));
        // Last connection closes — becomes absent.
        assert!(hub.disconnect_register(store, reg_a));
        assert_eq!(hub.present_registers(store), vec![reg_b]);
    }

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
    async fn events_since_replays_retained_events() {
        let hub = StreamHub::new();
        let store = Uuid::new_v4();
        let s1 = hub.publish(store, StreamKind::CartUpdated, serde_json::json!({"n": 1}));
        let _s2 = hub.publish(store, StreamKind::CartUpdated, serde_json::json!({"n": 2}));
        let _s3 = hub.publish(store, StreamKind::CartUpdated, serde_json::json!({"n": 3}));

        // Client last saw s1; expect s2 and s3 replayed in order.
        match hub.events_since(store, s1) {
            ReplayResult::Replay(events) => {
                let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
                assert_eq!(seqs, vec![s1 + 1, s1 + 2]);
            }
            other => panic!("expected replay, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_since_current_returns_empty() {
        let hub = StreamHub::new();
        let store = Uuid::new_v4();
        let s1 = hub.publish(store, StreamKind::CartUpdated, serde_json::json!({}));
        assert_eq!(
            hub.events_since(store, s1),
            ReplayResult::Replay(Vec::new())
        );
    }

    #[tokio::test]
    async fn events_since_beyond_history_requires_resnapshot() {
        let hub = StreamHub::new();
        let store = Uuid::new_v4();
        // Overflow the ring buffer so the earliest events are evicted.
        for i in 0..(HISTORY_CAPACITY as u64 + 50) {
            hub.publish(store, StreamKind::Heartbeat, serde_json::json!({ "i": i }));
        }
        // Asking from seq 1 (long evicted) must require a resnapshot.
        assert_eq!(hub.events_since(store, 1), ReplayResult::ResnapshotRequired);
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
