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
use crate::apps::reconciler::generate_slug;
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
    /// The organisation domain every imported id is rewritten to; this instance's own
    /// organisation when absent (PF-10, PF-43).
    #[serde(default)]
    pub org_domain: Option<String>,
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
    /// Per file, whether what the import wrote equals the checksum the bundle index carries,
    /// with the namespace mapping undone (MF-42). Empty when the bundle carries no checksums:
    /// an unverifiable transfer says so rather than claiming every file is equal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified: Vec<Verified>,
}

/// One file of a bundle as the import verified it (MF-42).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Verified {
    /// The path the bundle index gave the file.
    pub path: String,
    /// Whether the checksum matched.
    pub equal: bool,
}

impl ImportReport {
    /// `n of m files equal`, the line the `Change` body leads with (MF-42), or `None` when the
    /// bundle carried no checksums.
    pub fn verification_summary(&self) -> Option<String> {
        if self.verified.is_empty() {
            return None;
        }
        let equal = self.verified.iter().filter(|file| file.equal).count();
        Some(format!("{equal} of {} files equal", self.verified.len()))
    }
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
        // A complete export's README and schemas explain the bundle; they are not the project's
        // files, so they are never written into one (MF-41).
        if name == crate::api::export::README_PATH
            || name.starts_with(crate::api::export::SCHEMAS_DIR)
        {
            continue;
        }
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

    // One document that is not a manifest is the file's own format — Bento's config, a LinkML
    // source — and it travels byte for byte (MF-17, T-0923). Re-serialising the parse would
    // drop the comments and the layout that are half of what those formats carry, and would
    // make the bundle's checksum disagree with what was written (MF-42).
    let single = documents.len() == 1;
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
        let mut item = as_incoming(&document, path)?;
        if single && item.envelope.is_none() {
            item.content = text.to_owned();
        }
        incoming.push(item);
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
    for key in crate::apps::converge::BUILT_ANNOTATIONS {
        if envelope.metadata.annotations.contains_key(key) {
            refusals.push(format!(
                "{name}: annotation '{key}' is written by this instance's build lane and cannot \
                 be imported; the image is built here (AP-11, AP-13a)"
            ));
        }
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
            if state
                .mirror
                .get(namespace_for(&kind, project), &kind, &name)
                .is_some()
            {
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

/// The spaces a bundle carries: every `ContextSpace` it holds and the space each other
/// manifest declares.
///
/// An id in one of those spaces is the bundle's own and travels with it; an id in any other
/// space names another organisation — a federated registration, a peer's endpoint — and is
/// left alone, or an import would quietly re-point federation at this city (MF-22, PF-43).
fn own_spaces(manifests: &[ResourceEnvelope]) -> BTreeSet<String> {
    let mut spaces = BTreeSet::new();
    for envelope in manifests {
        if envelope.kind == "ContextSpace" {
            spaces.insert(envelope.metadata.name.clone());
        }
        if let Some(space) = space_of(envelope) {
            spaces.insert(space.to_owned());
        }
    }
    spaces
}

/// Rewrites one manifest into the target namespace: the metadata, the namespace of every
/// typed reference, and the organisation domain of every id it carries.
///
/// The URN is the part that is easy to forget and expensive to get wrong. An id follows
/// `urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}` (PF-10), and the gateway refuses a write
/// whose `{orgDomain}` is not the one owning the space (PF-43), so a bundle imported into
/// another organisation keeps pointing at the organisation it came from unless the segment
/// moves with it. The `{space}` segment is a ContextSpace name, not a project name: it changes
/// only when the space itself is renamed by the conflict policy, which happens later, in
/// [`plan_import`].
fn remap(
    envelope: &mut ResourceEnvelope,
    from: &str,
    to: &str,
    domain: &str,
    spaces: &BTreeSet<String>,
) {
    // An organization-scoped kind lives in namespace `org` whatever project imported it;
    // jc-core refuses the manifest otherwise, so the target project would write a file its
    // own CI rejects (PF-22, MF-22).
    let declared = envelope.metadata.namespace.clone();
    envelope.metadata.namespace =
        Some(namespace_of(&envelope.kind, declared.as_deref(), to).to_owned());
    remap_value(&mut envelope.spec, from, to, domain, spaces);
}

/// The namespace a kind is stored in: `org` for an organization-scoped kind, the project
/// otherwise. A kind that lives in either — `Role` — keeps `org` when the bundle wrote it
/// there and lands in the project when the bundle wrote it in one (PF-68, T-0820).
fn namespace_of<'a>(kind: &str, declared: Option<&str>, project: &'a str) -> &'a str {
    if resource::belongs_to_the_organization(kind, declared) {
        crate::permissions::ORG_NAMESPACE
    } else {
        project
    }
}

/// Where a reference to `kind` points, with nothing declared: what a typed reference means from
/// inside `project`.
fn namespace_for<'a>(kind: &str, project: &'a str) -> &'a str {
    namespace_of(kind, None, project)
}

