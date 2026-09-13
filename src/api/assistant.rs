//! The assistant finds data (AG-58, API/01 §18): the spaces, endpoints and data models of a
//! project whose manifests match the words of a question, each with the caller's access verdict
//! and the freshness of the pipeline feeding it. Answered from the mirror, never from a guess;
//! the same search is the kit run's `search_catalog` tool (Architecture/09 §9).

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::share;
use crate::api::pipelines::metrics_for;
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::resource::{is_dns1123, ResourceEnvelope};
use crate::state::AppState;
use crate::store::ListOptions;

/// The best matches only: a person reads a screen of cards, a model a page of context.
pub const MAX_ITEMS: usize = 20;
/// In the order ties are broken: what a person opens first.
const KINDS: [&str; 3] = ["Endpoint", "ContextSpace", "DataModel"];

#[derive(Debug, Deserialize)]
pub struct CatalogQuery {
    pub q: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub q: String,
    pub items: Vec<CatalogItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    pub kind: String,
    pub name: String,
    pub space: String,
    pub owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_slug: Option<String>,
    pub match_reason: Vec<String>,
    pub access: Access,
    pub freshness: Option<Freshness>,
    #[serde(skip)]
    hits: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Access {
    /// `allowed` or `restricted`.
    pub verdict: String,
    pub reason: String,
}

/// The runner's counters for the pipeline that feeds an endpoint, read when the search ran.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Freshness {
    pub pipeline: String,
    pub scraped_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<u64>,
}

impl Access {
    fn allowed(reason: impl Into<String>) -> Self {
        Self {
            verdict: "allowed".into(),
            reason: reason.into(),
        }
    }
    fn restricted(reason: impl Into<String>) -> Self {
        Self {
            verdict: "restricted".into(),
            reason: reason.into(),
        }
    }
    fn is_allowed(&self) -> bool {
        self.verdict == "allowed"
    }
}

/// The words of a question: two characters or more, lower-cased, each once.
pub fn words(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_lowercase)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Every string leaf of a value, joined: a title in four languages is four strings.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(text_of).collect::<Vec<_>>().join(" "),
        Value::Object(map) => map.values().map(text_of).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// A `contextSpaceRef` is a name or an object naming one.
