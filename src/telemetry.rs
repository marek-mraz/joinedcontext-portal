//! The Portal's own Prometheus surface (OPS-16, TS-22).
//!
//! `components/monitoring` scrapes `/metrics` on the Portal's `http` port every fifteen
//! seconds. That is the same port APISIX publishes the Portal on, so the edge refuses this
//! one path (`components/portal/apisix-routes.yaml`): the series stay inside the cluster the
//! way `components/monitoring/README.md` says they do.
//!
//! Every series is named here and nowhere else. Labels are bounded by what the Portal
//! declares — a route pattern, a lane, a kind — and never by a project name, a resource name
//! or anything else a caller writes, because an unbounded label is a memory leak with a
//! disclosure attached.

use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::http::header::CONTENT_TYPE;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

use crate::change::Lane;
use crate::state::AppState;

/// The Prometheus text format, the version every scraper since 2014 reads.
pub const TEXT_FORMAT: &str = "text/plain; version=0.0.4";

/// Requests the Portal answered, by route and status.
const REQUESTS: &str = "jc_portal_requests_total";
/// How long it took to answer one.
const REQUEST_SECONDS: &str = "jc_portal_request_duration_seconds";
/// Changes proposed, by the lane they were classified into and the kind they touch (CC-63).
const CHANGES: &str = "jc_portal_changes_total";

/// The paths that describe the process rather than the traffic.
const UNCOUNTED: &[&str] = &["/metrics", "/api/v1/health"];

/// The bucket edges every duration here is counted into, in seconds.
///
/// Without them the exporter renders a duration as a *summary*: a quantile computed inside one
/// replica, and quantiles from two replicas cannot be combined into one. Buckets can be summed,
/// so `histogram_quantile` over these stays correct however many Portal pods are running.
const SECONDS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

fn handle() -> &'static PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let handle = PrometheusBuilder::new()
            .set_buckets(SECONDS)
            .expect("the bucket list is not empty")
            .install_recorder()
            .expect("this process installs the one recorder");
        metrics::describe_counter!(REQUESTS, "requests answered by the Portal");
        metrics::describe_histogram!(REQUEST_SECONDS, "seconds to answer one request");
        metrics::describe_counter!(CHANGES, "changes proposed, by lane and kind");
        handle
    })
}

/// Installs the recorder. Called when the router is built: the `metrics::` macros no-op until
/// a recorder exists, so a Portal that starts serving before this runs counts nothing.
pub fn install() {
    let _ = handle();
}

async fn metrics() -> Response {
    ([(CONTENT_TYPE, TEXT_FORMAT)], handle().render()).into_response()
}

/// One proposed change, as it was classified (CC-63, MF-21).
///
/// The lane is the number worth watching: a rise in red proposals is a change in what people
/// are asking the platform to do, and no other component can see it.
pub fn proposed(lane: Lane, kind: &'static str) {
    metrics::counter!(
        CHANGES,
        "lane" => match lane {
            Lane::Green => "green",
            Lane::Yellow => "yellow",
            Lane::Red => "red",
        },
        "kind" => kind,
    )
    .increment(1);
}

/// Counts and times every request the Portal answers.
pub async fn record(request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        // The static handler serves the single-page application, and every deep link of it
        // is one route as far as the Portal is concerned.
        .unwrap_or_else(|| "ui".to_owned());
    if UNCOUNTED.contains(&route.as_str()) {
        return next.run(request).await;
    }

    let method = request.method().as_str().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let seconds = started.elapsed().as_secs_f64();

    metrics::counter!(
        REQUESTS,
        "route" => route.clone(),
        "method" => method.clone(),
        "status" => response.status().as_u16().to_string(),
    )
    .increment(1);
    metrics::histogram!(REQUEST_SECONDS, "route" => route, "method" => method).record(seconds);
    response
}

/// The scrape, outside `/api/v1` and outside its CSRF and session guards: it carries no
/// request of anyone's and is refused at the edge.
pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics))
}
