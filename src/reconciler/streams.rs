//! Deploying approved DataSource pipelines as Bento streams on the project runner (T-0646, PL-47).
//!
//! Approved pipelines reading external feeds via `kind: DataSource` are rendered into native Bento
//! stream definitions and applied over the runner's streams REST API. Live state reflects the
//! runner's actual response, never Git alone.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use jc_core::kinds::data_source::{DataSourceSpec, DataSourceType};
use jc_core::kinds::pipeline::{ComputeKind, PipelineSpec};
use jc_core::Condition;
use jcctl::bento::InputContext;
use serde_json::Value;

use crate::store::Mirror;

/// Timeout for requests sent to the runner's streams API.
const RUNNER_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a pipeline stream cannot be rendered from the manifest specifications.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The pipeline has no compute block or its compute kind is not bloblang.
    #[error("pipeline compute is missing or not bloblang")]
    MissingCompute,
    /// Serialization between norway and json values failed.
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    /// A custom configuration error.
    #[error("{0}")]
    Custom(String),
}

/// The result of attempting to apply one pipeline stream to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The runner accepted the stream and is executing it.
    Live,
    /// The pipeline is not eligible for streams mode (e.g. disabled or not a DataSource).
    Skipped(&'static str),
    /// The runner refused the stream, or an error occurred during render or network transport.
    Error(String),
}

/// Deploys and retires streams on project-specific pipeline runners.
pub struct StreamDeployer {
    runner_url: String,
    http: reqwest::Client,
    deployed: Mutex<HashSet<(String, String)>>,
}

impl StreamDeployer {
    /// Creates a new deployer targeting the specified runner URL template (may contain `{project}`).
    pub fn new(runner_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(RUNNER_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            runner_url: runner_url.into(),
            http,
            deployed: Mutex::new(HashSet::new()),
        }
    }