pub(crate) fn ref_name(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(map) => map.get("name").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn space_of(env: &ResourceEnvelope) -> String {
    if env.kind == "ContextSpace" {
        return env.metadata.name.clone();
    }
    ref_name(&env.spec["contextSpaceRef"])
        .or_else(|| env.metadata.labels.get("joinedcontext.com/space").cloned())
        .unwrap_or_default()
}

/// The fields a word can hit, named as `matchReason` names them.
fn fields(env: &ResourceEnvelope) -> Vec<(&'static str, String)> {
    let meta = serde_json::to_value(&env.metadata).unwrap_or(Value::Null);
    let labels: String = env
        .metadata
        .labels
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = vec![
        ("name", env.metadata.name.clone()),
        ("title", text_of(&meta["title"])),
        ("description", text_of(&meta["description"])),
        ("labels", labels),
    ];
    if env.kind == "Endpoint" {
        out.push(("slug", text_of(&env.spec["slug"])));
    }
    if env.kind == "DataModel" {
        out.push(("classes", text_of(&env.spec["classes"])));
    }
    if env.kind != "ContextSpace" {
        out.push(("space", space_of(env)));
    }
    out
}

/// How many words hit, and the fields they hit.
fn score(fields: &[(&'static str, String)], words: &[String]) -> (usize, Vec<String>) {
    let lowered: Vec<(&str, String)> = fields
        .iter()
        .map(|(name, text)| (*name, text.to_lowercase()))
        .collect();
    let hits = words
        .iter()
        .filter(|w| lowered.iter().any(|(_, text)| text.contains(w.as_str())))
        .count();
    let reasons = lowered
        .iter()
        .filter(|(_, text)| words.iter().any(|w| text.contains(w.as_str())))
        .map(|(name, _)| (*name).to_owned())
        .collect();
    (hits, reasons)
}

/// Whether the endpoint's audience admits a signed-in member of `project` (AG-58). The data's
/// own grants are the gateway's decision (EP-55); this says whether the door is open at all.
fn endpoint_access(spec: &Value, project: &str) -> Access {
    match spec["audience"].as_str().unwrap_or("project-list") {
        "public" => Access::allowed("audience public"),
        "organization" => Access::allowed("audience organization: every signed-in member"),
        "project-list" => {
            let named = spec["allowedProjects"]
                .as_array()
                .is_some_and(|list| list.iter().any(|p| p.as_str() == Some(project)));
            if named {
                Access::allowed(format!("audience project-list names {project}"))
            } else {
                Access::restricted(format!("audience project-list does not name {project}"))
            }
        }
        other => Access::restricted(format!("audience {other}")),
    }
}

fn title_of(env: &ResourceEnvelope) -> Option<String> {
    let meta = serde_json::to_value(&env.metadata).unwrap_or(Value::Null);
    let title = &meta["title"];
    title["en"]
        .as_str()
        .or_else(|| {
            title
                .as_object()
                .and_then(|m| m.values().find_map(Value::as_str))
        })
        .map(str::to_owned)
}

/// The endpoint each pipeline of the project writes through: the last segment of the
/// `targetEndpoint` URN, or the whole value when it is a bare name.
fn feeders(state: &AppState, project: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for env in state
        .mirror
        .list(project, "Pipeline", &ListOptions::default())
        .items
    {
        if let Some(target) = env.spec["targetEndpoint"].as_str() {
            let endpoint = target.rsplit(':').next().unwrap_or(target).to_owned();
            out.entry(endpoint).or_insert(env.metadata.name.clone());
        }
    }
    out
}

/// The search over the mirror, with the freshness read from the runner for the endpoints that
/// are returned and open to the caller.
pub async fn search(state: &AppState, project: &str, q: &str, scope: Option<&str>) -> Catalog {
    let words = words(q);
    let catalog = Catalog {
        q: q.to_owned(),
        items: Vec::new(),
    };
    if words.is_empty() {
        return catalog;
    }
    let opts = ListOptions::default();
    let endpoints = state.mirror.list(project, "Endpoint", &opts).items;
    let spaces = state.mirror.list(project, "ContextSpace", &opts).items;
    let models = state.mirror.list(project, "DataModel", &opts).items;

    // Which spaces an endpoint opens, so a space and a model inherit the verdict of a door.
    let mut space_access: BTreeMap<String, Access> = BTreeMap::new();
    let mut matched_endpoints_of_space: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut items: Vec<CatalogItem> = Vec::new();

    for env in &endpoints {
        let access = endpoint_access(&env.spec, project);
        let space = space_of(env);
        if access.is_allowed() {
            space_access.entry(space.clone()).or_insert_with(|| {
                Access::allowed(format!("endpoint {} admits you", env.metadata.name))
            });
        }
        let (hits, reasons) = score(&fields(env), &words);
        if hits == 0 {
            continue;
        }
        matched_endpoints_of_space
            .entry(space.clone())
            .or_default()
            .push(env.metadata.name.clone());
        items.push(item(env, project, space, hits, reasons, access));
    }
    for env in &spaces {
        let name = env.metadata.name.clone();
        let access = space_access
            .get(&name)
            .cloned()
            .unwrap_or_else(|| Access::restricted("no endpoint of the space admits you"));
        let (mut hits, mut reasons) = score(&fields(env), &words);
        if let Some(through) = matched_endpoints_of_space.get(&name) {
            hits = hits.max(1);
            reasons.extend(through.iter().map(|e| format!("endpoint {e}")));
        }
        if hits == 0 {
            continue;
        }
        items.push(item(env, project, name, hits, reasons, access));
    }
    for env in &models {
        let space = space_of(env);
        let access = space_access
            .get(&space)
            .cloned()
            .unwrap_or_else(|| Access::restricted("no endpoint of the model's space admits you"));
        let (hits, reasons) = score(&fields(env), &words);
        if hits == 0 {
            continue;
        }
        items.push(item(env, project, space, hits, reasons, access));
    }

    if let Some(scope) = scope {
        items.retain(|i| i.kind == scope);
    }
    let rank = |kind: &str| KINDS.iter().position(|k| *k == kind).unwrap_or(KINDS.len());
    items.sort_by(|a, b| {
        b.hits
            .cmp(&a.hits)
            .then_with(|| rank(&a.kind).cmp(&rank(&b.kind)))
            .then_with(|| a.name.cmp(&b.name))
    });
    items.truncate(MAX_ITEMS);

    // Freshness, read in parallel for the endpoints a pipeline feeds; a runner that does not
    // answer leaves `null`, never a guess (AG-58).
    let feeders = feeders(state, project);
    let mut reads = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if item.kind != "Endpoint" || !item.access.is_allowed() {
            continue;
        }
        let Some(pipeline) = feeders.get(&item.name).cloned() else {
            continue;
        };
        let state = state.clone();
        let project = project.to_owned();
        reads.push(tokio::spawn(async move {
            let metrics = metrics_for(&state, &project, &pipeline).await.ok()?;
            Some((
                index,
                Freshness {
                    pipeline,
                    scraped_at: metrics.scraped_at,
                    received: metrics.received,
                    errors: metrics.errors,
                },
            ))
        }));
    }
    for read in reads {
        if let Ok(Some((index, freshness))) = read.await {
            items[index].freshness = Some(freshness);
        }
    }
    Catalog {
        q: q.to_owned(),
        items,
    }
}

fn item(
    env: &ResourceEnvelope,
    project: &str,
    space: String,
    hits: usize,
    reasons: Vec<String>,
    access: Access,
) -> CatalogItem {
    // A restricted item shows its name and kind and nothing else (AG-58).
    let open = access.is_allowed();
    CatalogItem {
        kind: env.kind.clone(),
        name: env.metadata.name.clone(),
        space,
        owner: project.to_owned(),
        title: if open { title_of(env) } else { None },
        endpoint_slug: if open && env.kind == "Endpoint" {
            env.spec["slug"].as_str().map(str::to_owned)
        } else {
            None
        },
        match_reason: reasons,
        access,
        freshness: None,
        hits,
    }
}

pub async fn get_catalog(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<CatalogQuery>,
) -> Result<Json<Catalog>, ApiError> {
    let q = query.q.as_deref().unwrap_or("").trim();
    if q.is_empty() {
        return Err(ApiError::BadRequest("q must not be empty".into()));
    }
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let scope = match query.scope.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(kind) if KINDS.contains(&kind) => Some(kind),
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "scope '{other}' is not one of {}",
                KINDS.join(", ")
            )))
        }
    };
    Ok(Json(search(&state, &project, q, scope).await))
}

