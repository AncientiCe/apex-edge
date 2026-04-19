//! Hub role (primary vs standby) for HA deployments.
//!
//! In standby mode the hub accepts reads but rejects writes with `503 Service
//! Unavailable + Retry-After`. A Litestream sidecar (or equivalent) is responsible for
//! replicating the primary's WAL into the standby's local SQLite file so failover is
//! near-instant.

use apex_edge_metrics::ROLE_GAUGE;
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::Response,
};

/// Hub role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubRole {
    Primary,
    Standby,
}

impl HubRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Standby => "standby",
        }
    }
    pub fn from_env() -> Self {
        match std::env::var("APEX_EDGE_STANDBY").ok().as_deref() {
            Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES") => Self::Standby,
            _ => Self::Primary,
        }
    }
}

/// Emit the `apex_edge_role{role=...}` gauge.
pub fn report_role(role: HubRole) {
    metrics::gauge!(ROLE_GAUGE, 1.0, "role" => role.as_str());
    let other = match role {
        HubRole::Primary => "standby",
        HubRole::Standby => "primary",
    };
    metrics::gauge!(ROLE_GAUGE, 0.0, "role" => other);
}

fn is_write_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn is_exempt_path(path: &str) -> bool {
    // These writes are safe on a read-only standby (they don't mutate domain state).
    matches!(path, "/health" | "/ready")
        || path.starts_with("/audit/verify")
        || path == "/metrics"
        || path == "/openapi.json"
        || path.starts_with("/docs")
}

/// Axum middleware: reject writes on standby role, pass everything else through.
pub async fn standby_guard_middleware(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    req: Request,
    next: Next,
) -> Response {
    if state.role == HubRole::Standby
        && is_write_method(req.method())
        && !is_exempt_path(req.uri().path())
    {
        let mut resp = Response::new(Body::from(
            "{\"error\":\"standby\",\"message\":\"This hub is in standby mode; route writes to the primary.\"}",
        ));
        *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        resp.headers_mut()
            .insert("Retry-After", HeaderValue::from_static("30"));
        resp.headers_mut()
            .insert("X-ApexEdge-Role", HeaderValue::from_static("standby"));
        resp.headers_mut()
            .insert("Content-Type", HeaderValue::from_static("application/json"));
        return resp;
    }
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(
        "X-ApexEdge-Role",
        HeaderValue::from_static(state.role.as_str()),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_methods_detected() {
        assert!(is_write_method(&Method::POST));
        assert!(is_write_method(&Method::PUT));
        assert!(is_write_method(&Method::DELETE));
        assert!(!is_write_method(&Method::GET));
        assert!(!is_write_method(&Method::OPTIONS));
    }

    #[test]
    fn exempt_paths_pass_through() {
        assert!(is_exempt_path("/health"));
        assert!(is_exempt_path("/ready"));
        assert!(is_exempt_path("/metrics"));
        assert!(is_exempt_path("/audit/verify"));
        assert!(is_exempt_path("/audit/verify?from=1"));
        assert!(!is_exempt_path("/pos/command"));
    }
}
