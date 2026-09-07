//! Importing a manifest, a bundle or an archive into one project (T-0193, MF-20…MF-26).
//!
//! The mirror image of [`super::export`], and deliberately the same shape: an export is the
//! repository at a revision, so an import is a set of manifests written back to the
//! repository as one merge request. Nothing lands directly. What the caller uploads is
//! parsed, validated, remapped and planned, and the answer is a `202` with the same
//! `kind: Change` a single write produces — the reviewer sees one lane and one diff for the
//! whole bundle rather than one merge request per file (MF-21, CC-63).
//!
//! Four gates stand between an upload and a branch, and each one refuses rather than
//! repairs (MF-24): a manifest of a kind this platform does not serve, a `status` block the
//! Portal computes itself, a credential pasted as a literal string, and a reference to a
//! resource that is in neither the bundle nor the project. A bundle that half-imports is
//! worse than one that does not import, because the half that landed is now a project
//! nobody described.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use axum::extract::{FromRequest, Multipart, Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::api::mutate::{author_credentials, create_or_reuse_branch, find_literal_secret};
use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, FileWrite, GiteaClient};
use crate::resource::{self, ResourceEnvelope};
use crate::state::AppState;

/// Largest upload the endpoint reads. A project's whole configuration is manifests and a few
/// native files; anything past this is not a bundle.
const MAX_UPLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Entries an archive may hold. A bundle carrying a thousand files is a mistake or an attack,
/// and either way it must not become a thousand forge calls.
const MAX_ARCHIVE_ENTRIES: usize = 2_000;

/// The index an exported archive carries. It describes the bundle rather than belonging to the
/// project, so it is read for provenance and never written (MF-17).
const BUNDLE_KIND: &str = "Bundle";

/// What to do with a resource the project already has (MF-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Refuse the whole import. The default: an import that silently overwrote a live
    /// endpoint would be a deployment nobody proposed.
    #[default]
    Fail,
    /// Leave the existing resource alone and import the rest.
    Skip,
    /// Overwrite the existing resource with the imported one.
    Replace,
    /// Import under a new name, rewriting every reference in the bundle that named it (MF-26).
    Rename,
}

impl ConflictPolicy {
    fn parse(raw: &str) -> Result<Self, ApiError> {
        match raw {
            "fail" => Ok(Self::Fail),
            "skip" => Ok(Self::Skip),
            "replace" => Ok(Self::Replace),
            "rename" => Ok(Self::Rename),
            other => Err(ApiError::BadRequest(format!(
                "conflictPolicy '{other}' is not fail, skip, replace or rename (MF-23)"
            ))),
        }
    }
}

/// The options an import wizard sends beside the file (Architecture/06 §6).
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    /// The namespace every imported manifest is rewritten into; the project when absent (MF-22).
    #[serde(default)]
    pub target_namespace: Option<String>,
    #[serde(default)]
    pub conflict_policy: ConflictPolicy,
    #[serde(default)]
    pub dry_run: bool,
    /// A bundle to fetch rather than upload. Refused for now, see [`import`].
    #[serde(default)]
    pub url: Option<String>,
    /// The manifests themselves, for a caller that posts JSON rather than a file.
    #[serde(default)]
    pub manifests: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ImportQuery {
    #[serde(default)]
    pub dry_run: Option<String>,
}

/// What one import would do, answered on a dry run and echoed in the merge request body.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Resources that would be created.
    pub created: Vec<String>,
    /// Resources that would replace one the project already has.
    pub replaced: Vec<String>,
    /// Resources left alone because the project already has them (`skip`).
    pub skipped: Vec<String>,
    /// Resources imported under a new name, `old -> new` (`rename`).
    pub renamed: BTreeMap<String, String>,
    /// Files carried through untouched: `bento.yaml`, LinkML sources, schema artifacts.
    pub native_files: usize,
    /// The lane the whole bundle lands in: the riskiest of everything it carries (CC-63).
    pub lane: Lane,
    /// Where the bundle came from, when it carried a `kind: Bundle` index (MF-20).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// One manifest or native file on its way into the repository.
struct Incoming {
    path: Option<String>,
    envelope: Option<ResourceEnvelope>,
    content: String,
}

// --- parsing what was uploaded ---------------------------------------------------------