/// The organization's domain, from the `Organization` manifest of the repository; the project
/// name stands in when the mirror holds none, so a draft still renders.
pub fn org_domain(state: &AppState, fallback: &str) -> String {
    state
        .mirror
        .list(
            crate::api::blueprints::ORG_NAMESPACE,
            "Organization",
            &ListOptions::default(),
        )
        .items
        .into_iter()
        .find_map(|env| env.spec["domain"].as_str().map(str::to_owned))
        .unwrap_or_else(|| fallback.to_owned())
}

/// The share request rendered, not written (EP-72, API/01 §19): the manifests the person will
/// submit, refused for a caller who may not propose an Endpoint here (PF-50).
pub async fn propose_endpoint(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(params): Json<share::ProposeEndpoint>,
) -> Result<Json<share::Proposal>, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let proposal = share::render(&project, &org_domain(&state, &project), &params)
        .map_err(ApiError::BadRequest)?;
    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "Endpoint",
        Verb::Propose,
        Some(&proposal.endpoint),
    )?;
    Ok(Json(proposal))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/assistant/catalog", get(get_catalog))
        .route(
            "/projects/{project}/assistant/propose-endpoint",
            post(propose_endpoint),
        )
}

#[cfg(test)]
mod tests {
    use super::{endpoint_access, score, words};
    use serde_json::json;

    #[test]
    fn words_are_short_lower_and_unique() {
        assert_eq!(
            words("Where is the Bike  availability, bike?"),
            vec!["availability", "bike", "is", "the", "where"]
        );
        assert!(words("a , !").is_empty());
    }

    #[test]
    fn a_score_counts_words_and_names_fields() {
        let fields = vec![
            ("name", "helsinki-bikes".to_owned()),
            ("title", "City bikes".to_owned()),
            ("description", "Bike stations and availability".to_owned()),
        ];
        let (hits, reasons) = score(&fields, &words("bike availability"));
        assert_eq!(hits, 2);
        assert_eq!(reasons, vec!["name", "title", "description"]);
        assert_eq!(score(&fields, &words("parking")).0, 0);
    }

    #[test]
    fn the_audience_decides_the_verdict() {
        assert!(endpoint_access(&json!({ "audience": "public" }), "helsinki").is_allowed());
        assert!(endpoint_access(&json!({ "audience": "organization" }), "helsinki").is_allowed());
        assert!(endpoint_access(
            &json!({ "audience": "project-list", "allowedProjects": ["helsinki"] }),
            "helsinki"
        )
        .is_allowed());
        let shut = endpoint_access(
            &json!({ "audience": "project-list", "allowedProjects": ["espoo"] }),
            "helsinki",
        );
        assert!(!shut.is_allowed());
        assert!(shut.reason.contains("does not name helsinki"));
        assert!(!endpoint_access(&json!({}), "helsinki").is_allowed());
    }
}
