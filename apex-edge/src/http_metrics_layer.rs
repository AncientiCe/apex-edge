//! HTTP request metrics: count, duration, in-flight by route and status class.

use apex_edge_metrics::{
    request_path_to_route, status_class, HTTP_REQUESTS_IN_FLIGHT, HTTP_REQUESTS_TOTAL,
    HTTP_REQUEST_DURATION_SECONDS,
};
use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use std::time::Instant;
use tower::Layer;
use tower::Service;

/// Tower layer that records HTTP request count, duration, and in-flight gauge per route.
#[derive(Clone, Default)]
pub struct HttpMetricsLayer;

impl<S> Layer<S> for HttpMetricsLayer {
    type Service = HttpMetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HttpMetricsService { inner }
    }
}

#[derive(Clone)]
pub struct HttpMetricsService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for HttpMetricsService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let method = req.method().as_str().to_string();
        let path = req.uri().path().to_string();
        let route = request_path_to_route(&path);
        let route_labels = [("route", route)];
        metrics::increment_gauge!(HTTP_REQUESTS_IN_FLIGHT, 1.0, &route_labels);

        let start = Instant::now();
        let mut inner = self.inner.clone();
        let fut = async move {
            let res = inner.call(req).await;
            let status = res.as_ref().map(|r| r.status().as_u16()).unwrap_or(500);
            metrics::decrement_gauge!(HTTP_REQUESTS_IN_FLIGHT, 1.0, &route_labels);
            let counter_labels = [
                ("method", method),
                ("route", route.to_string()),
                ("status_class", status_class(status).to_string()),
            ];
            metrics::counter!(HTTP_REQUESTS_TOTAL, 1u64, &counter_labels);
            metrics::histogram!(
                HTTP_REQUEST_DURATION_SECONDS,
                start.elapsed().as_secs_f64(),
                &route_labels
            );
            res
        };
        Box::pin(fut)
    }
}