fn remap_value(value: &mut Value, from: &str, to: &str, domain: &str, spaces: &BTreeSet<String>) {
    match value {
        Value::Object(members) => {
            // A typed reference that named the source namespace names the one it landed in;
            // an organization-scoped kind is addressed as `org` from anywhere.
            let referenced = members
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| namespace_for(kind, to).to_owned());
            if let (Some(target), Some(Value::String(namespace))) =
                (referenced, members.get_mut("namespace"))
            {
                if namespace.as_str() == from || target == crate::permissions::ORG_NAMESPACE {
                    *namespace = target;
                }
            }
            for child in members.values_mut() {
                remap_value(child, from, to, domain, spaces);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| remap_value(item, from, to, domain, spaces)),
        Value::String(text) => {
            if let Some(rewritten) = remap_urn(text, domain, spaces) {
                *text = rewritten;
            }
        }
        _ => {}
    }
}

/// One id, or one anchored `idPattern`, with its organisation domain replaced (PF-10, R33).
///
/// A pattern is a URN prefix with `^` in front and the dots of the domain escaped
/// (`^urn:ngsi-ld:AirQualityObserved:hel\.fi:air-quality:.*$`), which is why the comparison
/// unescapes and the replacement escapes: a Policy or a registration anchored on the source
/// organisation would otherwise keep routing there after the move (MF-22, T-0826).
fn remap_urn(urn: &str, domain: &str, spaces: &BTreeSet<String>) -> Option<String> {
    let pattern = urn.starts_with('^');
    let body = urn.strip_prefix('^').unwrap_or(urn);
    if !body.starts_with("urn:ngsi-ld:") {
        return None;
    }
    let mut segments: Vec<String> = body.split(':').map(str::to_owned).collect();
    // urn : ngsi-ld : Type : orgDomain : space : localId (or `.*$` in a pattern)
    if segments.len() < 6 {
        return None;
    }
    if !spaces.contains(&segments[4]) {
        return None;
    }
    let replacement = if pattern {
        regex::escape(domain)
    } else {
        domain.to_owned()
    };
    if segments[3] == replacement {
        return None;
    }
    segments[3] = replacement;
    let joined = segments.join(":");
    Some(if pattern {
        format!("^{joined}")
    } else {
        joined
    })
}

/// Whether a string is a domain name the URN scheme can carry: labels of letters, digits and
/// hyphens, separated by dots. A `:` would split an id into a segment nobody meant.
fn is_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// Moves every id of a renamed space onto its new name (MF-23, MF-26).
fn rewrite_space(value: &mut Value, old: &str, new: &str) {
    match value {
        Value::Object(members) => members
            .values_mut()
            .for_each(|child| rewrite_space(child, old, new)),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_space(item, old, new)),
        Value::String(text) => {
            let pattern = text.starts_with('^');
            let body = text.strip_prefix('^').unwrap_or(text);
            if !body.starts_with("urn:ngsi-ld:") {
                return;
            }
            let mut segments: Vec<String> = body.split(':').map(str::to_owned).collect();
            if segments.len() < 6 || segments[4] != old {
                return;
            }
            segments[4] = new.to_owned();
            let joined = segments.join(":");
            *text = if pattern {
                format!("^{joined}")
            } else {
                joined
            };
        }
        _ => {}
    }
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
                "orgDomain" => {
                    options.org_domain = Some(String::from_utf8_lossy(&data).trim().to_owned())
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
    let (status, body) = import_bundle(&state, &user.0.identity, &project, &bytes, options).await?;
    Ok((status, Json(body)).into_response())
}

