//! The foreign-model mirror, on its schedule (DM-48, DM-49).
//!
//! `jcctl::foreign_models` decides everything: which references have a schema surface, what a
//! peer's catalogue means, whether a document's digest matches what the peer declares, and
//! what the repository already pins. This module is the two halves that judgement cannot have
//! — the socket and the forge — and nothing more.
//!
//! Every mirror is proposed, never applied. DM-49 asks for a changed digest to reach a
//! reviewer, and a new mirror is the same class of thing: a model another organisation
//! publishes, arriving in this repository without anybody here having written it. One rule is
//! also one rule to get wrong, where "created lands, changed is reviewed" is two.
//!
//! A peer whose models are all unchanged produces no branch and no commit, which is what makes
//! a daily mirror of a quiet peer silent (CC-18).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jcctl::foreign_models::{
    self, FetchError, Mirror as MirrorPlan, MirrorError, Outcome, SchemaApi, Surface,
};
use jcctl::loader::{RawManifest, Repository};

use super::proposal::{self, Proposal};
use crate::git::{GitError, GiteaClient};
use crate::reconciler::daemon::{stage, SyncError};

/// The kinds that carry a peer's schema surface (DM-48).
const REFERENCE_KINDS: [&str; 2] = ["SharedSpaceReference", "ContextSourceRegistration"];

/// What one pass over the repository's references did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// The merge requests this pass opened, by URL.
    pub proposed: Vec<String>,
    /// References that were due and had nothing to change.
    pub unchanged: usize,
    /// What a reviewer has to be told: a peer without a surface, a name that cannot be
    /// mirrored, a peer that did not answer.
    pub flags: Vec<String>,
}

/// Why a pass could not run at all. A single peer failing is a flag, not one of these.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("the repository could not be read: {0}")]
    Repository(#[from] SyncError),
    #[error("the repository does not load: {0}")]
    Load(#[from] jcctl::loader::LoadError),
    #[error("forge: {0}")]
    Git(#[from] GitError),
    #[error("no HTTP client for peer schema surfaces: {0}")]
    Client(String),
    #[error("{0}")]
    Mirror(#[from] MirrorError),
}

/// When each reference was last fetched, so a schedule means something between ticks.
///
/// In memory on purpose. A restarted Portal re-fetches each peer once, and an unchanged peer
/// produces no commit, so the cost of forgetting is one HTTP request per reference per restart
/// — much less than a table nothing else needs.
#[derive(Debug, Default)]
pub struct LastFetch(Mutex<HashMap<String, u64>>);

impl LastFetch {
    fn get(&self, key: &str) -> Option<u64> {
        self.0.lock().ok()?.get(key).copied()
    }

    fn set(&self, key: &str, at: u64) {
        if let Ok(mut map) = self.0.lock() {
            map.insert(key.to_owned(), at);
        }
    }
}

/// The hop to the peers, shared with the blocking task that does the fetching.
///
/// A trait object rather than a generic so a test can drive the whole pass over a peer it
/// controls: the surface is `https` by the time the module above accepts it, and no mock HTTP
/// server offers that.
pub type Peers = Arc<dyn SchemaApi + Send + Sync>;

/// `Arc<dyn SchemaApi>` as the sized value `foreign_models::mirror` takes.
struct Shared(Peers);

impl SchemaApi for Shared {
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>, FetchError> {
        self.0.get(url)
    }
}

/// How often the mirror looks at the repository. The reference's own `schedule` decides
/// whether a peer is fetched (24 hours by default, DM-49); this only has to be finer than
/// that, and an hour is fine enough to be sure a daily mirror happens on the day.
const TICK: Duration = Duration::from_secs(60 * 60);

/// The mirror on its own timer, on the replica that reconciles (DM-49, CC-03).
///
/// Its own task rather than a step of the reconcile loop: the loop runs every half minute and
/// this stages the repository, which is a tree listing and a file request per manifest. Two
/// replicas mirroring would open the same merge request twice, so the leader check is the
/// first thing each tick does — the same flag the reconcile loop maintains.
pub fn spawn_periodic(
    gitea: Arc<GiteaClient>,
    syncer: Arc<crate::reconciler::Syncer>,
    public_url: String,
    peers: Peers,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let last_fetch = LastFetch::default();
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            if !syncer.is_leader() {
                continue;
            }
            let now = crate::auth::session::now_unix().max(0) as u64;
            match run_once(&gitea, &public_url, &peers, &last_fetch, now).await {
                Ok(run) => {
                    for flag in &run.flags {
                        tracing::warn!(flag = %flag, "foreign model mirror");
                    }
                    if !run.proposed.is_empty() {
                        tracing::info!(
                            proposed = run.proposed.len(),
                            unchanged = run.unchanged,
                            "foreign model mirror opened merge requests"
                        );
                    }
                }
                Err(err) => tracing::warn!(error = %err, "the foreign model mirror did not run"),
            }
        }
    })
}

