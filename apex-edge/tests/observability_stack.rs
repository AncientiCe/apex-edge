use std::fs;
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
}

#[test]
fn observability_stack_assets_are_present_and_wired() {
    let root = repo_root();

    let compose_path = root.join("docker-compose.observability.yml");
    let compose = fs::read_to_string(&compose_path)
        .unwrap_or_else(|e| panic!("missing compose file at {:?}: {e}", compose_path));
    assert!(
        compose.contains("prometheus:") && compose.contains("grafana:"),
        "compose should define prometheus and grafana services"
    );
    assert!(
        compose.contains("host.docker.internal:host-gateway"),
        "compose should include host-gateway mapping for Linux compatibility"
    );

    let prometheus_path = root.join("observability/prometheus/prometheus.yml");
    let prometheus = fs::read_to_string(&prometheus_path)
        .unwrap_or_else(|e| panic!("missing prometheus config at {:?}: {e}", prometheus_path));
    assert!(
        prometheus.contains("host.docker.internal:3000"),
        "prometheus scrape target should point to apex-edge host metrics endpoint"
    );
    assert!(
        prometheus.contains("apex-edge-recording.yml"),
        "prometheus config should load recording rules"
    );

    let rules_path = root.join("observability/prometheus/rules/apex-edge-recording.yml");
    let rules = fs::read_to_string(&rules_path)
        .unwrap_or_else(|e| panic!("missing prometheus rule file at {:?}: {e}", rules_path));
    assert!(
        rules.contains("apex_edge:http_request_rate_5m")
            && rules.contains("apex_edge:transaction_finalize_latency_p95_5m"),
        "recording rules should include core traffic and transaction latency signals"
    );

    let datasources_path =
        root.join("observability/grafana/provisioning/datasources/datasources.yml");
    let datasources = fs::read_to_string(&datasources_path).unwrap_or_else(|e| {
        panic!(
            "missing grafana datasource provisioning at {:?}: {e}",
            datasources_path
        )
    });
    assert!(
        datasources.contains("type: prometheus") && datasources.contains("http://prometheus:9090"),
        "grafana datasource should auto-wire prometheus service"
    );

    let dashboards_provider_path =
        root.join("observability/grafana/provisioning/dashboards/dashboards.yml");
    let provider = fs::read_to_string(&dashboards_provider_path).unwrap_or_else(|e| {
        panic!(
            "missing grafana dashboard provisioning at {:?}: {e}",
            dashboards_provider_path
        )
    });
    assert!(
        provider.contains("observability/grafana/dashboards"),
        "grafana dashboard provider should point at repository dashboards directory"
    );

    let dashboard_files = [
        (
            "edge-system-health.json",
            "Edge System Health",
            "apex_edge_http_requests_total",
        ),
        (
            "dependencies-and-data-flows.json",
            "Dependencies & Data Flows",
            "apex_edge_db_operations_total",
        ),
        (
            "transaction-journey.json",
            "Transaction Journey",
            "apex_edge_pos_commands_total",
        ),
    ];
    for (file, expected_title, expected_metric) in dashboard_files {
        let dashboard_path = root.join("observability/grafana/dashboards").join(file);
        let text = fs::read_to_string(&dashboard_path).unwrap_or_else(|e| {
            panic!(
                "missing grafana dashboard file {} at {:?}: {e}",
                file, dashboard_path
            )
        });
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!("dashboard {} should be valid JSON: {e}", file);
        });
        assert_eq!(
            json.get("title").and_then(|v| v.as_str()),
            Some(expected_title),
            "dashboard {} should have title {}",
            file,
            expected_title
        );
        assert!(
            text.contains(expected_metric),
            "dashboard {} should query {}",
            file,
            expected_metric
        );
        assert!(
            json.get("panels")
                .and_then(|v| v.as_array())
                .map(|p| !p.is_empty())
                .unwrap_or(false),
            "dashboard {} should include at least one panel",
            file
        );
    }
}
