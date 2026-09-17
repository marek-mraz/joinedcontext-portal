//! Drift: what the broker holds and the repository does not declare, or no longer holds as it
//! does (CC-21, CC-38, UI-25, UI-26).
//!
//! Configuration cannot drift — every component reads it from the repository (CC-72) — so the
//! one thing worth comparing is a space's seed entities. The comparison is `jcctl`'s, made
//! against the same space surface every other client reads, as this Portal's own service
//! account: a reader that went straight to the broker would report what no Policy would let a
//! person see.

use crate::reconciler::realm;
use jcctl::entities::{action, seed_entities, Action, SeedEntity};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::RwLock;
use std::time::Duration;

/// A space that has not answered in this long is a failed scan, not a slow one.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How a seed entity drifted, which is what decides the two buttons (UI-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Kind {
    /// Both sides have it and an attribute the file declares differs.
    #[serde(rename = "MODIFIED")]
    Modified,
    /// The repository declares it and the broker does not hold it.
    #[serde(rename = "MISSING")]
    Missing,
}

/// One drifted seed entity, as the API serves it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drifted {
    /// The Context Space that holds it.
    pub space: String,
    /// The entity's URN.
    pub id: String,
    /// How it drifted.
    pub drift: Kind,
    /// One entry per declared attribute the broker answers differently.
    pub diff: Vec<Difference>,
    /// Both resolutions, or only `adopt` where reverting would be the one the broker cannot do.
    pub resolutions: Vec<&'static str>,
    /// The file that declares it, so a person can read the change they are resolving.
    pub source: String,
}

/// One attribute the two sides disagree about.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Difference {
    /// The attribute, as a path inside the entity.
    pub path: String,
    /// What the repository declares.
    pub declared: Value,
    /// What the broker answered, absent when it holds none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<Value>,
}

/// What one scan found in one project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    /// When the scan ran.
    pub observed_at: chrono::DateTime<chrono::Utc>,
    /// Every drifted entity of the project, in repository order.
    pub entities: Vec<Drifted>,
}

/// What the last scan found, by project. Read by the API, replaced by the reconciler.
#[derive(Debug, Default)]
pub struct Store {
    found: RwLock<BTreeMap<String, Found>>,
}

impl Store {
    /// What the last scan found in one project, or nothing when none has run.
    pub fn of(&self, project: &str) -> Option<Found> {
        let lock = self.found.read().unwrap_or_else(|p| p.into_inner());
        lock.get(project).cloned()
    }

    /// The spaces of one project that hold a drifted entity.
    pub fn drifted_spaces(&self, project: &str) -> Vec<String> {
        let mut spaces: Vec<String> = self
            .of(project)
            .map(|found| found.entities.iter().map(|e| e.space.clone()).collect())
            .unwrap_or_default();
        spaces.sort();
        spaces.dedup();
        spaces
    }

    /// Replaces everything the previous scan found: a project that no longer drifts has to
    /// stop being reported, and a project the scan could not read keeps nothing stale.
    pub fn replace_all(&self, scanned: BTreeMap<String, Found>) {
        let mut lock = self.found.write().unwrap_or_else(|p| p.into_inner());
        *lock = scanned;
    }
}

/// One realm client and the space surface, for reading and putting back seed entities.
pub struct Watch {
    http: reqwest::Client,
    /// The platform host the spaces are served on, without a trailing slash.
    base: String,
    /// `https://host/realms/{realm}`.
    issuer: String,
    client_id: String,
    client_secret: String,
}