/// One pass over every reference in the repository (DM-48, DM-49).
///
/// `public_url` is this instance's own base: a `SharedSpaceReference` names an Endpoint this
/// instance serves, so its surface cannot be derived without it. A Portal that does not know
/// its own URL mirrors only the peers that name an address themselves.
pub async fn run_once(
    gitea: &GiteaClient,
    public_url: &str,
    peers: &Peers,
    last_fetch: &LastFetch,
    now: u64,
) -> Result<Run, RunError> {
    let revision = gitea.branch_head(&gitea.default_branch().await?).await?;
    let scratch = stage(gitea, &revision).await?;
    let repository = Repository::load(scratch.path())?;

    let mut run = Run::default();
    for (id, resource) in repository.iter() {
        if !REFERENCE_KINDS.contains(&id.kind.as_str()) {
            continue;
        }
        let key = format!("{}/{}", id.namespace.as_deref().unwrap_or("-"), id.name);
        let schedule = schedule_of(&resource.manifest);
        if !foreign_models::due(schedule.as_ref(), last_fetch.get(&key), now) {
            continue;
        }

        let base = match foreign_models::surface_of(&resource.manifest, public_url, &repository) {
            Ok(Surface::At(base)) => base,
            Ok(Surface::None(reason)) => {
                run.flags.push(format!("{key}: {reason}"));
                continue;
            }
            Err(err) => {
                run.flags.push(format!("{key}: {err}"));
                continue;
            }
        };

        match mirror_one(
            gitea,
            &key,
            &resource.manifest,
            &base,
            &repository,
            peers,
            now,
        )
        .await
        {
            Ok(Some(url)) => run.proposed.push(url),
            Ok(None) => run.unchanged += 1,
            Err(err) => {
                // One unreachable peer is not a failed pass: the others still mirror, and the
                // reference stands with whatever the repository already pinned (DM-48).
                run.flags.push(format!("{key}: {err}"));
                continue;
            }
        }
        last_fetch.set(&key, now);
    }
    Ok(run)
}

/// Mirrors one reference and proposes the result, or `None` when nothing changed.
#[allow(clippy::too_many_arguments)]
async fn mirror_one(
    gitea: &GiteaClient,
    key: &str,
    reference: &RawManifest,
    base: &str,
    repository: &Repository,
    peers: &Peers,
    now: u64,
) -> Result<Option<String>, RunError> {
    let plan = fetch(reference.clone(), base.to_owned(), repository, peers, now).await?;

    let changed = plan
        .models
        .iter()
        .any(|model| model.outcome != Outcome::Unchanged);
    if !changed {
        return Ok(None);
    }

    // `write` is `jcctl`'s, so the manifest on the branch is serialised by the code that reads
    // it back. It writes into a directory of this run's own, never into the staged repository:
    // the loaded tree is what the flags above were computed against.
    let out = crate::reconciler::daemon::Scratch::new("mirror").map_err(RunError::Repository)?;
    foreign_models::write(out.path(), &plan)?;

    let mut files = BTreeMap::new();
    for model in &plan.models {
        if model.outcome == Outcome::Unchanged {
            continue;
        }
        collect(out.path(), &model.path, &mut files)?;
        for path in model.files.keys() {
            collect(out.path(), path, &mut files)?;
        }
    }

    let body = serde_json::to_string_pretty(&plan.to_json()).unwrap_or_default();
    let pull = proposal::open(
        gitea,
        &Proposal {
            branch: &branch_name(key, &files),
            title: &format!("mirror {} foreign model(s) for {key}", files.len()),
            body: &body,
            files,
            removed: Vec::new(),
            // Never. A mirror is another organisation's schema arriving in this repository,
            // and DM-49 wants a person to see it.
            auto_merge: false,
        },
    )
    .await?;
    tracing::info!(reference = %key, url = %pull.url, "opened a merge request for a peer's schema");
    Ok(Some(pull.url))
}

