//! Runs the virtual printer: a raw socket on 9100 and a browser page on 9101.

use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::net::TcpListener;
use virtual_printer::{build_app_with_state, socket, AppState, JobStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber_init();

    let print_port = env_port("VIRTUAL_PRINTER_PORT", 9100);
    let http_port = env_port("VIRTUAL_PRINTER_HTTP_PORT", 9101);

    let jobs = JobStore::default();
    let metrics_handle = PrometheusBuilder::new().install_recorder().ok();

    let printer = TcpListener::bind(("0.0.0.0", print_port)).await?;
    println!("Virtual printer accepting jobs on port {print_port}");
    tokio::spawn(socket::serve(printer, jobs.clone()));

    let app = build_app_with_state(AppState {
        jobs,
        metrics_handle,
    });
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], http_port));
    println!("Virtual printer page at http://localhost:{http_port}");
    axum::serve(TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

fn tracing_subscriber_init() {
    // Best effort: the tool is useful without logs, so a recorder that is already
    // installed is not worth failing startup over.
    let _ = tracing::subscriber::set_global_default(tracing_subscriber::FmtSubscriber::new());
}

fn env_port(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