impl Watch {
    /// A watch on one platform host, authenticating as the Portal's own client.
    pub fn new(
        base: impl Into<String>,
        issuer: &str,
        client_id: String,
        client_secret: String,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: base.into().trim_end_matches('/').to_owned(),
            issuer: issuer.trim_end_matches('/').to_owned(),
            client_id,
            client_secret,
        }
    }

    async fn token(&self) -> Result<String, String> {
        realm::token(&self.http, &self.issuer, &self.client_id, &self.client_secret).await
    }

    /// The entity the space holds under this id, or `None` when it holds none.
    pub async fn live(&self, token: &str, space: &str, id: &str) -> Result<Option<Value>, String> {
        let response = self
            .http
            .get(format!("{}/cs/{space}/ngsi-ld/v1/entities/{id}", self.base))
            .bearer_auth(token)
            // Normalized JSON without the `@context` document: what the repository declares.
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!("{space} answered {}", response.status()));
        }
        response.json().await.map(Some).map_err(|e| e.to_string())
    }

    /// Writes the declared entity back into the space it belongs to (UI-26, revert).
    ///
    /// An upsert, so the attributes the file does not declare are left as they are: reverting
    /// restores what Git owns and deletes nothing (CC-19).
    pub async fn put(&self, space: &str, entity: &Value) -> Result<(), String> {
        let token = self.token().await?;
        let response = self
            .http
            .post(format!(
                "{}/cs/{space}/ngsi-ld/v1/entityOperations/upsert",
                self.base
            ))
            .bearer_auth(&token)
            .json(&json!([entity]))
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() {
            true => Ok(()),
            false => Err(format!(
                "{space} refused the write: {}",
                response.status()
            )),
        }
    }

    /// The live entity a resolution is about to work on.
    pub async fn read(&self, space: &str, id: &str) -> Result<Option<Value>, String> {
        let token = self.token().await?;
        self.live(&token, space, id).await
    }

    /// Compares every seed entity of a staged checkout with the space that holds it.
    ///
    /// One token for the whole scan, and the entities of every project in one map: a scan is
    /// one pass of the reconciler, not one call per page view.
    pub async fn scan(&self, repo_dir: &Path) -> Result<BTreeMap<String, Found>, String> {
        let declared = seed_entities(repo_dir).map_err(|e| e.to_string())?;
        let observed_at = chrono::Utc::now();
        let mut found: BTreeMap<String, Found> = BTreeMap::new();
        if declared.is_empty() {
            return Ok(found);
        }

        let token = self.token().await?;
        for entity in &declared {
            let live = self.live(&token, &entity.space, &entity.id).await?;
            let Some(drifted) = drifted(entity, live.as_ref()) else {
                continue;
            };
            found
                .entry(entity.project.clone())
                .or_insert_with(|| Found {
                    observed_at,
                    entities: Vec::new(),
                })
                .entities
                .push(drifted);
        }
        // A project whose entities all match still has to answer "nothing drifted, as of now".
        for entity in &declared {
            found.entry(entity.project.clone()).or_insert_with(|| Found {
                observed_at,
                entities: Vec::new(),
            });
        }
        Ok(found)
    }
}

/// One declared entity and what the broker answered, as the API serves it, or `None` when the
/// two agree.
fn drifted(entity: &SeedEntity, live: Option<&Value>) -> Option<Drifted> {
    let source = entity.source.to_string_lossy().to_string();
    // The staged checkout is a temporary directory, and its name is nobody's business: the
    // path a person reads is the one inside the repository.
    let source = match source.find("projects/") {
        Some(at) => source[at..].to_owned(),
        None => source,
    };
    match action(&entity.body, live) {
        Action::Unchanged => None,
        Action::Create => Some(Drifted {
            space: entity.space.clone(),
            id: entity.id.clone(),
            drift: Kind::Missing,
            diff: Vec::new(),
            // Adopting an entity the broker does not hold would propose an empty file.
            resolutions: vec!["revert"],
            source,
        }),
        Action::Update => Some(Drifted {
            space: entity.space.clone(),
            id: entity.id.clone(),
            drift: Kind::Modified,
            diff: differences(&entity.body, live),
            resolutions: vec!["revert", "adopt"],
            source,
        }),
    }
}