/// Everything an upload holds, whatever shape it arrived in (Architecture/06 §6).
///
/// A zip is the archive an export produced, a YAML stream is one or more manifests, and JSON
/// is a single manifest or a `kind: List` of them. The three are told apart by content rather
/// than by a declared media type: a browser's `Content-Type` on a file part is whatever the
/// operating system guessed, and being wrong about it would refuse a valid bundle.
fn parse(bytes: &[u8]) -> Result<Vec<Incoming>, ApiError> {
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the upload is larger than the {MAX_UPLOAD_BYTES} byte import limit"
        )));
    }
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("the upload is empty".into()));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return parse_archive(bytes);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ApiError::BadRequest("the upload is neither a zip archive nor text".into()))?;
    parse_text(text, None)
}

fn parse_archive(bytes: &[u8]) -> Result<Vec<Incoming>, ApiError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| ApiError::BadRequest(format!("the archive did not open: {err}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ApiError::BadRequest(format!(
            "the archive holds {} entries, past the {MAX_ARCHIVE_ENTRIES} entry limit",
            archive.len()
        )));
    }
    let mut incoming = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| ApiError::BadRequest(format!("archive entry {index}: {err}")))?;
        if entry.is_dir() {
            continue;
        }
        // `enclosed_name` is None for a path that escapes the archive root, which is the
        // zip-slip a bundle from another instance could carry.
        let name = entry
            .enclosed_name()
            .ok_or_else(|| ApiError::BadRequest("the archive holds an escaping path".into()))?
            .to_string_lossy()
            .into_owned();
        let mut content = String::new();
        if entry.read_to_string(&mut content).is_err() {
            // A binary blob committed beside the manifests. An export counts these as
            // omitted; an import has nothing to write for them either.
            continue;
        }
        incoming.extend(parse_text(&content, Some(&name))?);
    }
    Ok(incoming)
}

/// One text file as manifests, or as a native file that travels unread.
fn parse_text(text: &str, path: Option<&str>) -> Result<Vec<Incoming>, ApiError> {
    let manifest_path = path.is_none_or(|p| p.ends_with(".yaml") || p.ends_with(".yml"));
    if !manifest_path {
        return Ok(vec![Incoming {
            path: path.map(str::to_owned),
            envelope: None,
            content: text.to_owned(),
        }]);
    }

    let mut documents: Vec<Value> = Vec::new();
    if text.trim_start().starts_with('{') {
        let value: Value = serde_json::from_str(text)
            .map_err(|err| ApiError::BadRequest(format!("the JSON did not parse: {err}")))?;
        documents.push(value);
    } else {
        for document in serde_yaml_ng::Deserializer::from_str(text) {
            let value = Value::deserialize(document)
                .map_err(|err| ApiError::BadRequest(format!("the YAML did not parse: {err}")))?;
            documents.push(value);
        }
    }

    // A top-level array is a list of manifests, which is what `jcctl import` sends and what a
    // hand-written multi-manifest JSON looks like.
    let documents: Vec<Value> = documents
        .into_iter()
        .flat_map(|document| match document {
            Value::Array(items) => items,
            other => vec![other],
        })
        .collect();

    let mut incoming = Vec::new();
    for document in documents {
        if document.is_null() {
            continue;
        }
        // A `kind: List` is the JSON export format, and its items are the manifests.
        if document.get("kind").and_then(Value::as_str) == Some("List") {
            for item in document
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                incoming.push(as_incoming(item, path)?);
            }
            continue;
        }
        incoming.push(as_incoming(&document, path)?);
    }
    if incoming.is_empty() && !text.trim().is_empty() {
        // A `.yaml` beside the manifests that is not a manifest: Bento's own config, a
        // LinkML source. It travels as it is.
        incoming.push(Incoming {
            path: path.map(str::to_owned),
            envelope: None,
            content: text.to_owned(),
        });
    }
    Ok(incoming)
}

