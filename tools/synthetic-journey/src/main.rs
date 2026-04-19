//! Synthetic POS journey prober.
//!
//! Runs a lightweight, deterministic "golden path" against an ApexEdge hub on a fixed
//! cadence and emits SLO-quality metrics on its own Prometheus endpoint:
//!
//!   apex_edge_synthetic_journey_total{step, outcome}
//!   apex_edge_synthetic_journey_duration_seconds{step}
//!
//! Steps covered:
//!   1. GET /health
//!   2. GET /metrics          (hub metrics exposure works)
//!   3. GET /audit/verify     (audit chain still valid)
//!   4. GET /openapi.json     (API surface intact)
//!
//! We deliberately keep steps read-only so the probe can run against any hub
//! (primary or standby) without causing state drift. A separate "writeful" probe
//! lives in the journey integration tests for CI.
//!
//! # Environment
//!
//! - APEX_EDGE_URL       (default http://127.0.0.1:3000)
//! - APEX_EDGE_INTERVAL  (secs between iterations, default 15)
//! - APEX_EDGE_TIMEOUT   (per-request timeout secs, default 5)
//! - SYNTHETIC_BIND      (prometheus bind address, default 0.0.0.0:9999)

use std::{net::SocketAddr, time::Duration};

const STEPS: &[(&str, &str)] = &[
    ("health", "/health"),
    ("metrics", "/metrics"),
    ("audit_verify", "/audit/verify"),
    ("openapi", "/openapi.json"),
];

fn getenv(k: &str, default: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .compact()
        .init();

    let base = getenv("APEX_EDGE_URL", "http://127.0.0.1:3000");
    let interval: u64 = getenv("APEX_EDGE_INTERVAL", "15").parse().unwrap_or(15);
    let timeout: u64 = getenv("APEX_EDGE_TIMEOUT", "5").parse().unwrap_or(5);
    let bind: SocketAddr = getenv("SYNTHETIC_BIND", "0.0.0.0:9999")
        .parse()
        .expect("valid SYNTHETIC_BIND");

    metrics_exporter_prometheus::PrometheusBuilder::new()
        .with_http_listener(bind)
        .install()
        .expect("install prometheus exporter");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout))
        .build()
        .expect("reqwest client");

    tracing::info!(%base, interval, timeout, %bind, "synthetic-journey starting");

    let mut ticker = tokio::time::interval(Duration::from_secs(interval));
    loop {
        ticker.tick().await;
        run_iteration(&client, &base).await;
    }
}

async fn run_iteration(client: &reqwest::Client, base: &str) {
    let total_start = std::time::Instant::now();
    let mut all_ok = true;
    for (label, path) in STEPS {
        let url = format!("{base}{path}");
        let step_start = std::time::Instant::now();
        let outcome = match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => "success",
            Ok(resp) => {
                tracing::warn!(step = label, status = %resp.status(), "step non-2xx");
                all_ok = false;
                "failure"
            }
            Err(err) => {
                tracing::warn!(step = label, error = %err, "step transport error");
                all_ok = false;
                "transport_error"
            }
        };
        metrics::counter!(
            "apex_edge_synthetic_journey_total",
            1u64,
            "step" => *label,
            "outcome" => outcome,
        );
        metrics::histogram!(
            "apex_edge_synthetic_journey_duration_seconds",
            step_start.elapsed().as_secs_f64(),
            "step" => *label,
        );
    }
    // End-to-end journey result: single counter the SLO burns against.
    let journey_outcome = if all_ok { "success" } else { "failure" };
    metrics::counter!(
        "apex_edge_synthetic_journey_total",
        1u64,
        "step" => "end_to_end",
        "outcome" => journey_outcome,
    );
    metrics::histogram!(
        "apex_edge_synthetic_journey_duration_seconds",
        total_start.elapsed().as_secs_f64(),
        "step" => "end_to_end",
    );
    tracing::info!(
        outcome = journey_outcome,
        elapsed_ms = total_start.elapsed().as_millis() as u64,
        "journey iteration complete"
    );
}
