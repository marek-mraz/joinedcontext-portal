//! Deploying approved DataSource pipelines as Bento streams on the project runner (T-0646, PL-47).
//!
//! Approved pipelines reading external feeds via `kind: DataSource` are rendered into native Bento
//! stream definitions and applied over the runner's streams REST API. Live state reflects the
//! runner's actual response, never Git alone.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use jc_core::kinds::data_source::{check_class, DataSourceSpec, DataSourceType};
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
    /// The pipeline class does not fit the data source (PL-04, PL-50).
    #[error("pipeline class does not fit data source: {0}")]
    Class(String),
    /// The author's `bento.yaml` is not YAML the runner would read.
    #[error("bento.yaml: {0}")]
    Bento(String),
    /// The author wrote an input and the pipeline names a DataSource too (PL-39).
    #[error("bento.yaml already declares an input, and spec.source.dataSourceRef names another one; keep one of the two")]
    BentoInput,
    /// Serialization between norway and json values failed.
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    /// A custom configuration error.
    #[error("{0}")]
    Custom(String),
}

/// The `bento.yaml` beside each Pipeline manifest, by `(project, pipeline)`: the author's
/// mapping the loader leaves alone (PL-03), read from the staged tree by the sync.
pub type Bentos = HashMap<(String, String), String>;

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
    /// The hash of the config each live stream was last PUT with: an unchanged render is not
    /// sent again, because the runner restarts a stream on every PUT and a periodic pipeline
    /// then emits on every pass instead of every period (PL-45, T-0659).
    rendered: Mutex<HashMap<(String, String), u64>>,
}