/// The fetch itself, on a blocking thread.
///
/// `foreign_models::mirror` is synchronous and so is the transport under it, which is the right
/// shape for a judgement that runs once a day: what it must not do is sit on a runtime worker
/// while a peer takes its time.
async fn fetch(
    reference: RawManifest,
    base: String,
    repository: &Repository,
    peers: &Peers,
    now: u64,
) -> Result<MirrorPlan, RunError> {
    let fetched_at = rfc3339(now);
    // The loaded repository cannot cross to the blocking thread, so the one thing that half
    // needs travels instead: the staged directory it was loaded from.
    let dir = repository.root().to_path_buf();
    let api = Shared(Arc::clone(peers));
    tokio::task::spawn_blocking(move || {
        let repository = Repository::load(&dir)?;
        Ok(foreign_models::mirror(
            &reference,
            &base,
            &fetched_at,
            &repository,
            &api,
        )?)
    })
    .await
    .map_err(|err| RunError::Client(format!("the mirror task did not finish: {err}")))?
}

/// Reads one file `write` produced back out of the run's directory.
fn collect(
    root: &Path,
    relative: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), RunError> {
    let path = root.join(relative);
    let body = std::fs::read(&path).map_err(|source| MirrorError::Io {
        path: relative.to_path_buf(),
        source,
    })?;
    // The forge writes text. Every document of a schema surface is text (LinkML, JSON-LD,
    // JSON), so a peer that sent something else has sent something a mirror does not carry.
    let text = String::from_utf8(body).map_err(|_| {
        MirrorError::Index(format!(
            "{} is not UTF-8 and is not a document a schema surface publishes",
            relative.display()
        ))
    })?;
    files.insert(relative.to_string_lossy().replace('\\', "/"), text);
    Ok(())
}

/// A branch name that is the same for the same peer state, so a retry after a failed forge
/// call continues where it stopped instead of opening a second merge request (CC-18).
fn branch_name(key: &str, files: &BTreeMap<String, String>) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    for (path, body) in files {
        path.hash(&mut hasher);
        body.hash(&mut hasher);
    }
    let slug: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("mirror/{slug}-{:08x}", hasher.finish() as u32)
}

/// The mirror schedule a reference declares, when it declares one (DM-49).
fn schedule_of(manifest: &RawManifest) -> Option<jc_core::kinds::Schedule> {
    serde_json::from_value(manifest.spec.get("schedule")?.clone()).ok()
}

/// Epoch seconds as the RFC 3339 timestamp the mirrored manifest records.
fn rfc3339(now: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(now as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_name_is_the_same_for_the_same_peer_state() {
        let mut files = BTreeMap::new();
        files.insert("projects/bb/models/air.yaml".to_owned(), "a: 1".to_owned());
        let first = branch_name("bb/peer", &files);
        assert_eq!(first, branch_name("bb/peer", &files));

        files.insert("projects/bb/models/air.yaml".to_owned(), "a: 2".to_owned());
        assert_ne!(
            first,
            branch_name("bb/peer", &files),
            "a different digest is a different branch, or the second run would push onto the \
             first one's review"
        );
    }

    #[test]
    fn a_branch_name_carries_nothing_a_git_ref_refuses() {
        let mut files = BTreeMap::new();
        files.insert("p".to_owned(), "c".to_owned());
        let name = branch_name("bb/peer name", &files);
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/'),
            "{name}"
        );
    }

    #[test]
    fn the_timestamp_is_the_shape_the_manifest_records() {
        assert_eq!(rfc3339(1_757_000_000), "2025-09-04T15:33:20Z");
    }

    #[test]
    fn a_reference_without_a_schedule_falls_back_to_the_default() {
        let manifest: RawManifest = serde_json::from_value(serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "SharedSpaceReference",
            "metadata": { "name": "peer", "namespace": "bb" },
            "spec": { "endpointSlug": "k7m2qz4tv6xh3n5jb2ryd3wcfa", "alias": "peer" }
        }))
        .expect("a manifest");
        assert!(schedule_of(&manifest).is_none());
        // No schedule and never fetched is due; fetched a minute ago is not.
        assert!(foreign_models::due(None, None, 1_000));
        assert!(!foreign_models::due(None, Some(940), 1_000));
    }
}