/// One bundle imported into a project: the plan when it is a dry run, the `Change` otherwise
/// (MF-18…MF-21). The route and the `jc_project_import` operation both call this, so an MCP
/// client and a browser import the same way (AG-59, T-0840).
pub async fn import_bundle(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    bytes: &[u8],
    options: ImportOptions,
) -> Result<(StatusCode, Value), ApiError> {
    let project = project.to_owned();
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

    let incoming = parse(bytes)?;
    authorize(state, identity, &project, &incoming)?;
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

    // The organisation whose ids the import writes: this instance's own, unless the caller
    // names another (a staging instance replaying a city's bundle under its own domain).
    let domain = match options.org_domain.clone().filter(|d| !d.trim().is_empty()) {
        Some(domain) => {
            if !is_domain(&domain) {
                return Err(ApiError::BadRequest(format!(
                    "orgDomain '{domain}' is not a domain name (PF-10)"
                )));
            }
            domain
        }
        None => crate::api::assistant::org_domain(state, &project),
    };

    let (report, files) = plan_import(
        &incoming,
        state,
        &project,
        &target,
        &domain,
        options.conflict_policy,
    )?;
    if options.dry_run {
        return Ok((
            StatusCode::OK,
            serde_json::to_value(report).map_err(|e| ApiError::Internal(e.to_string()))?,
        ));
    }
    if files.is_empty() {
        return Err(ApiError::BadRequest(
            "the bundle holds nothing to import".into(),
        ));
    }
    let headline = incoming.iter().find_map(|item| {
        item.envelope
            .as_ref()
            .map(|envelope| (envelope.kind.clone(), envelope.metadata.name.clone()))
    });
    let change = propose_bundle(
        state,
        identity,
        &project,
        report,
        files,
        headline.as_ref().map(|(k, n)| (k.as_str(), n.as_str())),
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        serde_json::to_value(change).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}

/// Commits a bundle of files and manifests as one merge request (MF-21, CC-63). `headline`
/// is the manifest (kind, name) Approvals shows for the bundle: the branch is named
/// `portal/create-{kind}-{name}-{hash}`, the form the change list reads, so the whole merge
/// request can be listed, approved and merged from the Portal.
/// ponytail: the change's plan counts that one manifest; a bundle-aware Change (every
/// manifest in the plan) is the upgrade.
pub async fn propose_bundle(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    report: ImportReport,
    files: Vec<(String, String)>,
    headline: Option<(&str, &str)>,
) -> Result<Change, ApiError> {
    let gitea: &GiteaClient = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let default_branch = gitea.default_branch().await?;
    let hash = digest(&files);
    let branch = match headline {
        Some((kind, name)) => format!(
            "portal/create-{}-{name}-{hash:08x}",
            kind.to_ascii_lowercase()
        ),
        None => format!("portal/import-{project}-{hash:08x}"),
    };
    let branch = create_or_reuse_branch(gitea, &branch, &default_branch).await?;

    let (author_name, author_email) = author_credentials(identity, project);
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
    let detail = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "imported bundle".into());
    // The one line an approver reads before the report: whether the transfer arrived whole
    // (MF-42). A bundle without checksums has no such line rather than a reassuring one.
    let body = match report.verification_summary() {
        Some(summary) => format!("{summary}\n\n{detail}"),
        None => detail,
    };
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
    Ok(change)
}

/// What the bundle index says each of its files hashes to, against what arrived (MF-42).
///
/// The comparison is made on the manifest as it was uploaded — before the namespace mapping,
/// the domain rewrite and the `imported-from` annotation this import applies — because that is
/// the written manifest with the mapping undone, and it is the form the exporter hashed. A
/// bundle with no `spec.files` answers an empty list: an import that cannot verify a transfer
/// says nothing about it rather than reporting every file equal.
fn verify(incoming: &[Incoming]) -> Result<Vec<Verified>, ApiError> {
    use sha2::{Digest, Sha256};

    let expected: BTreeMap<String, String> = incoming
        .iter()
        .filter_map(|item| item.envelope.as_ref())
        .find(|envelope| envelope.kind == BUNDLE_KIND)
        .and_then(|index| index.spec.get("files").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|file| {
            let path = file.get("path")?.as_str()?.to_owned();
            let sha256 = file.get("sha256")?.as_str()?.to_owned();
            Some((path, sha256))
        })
        .collect();
    if expected.is_empty() {
        return Ok(Vec::new());
    }

    let mut verified = Vec::new();
    for item in incoming {
        let Some(path) = item.path.as_deref() else {
            continue;
        };
        let Some(wanted) = expected.get(path) else {
            continue;
        };
        let bytes = match &item.envelope {
            Some(envelope) if envelope.kind == BUNDLE_KIND => continue,
            Some(envelope) => serde_yaml_ng::to_string(envelope)
                .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?,
            // A native file travels byte for byte, so the bytes that arrived are the bytes
            // that will be written.
            None => item.content.clone(),
        };
        let found = format!("{:x}", Sha256::digest(bytes.as_bytes()));
        verified.push(Verified {
            path: path.to_owned(),
            equal: &found == wanted,
        });
    }
    // A file the index lists and the upload does not hold was lost on the way: it is unequal,
    // not absent from the report (MF-42).
    for path in expected.keys() {
        if !verified.iter().any(|file| &file.path == path) {
            verified.push(Verified {
                path: path.clone(),
                equal: false,
            });
        }
    }
    verified.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(verified)
}

