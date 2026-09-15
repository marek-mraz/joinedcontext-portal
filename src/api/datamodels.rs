//! Reading and saving DataModel LinkML source and compiled schema artifacts (DM-01, DM-02, DM-22, DM-24, DM-56).

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use jc_core::kinds::{DataModelSpec, GeneratedArtifacts, SemVer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::api::mutate::{author_credentials, branch_name, create_or_reuse_branch};
use crate::auth::CurrentUser;
use crate::change::{Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, Operation, PlanSummary};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, FileWrite};
use crate::state::AppState;
use crate::tools::model_tools::{Artifacts, GenerateRequest, MAX_REQUEST_BYTES};

/// Single detected difference between the published model and the candidate LinkML source.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelChange {
    pub severity: String,
    pub subject: String,
    pub reason: String,
}

/// Result returned for `PUT /source?dryRun=All`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceDryRunResult {
    pub severity: String,
    pub changes: Vec<ModelChange>,
    pub version: String,
    pub artifacts: Artifacts,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePutQuery {
    pub version: Option<String>,
    #[serde(default, rename = "dryRun")]
    pub dry_run: Option<String>,
}

/// Confines `spec.linkml`: rejects absolute paths, `..` segments, and paths outside `datamodels/`.
pub fn confine_linkml_path(linkml: &str) -> Result<String, ApiError> {
    if linkml.starts_with('/') || linkml.contains('\\') {
        return Err(ApiError::BadRequest(
            "linkml path must be relative, not absolute".into(),
        ));
    }
    let trimmed = linkml.strip_prefix("./").unwrap_or(linkml);
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.split('/').any(|seg| seg == ".." || seg == ".")
    {
        return Err(ApiError::BadRequest(
            "linkml path must not contain '..' segments or be empty".into(),
        ));
    }
    if !trimmed.ends_with(".linkml.yaml") {
        return Err(ApiError::BadRequest(
            "linkml path must end with .linkml.yaml".into(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Decodes base64 Gitea content if base64 encoded, or returns string verbatim.
fn decode_content(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(bytes) = STANDARD.decode(trimmed.replace(['\n', '\r', ' '], "")) {
        if let Ok(s) = String::from_utf8(bytes) {
            if !s.is_empty()
                && s.chars()
                    .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
            {
                return s;
            }
        }
    }
    raw.to_string()
}

/// Reads the LinkML source from the forge for a DataModel in a project.
pub async fn read_source(state: &AppState, project: &str, name: &str) -> Result<String, ApiError> {
    let envelope = state
        .mirror
        .get(project, "DataModel", name)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "DataModel '{name}' not found in project '{project}'"
            ))
        })?;

    let space = envelope
        .spec
        .get("contextSpaceRef")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("DataModel spec missing contextSpaceRef".into()))?;

    let linkml = envelope
        .spec
        .get("linkml")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("DataModel spec missing linkml".into()))?;

    let confined = confine_linkml_path(linkml)?;

    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let default_branch = gitea.default_branch().await?;
    let repo_path = format!("projects/{project}/spaces/{space}/datamodels/{confined}");

    let file = gitea
        .get_file(&repo_path, &default_branch)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("source file '{repo_path}' not found in repository"))
        })?;

    Ok(decode_content(&file.content))
}

