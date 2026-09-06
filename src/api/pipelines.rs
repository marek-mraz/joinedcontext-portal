//! Runtime numbers of one Bento stream, read from the project's pipeline runner.
//!
//! The runner serves Prometheus text on its own port (PL-24). The Portal scrapes it for the
//! browser so the UI never talks to a workload directly and the runner needs no ingress; the
//! response carries counters only, never logs, configuration or secret values (PL-17).

use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// How long the runner has to answer. A metrics port that is slow is a runner that is busy
/// ingesting; the view would rather show the pipeline without numbers than hang on it.
const SCRAPE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Default, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineMetrics {
    pub pipeline: String,
    /// When the Portal read the runner, RFC 3339. The counters are as old as this instant.
    pub scraped_at: String,
    /// Cumulative since the runner started, exactly as it reports them: a rate is the view's
    /// job, history is Prometheus' job (OPS-16). An absent field is not zero, it is a counter
    /// this runner does not export.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_depth: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_p99_ms: Option<f64>,
}

/// One sample line of the Prometheus text exposition format.
struct Sample<'a> {
    name: &'a str,
    labels: &'a str,
    value: f64,
}

fn parse_line(line: &str) -> Option<Sample<'_>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (name, labels, rest) = match line.split_once('{') {
        Some((name, tail)) => {
            let (labels, rest) = tail.split_once('}')?;
            (name, labels, rest)
        }
        None => {
            let (name, rest) = line.split_once(' ')?;
            (name, "", rest)
        }
    };
    let value = rest.split_whitespace().next()?.parse().ok()?;
    Some(Sample {
        name: name.trim(),
        labels,
        value,
    })
}

/// Reads one label. Bento's label values are stream names, metric paths and quantiles, so the
/// escaping rules of the exposition format never come up; a value with an escaped quote in it
/// would simply not match.
fn label<'a>(labels: &'a str, key: &str) -> Option<&'a str> {
    labels.split(',').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"'))
    })
}

/// The counter a metric name stands for. Bento is a Benthos fork: the names keep the
/// `input_received` / `output_sent` shape and a deployment may prefix them (`bento_`) or
/// suffix a counter with `_total`, so a family is matched by suffix, never by equality.
fn family(name: &str) -> Option<Family> {
    let name = name.strip_suffix("_total").unwrap_or(name);
    if name.ends_with("input_received") {
        Some(Family::Received)
    } else if name.ends_with("output_sent") {
        Some(Family::Sent)
    } else if name.ends_with("output_error") || name.ends_with("processor_error") {
        Some(Family::Errors)
    } else if name.ends_with("buffer_backlog") {
        Some(Family::BufferDepth)
    } else if name.ends_with("output_latency_ns") {
        Some(Family::LatencyNs)
    } else {
        None
    }
}

enum Family {
    Received,
    Sent,
    Errors,
    BufferDepth,
    LatencyNs,
}