/// The rendered config as one number, so two passes can tell an unchanged stream apart.
fn config_hash(config: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config.to_string().hash(&mut hasher);
    hasher.finish()
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
            rendered: Mutex::new(HashMap::new()),
        }
    }

    /// The runner's Prometheus text for one project, or `None` when it does not answer.
    ///
    /// A stream can be Live and still read nothing — a source that refuses the runner's token,
    /// a mapping that throws on every message — and the counters are the only place that shows
    /// (T-0914).
    pub async fn metrics(&self, project: &str) -> Option<String> {
        let url = format!(
            "{}/metrics",
            self.runner_url
                .replace("{project}", project)
                .trim_end_matches('/')
        );
        let response = self.http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.text().await.ok()
    }

    /// Renders and PUTs every eligible Pipeline of `mirror`; returns (namespace, name, outcome).
    pub async fn converge(
        &self,
        mirror: &Mirror,
        bentos: &Bentos,
    ) -> Vec<(String, String, StreamOutcome)> {
        let mut outcomes = Vec::new();
        let mut current_live = HashSet::new();

        for ns in mirror.namespaces() {
            // What the runner holds, asked once per project and only when a render is unchanged.
            let mut running: Option<Option<HashSet<String>>> = None;
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

                let ds_ref_name = spec
                    .source
                    .as_ref()
                    .and_then(|s| s.data_source_ref.as_ref())
                    .map(|r| r.name().to_string());

                let ep_ref_name = spec
                    .source
                    .as_ref()
                    .and_then(|s| s.endpoint_ref.as_ref())
                    .map(|r| r.name().to_string());

                if ds_ref_name.is_none() && ep_ref_name.is_none() {
                    outcomes.push((
                        ns.clone(),
                        name,
                        StreamOutcome::Skipped("not a supported stream pipeline"),
                    ));
                    continue;
                }

                if !eligible(&spec) {
                    let reason = if !spec.enabled {
                        "pipeline is disabled"
                    } else if let Some(compute) = &spec.compute {
                        if compute.kind != ComputeKind::Bloblang {
                            "compute is not bloblang"
                        } else {
                            "endpoint pipeline requires bloblang mapping"
                        }
                    } else {
                        "not eligible for streams mode"
                    };
                    outcomes.push((ns.clone(), name, StreamOutcome::Skipped(reason)));
                    continue;
                }

                let target_ep_name = spec
                    .target_endpoint
                    .to_string()
                    .rsplit(':')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let ep_env = match mirror.get(&ns, "Endpoint", &target_ep_name) {
                    Some(e) => e,
                    None => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!(
                                "target endpoint {target_ep_name} is not in the mirror"
                            )),
                        ));
                        continue;
                    }
                };

                let target_slug = match ep_env.spec.get("slug").and_then(Value::as_str) {
                    Some(s) => s.to_string(),
                    None => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!(
                                "target endpoint {target_ep_name} has no slug"
                            )),
                        ));
                        continue;
                    }
                };

                let stream_json = if let Some(ds_name) = ds_ref_name {
                    let ds_env = match mirror.get(&ns, "DataSource", &ds_name) {
                        Some(e) => e,
                        None => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(format!(
                                    "data source {ds_name} is not in the mirror"
                                )),
                            ));
                            continue;
                        }
                    };

                    let ds_spec: DataSourceSpec = match serde_json::from_value(ds_env.spec.clone())
                    {
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

                    let bento = bentos.get(&(ns.clone(), name.clone())).map(String::as_str);
                    match render_stream(&spec, &name, &ns, &ds_spec, &ds_name, &target_slug, bento)
                    {
                        Ok(val) => val,
                        Err(err) => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(err.to_string()),
                            ));
                            continue;
                        }
                    }
                } else if let Some(source_ep_name) = ep_ref_name {
                    let source_ep_env = match mirror.get(&ns, "Endpoint", &source_ep_name) {
                        Some(e) => e,
                        None => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(format!(
                                    "source endpoint {source_ep_name} is not in the mirror"
                                )),
                            ));
                            continue;
                        }
                    };

                    let source_slug = match source_ep_env.spec.get("slug").and_then(Value::as_str) {
                        Some(s) => s,
                        None => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(format!(
                                    "source endpoint {source_ep_name} has no slug"
                                )),
                            ));
                            continue;
                        }
                    };

                    let source_is_public = source_ep_env
                        .spec
                        .get("audience")
                        .and_then(Value::as_str)
                        .is_none_or(|audience| audience == "public");

                    match render_endpoint_stream(
                        &spec,
                        &format!("{ns}/{name}"),
                        source_slug,
                        source_is_public,
                        &target_slug,
                    ) {
                        Ok(val) => val,
                        Err(err) => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(err.to_string()),
                            ));
                            continue;
                        }
                    }
                } else {
                    unreachable!();
                };

                let key = (ns.clone(), name.clone());
                let hash = config_hash(&stream_json);
                let mut unchanged = self
                    .rendered
                    .lock()
                    .map(|rendered| rendered.get(&key) == Some(&hash))
                    .unwrap_or(false);
                // A runner that restarted holds no streams, so an unchanged render it no longer
                // runs is sent again; a runner that does not answer the list keeps the hash's word.
                if unchanged {
                    if running.is_none() {
                        running = Some(self.running(&ns).await);
                    }
                    unchanged = running
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_none_or(|names| names.contains(&name));
                }
                let outcome = if unchanged {
                    StreamOutcome::Live
                } else {
                    self.deploy_stream(&ns, &name, &stream_json).await
                };
                if outcome == StreamOutcome::Live {
                    current_live.insert(key.clone());
                    if let Ok(mut rendered) = self.rendered.lock() {
                        rendered.insert(key, hash);
                    }
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
            if let Ok(mut rendered) = self.rendered.lock() {
                rendered.remove(&(project.to_string(), name.clone()));
            }
        }
    }

    /// The names of the streams the runner holds, or `None` when it gave no list.
    async fn running(&self, project: &str) -> Option<HashSet<String>> {
        let runner = self.runner_url.replace("{project}", project);
        let url = format!("{}/streams", runner.trim_end_matches('/'));
        let response = self.http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let streams: HashMap<String, Value> = response.json().await.ok()?;
        Some(streams.into_keys().collect())
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
///
/// `bento` is the author's `bento.yaml` beside the manifest, used when the pipeline has no
/// inline `compute` (PL-03).
#[allow(clippy::too_many_arguments)]
pub fn render_stream(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    source: &DataSourceSpec,
    source_name: &str,
    slug: &str,
    bento: Option<&str>,
) -> Result<Value, RenderError> {
    check_class(pipeline, source).map_err(|e| RenderError::Class(e.to_string()))?;

    let context = InputContext {
        source: source_name,
        project,
        pipeline: name,
        pipeline_spec: Some(pipeline),
    };
    let input_yaml = jcctl::bento::input_of(source, &context);
    let input_json: Value = serde_json::to_value(&input_yaml)?;

    let (input, input_processors) = if matches!(source.source_type, DataSourceType::Http) {
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
            "mutation": "root = if errored() { deleted() }"
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

    match (&pipeline.compute, bento) {
        (Some(compute), _) => {
            if compute.kind != ComputeKind::Bloblang {
                return Err(RenderError::MissingCompute);
            }
            if let Some(bloblang) = compute.bloblang.as_deref().filter(|b| !b.trim().is_empty()) {
                processors.push(serde_json::json!({
                    "mapping": bloblang
                }));
            }
        }
        (None, Some(bento)) => processors.extend(bento_processors(bento)?),
        (None, None) => {}
    }

    processors.push(serde_json::json!({
        "mapping": "root = if this.type() == \"array\" { this } else { [this] }"
    }));
    processors.push(serde_json::json!({
        "unarchive": {
            "format": "json_array"
        }
    }));
    // The gateway takes at most 1000 entities per batch operation; a page of 4000 stations
    // goes out as four requests, not one 400.
    processors.push(serde_json::json!({
        "split": {
            "size": 1000
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

/// The author's processors from a `bento.yaml`. Its `input` is refused, the DataSource is the
/// input (PL-39); its `output` and everything else at the top level are dropped: every stream
/// writes through the endpoint upsert rendered here (PL-16), and the runner's own resources
/// (rate limits, caches) come from its resources file.
fn bento_processors(bento: &str) -> Result<Vec<Value>, RenderError> {
    let config: Value =
        serde_yaml_ng::from_str(bento).map_err(|e| RenderError::Bento(e.to_string()))?;
    if config.get("input").is_some() {
        return Err(RenderError::BentoInput);
    }
    Ok(config
        .pointer("/pipeline/processors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn percent_encode(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for b in val.bytes() {
        match b {
            b' ' => out.push_str("%20"),
            b'&' => out.push_str("%26"),
            b'=' => out.push_str("%3D"),
            b'#' => out.push_str("%23"),
            b'%' => out.push_str("%25"),
            _ => out.push(b as char),
        }
    }
    out
}

/// Builds the Context Gateway entity query URL for an endpoint-sourced pipeline (PL-31).
pub fn endpoint_source_url(
    source_slug: &str,
    query: &jc_core::kinds::pipeline::SourceQuery,
) -> String {
    let mut params = Vec::new();
    if !query.ids.is_empty() {
        let ids_str = query
            .ids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        params.push(format!("id={}", percent_encode(&ids_str)));
    } else if let Some(entity_type) = &query.entity_type {
        params.push(format!("type={}", percent_encode(entity_type)));
    }

    if !query.attrs.is_empty() {
        params.push(format!("attrs={}", query.attrs.join(",")));
    }

    params.push("limit=1000".to_string());

    if let Some(q) = &query.q {
        if !q.is_empty() {
            params.push(format!("q={}", percent_encode(q)));
        }
    }
    if let Some(scope_q) = &query.scope_q {
        if !scope_q.is_empty() {
            params.push(format!("scopeQ={}", percent_encode(scope_q)));
        }
    }
    if let Some(geo_q) = &query.geo_q {
        if !geo_q.is_empty() {
            params.push(format!("geoQ={}", percent_encode(geo_q)));
        }
    }

    let query_str = params.join("&");
    format!("${{JC_GATEWAY_URL}}/api/endpoint/{source_slug}/ngsi-ld/v1/entities?{query_str}")
}

/// The runner's shared cache in which an on-change stream keeps the hash of the last page it
/// passed (PL-51); declared beside the runner's other shared resources.
pub const CHANGE_CACHE: &str = "pipeline_changes";
/// How often an on-change stream looks at its source when the pipeline names no period.
const CHANGE_POLL: &str = "10s";
/// The shortest look an on-change stream takes: a period under it is raised to it.
const CHANGE_POLL_FLOOR: Duration = Duration::from_secs(5);

/// `10s`, `2m`, `500ms`, `1h` as a duration; anything else is not one.
fn duration_of(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(number)),
        "s" => Some(Duration::from_secs(number)),
        "m" => Some(Duration::from_secs(number * 60)),
        "h" => Some(Duration::from_secs(number * 3600)),
        _ => None,
    }
}

/// The processors that pass a source page on only when a watched value changed since the page
/// the stream last passed (PL-51): the hash of every entity's id and watched values goes to the
/// message's metadata, the last hash comes out of the shared cache (a first run misses it, and
/// `catch` clears that miss), an equal hash drops the page, and a new one is stored before the
/// compute sees the page. Unlike a `dedupe`, a value that returns to an earlier one passes.
fn change_gate(stream_key: &str, watched: &[String]) -> Vec<Value> {
    let values = if watched.is_empty() {
        "e".to_owned()
    } else {
        watched
            .iter()
            .map(|attr| {
                let path = Value::String(format!("{attr}.value")).to_string();
                format!("e.get({path})")
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    vec![
        serde_json::json!({
            "mutation": format!(
                "meta jc_change = (if this.type() == \"array\" {{ this }} else {{ [this] }}).map_each(e -> [e.id, {values}]).format_json().string().hash(\"xxhash64\").encode(\"hex\")"
            )
        }),
        serde_json::json!({
            "branch": {
                "request_map": "root = \"\"",
                "processors": [{
                    "cache": { "resource": CHANGE_CACHE, "operator": "get", "key": stream_key }
                }],
                "result_map": "meta jc_last = this"
            }
        }),
        serde_json::json!({ "catch": [] }),
        serde_json::json!({ "mutation": "root = if @jc_last == @jc_change { deleted() }" }),
        serde_json::json!({
            "cache": {
                "resource": CHANGE_CACHE,
                "operator": "set",
                "key": stream_key,
                "value": "\"${! @jc_change }\""
            }
        }),
    ]
}

/// Renders a native Bento stream configuration for an endpoint-sourced pipeline (PL-31, PL-45):
/// on its clock, or, with `source.trigger.subscription`, on every change of a watched value
/// (PL-51). `stream_key` names the stream in the runner's shared change cache.
pub fn render_endpoint_stream(
    pipeline: &PipelineSpec,
    stream_key: &str,
    source_slug: &str,
    source_is_public: bool,
    target_slug: &str,
) -> Result<Value, RenderError> {
    let source = pipeline.source.as_ref();
    let query = source
        .and_then(|s| s.query.as_ref())
        .ok_or_else(|| RenderError::Custom("endpoint pipeline missing query".to_string()))?;
    let trigger = source.and_then(|s| s.trigger.as_ref());

    fn non_empty(v: &Option<String>) -> Option<&str> {
        v.as_deref().filter(|s| !s.trim().is_empty())
    }
    let interval = match trigger {
        Some(_) => match non_empty(&pipeline.period) {
            Some(period) => match duration_of(period) {
                Some(d) if d < CHANGE_POLL_FLOOR => {
                    format!("{}s", CHANGE_POLL_FLOOR.as_secs())
                }
                Some(_) => period.to_owned(),
                None => CHANGE_POLL.to_owned(),
            },
            None => CHANGE_POLL.to_owned(),
        },
        None => non_empty(&pipeline.period)
            .or_else(|| non_empty(&pipeline.schedule))
            .unwrap_or("60s")
            .to_owned(),
    };

    // A page the change gate hashes carries the watched attributes, whatever the query names.
    let mut query = query.clone();
    let watched: Vec<String> = match trigger {
        Some(trigger) if !trigger.subscription.watched_attributes.is_empty() => {
            trigger.subscription.watched_attributes.clone()
        }
        Some(_) => query.attrs.clone(),
        None => Vec::new(),
    };
    if !query.attrs.is_empty() {
        for attr in &watched {
            if !query.attrs.contains(attr) {
                query.attrs.push(attr.clone());
            }
        }
    }
    let source_url = endpoint_source_url(source_slug, &query);

    let input = serde_json::json!({
        "generate": {
            "interval": interval,
            "mapping": "root = \"\""
        }
    });

    // A public endpoint serves the public grant and expects no credential; the runner's token
    // is minted for the endpoints its ServiceAccount is bound to, and the gateway answers 401
    // to a token whose audience is another endpoint's slug — so sending one where none is
    // wanted is how a Live pipeline reads nothing (EP-27, PL-45, T-0914).
    let mut read = serde_json::json!({
        "url": source_url,
        "verb": "GET",
        "headers": {
            "Accept": "application/json"
        },
        "timeout": "30s"
    });
    if !source_is_public {
        read["oauth2"] = serde_json::json!({
            "enabled": true,
            "client_key": "${JC_CLIENT_ID}",
            "client_secret": "${JC_CLIENT_SECRET}",
            "token_url": "${JC_TOKEN_URL}"
        });
    }
    let p1 = serde_json::json!({ "try": [{ "http": read }] });
    let p2 = serde_json::json!({
        "mutation": "root = if errored() { deleted() }"
    });

    let mut processors = vec![p1, p2];
    if trigger.is_some() {
        processors.extend(change_gate(stream_key, &watched));
    }

    let bloblang = pipeline
        .compute
        .as_ref()
        .and_then(|c| c.bloblang.as_ref())
        .filter(|b| !b.trim().is_empty())
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
        "split": {
            "size": 1000
        }
    }));
    processors.push(serde_json::json!({
        "archive": {
            "format": "json_array"
        }
    }));

    let output = serde_json::json!({
        "http_client": {
            "url": format!("${{JC_GATEWAY_URL}}/api/endpoint/{target_slug}/ngsi-ld/v1/entityOperations/upsert?options=update"),
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
    if !spec.enabled {
        return false;
    }
    let has_ds = spec
        .source
        .as_ref()
        .and_then(|s| s.data_source_ref.as_ref())
        .is_some();
    if has_ds {
        return match &spec.compute {
            None => true,
            Some(c) => c.kind == ComputeKind::Bloblang,
        };
    }
    let has_ep = spec
        .source
        .as_ref()
        .and_then(|s| s.endpoint_ref.as_ref())
        .is_some()
        && spec
            .source
            .as_ref()
            .and_then(|s| s.query.as_ref())
            .is_some();
    if has_ep {
        return spec.compute.as_ref().is_some_and(|c| {
            c.kind == ComputeKind::Bloblang
                && c.bloblang.as_ref().is_some_and(|b| !b.trim().is_empty())
        });
    }
    false
}

/// Returns whether `spec` is configured to read an external DataSource.
pub fn is_data_source_pipeline(spec: &PipelineSpec) -> bool {
    spec.source
        .as_ref()
        .and_then(|s| s.data_source_ref.as_ref())
        .is_some()
}

/// Returns whether `spec` is configured as a stream pipeline (DataSource or Endpoint-sourced).
pub fn is_stream_pipeline(spec: &PipelineSpec) -> bool {
    if let Some(source) = &spec.source {
        if source.data_source_ref.is_some() {
            return true;
        }
        if source.endpoint_ref.is_some() && source.query.is_some() {
            return true;
        }
    }
    false
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
            None,
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
        // A batch operation at the gateway takes 1000 entities at most.
        assert_eq!(
            processors[processors.len() - 2]["split"]["size"],
            1000,
            "the batch is split before it is archived"
        );

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

    fn endpoint_pipeline_spec(
        type_name: Option<&str>,
        ids: Vec<jc_core::urn::Urn>,
        period: Option<&str>,
        schedule: Option<&str>,
    ) -> PipelineSpec {
        let mut source_json = serde_json::json!({
            "endpointRef": {
                "kind": "Endpoint",
                "name": "helsinki-all"
            },
            "query": {
                "attrs": ["availableBikeNumber"]
            }
        });
        if let Some(t) = type_name {
            source_json["query"]["type"] = serde_json::json!(t);
        }
        if !ids.is_empty() {
            source_json["query"]["ids"] = serde_json::json!(ids);
        }

        let mut pipe_json = serde_json::json!({
            "class": "scheduled",
            "source": source_json,
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki-kpi:kpi-writer"
        });
        if let Some(p) = period {
            pipe_json["period"] = serde_json::json!(p);
        }
        if let Some(s) = schedule {
            pipe_json["schedule"] = serde_json::json!(s);
        }
        serde_json::from_value(pipe_json).expect("valid endpoint PipelineSpec")
    }

    #[test]
    fn endpoint_source_renders_get_url_with_type_and_attrs() {
        let spec =
            endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], Some("15m"), None);
        let rendered = render_endpoint_stream(
            &spec,
            "helsinki/kpi",
            "source_slug_123",
            true,
            "target_slug_456",
        )
        .expect("rendered endpoint stream");

        assert_eq!(rendered["input"]["generate"]["interval"], "15m");
        let http = &rendered["pipeline"]["processors"][0]["try"][0]["http"];
        assert_eq!(
            http["url"],
            "${JC_GATEWAY_URL}/api/endpoint/source_slug_123/ngsi-ld/v1/entities?type=BikeHireDockingStation&attrs=availableBikeNumber&limit=1000"
        );
        assert_eq!(http["verb"], "GET");
        assert_eq!(http["headers"]["Accept"], "application/json");
        // A public source is read the way anyone reads it: no token, because the runner's
        // token names the endpoints its ServiceAccount is bound to and the gateway answers a
        // token for another slug 401 (T-0914).
        assert!(http["oauth2"].is_null(), "{http}");
        // The write always carries the runner's identity: nothing is written anonymously.
        assert_eq!(
            rendered["output"]["http_client"]["oauth2"]["client_key"],
            "${JC_CLIENT_ID}"
        );
        assert_eq!(
            rendered["output"]["http_client"]["url"],
            "${JC_GATEWAY_URL}/api/endpoint/target_slug_456/ngsi-ld/v1/entityOperations/upsert?options=update"
        );
    }

    #[test]
    fn a_source_that_is_not_public_is_read_with_the_runners_own_token() {
        let spec =
            endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], Some("15m"), None);
        let rendered = render_endpoint_stream(
            &spec,
            "helsinki/kpi",
            "source_slug_123",
            false,
            "target_slug_456",
        )
        .expect("rendered endpoint stream");

        let http = &rendered["pipeline"]["processors"][0]["try"][0]["http"];
        assert_eq!(http["oauth2"]["client_key"], "${JC_CLIENT_ID}");
        assert_eq!(http["oauth2"]["token_url"], "${JC_TOKEN_URL}");
    }

    #[test]
    fn a_schedule_is_the_clock_and_no_change_gate_is_rendered() {
        let spec = endpoint_pipeline_spec(
            Some("BikeHireDockingStation"),
            vec![],
            None,
            Some("*/15 * * * *"),
        );
        let rendered =
            render_endpoint_stream(&spec, "helsinki/kpi", "src", true, "dst").expect("render");
        assert_eq!(rendered["input"]["generate"]["interval"], "*/15 * * * *");
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert!(processors
            .iter()
            .all(|p| p.get("cache").is_none() && p.get("branch").is_none()));
    }

    #[test]
    fn an_on_change_trigger_renders_the_change_gate_before_the_compute() {
        let mut spec = endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], None, None);
        spec.source.as_mut().expect("source").trigger = Some(
            serde_json::from_value(serde_json::json!({
                "subscription": { "type": "BikeHireDockingStation", "watchedAttributes": ["availableBikeNumber", "status"] }
            }))
            .expect("trigger"),
        );
        let rendered = render_endpoint_stream(&spec, "helsinki/bikes-kpi", "src", true, "dst")
            .expect("render");

        // No period: the source is looked at every ten seconds, with the watched attributes.
        assert_eq!(rendered["input"]["generate"]["interval"], "10s");
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert!(processors[0]["try"][0]["http"]["url"]
            .as_str()
            .expect("url")
            .contains("attrs=availableBikeNumber,status&"));
        let hash = processors[2]["mutation"].as_str().expect("the hash");
        assert!(
            hash.contains(r#"e.get("availableBikeNumber.value"), e.get("status.value")"#),
            "{hash}"
        );
        assert_eq!(
            processors[3]["branch"]["processors"][0]["cache"],
            serde_json::json!({ "resource": CHANGE_CACHE, "operator": "get", "key": "helsinki/bikes-kpi" })
        );
        assert_eq!(processors[4], serde_json::json!({ "catch": [] }));
        assert_eq!(
            processors[5]["mutation"],
            "root = if @jc_last == @jc_change { deleted() }"
        );
        assert_eq!(processors[6]["cache"]["operator"], "set");
        assert_eq!(processors[6]["cache"]["key"], "helsinki/bikes-kpi");
        // The compute comes after the gate.
        assert_eq!(processors[7]["mapping"], "root = this");
    }

    #[test]
    fn an_on_change_period_under_five_seconds_is_raised_and_a_longer_one_kept() {
        let trigger: jc_core::kinds::pipeline::Trigger = serde_json::from_value(
            serde_json::json!({ "subscription": { "type": "BikeHireDockingStation" } }),
        )
        .expect("trigger");
        for (period, interval) in [("1s", "5s"), ("500ms", "5s"), ("30s", "30s"), ("2m", "2m")] {
            let mut spec =
                endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], Some(period), None);
            spec.source.as_mut().expect("source").trigger = Some(trigger.clone());
            let rendered = render_endpoint_stream(&spec, "k", "src", true, "dst").expect("render");
            assert_eq!(
                rendered["input"]["generate"]["interval"], interval,
                "{period}"
            );
            // No watched attributes named: the query's are watched.
            assert!(rendered["pipeline"]["processors"][2]["mutation"]
                .as_str()
                .expect("hash")
                .contains(r#"e.get("availableBikeNumber.value")"#));
        }
    }

    #[test]
    fn endpoint_source_ids_renders_id_param() {
        let urn: jc_core::urn::Urn = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:station-1"
            .parse()
            .expect("urn");
        let spec = endpoint_pipeline_spec(None, vec![urn], None, Some("*/15 * * * *"));
        let rendered = render_endpoint_stream(
            &spec,
            "helsinki/kpi",
            "source_slug_123",
            true,
            "target_slug_456",
        )
        .expect("rendered endpoint stream");

        assert_eq!(rendered["input"]["generate"]["interval"], "*/15 * * * *");
        let http = &rendered["pipeline"]["processors"][0]["try"][0]["http"];
        assert_eq!(
            http["url"],
            "${JC_GATEWAY_URL}/api/endpoint/source_slug_123/ngsi-ld/v1/entities?id=urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:station-1&attrs=availableBikeNumber&limit=1000"
        );
        assert!(!http["url"].as_str().unwrap().contains("type="));
    }

    #[test]
    fn datasource_without_compute_renders_without_error() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let rendered = render_stream(
            &spec,
            "citybikes-free",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "abc123",
            None,
        )
        .expect("renders without compute");
        let processors = rendered["pipeline"]["processors"].as_array().unwrap();
        assert!(!processors
            .iter()
            .any(|p| p.get("mapping").and_then(Value::as_str) == Some("root = this.data.bikes")));
    }

    const BENTO: &str = r#"
pipeline:
  processors:
    - mapping: root = this.data.stations
    - unarchive:
        format: json_array
output:
  http_client:
    url: https://somewhere.example/authored
"#;

    #[test]
    fn datasource_with_a_bento_mapping_renders_its_processors_and_the_endpoint_output() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let rendered = render_stream(
            &spec,
            "citybikes-gbfs",
            "helsinki",
            &ds,
            "hsl-citybikes-gbfs",
            "abc123",
            Some(BENTO),
        )
        .expect("renders from bento.yaml");
        let processors = rendered["pipeline"]["processors"].as_array().unwrap();
        // The HTTP poll and its error guard first, the author's two, then the shared tail.
        assert_eq!(processors[2]["mapping"], "root = this.data.stations");
        assert_eq!(processors[3]["unarchive"]["format"], "json_array");
        assert_eq!(processors.len(), 8);
        let url = rendered["output"]["http_client"]["url"].as_str().unwrap();
        assert!(url.contains("/api/endpoint/abc123/"), "{url}");
        assert!(!url.contains("authored"));
    }

    #[test]
    fn a_bento_with_its_own_input_is_refused_and_broken_yaml_names_itself() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let render =
            |bento: &str| render_stream(&spec, "p", "helsinki", &ds, "src", "abc123", Some(bento));
        assert!(matches!(
            render("input:\n  generate: {}\n").unwrap_err(),
            RenderError::BentoInput
        ));
        assert!(matches!(
            render("pipeline: [\n").unwrap_err(),
            RenderError::Bento(_)
        ));
        // An empty file is an author who has not written the mapping yet: the stream still renders.
        assert!(render("").is_ok());
    }

    #[tokio::test]
    async fn missing_source_endpoint_is_error() {
        let mirror = Mirror::new();
        let target_ep = serde_json::json!({
            "contextSpaceRef": "helsinki-kpi",
            "slug": "targetslug1234567890123456",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Endpoint".to_string(),
            metadata: ObjectMeta {
                name: "kpi-writer".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: target_ep,
            status: None,
        });

        let pipe = serde_json::json!({
            "class": "scheduled",
            "source": {
                "endpointRef": {
                    "kind": "Endpoint",
                    "name": "nonexistent-endpoint"
                },
                "query": {
                    "type": "BikeHireDockingStation",
                    "attrs": ["availableBikeNumber"]
                }
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki-kpi:kpi-writer"
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Pipeline".to_string(),
            metadata: ObjectMeta {
                name: "kpi-pipe".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: pipe,
            status: None,
        });

        let deployer = StreamDeployer::new("http://dummy-runner:4195");
        let outcomes = deployer.converge(&mirror, &Bentos::new()).await;
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0].2 {
            StreamOutcome::Error(msg) => {
                assert!(msg.contains("source endpoint nonexistent-endpoint is not in the mirror"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
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
        let outcomes = deployer.converge(&mirror, &Bentos::new()).await;
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
        let outcomes_err = deployer_err.converge(&mirror, &Bentos::new()).await;
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
        let outcomes = deployer.converge(&mirror, &Bentos::new()).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].2, StreamOutcome::Live);
    }

    #[tokio::test]
    async fn a_runner_that_restarted_gets_its_unchanged_streams_again() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;
        // The runner lists the stream after the first PUT, then restarts and lists nothing.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "citybikes-free": { "active": true } })),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        // PUT, then left running, then sent again to the empty runner: two PUTs in three passes.
        for _ in 0..3 {
            let outcomes = deployer.converge(&mirror, &Bentos::new()).await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live);
        }
    }

    #[test]
    fn an_http_feed_keeps_a_body_that_is_not_json() {
        let rendered = render_stream(
            &helsinki_pipeline_spec(),
            "p",
            "helsinki",
            &helsinki_datasource_spec(),
            "src",
            "abc123",
            None,
        )
        .expect("renders");
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        // `this` would parse an RSS or CSV body as JSON and fail on every fetch.
        assert_eq!(
            processors[1],
            serde_json::json!({ "mutation": "root = if errored() { deleted() }" })
        );
    }

    #[tokio::test]
    async fn an_unchanged_render_is_not_put_again_and_a_change_is() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        // Two passes over the same manifests: one PUT.
        for _ in 0..2 {
            let outcomes = deployer.converge(&mirror, &Bentos::new()).await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live);
        }
        // A changed period renders differently: the second PUT.
        let mut changed = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("the test pipeline");
        changed.spec["period"] = serde_json::json!("120s");
        mirror.upsert(changed);
        let outcomes = deployer.converge(&mirror, &Bentos::new()).await;
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
        let outcomes = deployer.converge(&mirror, &Bentos::new()).await;
        assert_eq!(outcomes[0].2, StreamOutcome::Live);

        let empty_mirror = Mirror::new();
        let outcomes_empty = deployer.converge(&empty_mirror, &Bentos::new()).await;
        assert!(outcomes_empty.is_empty());
    }

    #[test]
    fn runner_nats_source_renders_verbatim_and_refuses_scheduled_class() {
        let mut pipe = helsinki_pipeline_spec();
        pipe.class = jc_core::kinds::PipelineClass::Resident;
        pipe.period = None;

        let ds: DataSourceSpec = serde_json::from_value(serde_json::json!({
            "type": "nats",
            "input": {
                "urls": ["nats://nats.helsinki.fi:4222"],
                "subject": "city.bikes.updates"
            }
        }))
        .expect("valid runner DataSourceSpec");

        let rendered = render_stream(
            &pipe,
            "bikes-stream",
            "helsinki",
            &ds,
            "city-nats",
            "abc123456789",
            None,
        )
        .expect("renders nats stream");

        assert_eq!(
            rendered["input"]["nats"]["urls"][0],
            "nats://nats.helsinki.fi:4222"
        );
        assert_eq!(rendered["input"]["nats"]["subject"], "city.bikes.updates");
        assert_eq!(
            rendered["output"]["http_client"]["url"],
            "${JC_GATEWAY_URL}/api/endpoint/abc123456789/ngsi-ld/v1/entityOperations/upsert?options=update"
        );

        let mut sched_pipe = pipe;
        sched_pipe.class = jc_core::kinds::PipelineClass::Scheduled;
        sched_pipe.schedule = Some("*/10 * * * *".to_string());

        let err = render_stream(
            &sched_pipe,
            "bikes-stream",
            "helsinki",
            &ds,
            "city-nats",
            "abc123456789",
            None,
        )
        .unwrap_err();

        assert!(
            matches!(err, RenderError::Class(_)),
            "expected RenderError::Class, got: {err:?}"
        );
    }
}
