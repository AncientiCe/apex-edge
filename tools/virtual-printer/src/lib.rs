//! A printer that exists only on screen.
//!
//! It listens on port 9100 like any networked receipt printer, decodes what it is sent,
//! and shows the result in a browser. That is the whole point: the printing path in
//! this repo can be verified end to end by anyone, on any platform, with no hardware
//! and no vendor simulator.

pub mod decode;
pub mod socket;
pub mod ui;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use metrics::counter;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;

pub use decode::{decode_job, Alignment, DecodedElement, DecodedJob, Dialect, LineStyle};

/// How many jobs are kept. A demo left running all afternoon must not grow without
/// bound, and nobody scrolls back past the last few receipts anyway.
const DEFAULT_CAPACITY: usize = 50;

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: u64,
    pub received_at: DateTime<Utc>,
    pub bytes: usize,
    pub dialect: Dialect,
    pub text: String,
    pub elements: Vec<DecodedElement>,
    pub warnings: Vec<String>,
    /// The raw stream, so a byte-level disagreement can be read directly.
    pub hex: String,
}

#[derive(Debug, Clone)]
pub struct JobStore {
    inner: Arc<Mutex<Inner>>,
    capacity: usize,
}

#[derive(Debug, Default)]
struct Inner {
    jobs: VecDeque<Job>,
    next_id: u64,
}

impl Default for JobStore {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl JobStore {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            capacity: capacity.max(1),
        }
    }

    pub fn record(&self, bytes: &[u8]) -> Job {
        let decoded = decode_job(bytes);
        let mut inner = self.inner.lock().expect("job store lock");
        inner.next_id += 1;
        let job = Job {
            id: inner.next_id,
            received_at: Utc::now(),
            bytes: bytes.len(),
            dialect: decoded.dialect,
            text: decoded.text(),
            elements: decoded.elements,
            warnings: decoded.warnings,
            hex: decode::hex(bytes),
        };
        inner.jobs.push_back(job.clone());
        while inner.jobs.len() > self.capacity {
            inner.jobs.pop_front();
        }

        counter!("virtual_printer_jobs_total", "dialect" => job.dialect.label()).increment(1);
        counter!("virtual_printer_bytes_total").increment(bytes.len() as u64);
        if !job.warnings.is_empty() {
            counter!("virtual_printer_decode_warnings_total").increment(job.warnings.len() as u64);
        }
        job
    }

    pub fn jobs(&self) -> Vec<Job> {
        self.inner
            .lock()
            .expect("job store lock")
            .jobs
            .iter()
            .cloned()
            .collect()
    }

    pub fn job(&self, id: u64) -> Option<Job> {
        self.inner
            .lock()
            .expect("job store lock")
            .jobs
            .iter()
            .find(|job| job.id == id)
            .cloned()
    }

    pub fn clear(&self) {
        self.inner.lock().expect("job store lock").jobs.clear();
    }
}

#[derive(Clone)]
pub struct AppState {
    pub jobs: JobStore,
    pub metrics_handle: Option<PrometheusHandle>,
}

pub fn build_app(jobs: JobStore) -> Router {
    build_app_with_state(AppState {
        jobs,
        metrics_handle: None,
    })
}

pub fn build_app_with_state(state: AppState) -> Router {
    Router::new()
        .route("/", get(ui::page))
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs", delete(clear_jobs))
        .route("/api/jobs/:id", get(get_job))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true}))
}

async fn metrics(State(state): State<AppState>) -> String {
    state
        .metrics_handle
        .as_ref()
        .map(PrometheusHandle::render)
        .unwrap_or_default()
}

async fn list_jobs(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "jobs": state.jobs.jobs() }))
}

async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<Job>, StatusCode> {
    state.jobs.job(id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn clear_jobs(State(state): State<AppState>) -> StatusCode {
    state.jobs.clear();
    StatusCode::NO_CONTENT
}