/// Folds every series of one stream into the counters the view shows. Series of the same family
/// are summed (a stream may have several inputs or outputs); the latency takes the slowest
/// output rather than a sum, which would mean nothing.
/// The runner registers each stream under the pipeline's own name, so the pipeline name is
/// also the `stream` label to select on.
pub(crate) fn scrape(body: &str, pipeline: &str, scraped_at: String) -> PipelineMetrics {
    let mut metrics = PipelineMetrics {
        pipeline: pipeline.to_string(),
        scraped_at,
        ..PipelineMetrics::default()
    };

    for sample in body.lines().filter_map(parse_line) {
        if label(sample.labels, "stream") != Some(pipeline) {
            continue;
        }
        let Some(family) = family(sample.name) else {
            continue;
        };
        let add = |slot: &mut Option<u64>| *slot = Some(slot.unwrap_or(0) + sample.value as u64);
        match family {
            Family::Received => add(&mut metrics.received),
            Family::Sent => add(&mut metrics.sent),
            Family::Errors => add(&mut metrics.errors),
            Family::BufferDepth => add(&mut metrics.buffer_depth),
            Family::LatencyNs => {
                if label(sample.labels, "quantile") == Some("0.99") {
                    let ms = sample.value / 1_000_000.0;
                    metrics.latency_p99_ms =
                        Some(metrics.latency_p99_ms.map_or(ms, |seen| seen.max(ms)));
                }
            }
        }
    }

    metrics
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(SCRAPE_TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/metrics",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 200, description = "Runtime counters of the stream", body = PipelineMetrics),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 503, description = "No runner configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn get_metrics(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<PipelineMetrics>, ApiError> {
    let not_found = || ApiError::NotFound(format!("pipeline '{name}' not found in '{project}'"));
    // The names go into the runner URL, so they are checked before anything is built from them,
    // and a pipeline that is not mirrored is not disclosed as existing elsewhere (R20).
    if !is_dns1123(&project) || !is_dns1123(&name) {
        return Err(not_found());
    }
    state
        .mirror
        .get(&project, "Pipeline", &name)
        .ok_or_else(not_found)?;

    let template = state
        .config
        .pipeline_runner_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no pipeline runner is configured".into()))?;
    let url = format!(
        "{}/metrics",
        template
            .replace("{project}", &project)
            .trim_end_matches('/')
    );

    let response = http().get(&url).send().await.map_err(|err| {
        // The URL names an internal service; the reason is logged, the caller learns only that
        // the runner is unreachable.
        tracing::warn!(project = %project, error = %err, "pipeline runner metrics unreachable");
        ApiError::Unavailable("the pipeline runner did not answer".into())
    })?;
    if !response.status().is_success() {
        tracing::warn!(project = %project, status = %response.status(), "pipeline runner metrics refused");
        return Err(ApiError::Unavailable(
            "the pipeline runner did not answer".into(),
        ));
    }
    let body = response.text().await.map_err(|err| {
        tracing::warn!(project = %project, error = %err, "pipeline runner metrics unreadable");
        ApiError::Unavailable("the pipeline runner did not answer".into())
    })?;

    Ok(Json(scrape(&body, &name, now_rfc3339())))
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/pipelines/{name}/metrics",
        get(get_metrics),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"
# HELP input_received Benthos Counter metric
# TYPE input_received counter
input_received{label="mqtt",path="root.input",stream="aq-mqtt-ingest"} 128401
input_received{label="mqtt",path="root.input",stream="other-stream"} 77
output_sent{label="gateway",path="root.output",stream="aq-mqtt-ingest"} 128390
output_error{label="gateway",path="root.output",stream="aq-mqtt-ingest"} 2
processor_error{label="map",path="root.pipeline.processors.0",stream="aq-mqtt-ingest"} 1
buffer_backlog{stream="aq-mqtt-ingest"} 11
output_latency_ns{stream="aq-mqtt-ingest",quantile="0.5"} 1000000
output_latency_ns{stream="aq-mqtt-ingest",quantile="0.99"} 42500000
output_latency_ns_count{stream="aq-mqtt-ingest"} 128390
uptime_seconds 900
"#;

    fn scraped(stream: &str) -> PipelineMetrics {
        scrape(BODY, stream, "2026-09-06T16:20:11Z".into())
    }

    #[test]
    fn folds_the_series_of_one_stream() {
        let metrics = scraped("aq-mqtt-ingest");
        assert_eq!(
            metrics.received,
            Some(128401),
            "the neighbour's 77 is not ours"
        );
        assert_eq!(metrics.sent, Some(128390));
        assert_eq!(
            metrics.errors,
            Some(3),
            "output and processor errors are one number"
        );
        assert_eq!(metrics.buffer_depth, Some(11));
        assert_eq!(
            metrics.latency_p99_ms,
            Some(42.5),
            "nanoseconds are shown as milliseconds"
        );
    }

    #[test]
    fn a_stream_without_series_reports_no_numbers() {
        let metrics = scraped("not-running");
        assert_eq!(metrics.received, None, "absent is not zero");
        assert_eq!(metrics.errors, None);
        assert_eq!(metrics.latency_p99_ms, None);
    }

    #[test]
    fn prefixed_and_total_suffixed_names_are_the_same_families() {
        let body = concat!(
            "bento_input_received_total{stream=\"aq\"} 5\n",
            "bento_output_sent_total{stream=\"aq\"} 4\n"
        );
        let metrics = scrape(body, "aq", String::new());
        assert_eq!(metrics.received, Some(5));
        assert_eq!(metrics.sent, Some(4));
    }

    #[test]
    fn several_inputs_of_one_stream_are_summed() {
        let body = concat!(
            "input_received{label=\"a\",stream=\"aq\"} 5\n",
            "input_received{label=\"b\",stream=\"aq\"} 7\n",
            "output_latency_ns{label=\"a\",stream=\"aq\",quantile=\"0.99\"} 2000000\n",
            "output_latency_ns{label=\"b\",stream=\"aq\",quantile=\"0.99\"} 9000000\n"
        );
        let metrics = scrape(body, "aq", String::new());
        assert_eq!(metrics.received, Some(12));
        assert_eq!(
            metrics.latency_p99_ms,
            Some(9.0),
            "the slowest output, not their sum"
        );
    }

    #[test]
    fn comments_blank_lines_and_unlabelled_samples_are_ignored() {
        assert!(parse_line("# HELP x a counter").is_none());
        assert!(parse_line("   ").is_none());
        let sample = parse_line("uptime_seconds 900").expect("a sample without labels parses");
        assert_eq!(sample.name, "uptime_seconds");
        assert_eq!(sample.labels, "");
        assert_eq!(sample.value, 900.0);
    }
}
