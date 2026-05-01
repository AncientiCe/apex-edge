//! ApexEdge deployment conformance probe.

use serde::Serialize;

#[derive(Debug, Serialize)]
struct CheckResult {
    name: &'static str,
    ok: bool,
    detail: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let base_url = std::env::var("APEX_EDGE_CONFORMANCE_URL")
        .unwrap_or_else(|_| "http://localhost:3000".into());
    let client = reqwest::Client::new();
    let checks = vec![
        check_get(&client, &base_url, "health", "/health").await,
        check_get(&client, &base_url, "ready", "/ready").await,
        check_get(&client, &base_url, "openapi", "/openapi.json").await,
    ];
    let ok = checks.iter().all(|check| check.ok);
    println!("{}", serde_json::to_string_pretty(&checks)?);
    if ok {
        Ok(())
    } else {
        Err("conformance failed".into())
    }
}

async fn check_get(
    client: &reqwest::Client,
    base_url: &str,
    name: &'static str,
    path: &str,
) -> CheckResult {
    match client.get(format!("{base_url}{path}")).send().await {
        Ok(response) => {
            let status = response.status();
            CheckResult {
                name,
                ok: status.is_success(),
                detail: status.to_string(),
            }
        }
        Err(error) => CheckResult {
            name,
            ok: false,
            detail: error.to_string(),
        },
    }
}