    /// Renders and PUTs every eligible Pipeline of `mirror`; returns (namespace, name, outcome).
    pub async fn converge(&self, mirror: &Mirror) -> Vec<(String, String, StreamOutcome)> {
        let mut outcomes = Vec::new();
        let mut current_live = HashSet::new();

        for ns in mirror.namespaces() {
            let page = mirror.list(&ns, "Pipeline", &crate::store::ListOptions::default());
            for envelope in page.items {
                let name = envelope.metadata.name.clone();
                let spec: PipelineSpec = match serde_json::from_value(envelope.spec.clone()) {
                    Ok(s) => s,
                    Err(err) => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!("invalid pipeline spec: {err}")),
                        ));
                        continue;
                    }
                };

                let ds_ref_name = match spec
                    .source
                    .as_ref()
                    .and_then(|s| s.data_source_ref.as_ref())
                    .map(|r| r.name().to_string())
                {
                    Some(name) => name,
                    None => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Skipped("not a DataSource pipeline"),
                        ));
                        continue;
                    }
                };

                if !eligible(&spec) {
                    let reason = if !spec.enabled {
                        "pipeline is disabled"
                    } else {
                        "compute is not bloblang"
                    };
                    outcomes.push((ns.clone(), name, StreamOutcome::Skipped(reason)));
                    continue;
                }

                let ds_env = match mirror.get(&ns, "DataSource", &ds_ref_name) {
                    Some(e) => e,
                    None => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!(
                                "data source {ds_ref_name} is not in the mirror"
                            )),
                        ));
                        continue;
                    }
                };

                let ds_spec: DataSourceSpec = match serde_json::from_value(ds_env.spec.clone()) {
                    Ok(s) => s,
                    Err(err) => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!("invalid data source spec: {err}")),
                        ));
                        continue;
                    }
                };

                let ep_name = spec
                    .target_endpoint
                    .to_string()
                    .rsplit(':')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let ep_env = match mirror.get(&ns, "Endpoint", &ep_name) {
                    Some(e) => e,
                    None => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!(
                                "target endpoint {ep_name} is not in the mirror"
                            )),
                        ));
                        continue;
                    }
                };

                let slug = match ep_env.spec.get("slug").and_then(Value::as_str) {
                    Some(s) => s.to_string(),
                    None => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!("target endpoint {ep_name} has no slug")),
                        ));
                        continue;
                    }
                };

                let stream_json =
                    match render_stream(&spec, &name, &ns, &ds_spec, &ds_ref_name, &slug) {
                        Ok(val) => val,
                        Err(err) => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(err.to_string()),
                            ));
                            continue;
                        }
                    };

                let outcome = self.deploy_stream(&ns, &name, &stream_json).await;
                if outcome == StreamOutcome::Live {
                    current_live.insert((ns.clone(), name.clone()));
                }
                outcomes.push((ns.clone(), name, outcome));
            }
        }

        // Retire streams that were deployed previously but are no longer active or eligible.
        let to_retire: Vec<(String, String)> = {
            let deployed = self.deployed.lock().unwrap_or_else(|p| p.into_inner());
            deployed.difference(&current_live).cloned().collect()
        };
        let mut by_project: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (ns, name) in to_retire {
            by_project.entry(ns).or_default().push(name);
        }
        for (project, names) in by_project {
            self.retire(&project, &names).await;
        }

        if let Ok(mut deployed) = self.deployed.lock() {
            *deployed = current_live;
        }

        outcomes
    }

    /// DELETE {runner}/streams/{name} for pipelines that were deployed last run and are gone or disabled now.
    pub async fn retire(&self, project: &str, names: &[String]) {
        let runner = self
            .runner_url
            .replace("{project}", project)
            .trim_end_matches('/')
            .to_owned();
        for name in names {
            let url = format!("{runner}/streams/{name}");
            match self.http.delete(&url).send().await {
                Ok(resp) => {
                    if !resp.status().is_success()
                        && resp.status() != reqwest::StatusCode::NOT_FOUND
                    {
                        tracing::warn!(
                            project = %project,
                            pipeline = %name,
                            status = %resp.status(),
                            "failed to delete stream from runner"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        project = %project,
                        pipeline = %name,
                        error = %err,
                        "failed to delete stream from runner"
                    );
                }
            }
            if let Ok(mut deployed) = self.deployed.lock() {
                deployed.remove(&(project.to_string(), name.clone()));
            }
        }
    }

    async fn deploy_stream(&self, project: &str, name: &str, stream_json: &Value) -> StreamOutcome {
        let runner = self
            .runner_url
            .replace("{project}", project)
            .trim_end_matches('/')
            .to_owned();
        let url = format!("{runner}/streams/{name}");

        let mut response = match self.http.put(&url).json(stream_json).send().await {
            Ok(resp) => resp,
            Err(err) => {
                return StreamOutcome::Error(format!("the pipeline runner did not answer: {err}"));
            }
        };

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            response = match self.http.post(&url).json(stream_json).send().await {
                Ok(resp) => resp,
                Err(err) => {
                    return StreamOutcome::Error(format!(
                        "the pipeline runner did not answer: {err}"
                    ));
                }
            };
        }

        let status = response.status();
        if status.is_success() {
            StreamOutcome::Live
        } else {
            let body = response.text().await.unwrap_or_default();
            let truncated = truncate_body(&body, 500);
            StreamOutcome::Error(format!("runner answered {status}: {truncated}"))
        }
    }
}