fn as_incoming(document: &Value, path: Option<&str>) -> Result<Incoming, ApiError> {
    let looks_like_manifest = document.get("kind").is_some() && document.get("metadata").is_some();
    if !looks_like_manifest {
        return Ok(Incoming {
            path: path.map(str::to_owned),
            envelope: None,
            content: serde_yaml_ng::to_string(document)
                .map_err(|e| ApiError::Internal(format!("document did not serialise: {e}")))?,
        });
    }
    let envelope: ResourceEnvelope = serde_json::from_value(document.clone()).map_err(|err| {
        ApiError::BadRequest(format!(
            "{}: not a manifest this platform serves: {err}",
            path.unwrap_or("the upload")
        ))
    })?;
    Ok(Incoming {
        path: path.map(str::to_owned),
        envelope: Some(envelope),
        content: String::new(),
    })
}

// --- the gates (MF-24) -------------------------------------------------------------------

/// Everything wrong with one manifest, in the order a person would read it.
fn refusals(envelope: &ResourceEnvelope, raw: &Value) -> Vec<String> {
    let name = &envelope.metadata.name;
    let mut refusals = Vec::new();
    if envelope.api_version != resource::API_VERSION {
        refusals.push(format!(
            "{name}: apiVersion '{}' is not supported (expected '{}')",
            envelope.api_version,
            resource::API_VERSION
        ));
    }
    if resource::by_kind(&envelope.kind).is_none() {
        refusals.push(format!(
            "{name}: kind '{}' is not a resource this platform serves",
            envelope.kind
        ));
    }
    if let Err(problem) = jc_core::names::validate_dns1123_label(name) {
        refusals.push(format!("{name}: {problem}"));
    }
    if envelope.status.is_some() || raw.get("status").is_some() {
        refusals.push(format!(
            "{name}: status is computed by the platform and cannot be imported (MF-04)"
        ));
    }
    if let Some(key) = find_literal_secret(&envelope.spec) {
        refusals.push(format!(
            "{name}: literal secret in field '{key}'; use secretRef instead (MF-24)"
        ));
    }
    refusals
}

/// Field names whose *string* value names another manifest whose kind is fixed by the field.
///
/// A typed reference (`{ kind, name }`) says what it points at and is resolved generically.
/// A bare string does not, so only the fields whose kind is settled by the schema are checked
/// here; guessing at the rest would refuse valid bundles.
const STRING_REFS: &[(&str, &str)] = &[("contextSpaceRef", "ContextSpace")];

