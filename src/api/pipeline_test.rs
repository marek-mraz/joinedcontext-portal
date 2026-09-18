//! Testing a candidate pipeline before it is proposed (T-0591, PL-43, MF-38, API/01 §7a).
//!
//! The mapping runs where it will run: as an ephemeral stream on the project's runner, through
//! its streams API. The Portal renders the harness with jcctl, creates the stream, waits at
//! most three seconds for what the harness posts back to the capture route on the internal
//! listener, deletes the stream whatever happened, and answers the trace. Nothing is written
//! and no `secretRef` is resolved: the harness carries the sample and the mapping, nothing else.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::{PipelineSpec, Verb};
use jcctl::pipeline_test::{
    harness, lint_errors, trace, Captured, Sample, SampleFormat, TestError, TestTrace, EMPTY_BODY,
    FETCH_FAILED, MAX_MESSAGES, MAX_SAMPLE_BYTES, STREAM_PREFIX,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::agents::share;
use crate::api::dry_run::Probe;
use crate::api::pipelines::http;
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// How long the runner has to produce the trace (Architecture/08 §7).
const DEADLINE: Duration = Duration::from_secs(3);
/// Silence after the last captured message that ends the wait early.
const QUIET: Duration = Duration::from_millis(300);
/// The request body: the sample's five mebibytes plus the manifest and the JSON around them.
const BODY_LIMIT: usize = MAX_SAMPLE_BYTES + 1024 * 1024;

/// The request of API/01 §7a.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRequest {
    pub pipeline: Value,
    pub sample: Sample,
}

/// A test in flight: which project it belongs to and where its captured messages go.
struct Running {
    project: String,
    sender: mpsc::UnboundedSender<Captured>,
}

/// One test per project at a time, keyed by the test id the capture route is called with.
static RUNNING: LazyLock<Mutex<HashMap<String, Running>>> = LazyLock::new(Mutex::default);

/// Holds the project's slot while the test runs and frees it however the test ends.
struct Slot(String);