/// Classifies changes between previous and next LinkML YAML documents.
pub fn classify_linkml_changes(prev_val: &Value, next_val: &Value) -> Vec<ModelChange> {
    let mut changes = Vec::new();
    let empty_map = serde_json::Map::new();

    let prev_classes = prev_val
        .get("classes")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let next_classes = next_val
        .get("classes")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    for (name, prev_class) in prev_classes {
        if let Some(next_class) = next_classes.get(name) {
            let prev_class_slots: Vec<&str> = prev_class
                .get("slots")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let next_class_slots: Vec<&str> = next_class
                .get("slots")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for slot in prev_class_slots {
                if !next_class_slots.contains(&slot) {
                    changes.push(ModelChange {
                        severity: "breaking".into(),
                        subject: format!("{name}.{slot}"),
                        reason: format!("slot '{slot}' was removed from class '{name}'"),
                    });
                }
            }
        } else {
            changes.push(ModelChange {
                severity: "breaking".into(),
                subject: name.clone(),
                reason: format!("class '{name}' was removed"),
            });
        }
    }

    for (name, _) in next_classes {
        if !prev_classes.contains_key(name) {
            changes.push(ModelChange {
                severity: "additive".into(),
                subject: name.clone(),
                reason: format!("class '{name}' was added"),
            });
        }
    }

    let prev_slots = prev_val
        .get("slots")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let next_slots = next_val
        .get("slots")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    for (name, prev_slot) in prev_slots {
        if let Some(next_slot) = next_slots.get(name) {
            let prev_req = prev_slot
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let next_req = next_slot
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !prev_req && next_req {
                changes.push(ModelChange {
                    severity: "breaking".into(),
                    subject: name.clone(),
                    reason: format!("slot '{name}' became required"),
                });
            } else if prev_req && !next_req {
                changes.push(ModelChange {
                    severity: "additive".into(),
                    subject: name.clone(),
                    reason: format!("slot '{name}' is no longer required"),
                });
            }

            let prev_multi = prev_slot
                .get("multivalued")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let next_multi = next_slot
                .get("multivalued")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if prev_multi != next_multi {
                changes.push(ModelChange {
                    severity: "breaking".into(),
                    subject: name.clone(),
                    reason: format!("slot '{name}' multivalued changed"),
                });
            }

            let prev_range = prev_slot.get("range").and_then(Value::as_str);
            let next_range = next_slot.get("range").and_then(Value::as_str);
            if prev_range != next_range {
                let numeric = ["integer", "float", "double", "decimal"];
                let p = prev_range.unwrap_or("string");
                let n = next_range.unwrap_or("string");
                let widens = (p == "integer" && numeric.contains(&n) && n != "integer")
                    || (n == "string" && p != "string");
                if widens {
                    changes.push(ModelChange {
                        severity: "additive".into(),
                        subject: name.clone(),
                        reason: format!("range of slot '{name}' widened from {p} to {n}"),
                    });
                } else {
                    changes.push(ModelChange {
                        severity: "breaking".into(),
                        subject: name.clone(),
                        reason: format!("range of slot '{name}' changed from {p} to {n}"),
                    });
                }
            }
        } else {
            changes.push(ModelChange {
                severity: "breaking".into(),
                subject: name.clone(),
                reason: format!("slot '{name}' was removed"),
            });
        }
    }

    for (name, next_slot) in next_slots {
        if !prev_slots.contains_key(name) {
            let next_req = next_slot
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if next_req {
                changes.push(ModelChange {
                    severity: "breaking".into(),
                    subject: name.clone(),
                    reason: format!("new required slot '{name}' was added"),
                });
            } else {
                changes.push(ModelChange {
                    severity: "additive".into(),
                    subject: name.clone(),
                    reason: format!("new optional slot '{name}' was added"),
                });
            }
        }
    }

    changes
}

pub fn overall_severity(changes: &[ModelChange]) -> &'static str {
    if changes.iter().any(|c| c.severity == "breaking") {
        "breaking"
    } else if changes.iter().any(|c| c.severity == "additive") {
        "additive"
    } else {
        "none"
    }
}

