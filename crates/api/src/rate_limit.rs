//! In-process rate limiting for auth and POS routes.
//!
//! Limits are per client IP, sliding 60-second window. Unset ConnectInfo (oneshot
//! tests) is not limited so existing handler tests stay independent of this layer.

use apex_edge_metrics::{RATE_LIMIT_DECISIONS_TOTAL, RATE_LIMIT_REJECTED_TOTAL};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateBucket {
    Auth,
    Pos,
}

impl RateBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Pos => "pos",
        }
    }

    pub fn from_path(path: &str) -> Option<Self> {
        if path.starts_with("/auth/") {
            Some(Self::Auth)
        } else if path.starts_with("/pos/") {
            Some(Self::Pos)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimitSettings {
    pub auth_per_minute: u32,
    pub pos_per_minute: u32,
}

impl Default for RateLimitSettings {
    fn default() -> Self {
        Self {
            auth_per_minute: 30,
            pos_per_minute: 120,
        }
    }
}

impl RateLimitSettings {
    pub fn from_env() -> Self {
        Self {
            auth_per_minute: env_u32("APEX_EDGE_RATE_LIMIT_AUTH_PER_MINUTE", 30),
            pos_per_minute: env_u32("APEX_EDGE_RATE_LIMIT_POS_PER_MINUTE", 120),
        }
    }

    fn limit_for(self, bucket: RateBucket) -> u32 {
        match bucket {
            RateBucket::Auth => self.auth_per_minute,
            RateBucket::Pos => self.pos_per_minute,
        }
    }
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

type Buckets = HashMap<(String, &'static str), VecDeque<Instant>>;

#[derive(Clone)]
pub struct RateLimiter {
    settings: RateLimitSettings,
    inner: Arc<Mutex<Buckets>>,
}

impl RateLimiter {
    pub fn new(settings: RateLimitSettings) -> Self {
        Self {
            settings,
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns true when the request is within quota.
    pub fn allow(&self, client_key: &str, bucket: RateBucket, now: Instant) -> bool {
        let limit = self.settings.limit_for(bucket);
        if limit == 0 {
            return true;
        }
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map
            .entry((client_key.to_string(), bucket.as_str()))
            .or_default();
        while entry
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= WINDOW)
        {
            entry.pop_front();
        }
        if entry.len() as u32 >= limit {
            return false;
        }
        entry.push_back(now);
        true
    }
}

pub async fn rate_limit_middleware(
    State(app): State<crate::pos::AppState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(bucket) = RateBucket::from_path(req.uri().path()) else {
        return next.run(req).await;
    };
    let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() else {
        return next.run(req).await;
    };
    let allowed = app
        .rate_limiter
        .allow(&addr.ip().to_string(), bucket, Instant::now());
    let outcome = if allowed { "allowed" } else { "rejected" };
    metrics::counter!(
        RATE_LIMIT_DECISIONS_TOTAL,
        "bucket" => bucket.as_str(),
        "outcome" => outcome
    )
    .increment(1);
    if !allowed {
        metrics::counter!(RATE_LIMIT_REJECTED_TOTAL, "bucket" => bucket.as_str()).increment(1);
        let mut resp = (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        resp.headers_mut()
            .insert("retry-after", axum::http::HeaderValue::from_static("60"));
        return resp;
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_and_pos_paths_are_bucketed_and_other_paths_are_not() {
        assert_eq!(
            RateBucket::from_path("/auth/sessions/exchange"),
            Some(RateBucket::Auth)
        );
        assert_eq!(RateBucket::from_path("/pos/command"), Some(RateBucket::Pos));
        assert_eq!(RateBucket::from_path("/health"), None);
        assert_eq!(RateBucket::from_path("/catalog/products"), None);
    }

    #[test]
    fn sliding_window_rejects_the_request_that_exceeds_the_limit() {
        let limiter = RateLimiter::new(RateLimitSettings {
            auth_per_minute: 2,
            pos_per_minute: 0,
        });
        let t0 = Instant::now();
        assert!(limiter.allow("127.0.0.1", RateBucket::Auth, t0));
        assert!(limiter.allow("127.0.0.1", RateBucket::Auth, t0));
        assert!(!limiter.allow("127.0.0.1", RateBucket::Auth, t0));
    }

    #[test]
    fn zero_limit_disables_the_bucket() {
        let limiter = RateLimiter::new(RateLimitSettings {
            auth_per_minute: 0,
            pos_per_minute: 0,
        });
        for _ in 0..50 {
            assert!(limiter.allow("10.0.0.1", RateBucket::Auth, Instant::now()));
        }
    }

    #[test]
    fn clients_are_isolated() {
        let limiter = RateLimiter::new(RateLimitSettings {
            auth_per_minute: 1,
            pos_per_minute: 1,
        });
        let now = Instant::now();
        assert!(limiter.allow("10.0.0.1", RateBucket::Auth, now));
        assert!(limiter.allow("10.0.0.2", RateBucket::Auth, now));
        assert!(!limiter.allow("10.0.0.1", RateBucket::Auth, now));
    }
}