impl Slot {
    fn take(
        project: &str,
        id: &str,
        sender: mpsc::UnboundedSender<Captured>,
    ) -> Result<Self, ApiError> {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running.values().any(|r| r.project == project) {
            return Err(ApiError::Conflict(format!(
                "a pipeline test is already running in '{project}'; wait for it"
            )));
        }
        running.insert(
            id.to_owned(),
            Running {
                project: project.to_owned(),
                sender,
            },
        );
        Ok(Self(id.to_owned()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// The manifest's spec, once the kind has accepted the whole manifest (MF-37).
fn spec_of(pipeline: &Value, project: &str) -> Result<PipelineSpec, ApiError> {
    if pipeline["kind"] != "Pipeline" {
        return Err(ApiError::BadRequest(
            "pipeline.kind must be 'Pipeline'".into(),
        ));
    }
    // A candidate belongs to the project of the URL: the studio's draft names no namespace,
    // and one naming another project would be tested on the wrong runner.
    let mut pipeline = pipeline.clone();
    match pipeline["metadata"]["namespace"].as_str() {
        None => {
            if let Some(metadata) = pipeline["metadata"].as_object_mut() {
                metadata.insert("namespace".into(), Value::String(project.to_owned()));
            }
        }
        Some(namespace) if namespace != project => {
            return Err(ApiError::BadRequest(format!(
                "pipeline.metadata.namespace '{namespace}' is not the project '{project}'"
            )));
        }
        Some(_) => {}
    }
    let text = serde_json::to_string(&pipeline).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Some(checked) = jc_core::registry::validate_yaml("Pipeline", &text) {
        checked.map_err(|e| ApiError::BadRequest(format!("spec is not a valid Pipeline: {e}")))?;
    }
    serde_json::from_value(pipeline["spec"].clone())
        .map_err(|e| ApiError::BadRequest(format!("spec is not a valid Pipeline: {e}")))
}

/// Waits for what the harness posts: until the runner has been quiet after its first message,
/// until enough messages, or until the deadline.
async fn collect(receiver: &mut mpsc::UnboundedReceiver<Captured>) -> Vec<Captured> {
    let mut captured = Vec::new();
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while captured.len() < MAX_MESSAGES {
        let wait = if captured.is_empty() {
            deadline
        } else {
            (tokio::time::Instant::now() + QUIET).min(deadline)
        };
        match tokio::time::timeout_at(wait, receiver.recv()).await {
            Ok(Some(message)) => captured.push(message),
            _ => break,
        }
    }
    captured
}

/// Executes a candidate pipeline test and returns the trace (PL-43, MF-38).
pub async fn execute_test_pipeline(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    request: TestRequest,
) -> Result<TestTrace, ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let spec = spec_of(&request.pipeline, project)?;
    let has_source = spec
        .source
        .as_ref()
        .is_some_and(|s| s.data_source_ref.is_some() || s.endpoint_ref.is_some());
    if !has_source {
        return Err(ApiError::BadRequest(
            "pipeline.spec.source must declare dataSourceRef or endpointRef".into(),
        ));
    }
    crate::permissions::for_request(state, identity, project).check(
        "Pipeline",
        Verb::Propose,
        Some(&request.pipeline),
    )?;
    run_harness(state, project, &spec, &request.sample).await
}

/// `POST /api/v1/projects/{project}/pipelines/test` (PL-43, MF-38).
pub async fn test_pipeline(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<TestRequest>,
) -> Result<Json<TestTrace>, ApiError> {
    execute_test_pipeline(&user.0.identity, &state, &project, request)
        .await
        .map(Json)
}

/// One run of `spec` over `sample` on the project's runner: the harness (PL-43) as an
/// ephemeral stream, its captured messages as the trace, the stream deleted whatever happened.
pub(crate) async fn run_harness(
    state: &AppState,
    project: &str,
    spec: &PipelineSpec,
    sample: &Sample,
) -> Result<TestTrace, ApiError> {
    let runner = state
        .config
        .pipeline_runner_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no pipeline runner is configured".into()))?
        .replace("{project}", project)
        .trim_end_matches('/')
        .to_owned();
    let capture = state
        .config
        .pipeline_test_capture_url
        .as_deref()
        .ok_or_else(|| {
            ApiError::Unavailable("no capture route is configured for pipeline tests".into())
        })?;

    let id = share::slug();
    let config = harness(
        spec,
        sample,
        &format!("{capture}/internal/pipeline-tests/{id}"),
    )
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let _slot = Slot::take(project, &id, sender)?;

    let stream = format!("{runner}/streams/{STREAM_PREFIX}{id}");
    let created = http().post(&stream).json(&config).send().await.map_err(|err| {
        tracing::warn!(project = %project, error = %err, "pipeline runner unreachable for a test");
        ApiError::Unavailable("the pipeline runner did not answer".into())
    })?;
    let status = created.status();
    let refusal = created.text().await.unwrap_or_default();

    let mut result = if status.is_success() {
        trace(&collect(&mut receiver).await)
    } else if status.is_client_error() {
        TestTrace {
            errors: lint_errors(&refusal),
            ..TestTrace::default()
        }
    } else {
        tracing::warn!(project = %project, status = %status, "pipeline runner refused a test stream");
        return Err(ApiError::Unavailable(
            "the pipeline runner did not answer".into(),
        ));
    };

    // Gone whatever happened; a runner that keeps it answers 409 on the next test's create,
    // which is why that failure is worth a log line and nothing else.
    if let Err(err) = http().delete(&stream).send().await {
        tracing::warn!(project = %project, error = %err, "pipeline test stream not deleted");
        result.errors.push(TestError {
            stage: "runner".into(),
            line: None,
            message: "the test stream could not be deleted; the runner keeps it until it is".into(),
        });
    }
    Ok(result)
}

/// The URL an `http` DataSource is probed at, or why it is not (MF-39): `None` for every other
/// type, a skipped probe for a source that declares a credential (a dry run resolves none,
/// MF-38) or names no URL.
pub(crate) fn probe_plan(spec: &Value) -> Option<Result<String, Probe>> {
    if spec["type"] != "http" {
        return None;
    }
    let http = &spec["http"];
    if !http["authorization"].is_null() {
        return Some(Err(Probe::skipped(
            "the source declares a credential; a dry run resolves none (MF-38)",
        )));
    }
    match http["url"].as_str() {
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
            Some(Ok(url.to_owned()))
        }
        _ => Some(Err(Probe::skipped("the source names no http(s) URL"))),
    }
}

/// One fetch of an `http` DataSource on the project's runner, beside the dry run's plan
/// (MF-39). Nothing here fails the dry run: a runner that is missing or busy, or a feed that
/// does not answer, is a skipped probe with the reason.
pub(crate) async fn probe_source(state: &AppState, project: &str, spec: &Value) -> Option<Probe> {
    let url = match probe_plan(spec)? {
        Ok(url) => url,
        Err(skipped) => return Some(skipped),
    };
    // The harness reads only `compute`, which a probe has none of; the rest is the minimum a
    // PipelineSpec needs to exist.
    let probe_spec: PipelineSpec = match serde_json::from_value(serde_json::json!({
        "class": "auto",
        "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
    })) {
        Ok(spec) => spec,
        Err(err) => return Some(Probe::skipped(format!("probe spec: {err}"))),
    };
    let sample = Sample {
        text: None,
        url: Some(url),
        format: SampleFormat::Json,
    };
    Some(
        match run_harness(state, project, &probe_spec, &sample).await {
            Ok(trace) => answer_of(trace),
            Err(err) => Probe::skipped(err.to_string()),
        },
    )
}

/// The probe's answer from its trace: the records, or what the fetch or the parse said. A test
/// stream the runner kept is already logged by `run_harness`, beside the answer, never in its place.
fn answer_of(trace: TestTrace) -> Probe {
    match trace.errors.iter().find(|error| error.stage != "runner") {
        None if trace.input.events > 0 => Probe {
            records: Some(trace.input.events),
            bytes: Some(trace.input.bytes),
            sample: trace.input.sample,
            skipped: None,
        },
        None => Probe::skipped("the feed answered nothing within the test's three seconds"),
        Some(error) => Probe::skipped(outcome_of(&error.message)),
    }
}

/// The reason a probe that ran gives for a feed that failed it, as opposed to a probe that did not
/// run at all (a credential, no runner, a test already running): every answer `answer_of` words
/// about the feed itself names the feed first.
pub(crate) fn feed_failed(probe: &Probe) -> Option<&str> {
    probe
        .skipped
        .as_deref()
        .filter(|reason| reason.starts_with("the feed "))
}

/// A failed fetch or parse in plain words: the HTTP status, a timeout, an empty body, or the
/// last clause of the transport or JSON error (`connection refused`, `no such host`).
fn outcome_of(error: &str) -> String {
    let last = |text: &str| text.rsplit(": ").next().unwrap_or(text).to_owned();
    let Some(fetch) = error.strip_prefix(FETCH_FAILED) else {
        return format!("the feed is not JSON: {}", last(error));
    };
    let status = fetch
        .split_once("unexpected response code (")
        .and_then(|(_, rest)| rest.split_once("): "))
        .map(|(_, status)| status.split(", Error:").next().unwrap_or(status));
    match status {
        Some(status) => format!("the feed answered {status}"),
        None if fetch.contains("Client.Timeout") || fetch.contains("deadline exceeded") => {
            "the feed did not answer within the test's three seconds".to_owned()
        }
        None if fetch.ends_with(EMPTY_BODY) => EMPTY_BODY.to_owned(),
        None => format!("the feed could not be reached: {}", last(fetch)),
    }
}

/// `POST /internal/pipeline-tests/{id}`: what the harness produced, one message per call.
///
/// The id is 130 random bits minted for this test and known to the harness alone; a message
/// for a test that is not running is dropped with a 404, whoever sent it.
pub async fn capture(Path(id): Path<String>, body: Bytes) -> StatusCode {
    let Ok(message) = serde_json::from_slice::<Captured>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let sent = RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|running| running.sender.send(message).is_ok());
    match sent {
        Some(true) => StatusCode::NO_CONTENT,
        _ => StatusCode::NOT_FOUND,
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/pipelines/test", post(test_pipeline))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

pub fn internal_router() -> Router<AppState> {
    // The harness posts a whole fetched feed back as one message, so the capture route takes
    // what the test route takes; axum's default two mebibytes turned a 1.2 MB feed into 413.
    Router::new()
        .route("/internal/pipeline-tests/{id}", post(capture))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn harness_for(sample: Sample) -> Value {
        let spec: PipelineSpec = serde_json::from_value(json!({
            "class": "auto",
            "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
        }))
        .expect("spec");
        harness(&spec, &sample, "http://portal-internal:9090/x").expect("harness")
    }

    #[test]
    fn a_url_sample_is_fetched_once_by_a_processor_and_a_text_sample_is_left_alone() {
        let url = harness_for(Sample {
            text: None,
            url: Some("https://feed.example/x.json".into()),
            format: SampleFormat::Json,
        });
        assert_eq!(url["input"]["generate"]["count"], 1);
        assert!(url["input"].get("http_client").is_none());
        let processors = url["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert_eq!(processors[0]["http"]["url"], "https://feed.example/x.json");
        assert_eq!(processors[0]["http"]["timeout"], "3s");
        assert!(processors[1]["mapping"]
            .as_str()
            .unwrap()
            .contains(EMPTY_BODY));
        assert!(processors[2]["catch"][0]["mapping"]
            .as_str()
            .unwrap()
            .starts_with("meta jc_fetch_error"));

        let text = harness_for(Sample {
            text: Some("[1]".into()),
            url: None,
            format: SampleFormat::Json,
        });
        assert!(text["input"]["generate"]["mapping"]
            .as_str()
            .unwrap()
            .contains("decode"));
        assert!(!text.to_string().contains("meta jc_fetch_error ="));
        assert!(text["pipeline"]["processors"][0].get("http").is_none());
    }

    #[test]
    fn a_message_that_failed_reaches_the_capture_route_as_an_envelope() {
        let config = harness_for(Sample {
            text: Some("<html>no</html>".into()),
            url: None,
            format: SampleFormat::Json,
        });
        let envelope = config["pipeline"]["processors"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["mapping"].as_str())
            .find(|m| m.starts_with("let failed = errored()"))
            .expect("jcctl's envelope");
        // If jcctl rewrites its envelope, the patch no longer applies and this says so.
        assert!(!envelope.contains("let out = this\n"), "{envelope}");
        assert!(envelope.contains("let out = if $failed { null } else { this }"));
        assert!(envelope.contains("meta(\"jc_fetch_error\").or(error())"));
    }

    fn trace_with(events: usize, errors: &[(&str, &str)]) -> TestTrace {
        TestTrace {
            input: jcctl::pipeline_test::InputStage {
                events,
                bytes: events * 10,
                sample: Some(json!({ "id": 1 })),
            },
            errors: errors
                .iter()
                .map(|(stage, message)| TestError {
                    stage: (*stage).into(),
                    line: None,
                    message: (*message).into(),
                })
                .collect(),
            ..TestTrace::default()
        }
    }

    #[test]
    fn the_probe_names_what_the_fetch_answered_and_a_kept_stream_never_replaces_it() {
        let kept = (
            "runner",
            "the test stream could not be deleted; the runner keeps it until it is",
        );
        let records = answer_of(trace_with(2, &[kept]));
        assert_eq!((records.records, records.bytes), (Some(2), Some(20)));
        assert!(records.skipped.is_none());

        // What Bento 1.21.1 posted for each outcome, run against a local feed.
        for (error, said) in [
            ("fetch: http://127.0.0.1:42019/x: Get \"http://127.0.0.1:42019/x\": dial tcp 127.0.0.1:42019: connect: connection refused", "the feed could not be reached: connection refused"),
            ("fetch: http://127.0.0.1:42013/missing: HTTP request returned unexpected response code (404): 404 Not Found, Error: not here", "the feed answered 404 Not Found"),
            ("fetch: http://127.0.0.1:42013/slow: Get \"http://127.0.0.1:42013/slow\": context deadline exceeded (Client.Timeout exceeded while awaiting headers)", "the feed did not answer within the test's three seconds"),
            ("fetch: https://127.0.0.1:42013/tls: Get \"https://127.0.0.1:42013/tls\": tls: first record does not look like a TLS handshake", "the feed could not be reached: first record does not look like a TLS handshake"),
            ("fetch: http://no-such-host.invalid/x: Get \"http://no-such-host.invalid/x\": dial tcp: lookup no-such-host.invalid on 192.168.65.7:53: no such host", "the feed could not be reached: no such host"),
            ("fetch: failed assignment (line 1): the feed answered an empty body", EMPTY_BODY),
            ("failed to parse message into JSON array: invalid character '<' looking for beginning of value", "the feed is not JSON: invalid character '<' looking for beginning of value"),
        ] {
            let answer = answer_of(trace_with(1, &[("mapping", error), kept]));
            assert_eq!(answer.skipped.as_deref(), Some(said));
            assert_eq!(feed_failed(&answer), Some(said));
        }
        assert_eq!(
            answer_of(trace_with(0, &[kept])).skipped.as_deref(),
            Some("the feed answered nothing within the test's three seconds")
        );
        assert!(feed_failed(&answer_of(trace_with(0, &[kept]))).is_some());
        assert!(feed_failed(&records).is_none());
        assert!(feed_failed(&Probe::skipped(
            "the source declares a credential; a dry run resolves none (MF-38)"
        ))
        .is_none());
    }

    #[test]
    fn a_manifest_of_another_kind_or_a_broken_spec_is_400() {
        assert!(spec_of(&json!({ "kind": "Endpoint", "spec": {} }), "helsinki").is_err());
        assert!(spec_of(&json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "x" },
            "spec": { "class": "scheduled", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:all" }
        }), "helsinki")
        .is_err(), "scheduled without a schedule is what the kind refuses (MF-37)");
        // The draft names no namespace: the project of the URL is filled in. Another project's
        // namespace is refused.
        let draft = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "x" },
            "spec": { "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:all" }
        });
        let spec = spec_of(&draft, "helsinki").expect("a valid pipeline");
        assert!(spec.compute.is_none());
        let mut foreign = draft.clone();
        foreign["metadata"]["namespace"] = json!("espoo");
        assert!(spec_of(&foreign, "helsinki").is_err());
        assert!(spec_of(&foreign, "espoo").is_ok());
    }

    #[test]
    fn only_an_http_source_without_a_credential_is_probed() {
        assert!(probe_plan(&json!({ "type": "mqtt", "mqtt": {} })).is_none());
        let with_credential = json!({ "type": "http", "http": {
            "url": "https://feeds.example/bikes.json",
            "authorization": { "scheme": "bearer", "headerRef": { "name": "feed", "key": "token" } }
        }});
        let skipped = probe_plan(&with_credential)
            .expect("http")
            .expect_err("skipped");
        assert!(skipped
            .skipped
            .as_deref()
            .is_some_and(|r| r.contains("MF-38")));
        assert!(!format!("{skipped:?}").contains("token\": \"")); // the ref's key, never a value
        assert_eq!(
            probe_plan(
                &json!({ "type": "http", "http": { "url": "https://feeds.example/bikes.json" } })
            ),
            Some(Ok("https://feeds.example/bikes.json".to_owned()))
        );
        assert!(
            probe_plan(&json!({ "type": "http", "http": { "url": "ftp://x" } }))
                .expect("http")
                .is_err()
        );
    }

    #[tokio::test]
    async fn one_test_per_project_and_the_slot_is_freed_when_it_ends() {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let first = Slot::take("slot-project", "id-1", sender.clone()).expect("free");
        let second = Slot::take("slot-project", "id-2", sender.clone());
        assert!(matches!(second, Err(ApiError::Conflict(_))));
        assert!(Slot::take("other-project", "id-3", sender.clone()).is_ok());
        drop(first);
        assert!(Slot::take("slot-project", "id-4", sender).is_ok());
    }

    #[tokio::test]
    async fn a_message_for_no_running_test_is_404_and_a_captured_one_reaches_the_test() {
        let body = Bytes::from(r#"{"input":"a","output":{"id":"x"},"error":null}"#);
        assert_eq!(
            capture(Path("unknown".into()), body.clone()).await,
            StatusCode::NOT_FOUND
        );
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let _slot = Slot::take("capture-project", "id-c", sender).expect("free");
        assert_eq!(
            capture(Path("id-c".into()), body).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            capture(Path("id-c".into()), Bytes::from("not json")).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            collect(&mut receiver).await.len(),
            1,
            "one message, then the quiet period ends the wait"
        );
    }
}