/// What the import would do, and the files it would write.
#[allow(clippy::type_complexity)]
fn plan_import(
    incoming: &[Incoming],
    state: &AppState,
    project: &str,
    target: &str,
    domain: &str,
    policy: ConflictPolicy,
) -> Result<(ImportReport, Vec<(String, String)>), ApiError> {
    let mut manifests: Vec<ResourceEnvelope> = Vec::new();
    let mut natives: Vec<(String, String)> = Vec::new();
    let mut source: Option<String> = None;
    let mut refused: Vec<String> = Vec::new();
    let verified = verify(incoming)?;

    for item in incoming {
        let Some(envelope) = item.envelope.clone() else {
            if let Some(path) = &item.path {
                natives.push((path.clone(), item.content.clone()));
            }
            continue;
        };
        if envelope.kind == BUNDLE_KIND {
            // The index describes the bundle, so it is provenance and not a resource (MF-20).
            // The index is the platform's `Bundle` (T-0823): the project is its name and the
            // revision is `sourceRevision`. An archive downloaded from an older Portal carries
            // `spec.project` and `spec.revision` instead, and still says where it came from.
            let from = envelope
                .spec
                .get("project")
                .and_then(Value::as_str)
                .unwrap_or(&envelope.metadata.name)
                .to_owned();
            let revision = envelope
                .spec
                .get("sourceRevision")
                .or_else(|| envelope.spec.get("revision"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            source = Some(format!("{from}@{revision}"));
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

    // The destination project is the one in the URL, and its own manifest already describes
    // it: a `Project` carried by the bundle would be written at `projects/{its own name}/`,
    // a directory of this repository that belongs to another project (PF-22, MF-22).
    manifests.retain(|envelope| envelope.kind != "Project");

    // The source namespace is whatever the bundle was exported from; a bundle whose manifests
    // disagree about it is remapped from each one's own, which is what a hand-assembled
    // multi-project bundle needs.
    // The origin travels with each manifest because a rename is named after it.
    let spaces = own_spaces(&manifests);
    let manifests: Vec<(String, ResourceEnvelope)> = manifests
        .into_iter()
        .map(|mut envelope| {
            let from = envelope
                .metadata
                .namespace
                .clone()
                .unwrap_or_else(|| target.to_owned());
            remap(&mut envelope, &from, target, domain, &spaces);
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
        verified,
    };

    // Conflicts first: a rename rewrites references, so it has to happen before anything is
    // resolved or planned.
    let mut keep: Vec<ResourceEnvelope> = Vec::new();
    for (origin, envelope) in manifests {
        let stored = namespace_of(
            &envelope.kind,
            envelope.metadata.namespace.as_deref(),
            project,
        );
        let existing = state
            .mirror
            .get(stored, &envelope.kind, &envelope.metadata.name)
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
                while state.mirror.get(stored, &envelope.kind, &new).is_some() {
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
                // A renamed ContextSpace is the one rename that also moves ids: the `{space}`
                // segment of every URN is that name (PF-10, MF-26).
                if kind == "ContextSpace" {
                    rewrite_space(&mut envelope.spec, old, new);
                }
            }
        }
    }

    // The slug is the endpoint's public address and the platform mints it, per environment
    // (EP-02, EP-75, CC-74). A bundle carries the source's, so an import that kept it would
    // serve the destination's dataset at the source's capability URL — and two imports of one
    // bundle would answer at one address, where the gateway's table keeps only the last
    // (T-0821). The one slug that survives is the one this instance already minted for this
    // endpoint, so updating a project by re-importing its bundle does not move its endpoints
    // under the people using them.
    for envelope in &mut keep {
        if envelope.kind != "Endpoint" {
            continue;
        }
        let stored = namespace_of(
            &envelope.kind,
            envelope.metadata.namespace.as_deref(),
            project,
        );
        let held = state
            .mirror
            .get(stored, "Endpoint", &envelope.metadata.name)
            .and_then(|existing| {
                existing
                    .spec
                    .get("slug")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let slug = held.unwrap_or_else(|| generate_slug().to_string());
        if let Some(spec) = envelope.spec.as_object_mut() {
            spec.insert("slug".to_owned(), Value::String(slug));
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
            .get(
                namespace_of(
                    &envelope.kind,
                    envelope.metadata.namespace.as_deref(),
                    project,
                ),
                &envelope.kind,
                &envelope.metadata.name,
            )
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

        // The manifest's own namespace decides where it lands, which is what keeps an
        // organization role in `users/` and a project's own role in its project (PF-68).
        let home = namespace_of(
            &envelope.kind,
            envelope.metadata.namespace.as_deref(),
            project,
        );
        let path =
            resource::repository_path(info, home, space_of(envelope), &envelope.metadata.name)
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

/// Who may propose what the bundle holds (T-0798, PF-50): the caller's bindings in this
/// project, checked per manifest before anything is planned, so a dry run discloses nothing
/// either. `mutate::propose_with_identity` asks the same two questions of a single manifest.
fn authorize(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    incoming: &[Incoming],
) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    for item in incoming {
        match &item.envelope {
            Some(envelope) if envelope.kind == BUNDLE_KIND => {}
            Some(envelope) => {
                let raw = serde_json::to_value(envelope)
                    .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
                effective.check(&envelope.kind, jc_core::kinds::Verb::Propose, Some(&raw))?;
                crate::permissions::within_own_rights(state, identity, &raw, "proposer")?;
            }
            None => {
                let Some(path) = &item.path else { continue };
                // A native file is proposed under the kind its directory names; a path that
                // names no kind has no role that could grant it.
                let kind = native_kind(&reproject(path, project)).ok_or_else(|| {
                    ApiError::Denied(format!(
                        "'{path}' belongs to no kind this platform serves, so no role grants \
                         proposing it (PF-50)"
                    ))
                })?;
                effective.check(kind, jc_core::kinds::Verb::Propose, None)?;
            }
        }
    }
    Ok(())
}

/// The kind a native file belongs to: the plural directory after `projects/{project}/`, or
/// after `spaces/{space}/` for the kinds that live under a space.
pub(crate) fn native_kind(path: &str) -> Option<&'static str> {
    let mut segments = path.split('/').skip(2);
    let first = segments.next()?;
    let plural = if first == "spaces" {
        segments.nth(1)?
    } else {
        first
    };
    resource::by_plural(plural).map(|info| info.kind)
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
pub(crate) fn riskiest(left: Lane, right: Lane) -> Lane {
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

#[cfg(test)]
mod verification_tests {
    use super::{ImportReport, Verified};
    use crate::change::Lane;

    fn report(verified: Vec<Verified>) -> ImportReport {
        ImportReport {
            created: Vec::new(),
            replaced: Vec::new(),
            skipped: Vec::new(),
            renamed: std::collections::BTreeMap::new(),
            native_files: 0,
            lane: Lane::Green,
            source: None,
            verified,
        }
    }

    fn file(path: &str, equal: bool) -> Verified {
        Verified {
            path: path.to_owned(),
            equal,
        }
    }

    /// MF-42: the line the change body leads with counts what matched out of what was checked.
    #[test]
    fn the_summary_counts_what_matched_out_of_what_was_checked() {
        let files: Vec<Verified> = (0..10)
            .map(|index| file(&format!("f{index}.yaml"), index != 3))
            .collect();
        assert_eq!(
            report(files).verification_summary().as_deref(),
            Some("9 of 10 files equal")
        );
    }

    #[test]
    fn a_transfer_that_all_arrived_says_so_in_the_same_words() {
        assert_eq!(
            report(vec![file("a.yaml", true), file("b.yaml", true)])
                .verification_summary()
                .as_deref(),
            Some("2 of 2 files equal")
        );
    }

    /// A bundle with no checksums is unverifiable, and says nothing rather than something
    /// reassuring: `0 of 0 files equal` would read as a verified transfer.
    #[test]
    fn a_bundle_that_carried_no_checksums_says_nothing_about_verification() {
        assert_eq!(report(Vec::new()).verification_summary(), None);
    }
}