/// Renders a native Bento stream configuration for an approved DataSource pipeline.
pub fn render_stream(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    source: &DataSourceSpec,
    source_name: &str,
    slug: &str,
) -> Result<Value, RenderError> {
    let context = InputContext {
        source: source_name,
        project,
        pipeline: name,
    };
    let input_yaml = jcctl::bento::input_of(source, &context);
    let input_json: Value = serde_json::to_value(&input_yaml)?;

    let (input, input_processors) = if source.source_type == DataSourceType::Http {
        let interval = pipeline.period.as_deref().unwrap_or("60s");
        let http_client = match input_json {
            Value::Object(mut map) => map
                .remove("http_client")
                .unwrap_or_else(|| serde_json::json!({})),
            _ => serde_json::json!({}),
        };
        let p1 = serde_json::json!({
            "try": [{
                "http": http_client
            }]
        });
        let p2 = serde_json::json!({
            "mapping": "root = if errored() { deleted() } else { this }"
        });
        let inp = serde_json::json!({
            "generate": {
                "interval": interval,
                "mapping": "root = \"\""
            }
        });
        (inp, vec![p1, p2])
    } else {
        (input_json, Vec::new())
    };

    let mut processors = input_processors;

    for p in jcctl::bento::prepended_processors(source) {
        processors.push(serde_json::to_value(&p)?);
    }

    let bloblang = pipeline
        .compute
        .as_ref()
        .and_then(|c| c.bloblang.as_ref())
        .ok_or(RenderError::MissingCompute)?;
    processors.push(serde_json::json!({
        "mapping": bloblang
    }));

    processors.push(serde_json::json!({
        "mapping": "root = if this.type() == \"array\" { this } else { [this] }"
    }));
    processors.push(serde_json::json!({
        "unarchive": {
            "format": "json_array"
        }
    }));

    processors.push(serde_json::json!({
        "archive": {
            "format": "json_array"
        }
    }));

    let output = serde_json::json!({
        "http_client": {
            "url": format!("${{JC_GATEWAY_URL}}/api/endpoint/{slug}/ngsi-ld/v1/entityOperations/upsert?options=update"),
            "verb": "POST",
            "headers": {
                "Content-Type": "application/json"
            },
            "oauth2": {
                "enabled": true,
                "client_key": "${JC_CLIENT_ID}",
                "client_secret": "${JC_CLIENT_SECRET}",
                "token_url": "${JC_TOKEN_URL}"
            },
            "timeout": "30s",
            "rate_limit": "pipeline_egress"
        }
    });

    Ok(serde_json::json!({
        "input": input,
        "pipeline": {
            "processors": processors
        },
        "output": output
    }))
}

/// Checks whether a pipeline is eligible for deployment into the runner as a resident stream.
pub fn eligible(spec: &PipelineSpec) -> bool {
    let has_ds = spec
        .source
        .as_ref()
        .and_then(|s| s.data_source_ref.as_ref())
        .is_some();
    let bloblang_ok = spec.compute.as_ref().is_some_and(|c| {
        c.kind == ComputeKind::Bloblang && c.bloblang.as_ref().is_some_and(|b| !b.trim().is_empty())
    });
    has_ds && spec.enabled && bloblang_ok
}

/// Returns whether `spec` is configured to read an external DataSource.
pub fn is_data_source_pipeline(spec: &PipelineSpec) -> bool {
    spec.source
        .as_ref()
        .and_then(|s| s.data_source_ref.as_ref())
        .is_some()
}

/// One `StreamDeployed` condition with the runner's word on why.
pub fn make_condition(
    condition_type: &str,
    status: &str,
    reason: &str,
    message: &str,
) -> Condition {
    Condition {
        r#type: condition_type.to_string(),
        status: status.to_string(),
        reason: Some(reason.to_string()),
        message: Some(message.to_string()),
        last_transition_time: Some(chrono::Utc::now()),
    }
}