/// Every reference in one manifest, as `(kind, name)`.
fn references(spec: &Value, found: &mut BTreeSet<(String, String)>) {
    match spec {
        Value::Object(members) => {
            for (key, value) in members {
                // A secretRef names a Secret in the cluster, not a manifest in the repository.
                if key == "secretRef" || key == "apiTokenRef" {
                    continue;
                }
                if let Some((_, kind)) = STRING_REFS.iter().find(|(field, _)| field == key) {
                    if let Some(name) = value.as_str() {
                        found.insert(((*kind).to_owned(), name.to_owned()));
                        continue;
                    }
                }
                if key.ends_with("Ref") {
                    if let (Some(kind), Some(name)) = (
                        value.get("kind").and_then(Value::as_str),
                        value.get("name").and_then(Value::as_str),
                    ) {
                        found.insert((kind.to_owned(), name.to_owned()));
                        continue;
                    }
                }
                references(value, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| references(item, found)),
        _ => {}
    }
}

/// References the bundle names and neither it nor the project holds (MF-24).
fn unresolved(manifests: &[ResourceEnvelope], state: &AppState, project: &str) -> Vec<String> {
    let inside: BTreeSet<(String, String)> = manifests
        .iter()
        .map(|envelope| (envelope.kind.clone(), envelope.metadata.name.clone()))
        .collect();
    let mut missing = BTreeSet::new();
    for envelope in manifests {
        let mut named = BTreeSet::new();
        references(&envelope.spec, &mut named);
        for (kind, name) in named {
            if inside.contains(&(kind.clone(), name.clone())) {
                continue;
            }
            if state.mirror.get(project, &kind, &name).is_some() {
                continue;
            }
            missing.insert(format!(
                "{}: {kind} '{name}' is in neither the bundle nor project '{project}'",
                envelope.metadata.name
            ));
        }
    }
    missing.into_iter().collect()
}

// --- namespace remapping (MF-22) ---------------------------------------------------------

/// Rewrites one manifest into the target namespace: the metadata, the namespace of every
/// typed reference, and the space segment of every entity URN it carries.
///
/// The URN is the part that is easy to forget and expensive to get wrong. An id follows
/// `urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}` (PF-10), so a bundle imported into
/// another project keeps pointing at the old space unless the segment moves with it.
fn remap(envelope: &mut ResourceEnvelope, from: &str, to: &str) {
    envelope.metadata.namespace = Some(to.to_owned());
    remap_value(&mut envelope.spec, from, to);
}

fn remap_value(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::Object(members) => {
            if members.get("kind").and_then(Value::as_str).is_some() {
                if let Some(Value::String(namespace)) = members.get_mut("namespace") {
                    if namespace == from {
                        *namespace = to.to_owned();
                    }
                }
            }
            for child in members.values_mut() {
                remap_value(child, from, to);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| remap_value(item, from, to)),
        Value::String(text) => {
            if let Some(rewritten) = remap_urn(text, from, to) {
                *text = rewritten;
            }
        }
        _ => {}
    }
}

/// The space segment of an NGSI-LD id, moved to the target namespace (PF-10).
fn remap_urn(urn: &str, from: &str, to: &str) -> Option<String> {
    if !urn.starts_with("urn:ngsi-ld:") {
        return None;
    }
    let mut segments: Vec<&str> = urn.split(':').collect();
    // urn : ngsi-ld : Type : orgDomain : space : localId
    if segments.len() < 6 || segments[4] != from {
        return None;
    }
    segments[4] = to;
    Some(segments.join(":"))
}

/// Rewrites every reference to `old` so it names `new` (MF-23 `rename`, MF-26).
fn rewrite_reference(value: &mut Value, kind: &str, old: &str, new: &str) {
    match value {
        Value::Object(members) => {
            for (key, child) in members.iter_mut() {
                if let Some((_, fixed)) = STRING_REFS.iter().find(|(field, _)| field == key) {
                    if *fixed == kind && child.as_str() == Some(old) {
                        *child = Value::String(new.to_owned());
                        continue;
                    }
                }
                if key.ends_with("Ref")
                    && child.get("kind").and_then(Value::as_str) == Some(kind)
                    && child.get("name").and_then(Value::as_str) == Some(old)
                {
                    child["name"] = Value::String(new.to_owned());
                    continue;
                }
                rewrite_reference(child, kind, old, new);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_reference(item, kind, old, new)),
        _ => {}
    }
}

/// The name a renamed resource takes: the original suffixed with where it came from, so
/// importing the same bundle twice lands on the same name instead of `-1`, `-2`, `-3`.
///
/// The suffix is the origin and not the project being imported into: everything in the
/// project already belongs to the project, and `ovzdusie-helsinki` is the only form that
/// says which of the two spaces named `ovzdusie` this one is.
fn renamed(name: &str, origin: &str) -> String {
    let suffix: String = origin
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if suffix.is_empty() {
        format!("{name}-imported")
    } else {
        format!("{name}-{suffix}")
    }
}

// --- the endpoint -------------------------------------------------------------------------

/// The upload and the options, however the caller sent them.
///
/// One extractor consumes the body, so the dispatch is here rather than in the signature: a
/// wizard posts `multipart/form-data` with the file and the option fields beside it, and
/// `jcctl` posts JSON.
async fn read_request(
    state: &AppState,
    request: Request,
) -> Result<(Vec<u8>, ImportOptions), ApiError> {
    let multipart = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("multipart/"));

    if multipart {
        let mut multipart = Multipart::from_request(request, state)
            .await
            .map_err(|err| ApiError::BadRequest(format!("the upload did not parse: {err}")))?;
        let mut bytes: Vec<u8> = Vec::new();
        let mut options = ImportOptions::default();
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|err| ApiError::BadRequest(format!("the upload did not parse: {err}")))?
        {
            let name = field.name().unwrap_or_default().to_owned();
            let data = field
                .bytes()
                .await
                .map_err(|err| ApiError::BadRequest(format!("field '{name}': {err}")))?;
            match name.as_str() {
                "file" | "bundle" => bytes = data.to_vec(),
                "targetNamespace" => {
                    options.target_namespace = Some(String::from_utf8_lossy(&data).into_owned())
                }
                "conflictPolicy" => {
                    options.conflict_policy =
                        ConflictPolicy::parse(String::from_utf8_lossy(&data).trim())?
                }
                "dryRun" => {
                    options.dry_run =
                        matches!(String::from_utf8_lossy(&data).trim(), "true" | "All")
                }
                "url" => options.url = Some(String::from_utf8_lossy(&data).into_owned()),
                _ => {}
            }
        }
        return Ok((bytes, options));
    }

    let Json(body) = Json::<Value>::from_request(request, state)
        .await
        .map_err(|_| {
            ApiError::BadRequest("send a multipart file or a JSON body with 'manifests'".into())
        })?;
    let options: ImportOptions = serde_json::from_value(body.clone())
        .map_err(|err| ApiError::BadRequest(format!("the import options did not parse: {err}")))?;
    let manifests = match options.manifests.clone() {
        Some(manifests) => manifests,
        // A bare manifest posted as the whole body, which is what `jcctl import` sends.
        None if body.get("kind").is_some() => body.clone(),
        None => return Ok((Vec::new(), options)),
    };
    let text = serde_json::to_vec(&manifests)
        .map_err(|err| ApiError::Internal(format!("the manifests did not serialise: {err}")))?;
    Ok((text, options))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/import",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project the bundle is imported into"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' to validate and plan without proposing"),
    ),
    responses(
        (status = 202, description = "One merge request for the whole bundle", body = Change),
        (status = 200, description = "Dry run: what the import would do", body = ImportReport),
        (status = 400, description = "The bundle was refused", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 409, description = "A resource already exists and the policy is 'fail'", body = ProblemDetails),
        (status = 501, description = "Importing from a URL is not implemented", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn import(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ImportQuery>,
    request: Request,
) -> Result<Response, ApiError> {
    if !resource::is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let (bytes, mut options) = read_request(&state, request).await?;
    if query.dry_run.as_deref() == Some("All") {
        options.dry_run = true;
    }
    if options.url.is_some() {
        // MF-20 allows a URL, and fetching one means the Portal opening a connection to a
        // host a caller named. That is an egress decision with no policy behind it yet, and
        // inventing one here would be inventing an SSRF surface.
        return Err(ApiError::NotImplemented(
            "importing from a URL needs an egress policy that does not exist yet; upload the \
             bundle instead (MF-20)"
                .into(),
        ));
    }

    let incoming = parse(&bytes)?;
    let target = options
        .target_namespace
        .clone()
        .unwrap_or_else(|| project.clone());
    if !resource::is_dns1123(&target) {
        return Err(ApiError::BadRequest(format!(
            "targetNamespace '{target}' is not a DNS-1123 label"
        )));
    }
    if target != project {
        // The repository path of every kind starts `projects/{project}/`, so a namespace
        // that is not the project would write manifests the reconciler never reads.
        return Err(ApiError::BadRequest(format!(
            "targetNamespace '{target}' must be the project '{project}'"
        )));
    }

    let (report, files) = plan_import(
        &incoming,
        &state,
        &project,
        &target,
        options.conflict_policy,
    )?;
    if options.dry_run {
        return Ok((StatusCode::OK, Json(report)).into_response());
    }
    if files.is_empty() {
        return Err(ApiError::BadRequest(
            "the bundle holds nothing to import".into(),
        ));
    }
    propose_bundle(&user, &state, &project, report, files).await
}

/// What the import would do, and the files it would write.
#[allow(clippy::type_complexity)]
fn plan_import(
    incoming: &[Incoming],
    state: &AppState,
    project: &str,
    target: &str,
    policy: ConflictPolicy,
) -> Result<(ImportReport, Vec<(String, String)>), ApiError> {
    let mut manifests: Vec<ResourceEnvelope> = Vec::new();
    let mut natives: Vec<(String, String)> = Vec::new();
    let mut source: Option<String> = None;
    let mut refused: Vec<String> = Vec::new();

    for item in incoming {
        let Some(envelope) = item.envelope.clone() else {
            if let Some(path) = &item.path {
                natives.push((path.clone(), item.content.clone()));
            }
            continue;
        };
        if envelope.kind == BUNDLE_KIND {
            // The index describes the bundle, so it is provenance and not a resource (MF-20).
            source = envelope
                .spec
                .get("project")
                .and_then(Value::as_str)
                .map(|from| {
                    let revision = envelope
                        .spec
                        .get("revision")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    format!("{from}@{revision}")
                });
            continue;
        }
        let raw = serde_json::to_value(&envelope)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
        refused.extend(refusals(&envelope, &raw));
        manifests.push(envelope);
    }
    if !refused.is_empty() {
        return Err(ApiError::BadRequest(refused.join("; ")));
    }
    if manifests.is_empty() && natives.is_empty() {
        return Err(ApiError::BadRequest(
            "the upload holds no manifest this platform serves".into(),
        ));
    }

    // The source namespace is whatever the bundle was exported from; a bundle whose manifests
    // disagree about it is remapped from each one's own, which is what a hand-assembled
    // multi-project bundle needs.
    // The origin travels with each manifest because a rename is named after it.
    let manifests: Vec<(String, ResourceEnvelope)> = manifests
        .into_iter()
        .map(|mut envelope| {
            let from = envelope
                .metadata
                .namespace
                .clone()
                .unwrap_or_else(|| target.to_owned());
            remap(&mut envelope, &from, target);
            (from, envelope)
        })
        .collect();

    let mut report = ImportReport {
        created: Vec::new(),
        replaced: Vec::new(),
        skipped: Vec::new(),
        renamed: BTreeMap::new(),
        native_files: natives.len(),
        lane: Lane::Green,
        source,
    };

    // Conflicts first: a rename rewrites references, so it has to happen before anything is
    // resolved or planned.
    let mut keep: Vec<ResourceEnvelope> = Vec::new();
    for (origin, envelope) in manifests {
        let existing = state
            .mirror
            .get(project, &envelope.kind, &envelope.metadata.name)
            .is_some();
        if !existing {
            report.created.push(envelope.metadata.name.clone());
            keep.push(envelope);
            continue;
        }
        match policy {
            ConflictPolicy::Fail => {
                return Err(ApiError::Conflict(format!(
                    "{} '{}' already exists in project '{project}'; choose skip, replace or \
                     rename (MF-23)",
                    envelope.kind, envelope.metadata.name
                )))
            }
            ConflictPolicy::Skip => report.skipped.push(envelope.metadata.name.clone()),
            ConflictPolicy::Replace => {
                report.replaced.push(envelope.metadata.name.clone());
                keep.push(envelope);
            }
            ConflictPolicy::Rename => {
                let old = envelope.metadata.name.clone();
                let stem = renamed(&old, &origin);
                let mut new = stem.clone();
                let mut attempt = 2;
                while state.mirror.get(project, &envelope.kind, &new).is_some() {
                    new = format!("{stem}-{attempt}");
                    attempt += 1;
                }
                if !resource::is_dns1123(&new) {
                    return Err(ApiError::BadRequest(format!(
                        "renaming '{old}' would produce '{new}', which is not a name this \
                         platform can store; rename it in the source project first (MF-23)"
                    )));
                }
                report.renamed.insert(old.clone(), new.clone());
                let mut envelope = envelope;
                envelope.metadata.name = new;
                keep.push(envelope);
            }
        }
    }
    for (old, new) in &report.renamed {
        let kind = keep
            .iter()
            .find(|envelope| &envelope.metadata.name == new)
            .map(|envelope| envelope.kind.clone());
        if let Some(kind) = kind {
            for envelope in &mut keep {
                rewrite_reference(&mut envelope.spec, &kind, old, new);
            }
        }
    }

    let missing = unresolved(&keep, state, project);
    if !missing.is_empty() {
        return Err(ApiError::BadRequest(missing.join("; ")));
    }

    let mut files: Vec<(String, String)> = Vec::new();
    for envelope in &keep {
        let info = resource::by_kind(&envelope.kind)
            .ok_or_else(|| ApiError::Internal(format!("kind '{}' vanished", envelope.kind)))?;
        let operation = if state
            .mirror
            .get(project, &envelope.kind, &envelope.metadata.name)
            .is_some()
        {
            Operation::Update
        } else {
            Operation::Create
        };
        report.lane = riskiest(
            report.lane,
            change::classify(info.kind, operation, &envelope.spec),
        );

        let path =
            resource::repository_path(info, project, space_of(envelope), &envelope.metadata.name)
                .map_err(ApiError::BadRequest)?;
        let mut committed = envelope.clone();
        committed.strip_status();
        annotate(&mut committed, report.source.as_deref());
        let content = serde_yaml_ng::to_string(&committed)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
        files.push((path, content));
    }
    // A native file keeps the path it had in the archive, rewritten into this project.
    for (path, content) in natives {
        files.push((reproject(&path, project), content));
    }
    Ok((report, files))
}

/// The space a manifest lives under, for the kinds whose path has a `{space}` segment.
///
/// `spec.contextSpaceRef` is the answer for every kind that has one, and it is the right
/// answer: an Endpoint of the air quality space belongs under `spaces/ovzdusie/`, not under a
/// directory named after the project. The label is the override for a kind that has no
/// `contextSpaceRef` but still lives under a space, and the namespace is the last resort.
fn space_of(envelope: &ResourceEnvelope) -> Option<&str> {
    envelope
        .metadata
        .labels
        .get("joinedcontext.com/space")
        .map(String::as_str)
        .or_else(|| match envelope.spec.get("contextSpaceRef") {
            Some(Value::String(name)) => Some(name.as_str()),
            Some(reference) => reference.get("name").and_then(Value::as_str),
            None => None,
        })
        .or(envelope.metadata.namespace.as_deref())
}

/// The riskier of two lanes. One merge request carries the whole bundle, so it has to be
/// reviewed at the level of the riskiest thing in it (CC-63).
fn riskiest(left: Lane, right: Lane) -> Lane {
    let rank = |lane: Lane| match lane {
        Lane::Green => 0,
        Lane::Yellow => 1,
        Lane::Red => 2,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

/// Tags an imported manifest with where it came from (MF-20, MF-08).
fn annotate(envelope: &mut ResourceEnvelope, source: Option<&str>) {
    let value = source.unwrap_or("upload").to_owned();
    envelope
        .metadata
        .annotations
        .insert(jc_core::annotations::IMPORTED_FROM.to_owned(), value);
}

/// `projects/<anything>/rest` becomes `projects/<project>/rest`, so an archive from another
/// instance lands in this project rather than recreating the one it was exported from.
fn reproject(path: &str, project: &str) -> String {
    let mut segments = path.split('/');
    match (segments.next(), segments.next()) {
        (Some("projects"), Some(_)) => {
            format!(
                "projects/{project}/{}",
                segments.collect::<Vec<_>>().join("/")
            )
        }
        _ => format!("projects/{project}/{path}"),
    }
}

/// One branch, every file, one merge request (MF-21, CC-63).
async fn propose_bundle(
    user: &CurrentUser,
    state: &AppState,
    project: &str,
    report: ImportReport,
    files: Vec<(String, String)>,
) -> Result<Response, ApiError> {
    let gitea: &GiteaClient = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let default_branch = gitea.default_branch().await?;
    let branch = format!("portal/import-{project}-{:08x}", digest(&files));
    create_or_reuse_branch(gitea, &branch, &default_branch).await?;

    let (author_name, author_email) = author_credentials(user, project);
    for (path, content) in &files {
        let existing = gitea
            .get_file(path, &branch)
            .await
            .ok()
            .flatten()
            .map(|file| file.sha);
        let message = format!("import {path}");
        gitea
            .put_file(&FileWrite {
                path,
                branch: &branch,
                message: &message,
                content,
                sha: existing.as_deref(),
                author: Author {
                    name: &author_name,
                    email: &author_email,
                },
            })
            .await?;
    }

    let title = format!("import {} resources into {project}", files.len());
    let body = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "imported bundle".into());
    let pull = gitea
        .create_pull_request(&branch, &default_branch, &title, &body)
        .await?;

    let summary = crate::change::PlanSummary::new(
        report.created.len(),
        report.replaced.len() + report.renamed.len(),
        0,
    );
    let change = Change::new(
        ChangeMeta::from_merge_request(pull.number, project),
        ChangeStatus::new(report.lane, ChangePhase::PendingApproval, summary)
            .with_merge_request(pull.url),
    );
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// A branch name that is the same for the same bundle, so a retry after a failed forge call
/// reuses the branch instead of leaving one behind per attempt.
fn digest(files: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    for (path, content) in files {
        path.hash(&mut hasher);
        content.hash(&mut hasher);
    }
    hasher.finish()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/import", axum::routing::post(import))
}
