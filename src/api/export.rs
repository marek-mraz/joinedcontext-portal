//! One-click configuration export (T-0192, CC-49, MF-16…MF-19).
//!
//! The repository at a revision *is* the export, so this module never invents a format: it reads
//! the forge at that revision, strips what must not leave (`status`, literal secret values) and
//! hands back what a `git archive` of the same path would hold. Nothing here reads the live
//! mirror: an export names a revision, and the mirror only ever knows one.

use std::io::Write;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::api::mutate::SECRET_KEYS;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::git::GiteaClient;
use crate::resource::{self, ResourceEnvelope, API_VERSION};
use crate::state::AppState;

/// Revision picker page size (`limit`), and the ceiling a caller may ask for.
const DEFAULT_REVISIONS: usize = 20;
const MAX_REVISIONS: usize = 100;

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    /// Comma-separated plurals, e.g. `endpoints,pipelines`.
    #[serde(default)]
    pub kinds: Option<String>,
    /// Comma-separated `metadata.name` values.
    #[serde(default)]
    pub names: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevisionsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionList {
    pub items: Vec<Revision>,
}

/// Drops every string value stored under a credential key, at any depth (MF-17).
///
/// The key stays with an empty string rather than disappearing: a bundle must stay re-importable,
/// and a manifest that lost a required field would not validate on the way back in.
pub(crate) fn strip_secret_values(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if SECRET_KEYS.contains(&key.as_str()) && child.is_string() {
                    *child = Value::String(String::new());
                } else {
                    strip_secret_values(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_secret_values),
        _ => {}
    }
}

fn gitea(state: &AppState) -> Result<&GiteaClient, ApiError> {
    state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no repository is configured".into()))
}

