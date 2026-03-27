use std::sync::Arc;

use fake_hq::{build_app, storage::Storage, AppState};
use metrics_exporter_prometheus::PrometheusBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port = std::env::var("FAKE_HQ_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(3031);
    let db_path = std::env::var("FAKE_HQ_DB_PATH").unwrap_or_else(|_| "fake_hq.db".to_string());
    let storage = Arc::new(Storage::open(&db_path)?);
    storage.init_schema()?;
    let metrics_handle = PrometheusBuilder::new().install_recorder().ok();
    let state = Arc::new(AppState {
        storage,
        metrics_handle,
    });
    let app = build_app(state);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    println!("Fake HQ listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