fn truncate_body(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn helsinki_pipeline_spec() -> PipelineSpec {
        serde_json::from_value(serde_json::json!({
            "class": "auto",
            "period": "60s",
            "source": {
                "dataSourceRef": {
                    "kind": "DataSource",
                    "name": "hsl-citybikes-free"
                }
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this.data.bikes"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
        }))
        .expect("valid PipelineSpec")
    }

    fn helsinki_datasource_spec() -> DataSourceSpec {
        serde_json::from_value(serde_json::json!({
            "type": "http",
            "http": {
                "url": "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_status.json",
                "timeout": "15s"
            }
        }))
        .expect("valid DataSourceSpec")
    }

    fn helsinki_test_mirror() -> Mirror {
        let mirror = Mirror::new();
        let ds = serde_json::json!({
            "type": "http",
            "http": {
                "url": "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_status.json",
                "timeout": "15s"
            }
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "DataSource".to_string(),
            metadata: ObjectMeta {
                name: "hsl-citybikes-free".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: ds,
            status: None,
        });

        let ep = serde_json::json!({
            "contextSpaceRef": "helsinki",
            "slug": "abc123456789012345678901234",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Endpoint".to_string(),
            metadata: ObjectMeta {
                name: "helsinki-all".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: ep,
            status: None,
        });

        let pipe = serde_json::json!({
            "class": "auto",
            "period": "60s",
            "source": {
                "dataSourceRef": {
                    "kind": "DataSource",
                    "name": "hsl-citybikes-free"
                }
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this.data.bikes"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Pipeline".to_string(),
            metadata: ObjectMeta {
                name: "citybikes-free".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: pipe,
            status: None,
        });

        mirror
    }

    #[test]
    fn renders_the_static_stream_shape() {
        let pipeline = helsinki_pipeline_spec();
        let ds = helsinki_datasource_spec();

        let rendered = render_stream(
            &pipeline,
            "citybikes-free",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "abc123",
        )
        .expect("rendered stream");

        assert_eq!(rendered["input"]["generate"]["interval"], "60s");
        assert_eq!(
            rendered["pipeline"]["processors"][0]["try"][0]["http"]["url"],
            "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_status.json"
        );

        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors array");
        assert!(processors
            .iter()
            .any(|p| p.get("mapping").and_then(Value::as_str) == Some("root = this.data.bikes")));
        assert!(processors.iter().any(|p| {
            p.get("mapping").and_then(Value::as_str)
                == Some("root = if this.type() == \"array\" { this } else { [this] }")
        }));
        assert!(processors.iter().any(|p| {
            p.get("unarchive")
                .and_then(|u| u.get("format"))
                .and_then(Value::as_str)
                == Some("json_array")
        }));
        assert!(processors.last().unwrap().get("archive").is_some());

        assert_eq!(
            rendered["output"]["http_client"]["url"],
            "${JC_GATEWAY_URL}/api/endpoint/abc123/ngsi-ld/v1/entityOperations/upsert?options=update"
        );
        assert_eq!(
            rendered["output"]["http_client"]["oauth2"]["client_secret"],
            "${JC_CLIENT_SECRET}"
        );

        let json_str = serde_json::to_string(&rendered).unwrap();
        assert!(!json_str.contains("Authorization"));
    }

    #[test]
    fn a_disabled_or_foreign_pipeline_is_not_eligible() {
        let mut spec = helsinki_pipeline_spec();
        assert!(eligible(&spec));

        spec.enabled = false;
        assert!(!eligible(&spec));

        spec.enabled = true;
        spec.source = None;
        assert!(!eligible(&spec));

        let mut spec2 = helsinki_pipeline_spec();
        spec2.compute.as_mut().unwrap().kind = ComputeKind::Wasm;
        assert!(!eligible(&spec2));
    }

    #[tokio::test]
    async fn put_success_is_live_and_refusal_is_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let outcomes = deployer.converge(&mirror).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].2, StreamOutcome::Live);

        let server_err = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(
                wiremock::ResponseTemplate::new(400).set_body_string("lint error in line 3"),
            )
            .mount(&server_err)
            .await;

        let deployer_err = StreamDeployer::new(server_err.uri());
        let outcomes_err = deployer_err.converge(&mirror).await;
        assert_eq!(outcomes_err.len(), 1);
        match &outcomes_err[0].2 {
            StreamOutcome::Error(err) => {
                assert!(err.contains("400") && err.contains("lint error"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn put_404_falls_back_to_post() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let outcomes = deployer.converge(&mirror).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].2, StreamOutcome::Live);
    }

    #[tokio::test]
    async fn retire_deletes_what_disappeared() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let outcomes = deployer.converge(&mirror).await;
        assert_eq!(outcomes[0].2, StreamOutcome::Live);

        let empty_mirror = Mirror::new();
        let outcomes_empty = deployer.converge(&empty_mirror).await;
        assert!(outcomes_empty.is_empty());
    }
}