/// Every declared attribute the broker answers differently, as one path each.
///
/// Only what the file declares: a space also holds what its pipelines and its devices write,
/// and telemetry beside a seeded entity is not drift (CC-07, CC-69).
fn differences(declared: &Value, live: Option<&Value>) -> Vec<Difference> {
    let (Some(declared), Some(live)) = (declared.as_object(), live.and_then(Value::as_object))
    else {
        return Vec::new();
    };
    let mut differences = Vec::new();
    for (name, value) in declared {
        if matches!(name.as_str(), "id" | "createdAt" | "modifiedAt") {
            continue;
        }
        let held = live.get(name);
        if held == Some(value) {
            continue;
        }
        // One level in: an NGSI-LD attribute is an object, and "airQualityIndex.value" is what
        // a person reads, not the whole Property beside the whole Property.
        match (value.as_object(), held.and_then(Value::as_object)) {
            (Some(members), Some(held_members)) => {
                for (member, value) in members {
                    let held = held_members.get(member);
                    if held != Some(value) {
                        differences.push(Difference {
                            path: format!("{name}.{member}"),
                            declared: value.clone(),
                            live: held.cloned(),
                        });
                    }
                }
            }
            _ => differences.push(Difference {
                path: name.clone(),
                declared: value.clone(),
                live: held.cloned(),
            }),
        }
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entity(local: &str, index: u32) -> SeedEntity {
        SeedEntity {
            project: "mesto".to_owned(),
            space: "ovzdusie".to_owned(),
            id: format!("urn:ngsi-ld:AirQualityObserved:bb.sk:ovzdusie:{local}"),
            body: json!({
                "id": format!("urn:ngsi-ld:AirQualityObserved:bb.sk:ovzdusie:{local}"),
                "type": "AirQualityObserved",
                "airQualityIndex": { "type": "Property", "value": index }
            }),
            source: PathBuf::from("/tmp/stage-1234/projects/mesto/spaces/ovzdusie/entities/seed/s.json"),
        }
    }

    #[test]
    fn an_entity_the_broker_holds_as_declared_is_not_reported() {
        let entity = entity("1", 42);
        assert!(drifted(&entity, Some(&entity.body)).is_none());
    }

    #[test]
    fn a_changed_attribute_is_modified_with_both_values_and_both_resolutions() {
        let entity = entity("1", 42);
        let mut live = entity.body.clone();
        live["airQualityIndex"]["value"] = json!(7);
        live["temperature"] = json!({ "type": "Property", "value": 17.4 });

        let drifted = drifted(&entity, Some(&live)).expect("the value moved");
        assert_eq!(drifted.drift, Kind::Modified);
        assert_eq!(drifted.resolutions, vec!["revert", "adopt"]);
        assert_eq!(
            drifted.diff,
            vec![Difference {
                path: "airQualityIndex.value".to_owned(),
                declared: json!(42),
                live: Some(json!(7)),
            }],
            "only the declared attribute, and the telemetry beside it is not drift"
        );
        assert_eq!(
            drifted.source,
            "projects/mesto/spaces/ovzdusie/entities/seed/s.json",
            "the path inside the repository, not the staging directory"
        );
    }

    #[test]
    fn an_entity_the_broker_lost_is_missing_and_cannot_be_adopted() {
        let drifted = drifted(&entity("1", 42), None).expect("the broker holds none");
        assert_eq!(drifted.drift, Kind::Missing);
        assert_eq!(
            drifted.resolutions,
            vec!["revert"],
            "adopting nothing would propose an empty file"
        );
        assert!(drifted.diff.is_empty());
    }

    #[test]
    fn the_store_answers_per_project_and_forgets_what_a_scan_no_longer_finds() {
        let store = Store::default();
        assert!(store.of("mesto").is_none());

        let drifted = drifted(&entity("1", 42), None).expect("missing");
        store.replace_all(BTreeMap::from([(
            "mesto".to_owned(),
            Found {
                observed_at: chrono::Utc::now(),
                entities: vec![drifted],
            },
        )]));
        assert_eq!(store.drifted_spaces("mesto"), vec!["ovzdusie".to_owned()]);

        store.replace_all(BTreeMap::from([(
            "mesto".to_owned(),
            Found {
                observed_at: chrono::Utc::now(),
                entities: Vec::new(),
            },
        )]));
        assert!(store.drifted_spaces("mesto").is_empty());
        assert!(
            store.of("mesto").is_some(),
            "the project still answers: nothing drifted, as of the last scan"
        );
    }
}