pub fn bump_version(current: &SemVer, severity: &str) -> Result<SemVer, ApiError> {
    let (major, minor, patch) = (current.major(), current.minor(), current.patch());
    let bumped = match severity {
        "breaking" => format!("{}.0.0", major + 1),
        "additive" => format!("{}.{}.0", major, minor + 1),
        _ => format!("{}.{}.{}", major, minor, patch + 1),
    };
    SemVer::new(&bumped).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// The artifacts Model Tools renders from one LinkML source (DM-02); the export reuses it for a
/// model whose repository holds no JSON Schema (MF-41).
pub(crate) async fn compile_artifacts(
    state: &AppState,
    source: &str,
) -> Result<Artifacts, ApiError> {
    let base = state
        .config
        .model_tools_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no model tools service is configured".into()))?;
    let url = format!("{}/generate", base.trim_end_matches('/'));

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let response = client
        .post(&url)
        .json(&GenerateRequest {
            source: source.to_string(),
        })
        .send()
        .await
        .map_err(|err| {
            tracing::warn!(route = "generate", error = %err, "model tools unreachable");
            ApiError::Unavailable("the model tools service did not answer".into())
        })?;

    if !response.status().is_success() {
        return Err(ApiError::Unavailable(
            "the model tools service did not answer".into(),
        ));
    }

    response.json::<Artifacts>().await.map_err(|err| {
        tracing::warn!(route = "generate", error = %err, "model tools answered unreadably");
        ApiError::Unavailable("the model tools service did not answer".into())
    })
}

/// What saving `source` as the model `name` would do (DM-22, DM-24): the changes against the
/// published source, the version it takes (`version`, or the published one bumped by the
/// severity) and what Model Tools compiles, with the source parsed. A breaking change under the
/// published major and a source Model Tools refuses are errors. The route and the assistant's
/// `change_resource` check a source the same way (AG-77).
pub(crate) async fn check_source(
    state: &AppState,
    project: &str,
    name: &str,
    spec: &Value,
    source: &str,
    version: Option<&str>,
) -> Result<(SourceDryRunResult, Value), ApiError> {
    let next_val: Value = serde_yaml_ng::from_str(source)
        .map_err(|e| ApiError::BadRequest(format!("invalid yaml body: {e}")))?;

    let published_source = read_source(state, project, name).await.ok();
    let prev_val: Value = published_source
        .as_deref()
        .and_then(|s| serde_yaml_ng::from_str(s).ok())
        .unwrap_or(Value::Null);

    let changes = classify_linkml_changes(&prev_val, &next_val);
    let severity = overall_severity(&changes);

    let current_version_str = spec
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("0.1.0");
    let published_version = SemVer::new(current_version_str)
        .map_err(|e| ApiError::BadRequest(format!("invalid current version: {e}")))?;

    let target_version = match version {
        Some(v) => {
            SemVer::new(v).map_err(|e| ApiError::BadRequest(format!("invalid version: {e}")))?
        }
        None => bump_version(&published_version, severity)?,
    };

    if severity == "breaking" && target_version.major() <= published_version.major() {
        let breaking_reasons: Vec<String> = changes
            .iter()
            .filter(|c| c.severity == "breaking")
            .map(|c| c.reason.clone())
            .collect();
        let suggested = bump_version(&published_version, "breaking")?;
        return Err(ApiError::Invalid {
            detail: format!(
                "a breaking change cannot be saved under version {target_version}; publish it as {suggested}"
            ),
            errors: breaking_reasons,
        });
    }

    let artifacts = compile_artifacts(state, source).await?;
    if !artifacts.errors.is_empty() {
        return Err(ApiError::Invalid {
            detail: artifacts.errors.join("; "),
            errors: artifacts.errors.clone(),
        });
    }

    Ok((
        SourceDryRunResult {
            severity: severity.to_string(),
            changes,
            version: target_version.to_string(),
            artifacts,
        },
        next_val,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/datamodels/{name}/source",
    tag = "datamodels",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "DataModel name"),
    ),
    responses(
        (status = 200, description = "LinkML source in YAML format", content_type = "text/yaml", body = String),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 503, description = "Service unavailable", body = ProblemDetails),
    )
)]
pub async fn get_source(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let content = read_source(&state, &project, &name).await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/yaml; charset=utf-8")],
        content,
    )
        .into_response())
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/datamodels/{name}/source",
    tag = "datamodels",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "DataModel name"),
        ("version" = Option<String>, Query, description = "Target semver"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
    ),
    request_body(
        content = String,
        description = "LinkML source in YAML format",
        content_type = "text/yaml",
    ),
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run result", body = SourceDryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 503, description = "Service unavailable", body = ProblemDetails),
    )
)]
pub async fn put_source(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Query(query): Query<SourcePutQuery>,
    body: Bytes,
) -> Result<Response, ApiError> {
    if body.len() > MAX_REQUEST_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the source is larger than the {MAX_REQUEST_BYTES} byte limit"
        )));
    }

    let is_dry = query
        .dry_run
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("all") || s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let envelope = state
        .mirror
        .get(&project, "DataModel", &name)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "DataModel '{name}' not found in project '{project}'"
            ))
        })?;

    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "DataModel",
        jc_core::kinds::Verb::Propose,
        Some(&envelope.spec),
    )?;

    let space = envelope
        .spec
        .get("contextSpaceRef")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("DataModel spec missing contextSpaceRef".into()))?;

    let linkml = envelope
        .spec
        .get("linkml")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("DataModel spec missing linkml".into()))?;

    let confined_linkml = confine_linkml_path(linkml)?;

    let source_str = std::str::from_utf8(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid utf-8 body: {e}")))?
        .to_string();

    let (checked, next_val) = check_source(
        &state,
        &project,
        &name,
        &envelope.spec,
        &source_str,
        query.version.as_deref(),
    )
    .await?;

    if is_dry {
        return Ok((StatusCode::OK, Json(checked)).into_response());
    }
    let SourceDryRunResult {
        severity,
        version,
        artifacts,
        ..
    } = checked;
    let severity = severity.as_str();
    let target_version =
        SemVer::new(&version).map_err(|e| ApiError::BadRequest(format!("invalid version: {e}")))?;

    let major = target_version.major();
    let empty_map = serde_json::Map::new();
    let next_classes = next_val
        .get("classes")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let mut classes: Vec<String> = next_classes.keys().cloned().collect();
    classes.sort();

    let artifacts_spec = GeneratedArtifacts {
        json_schema: Some(format!("./json-schema/{name}.v{major}.json")),
        context: Some(format!("./context/{name}.v{major}.jsonld")),
        docs: Some(format!("./docs/{name}.md")),
        example: Some(format!("./examples/{name}.example.jsonld")),
    };

    let mut new_spec = envelope.spec.clone();
    new_spec["version"] = Value::String(target_version.to_string());
    new_spec["classes"] = serde_json::to_value(&classes).unwrap_or(Value::Array(Vec::new()));
    new_spec["artifacts"] = serde_json::to_value(&artifacts_spec).unwrap_or(Value::Null);

    let typed_spec: DataModelSpec = serde_json::from_value(new_spec.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid DataModel spec: {e}")))?;
    typed_spec
        .validate()
        .map_err(|e| ApiError::BadRequest(format!("DataModel validation error: {e}")))?;

    let mut updated_envelope = envelope.clone();
    updated_envelope.spec = new_spec;
    updated_envelope.strip_status();

    let manifest_yaml = serde_yaml_ng::to_string(&updated_envelope)
        .map_err(|e| ApiError::Internal(format!("serialize manifest to yaml: {e}")))?;

    let json_schema_content =
        serde_json::to_string_pretty(&artifacts.json_schema.clone().unwrap_or_else(|| json!({})))
            .map_err(|e| ApiError::Internal(format!("serialize json schema: {e}")))?;

    let context_content =
        serde_json::to_string_pretty(&artifacts.context.clone().unwrap_or_else(|| json!({})))
            .map_err(|e| ApiError::Internal(format!("serialize context: {e}")))?;

    let docs_content = artifacts.docs.clone().unwrap_or_default();

    let example_content =
        serde_json::to_string_pretty(&artifacts.example.clone().unwrap_or_else(|| json!({})))
            .map_err(|e| ApiError::Internal(format!("serialize example: {e}")))?;

    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let default_branch = gitea.default_branch().await?;
    let branch = branch_name(&project, "datamodel", &name, Operation::Update);
    let branch = create_or_reuse_branch(gitea, &branch, &default_branch).await?;

    let (author_name, author_email) = author_credentials(&user.0.identity, &project);
    let commit_msg = format!("update DataModel {name} source and artifacts");

    let manifest_path = format!("projects/{project}/spaces/{space}/datamodels/{name}.yaml");
    let source_path = format!("projects/{project}/spaces/{space}/datamodels/{confined_linkml}");
    let schema_path =
        format!("projects/{project}/spaces/{space}/datamodels/json-schema/{name}.v{major}.json");
    let context_path =
        format!("projects/{project}/spaces/{space}/datamodels/context/{name}.v{major}.jsonld");
    let docs_path = format!("projects/{project}/spaces/{space}/datamodels/docs/{name}.md");
    let example_path =
        format!("projects/{project}/spaces/{space}/datamodels/examples/{name}.example.jsonld");

    let writes = [
        (&manifest_path, manifest_yaml.as_str()),
        (&source_path, source_str.as_str()),
        (&schema_path, json_schema_content.as_str()),
        (&context_path, context_content.as_str()),
        (&docs_path, docs_content.as_str()),
        (&example_path, example_content.as_str()),
    ];

    for (file_p, content) in writes {
        let existing_sha = gitea
            .get_file(file_p, &branch)
            .await
            .ok()
            .flatten()
            .map(|f| f.sha);

        let file_write = FileWrite {
            path: file_p,
            branch: &branch,
            message: &commit_msg,
            content,
            sha: existing_sha.as_deref(),
            author: Author {
                name: &author_name,
                email: &author_email,
            },
        };
        gitea.put_file(&file_write).await?;
    }

    let lane = match severity {
        "breaking" => Lane::Red,
        "additive" => Lane::Yellow,
        _ => Lane::Green,
    };

    crate::telemetry::proposed(lane, "DataModel");

    let pr_title = format!("update DataModel {name}");
    let pr_body = format!(
        "Proposed update of DataModel `{name}` source, manifest and generated artifacts in project `{project}` via joinedcontext Portal."
    );

    let pr = gitea
        .create_pull_request(&branch, &default_branch, &pr_title, &pr_body)
        .await?;

    let change_meta = ChangeMeta::from_merge_request(pr.number, &project);
    let change_status = ChangeStatus::new(
        lane,
        ChangePhase::PendingApproval,
        PlanSummary {
            create: 0,
            update: 6,
            delete: 0,
        },
    )
    .with_merge_request(pr.url);
    let change = Change::new(change_meta, change_status);

    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/datamodels/{name}/source",
            get(get_source).put(put_source),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
}