/// A revision is put into a forge URL, so it is checked before it is used. Branch names and
/// commit shas are the whole of what a caller may name here.
fn valid_revision(revision: &str) -> bool {
    !revision.is_empty()
        && revision.len() <= 128
        && !revision.starts_with('-')
        && !revision.contains("..")
        && revision
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

fn selected(raw: Option<&str>) -> Option<Vec<String>> {
    let list: Vec<String> = raw?
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    (!list.is_empty()).then_some(list)
}

/// `kinds=endpoints,pipelines` as the kind names the manifests carry. An unknown plural filters
/// everything out rather than being ignored: a filter that silently does nothing is a bug that
/// ships a bigger bundle than the caller asked for.
fn kind_filter(raw: Option<&str>) -> Option<Vec<String>> {
    selected(raw).map(|plurals| {
        plurals
            .iter()
            .map(|plural| {
                resource::by_plural(plural)
                    .map(|info| info.kind.to_string())
                    .unwrap_or_else(|| format!("unknown:{plural}"))
            })
            .collect()
    })
}

fn is_manifest_path(path: &str) -> bool {
    path.ends_with(".yaml") || path.ends_with(".yml")
}

/// One file of the project subtree, ready to be written into the answer.
struct Exported {
    path: String,
    content: String,
    manifest: Option<ResourceEnvelope>,
}

/// Reads `projects/{project}/` at one revision, sanitising every manifest it finds.
///
/// A file the forge cannot hand back as text (a binary blob committed next to the manifests) is
/// counted and left out rather than corrupted; the count is what the bundle index reports as
/// omitted, so a caller always learns that something was not included (MF-18).
async fn read_project(
    gitea: &GiteaClient,
    project: &str,
    revision: &str,
) -> Result<(Vec<Exported>, usize), ApiError> {
    let prefix = format!("projects/{project}/");
    let paths: Vec<String> = gitea
        .list_tree(revision)
        .await?
        .into_iter()
        .filter(|path| path.starts_with(&prefix))
        .collect();

    let mut files = Vec::new();
    let mut omitted = 0usize;
    for path in paths {
        let file = match gitea.get_file(&path, revision).await {
            Ok(Some(file)) => file,
            Ok(None) => {
                omitted += 1;
                continue;
            }
            Err(err) => {
                // Not fatal: one unreadable file must not cost the operator the whole export.
                tracing::warn!(path = %path, error = %err, "file skipped in export");
                omitted += 1;
                continue;
            }
        };

        let manifest = if is_manifest_path(&path) {
            serde_yaml_ng::from_str::<ResourceEnvelope>(&file.content).ok()
        } else {
            None
        };

        match manifest {
            Some(mut envelope) if resource::by_kind(&envelope.kind).is_some() => {
                // Status is the Portal's own computation and a secret is nobody's (MF-17).
                envelope.strip_status();
                strip_secret_values(&mut envelope.spec);
                let content = serde_yaml_ng::to_string(&envelope).map_err(|e| {
                    ApiError::Internal(format!("manifest '{path}' did not serialise: {e}"))
                })?;
                files.push(Exported {
                    path,
                    content,
                    manifest: Some(envelope),
                });
            }
            // Native files (`bento.yaml`, LinkML, generated schema artifacts) travel as they
            // are: they are the pipeline's or the model's own format, not ours to rewrite.
            _ => files.push(Exported {
                path,
                content: file.content,
                manifest: None,
            }),
        }
    }
    Ok((files, omitted))
}

fn matches_filters(
    envelope: &ResourceEnvelope,
    kinds: Option<&Vec<String>>,
    names: Option<&Vec<String>>,
) -> bool {
    kinds.is_none_or(|wanted| wanted.contains(&envelope.kind))
        && names.is_none_or(|wanted| wanted.contains(&envelope.metadata.name))
}

fn attachment(body: Vec<u8>, content_type: &'static str, filename: String) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

/// The `kind: Bundle` index that makes an archive re-importable (MF-17).
fn bundle_index(
    project: &str,
    revision: &str,
    exporter: &str,
    contents: Vec<Value>,
    files: usize,
    omitted: usize,
) -> Result<String, ApiError> {
    let bundle = serde_json::json!({
        "apiVersion": API_VERSION,
        "kind": "Bundle",
        "metadata": { "name": project, "namespace": project },
        "spec": {
            "project": project,
            "revision": revision,
            "exporter": exporter,
            "files": files,
            "omitted": omitted,
            "contents": contents,
        }
    });
    serde_yaml_ng::to_string(&bundle)
        .map_err(|e| ApiError::Internal(format!("bundle index did not serialise: {e}")))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/export",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("format" = Option<String>, Query, description = "yaml (default), json or zip"),
        ("revision" = Option<String>, Query, description = "Commit or branch; default branch head when absent"),
        ("kinds" = Option<String>, Query, description = "Comma-separated plurals to include"),
        ("names" = Option<String>, Query, description = "Comma-separated resource names to include"),
    ),
    responses(
        (status = 200, description = "The project's configuration at the revision"),
        (status = 400, description = "Unknown format or malformed revision", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or revision", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails)
    )
)]
pub async fn export(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    if !resource::is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let format = query.format.as_deref().unwrap_or("yaml").to_string();
    if !matches!(format.as_str(), "yaml" | "json" | "zip") {
        return Err(ApiError::BadRequest(format!(
            "format '{format}' is not yaml, json or zip"
        )));
    }
    let gitea = gitea(&state)?;

    let revision = match query.revision.as_deref() {
        Some(revision) => {
            if !valid_revision(revision) {
                return Err(ApiError::BadRequest(
                    "revision must be a commit id or a branch name".into(),
                ));
            }
            revision.to_string()
        }
        None => {
            let branch = gitea.default_branch().await?;
            gitea.branch_head(&branch).await?
        }
    };

    let (files, omitted) = read_project(gitea, &project, &revision).await?;
    let kinds = kind_filter(query.kinds.as_deref());
    let names = selected(query.names.as_deref());
    let short = revision.chars().take(7).collect::<String>();

    if format == "zip" {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut contents = Vec::new();
        let mut written = 0usize;
        for file in &files {
            // A native file has no kind to filter on and belongs to whatever manifest sits
            // beside it, so the archive keeps it whatever the filters say.
            if let Some(envelope) = &file.manifest {
                if !matches_filters(envelope, kinds.as_ref(), names.as_ref()) {
                    continue;
                }
                contents.push(serde_json::json!({
                    "kind": envelope.kind, "name": envelope.metadata.name
                }));
            }
            let zipped = |err: zip::result::ZipError| {
                ApiError::Internal(format!("archive entry failed: {err}"))
            };
            writer.start_file(&file.path, options).map_err(zipped)?;
            writer
                .write_all(file.content.as_bytes())
                .map_err(|e| ApiError::Internal(format!("archive write failed: {e}")))?;
            written += 1;
        }
        let index = bundle_index(
            &project,
            &revision,
            &user.0.identity.username,
            contents,
            written,
            omitted,
        )?;
        writer
            .start_file(format!("projects/{project}/bundle.yaml"), options)
            .map_err(|err| ApiError::Internal(format!("archive entry failed: {err}")))?;
        writer
            .write_all(index.as_bytes())
            .map_err(|e| ApiError::Internal(format!("archive write failed: {e}")))?;
        let cursor = writer
            .finish()
            .map_err(|err| ApiError::Internal(format!("archive did not close: {err}")))?;
        return Ok(attachment(
            cursor.into_inner(),
            "application/zip",
            format!("{project}-{short}.zip"),
        ));
    }

    let manifests: Vec<&ResourceEnvelope> = files
        .iter()
        .filter_map(|file| file.manifest.as_ref())
        .filter(|envelope| matches_filters(envelope, kinds.as_ref(), names.as_ref()))
        .collect();

    if format == "json" {
        let list = serde_json::json!({
            "apiVersion": API_VERSION,
            "kind": "List",
            "metadata": { "revision": revision, "omitted": omitted },
            "items": manifests,
        });
        let body = serde_json::to_vec_pretty(&list)
            .map_err(|e| ApiError::Internal(format!("export did not serialise: {e}")))?;
        return Ok(attachment(
            body,
            "application/json",
            format!("{project}-{short}.json"),
        ));
    }

    let mut body = String::new();
    for envelope in manifests {
        let document = serde_yaml_ng::to_string(envelope)
            .map_err(|e| ApiError::Internal(format!("export did not serialise: {e}")))?;
        body.push_str("---\n");
        body.push_str(&document);
    }
    Ok(attachment(
        body.into_bytes(),
        "application/yaml",
        format!("{project}-{short}.yaml"),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/revisions",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("limit" = Option<usize>, Query, description = "1…100, default 20"),
    ),
    responses(
        (status = 200, description = "Commits touching the project, newest first", body = RevisionList),
        (status = 400, description = "limit out of range", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails)
    )
)]
pub async fn revisions(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<RevisionsQuery>,
) -> Result<Json<RevisionList>, ApiError> {
    if !resource::is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let limit = query.limit.unwrap_or(DEFAULT_REVISIONS);
    if limit == 0 || limit > MAX_REVISIONS {
        return Err(ApiError::BadRequest(format!(
            "limit must be between 1 and {MAX_REVISIONS}"
        )));
    }
    let gitea = gitea(&state)?;
    let branch = gitea.default_branch().await?;
    let commits = gitea
        .list_commits(&branch, &format!("projects/{project}"), limit)
        .await?;
    Ok(Json(RevisionList {
        items: commits
            .into_iter()
            .map(|commit| Revision {
                sha: commit.sha,
                message: commit.message,
                author: commit.author,
                date: commit.date,
            })
            .collect(),
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/export", get(export))
        .route("/projects/{project}/revisions", get(revisions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_values_are_emptied_wherever_they_hide() {
        let mut spec = serde_json::json!({
            "url": "mqtt://broker",
            "auth": { "password": "hunter2", "secretRef": { "name": "mqtt", "key": "password" } },
            "outputs": [{ "apiKey": "jc_dead_beef" }],
        });
        strip_secret_values(&mut spec);
        let dumped = spec.to_string();
        assert!(!dumped.contains("hunter2"), "{dumped}");
        assert!(!dumped.contains("jc_dead_beef"), "{dumped}");
        assert_eq!(spec["auth"]["password"], "");
        assert_eq!(
            spec["auth"]["secretRef"]["key"], "password",
            "a reference to a secret is not a secret and must survive the export"
        );
        assert_eq!(spec["url"], "mqtt://broker");
    }

    #[test]
    fn a_revision_is_a_commit_or_a_branch_and_never_a_path_traversal() {
        assert!(valid_revision("main"));
        assert!(valid_revision("8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8"));
        assert!(valid_revision("release/2026-09"));
        assert!(!valid_revision(""));
        assert!(!valid_revision("../../etc/passwd"));
        assert!(!valid_revision("main;rm -rf"));
        assert!(!valid_revision("--upload-pack=x"));
        assert!(!valid_revision(&"a".repeat(129)));
    }

    #[test]
    fn an_unknown_plural_filters_everything_out_rather_than_nothing() {
        let filter = kind_filter(Some("endpoints,teapots")).expect("a filter");
        assert!(filter.contains(&"Endpoint".to_string()));
        assert!(filter.contains(&"unknown:teapots".to_string()));
        assert!(kind_filter(None).is_none(), "no filter is not an empty one");
        assert!(kind_filter(Some(" , ")).is_none());
    }
}
