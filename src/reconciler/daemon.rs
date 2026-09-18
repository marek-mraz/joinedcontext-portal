//! The reconciler loop against the Git repository (T-0191, MF-04, CC-03, CC-08).
//!
//! Re-reads the declared manifests from Gitea at the default branch HEAD, loads them with
//! `jcctl`, compiles their live status and swaps them into the in-memory mirror atomically.
//!
//! The loading is `jcctl`'s, not the Portal's: the same code that validates a repository for
//! `jcctl plan` decides here what a manifest is, which kinds exist and which two files claim
//! one identity. A Portal that parsed manifests its own way would eventually disagree with
//! the CLI, and the disagreement would show up as a resource that CI accepts and the Portal
//! cannot see.
//!
//! A run that cannot load the repository leaves the mirror on the last revision that loaded
//! and records why in the status. Serving half a repository is worse than serving a slightly
//! old one: a resource missing from the Portal reads as deleted.
//!
//! Only the elected leader reconciles ([`super::leader`]): streams, apps and roles. Every
//! replica loads the repository into its own mirror, a follower read-only, so each one serves
//! the projects and a new pod of a rolling update is ready while the old one holds the lock
//! (OPS-51).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use utoipa::ToSchema;

use super::groups::GroupSync;
use super::leader::Leadership;
use super::registrations::RegistrationOutcome;
use super::streams::{
    eligible, is_stream_pipeline, make_condition, Bentos, StreamDeployer, StreamOutcome,
};
use super::subscriptions::SubscriptionOutcome;
use crate::activity::{ActivityEvent, ActivityStore};
use crate::apps::converge::{Converger, Outcome};
use crate::git::{Author, FileWrite, GitError, GiteaClient};
use crate::resource::ResourceEnvelope;
use crate::store::Mirror;

/// The resident runner's Deployment, which mounts the Secret this reconciler writes (T-0927).
/// One name, because a deployment that runs a second runner gives it the same chart and the
/// same Secret; a runner per project is a task of its own.
const PIPELINE_RUNNER_DEPLOYMENT: &str = "pipeline-runner";

/// Status of the background Git mirror synchronization.
///
/// Served to the browser, so it deliberately never carries a token,
/// repository URL, or branch name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub last_sync: Option<i64>,
    pub revision: Option<String>,
    pub manifests: usize,
    pub last_error: Option<String>,
    /// Whether this replica is the one that reconciles (CC-03). A Portal without a database
    /// has no election to run and reconciles on its own, so it reports itself as the leader.
    pub leader: bool,
}

/// Errors returned during repository synchronization.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("{0}")]
    Empty(String),
    /// The repository does not load as a set of manifests (CC-08, MF-05, MF-06).
    #[error("repository does not load: {0}")]
    Load(#[from] jcctl::loader::LoadError),
    /// `users/` does not compile: a binding names a role that does not exist, or a spec does
    /// not fit its kind (T-0527).
    #[error("roles do not compile: {0}")]
    Roles(String),
    /// The scratch directory the loader reads from could not be written.
    #[error("cannot stage the repository at {path}: {source}")]
    Scratch {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Background reconciler synchronizing repository manifests into the in-memory mirror.
pub struct Syncer {
    gitea: Arc<GiteaClient>,
    mirror: Arc<Mirror>,
    status: Arc<RwLock<SyncStatus>>,
    running: Arc<Mutex<()>>,
    /// `None` when the Portal has no database: a single replica needs no election.
    leadership: Option<Arc<Leadership>>,
    /// `None` when this Portal applies no app objects: outside a cluster, or without the
    /// settings that say which namespace they belong in (T-0411, AP-18).
    converger: Option<Arc<Converger>>,
    streams: Option<Arc<StreamDeployer>>,
    /// `None` when no gateway address is configured: a `Subscription` is then read from the
    /// repository and written into no broker (T-0931, CC-72).
    registrations: Option<Arc<super::registrations::RegistrationSync>>,
    /// The seed-entity drift scan and what its last run found (CC-21, UI-25, UI-26).
    drift: Option<(Arc<super::drift::Watch>, Arc<super::drift::Store>)>,
    subscriptions: Option<Arc<super::subscriptions::SubscriptionSync>>,
    /// `None` when no Keycloak admin client is configured: the `Group` manifests are then read
    /// and served, and the realm is written by nobody (PF-63).
    groups: Option<Arc<GroupSync>>,
    /// Where a run says what it did (OPS-48). `None` leaves the loop silent, which is what a
    /// Portal built without a state does in a unit test.
    activity: Option<ActivityStore>,
    /// Where the static host reads app bundles from, when this Portal serves any (AP-14): the
    /// run checks that the build each manifest names is actually there (AP-72).
    apps_dir: Option<String>,
    /// The artifact store's admin API, held with the root credential. `None` leaves every
    /// organization without a scoped credential and the store untouched (PF-32).
    artifact_store: Option<Arc<crate::artifact_store::Client>>,
    /// Where an organization's reader credential is handed to the workloads that serve
    /// artifacts: the cluster to write the Secret into, and the namespace it belongs in
    /// (T-0925). `None` mints the credentials and hands them to nobody.
    credentials: Option<(Arc<crate::apps::kube::KubeClient>, String)>,
    /// Which secret backend answers a pipeline's `secretRef` (T-0927, PL-15). `None` leaves a
    /// pipeline that declares one undeployed, with the reason on the Pipeline.
    pipeline_secrets: Option<crate::pipeline_secrets::Resolver>,
}

impl Syncer {
    pub fn new(gitea: Arc<GiteaClient>, mirror: Arc<Mirror>) -> Self {
        Self {
            gitea,
            mirror,
            status: Arc::new(RwLock::new(SyncStatus {
                leader: true,
                ..SyncStatus::default()
            })),
            running: Arc::new(Mutex::new(())),
            leadership: None,
            converger: None,
            streams: None,
            registrations: None,
            drift: None,
            subscriptions: None,
            groups: None,
            activity: None,
            apps_dir: None,
            artifact_store: None,
            credentials: None,
            pipeline_secrets: None,
        }
    }

    /// Makes each run mint the scoped artifact-store credentials of every Organization it reads
    /// (PF-32, ADR-N-015). Without one the store is never touched, which is what a Portal
    /// outside a cluster does.
    pub fn with_artifact_store(mut self, store: Arc<crate::artifact_store::Client>) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// Which backend resolves a pipeline's `secretRef`s into the runner's environment
    /// (T-0927, PL-15). Writing the Secret needs the cluster too: without
    /// [`with_credential_secrets`](Self::with_credential_secrets) the values resolve and reach
    /// nobody, so the pipelines that need them stay undeployed.
    pub fn with_pipeline_secrets(mut self, resolver: crate::pipeline_secrets::Resolver) -> Self {
        self.pipeline_secrets = Some(resolver);
        self
    }

    /// Makes each run hand every organization's reader credential to the workloads that serve
    /// its artifacts, as a Secret in `namespace` (T-0925, PF-32).
    ///
    /// Only the reader travels. The writer stays derived and unwritten: the one process that
    /// holds the root secret can mint it whenever `jcctl` or a build lane asks, and a key that
    /// can replace an artifact has no reason to sit in a namespace a serving pod reads.
    pub fn with_credential_secrets(
        mut self,
        kube: Arc<crate::apps::kube::KubeClient>,
        namespace: impl Into<String>,
    ) -> Self {
        self.credentials = Some((kube, namespace.into()));
        self
    }

    /// Where the static host reads app bundles from, so each run can say which app names a
    /// build that never arrived (AP-72).
    pub fn with_apps_dir(mut self, apps_dir: Option<String>) -> Self {
        self.apps_dir = apps_dir;
        self
    }

    /// Makes each run tell the activity feed what it applied (OPS-48, UI-31).
    pub fn with_activity(mut self, activity: ActivityStore) -> Self {
        self.activity = Some(activity);
        self
    }

    /// Deploys Bento streams for approved DataSource pipelines on each run (PL-47).
    /// Makes each run bring the realm's managed groups to what the manifests say (PF-63).
    pub fn with_groups(mut self, groups: Arc<GroupSync>) -> Self {
        self.groups = Some(groups);
        self
    }

    pub fn with_streams(mut self, deployer: Arc<StreamDeployer>) -> Self {
        self.streams = Some(deployer);
        self
    }

    /// Makes each run write what every `Subscription` manifest declares into its space, and
    /// remove the subscription of a manifest that is gone (CC-72, DS-16).
    /// The broker projection of `ContextSourceRegistration` manifests (T-0345, PF-48).
    pub fn with_registrations(
        mut self,
        registrations: Arc<super::registrations::RegistrationSync>,
    ) -> Self {
        self.registrations = Some(registrations);
        self
    }

    pub fn with_subscriptions(
        mut self,
        subscriptions: Arc<super::subscriptions::SubscriptionSync>,
    ) -> Self {
        self.subscriptions = Some(subscriptions);
        self
    }

    /// Makes each run compare the seed entities the repository declares against what the
    /// spaces hold, and keep the answer where the API reads it (CC-21).
    pub fn with_drift(
        mut self,
        watch: Arc<super::drift::Watch>,
        store: Arc<super::drift::Store>,
    ) -> Self {
        self.drift = Some((watch, store));
        self
    }

    /// Makes this replica compete for the reconciler role instead of assuming it (CC-03).
    pub fn with_leadership(mut self, leadership: Arc<Leadership>) -> Self {
        self.status
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .leader = leadership.is_leader();
        self.leadership = Some(leadership);
        self
    }

    /// Applies every App's Kubernetes objects on each run (T-0411).
    ///
    /// A Portal without one reads apps and deploys nothing, which is what running outside a
    /// cluster looks like.
    pub fn with_converger(mut self, converger: Arc<Converger>) -> Self {
        self.converger = Some(converger);
        self
    }

    /// Whether this replica currently reconciles.
    pub fn is_leader(&self) -> bool {
        self.leadership.as_ref().is_none_or(|l| l.is_leader())
    }

    pub fn status(&self) -> SyncStatus {
        self.status
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Synchronizes the mirror once against the Git repository.
    ///
    /// A run that finds another run of this replica in flight waits for it and then runs
    /// itself: a merge that lands while a sync is fetching is not in that sync, and the
    /// approval that asked for the refresh must not wait a whole tick for it (CC-08).
    /// A replica that is not the leader (CC-03) loads the mirror and converges nothing: its
    /// streams, apps and roles are the leader's to apply (OPS-51).
    pub async fn sync_once(&self) -> Result<usize, SyncError> {
        let _guard = self.running.lock().await;

        let leader = self.claim_leadership().await;
        if !leader {
            tracing::debug!("another replica holds the reconciler lock, loading the mirror only");
        }

        let before = self.status();
        match self.do_sync(leader).await {
            Ok((count, revision)) => {
                let landed = before.revision.as_deref() != Some(revision.as_str());
                {
                    let now = crate::auth::session::now_unix();
                    let mut status = self.status.write().unwrap_or_else(|p| p.into_inner());
                    status.last_sync = Some(now);
                    status.revision = Some(revision.clone());
                    status.manifests = count;
                    status.last_error = None;
                    status.leader = leader;
                }
                // Only a revision the mirror had not seen is news: the loop runs every tick and
                // a feed of "nothing changed" is a feed nobody reads.
                if landed {
                    self.say_applied(&revision, count).await;
                }
                Ok(count)
            }
            Err(err) => {
                let message = err.to_string();
                let repeated = before.last_error.as_deref() == Some(message.as_str());
                self.status
                    .write()
                    .unwrap_or_else(|p| p.into_inner())
                    .last_error = Some(message.clone());
                if !repeated {
                    self.say_drifted(&message).await;
                }
                Err(err)
            }
        }
    }

    /// One `config.applied` per project the mirror holds, because a project's feed shows the
    /// runs that touched it and a run touches the whole repository at once.
    async fn say_applied(&self, revision: &str, manifests: usize) {
        if self.activity.is_none() {
            return;
        }
        let short: String = revision.chars().take(7).collect();
        let events: Vec<ActivityEvent> = self
            .mirror
            .namespaces()
            .into_iter()
            .map(|project| ActivityEvent {
                time: chrono::Utc::now(),
                project,
                space: None,
                kind: "config.applied".to_string(),
                source: "reconciler".to_string(),
                summary: format!("The repository at {short} is live: {manifests} manifests."),
                severity: "info".to_string(),
                correlation_id: Some(revision.to_string()),
                details: serde_json::json!({ "revision": revision, "manifests": manifests }),
            })
            .collect();
        self.record(events).await;
    }

    /// A run that could not load the repository: the mirror keeps the last revision that did,
    /// so the feed is the only place this is visible to a person (CC-08).
    async fn say_drifted(&self, message: &str) {
        if self.activity.is_none() {
            return;
        }
        let events: Vec<ActivityEvent> = self
            .mirror
            .namespaces()
            .into_iter()
            .map(|project| ActivityEvent {
                time: chrono::Utc::now(),
                project,
                space: None,
                kind: "config.drifted".to_string(),
                source: "reconciler".to_string(),
                summary: format!("The repository did not load: {message}"),
                severity: "error".to_string(),
                correlation_id: None,
                details: serde_json::Value::Null,
            })
            .collect();
        self.record(events).await;
    }

    /// One `config.drifted` per group the console and the repository disagreed on, so the feed
    /// carries what the reconcile overwrote (PF-63, OPS-48).
    async fn say_group_drift(&self, outcomes: &[super::groups::GroupOutcome]) {
        if self.activity.is_none() {
            return;
        }
        let events: Vec<ActivityEvent> = outcomes
            .iter()
            .filter(|outcome| !outcome.drift.is_empty() || outcome.error.is_some())
            .map(|outcome| ActivityEvent {
                time: chrono::Utc::now(),
                project: crate::permissions::ORG_NAMESPACE.to_string(),
                space: None,
                kind: "config.drifted".to_string(),
                source: "reconciler".to_string(),
                summary: match &outcome.error {
                    Some(err) => format!("Group {}: {err}", outcome.name),
                    None => format!("Group {}: {}", outcome.name, outcome.drift.join("; ")),
                },
                severity: if outcome.error.is_some() {
                    "error".to_string()
                } else {
                    "warning".to_string()
                },
                correlation_id: None,
                details: serde_json::json!({ "group": outcome.name, "drift": outcome.drift }),
            })
            .collect();
        self.record(events).await;
    }

    async fn record(&self, events: Vec<ActivityEvent>) {
        let Some(activity) = self.activity.as_ref() else {
            return;
        };
        if events.is_empty() {
            return;
        }
        if let Err(err) = activity.append(&events).await {
            tracing::warn!(error = %err, "the run is not in the activity feed");
        }
    }

    /// Whether this replica may reconcile now, asking the database every time (T-0191).
    ///
    /// A database that cannot be reached demotes this replica rather than promoting it: two
    /// leaders are worse than none, because none only delays a mirror refresh.
    async fn claim_leadership(&self) -> bool {
        let Some(leadership) = self.leadership.as_ref() else {
            return true;
        };
        match leadership.acquire().await {
            Ok(leader) => {
                self.status
                    .write()
                    .unwrap_or_else(|p| p.into_inner())
                    .leader = leader;
                leader
            }
            Err(err) => {
                tracing::warn!(error = %err, "cannot reach the database to elect a reconciler");
                self.status
                    .write()
                    .unwrap_or_else(|p| p.into_inner())
                    .leader = false;
                false
            }
        }
    }

    /// Whether this replica's mirror holds the repository: a sync of its own succeeded once
    /// (OPS-51). A later failed run keeps the last revision that loaded, so it stays ready.
    pub fn is_ready(&self) -> bool {
        self.status().last_sync.is_some()
    }

    async fn do_sync(&self, leader: bool) -> Result<(usize, String), SyncError> {
        // 1. Resolve default branch and its commit revision
        let default_branch = self.gitea.default_branch().await?;
        let revision = self.gitea.branch_head(&default_branch).await?;

        // 2, 3. The repository as files on disk, this run's own.
        let scratch = stage(&self.gitea, &revision).await?;

        // 4. Load and validate the whole repository the way `jcctl plan` does (CC-08, MF-05).
        let repository = jcctl::loader::Repository::load(scratch.path())?;
        for (id, path, expected) in repository.misplaced() {
            tracing::warn!(resource = %id, path = %path.display(), %expected, "manifest is not at the path its kind declares");
        }

        // 4b. The author's mapping beside a Pipeline manifest (PL-03): the loader leaves
        //     `bento.yaml` alone, so the streams read it from the staged tree by name.
        let mut bentos = Bentos::new();
        for (id, resource) in repository.iter() {
            if id.kind != "Pipeline" {
                continue;
            }
            let beside = scratch
                .path()
                .join(&resource.path)
                .with_file_name("bento.yaml");
            if let Ok(text) = std::fs::read_to_string(&beside) {
                let namespace = id
                    .namespace
                    .clone()
                    .unwrap_or_else(|| namespace_of(&resource.path.to_string_lossy()));
                bentos.insert((namespace, id.name.clone()), text);
            }
        }

        // 5. Compile the live status of every resource and swap the mirror in one step, so a
        //    reader never sees a half-built repository (MF-04).
        let fresh_mirror = Mirror::new();
        let mut loaded = 0usize;
        for (_, resource) in repository.iter() {
            let path = resource.path.to_string_lossy().to_string();
            // The kind's own invariants, the check `jcctl validate` runs (T-0412). A manifest
            // that reached `main` before the Portal refused them at write time is still shown,
            // with the reason in the log, so an operator can fix it rather than lose it.
            if let Ok(yaml) = serde_json::to_string(&resource.manifest) {
                if let Some(Err(err)) =
                    jc_core::registry::validate_yaml(&resource.manifest.kind, &yaml)
                {
                    tracing::warn!(path = %path, error = %err, "manifest fails its kind's validation");
                }
            }
            let mut envelope: ResourceEnvelope = match serde_json::to_value(&resource.manifest)
                .and_then(serde_json::from_value)
            {
                Ok(envelope) => envelope,
                Err(err) => {
                    // The loader accepted the envelope, so this is a metadata member the
                    // Portal's own view does not know. Skipping one resource is right
                    // here: the manifest is valid, the Portal simply cannot show it.
                    tracing::warn!(path = %path, error = %err, "manifest does not fit the Portal's resource view");
                    continue;
                }
            };

            if envelope
                .metadata
                .namespace
                .as_deref()
                .unwrap_or("")
                .is_empty()
            {
                envelope.metadata.namespace = Some(namespace_of(&path));
            }

            // The one part of `status` the repository owns: what the build lane wrote back
            // when it published the artifact (AP-13a). The rest is computed here (MF-04).
            let build = envelope
                .status
                .as_ref()
                .and_then(|status| status.build.clone());
            envelope.strip_status();
            envelope.status = Some(crate::resource::Status {
                phase: crate::resource::Phase::Live,
                observed_revision: Some(revision.clone()),
                // The branch, not the revision: a Source link should keep working after the
                // next commit, and the observed revision is right there beside it.
                source_url: Some(self.gitea.browse_url(&path, &default_branch)),
                conditions: Vec::new(),
                build,
            });

            fresh_mirror.upsert(envelope);
            loaded += 1;
        }

        if loaded == 0 {
            return Err(SyncError::Empty(
                "no resource of a known kind in the staged manifests".to_string(),
            ));
        }

        // A follower stops here: what the runner accepted, what the cluster runs and what the
        //    forge enforces are the leader's to converge, so its stream pipelines say so.
        if !leader {
            mark_streams_pending(
                &fresh_mirror,
                "Follower",
                "another replica reconciles the streams; this one serves the repository",
            );
            self.mirror.replace_all(&fresh_mirror);
            return Ok((loaded, revision));
        }

        // 5a'. Resolve what every pipeline's `secretRef`s name and hand the values to the
        //      runner as one Secret (T-0927, PL-15). A pipeline whose reference does not
        //      resolve, or whose `envVar` another pipeline already claims, is named here and
        //      refused by the wave below rather than started without its credential.
        let refused = self
            .resolve_pipeline_secrets(&fresh_mirror, scratch.path())
            .await;

        // 5b. Deploy resident streams for eligible DataSource pipelines (PL-47).
        if let Some(deployer) = self.streams.as_ref() {
            let outcomes = deployer.converge(&fresh_mirror, &bentos, &refused).await;
            // A Live stream that reads nothing is the failure nobody sees: the runner keeps the
            // stream, the Portal says Live, and the counters are the only witness (T-0914). One
            // scrape per project with a Live stream, read for each of them.
            let mut counters: BTreeMap<String, Option<String>> = BTreeMap::new();
            for (ns, _, outcome) in &outcomes {
                if matches!(outcome, StreamOutcome::Live) && !counters.contains_key(ns) {
                    counters.insert(ns.clone(), deployer.metrics(ns).await);
                }
            }
            for (ns, name, outcome) in outcomes {
                let Some(mut envelope) = fresh_mirror.get(&ns, "Pipeline", &name) else {
                    continue;
                };
                let is_stream = serde_json::from_value::<jc_core::kinds::pipeline::PipelineSpec>(
                    envelope.spec.clone(),
                )
                .map(|s| is_stream_pipeline(&s))
                .unwrap_or(false);

                match outcome {
                    StreamOutcome::Live => {
                        if let Some(status) = envelope.status.as_mut() {
                            status.phase = crate::resource::Phase::Live;
                            status.conditions = match counters
                                .get(&ns)
                                .and_then(Option::as_deref)
                                .and_then(|body| failing(body, &name))
                            {
                                Some(said) => vec![make_condition(
                                    "StreamWriting",
                                    "False",
                                    "NothingWritten",
                                    &said,
                                )],
                                None => Vec::new(),
                            };
                        }
                        fresh_mirror.upsert(envelope);
                    }
                    StreamOutcome::Error(err) => {
                        if let Some(status) = envelope.status.as_mut() {
                            status.phase = crate::resource::Phase::Error;
                            status.conditions = vec![make_condition(
                                "StreamDeployed",
                                "False",
                                "RunnerRefused",
                                &err,
                            )];
                        }
                        fresh_mirror.upsert(envelope);
                    }
                    StreamOutcome::Skipped(why) => {
                        if is_stream {
                            if let Some(status) = envelope.status.as_mut() {
                                status.phase = crate::resource::Phase::Pending;
                                let reason = if why.contains("quota") {
                                    "QuotaExceeded"
                                } else if why.contains("disabled") || why.contains("paused") {
                                    "Paused"
                                } else {
                                    "Skipped"
                                };
                                status.conditions =
                                    vec![make_condition("StreamDeployed", "False", reason, why)];
                            }
                            fresh_mirror.upsert(envelope);
                        }
                    }
                }
            }
        } else {
            mark_streams_pending(
                &fresh_mirror,
                "NoRunner",
                "no pipeline runner is configured (JC_PORTAL_PIPELINE_RUNNER_URL)",
            );
        }

        // A pipeline whose credential did not resolve says so last, over whatever the wave
        // above wrote: it is stopped whether or not this Portal has a runner to deploy to, and
        // the missing reference is the reason the author can act on (T-0927, PL-15). The
        // condition names the reference, never the value.
        for ((namespace, name), reason) in &refused {
            let Some(mut envelope) = fresh_mirror.get(namespace, "Pipeline", name) else {
                continue;
            };
            if let Some(status) = envelope.status.as_mut() {
                status.phase = crate::resource::Phase::Error;
                status.conditions = vec![make_condition(
                    "StreamDeployed",
                    "False",
                    "SecretUnresolved",
                    reason,
                )];
            }
            fresh_mirror.upsert(envelope);
        }

        // 5b'. What every `Subscription` manifest declares, written into the space it names
        //      (T-0931, CC-72). The broker holds the effect, so the status of each manifest is
        //      where a person sees whether the declaration arrived.
        if let Some(subscriptions) = self.subscriptions.as_ref() {
            let outcomes = subscriptions
                .converge(
                    &fresh_mirror,
                    &self.mirror,
                    scratch.path(),
                    self.pipeline_secrets.as_ref(),
                )
                .await;
            for (namespace, name, outcome) in outcomes {
                let Some(mut envelope) = fresh_mirror.get(&namespace, "Subscription", &name) else {
                    continue;
                };
                if let Some(status) = envelope.status.as_mut() {
                    match &outcome {
                        SubscriptionOutcome::Written => {
                            status.phase = crate::resource::Phase::Live;
                            status.conditions = Vec::new();
                        }
                        SubscriptionOutcome::Error(reason) => {
                            status.phase = crate::resource::Phase::Error;
                            status.conditions = vec![make_condition(
                                "SubscriptionWritten",
                                "False",
                                "SpaceRefused",
                                reason,
                            )];
                        }
                    }
                }
                fresh_mirror.upsert(envelope);
            }
        }

        // 5b''. Every `ContextSourceRegistration`, written into the tenant of the hub space it
        //       names (T-0345, PF-48). A hub is a configuration, so the manifest's status is
        //       where a person sees whether the member was actually registered.
        if let Some(registrations) = self.registrations.as_ref() {
            let outcomes = registrations.converge(&fresh_mirror, &self.mirror).await;
            for (namespace, name, outcome) in outcomes {
                let Some(mut envelope) =
                    fresh_mirror.get(&namespace, "ContextSourceRegistration", &name)
                else {
                    continue;
                };
                if let Some(status) = envelope.status.as_mut() {
                    match &outcome {
                        RegistrationOutcome::Written => {
                            status.phase = crate::resource::Phase::Live;
                            status.conditions = Vec::new();
                        }
                        RegistrationOutcome::Error(reason) => {
                            status.phase = crate::resource::Phase::Error;
                            status.conditions = vec![make_condition(
                                "RegistrationWritten",
                                "False",
                                "BrokerRefused",
                                reason,
                            )];
                        }
                    }
                }
                fresh_mirror.upsert(envelope);
            }
        }

        // 5c. The realm's managed groups, brought to what `users/groups/` says (PF-63). The
        //     drift lands on the Group manifests of this run's mirror, so the Access page shows
        //     where the console and the repository disagreed.
        if let Some(groups) = self.groups.as_ref() {
            let outcomes = groups.converge(&fresh_mirror).await;
            for outcome in &outcomes {
                match (&outcome.error, outcome.drift.is_empty()) {
                    (Some(err), _) => {
                        tracing::warn!(group = %outcome.name, error = %err, "group did not converge")
                    }
                    (None, false) => {
                        tracing::info!(group = %outcome.name, drift = %outcome.drift.join("; "), "group brought back to the manifest")
                    }
                    (None, true) => {}
                }
                for warning in &outcome.warnings {
                    tracing::warn!(group = %outcome.name, %warning, "group member is not a realm user yet");
                }
            }
            super::groups::record(&fresh_mirror, &outcomes);
            self.say_group_drift(&outcomes).await;
        }

        self.mirror.replace_all(&fresh_mirror);

        // 6. Converge what an App compiles into (T-0411, AP-18, AP-21). The mirror is already
        //    swapped, so a cluster that refuses one object leaves the Portal serving the
        //    repository correctly and says why in the log; one app's failure is not the run's.
        if let Some(converger) = self.converger.as_ref() {
            // What each project may deploy (PF-74); the mirror is already swapped, so this is
            // the repository as it now stands.
            let beyond: std::collections::HashSet<(String, String)> = self
                .mirror
                .namespaces()
                .into_iter()
                .flat_map(|ns| {
                    crate::quotas::beyond(&self.mirror, &ns, "apps")
                        .into_iter()
                        .map(move |name| (ns.clone(), name))
                })
                .collect();
            for (app, outcome) in converger.converge(&repository, &beyond).await {
                match outcome {
                    Ok(Outcome::Applied) => tracing::info!(%app, "app objects applied"),
                    Ok(Outcome::Deleted) => tracing::info!(%app, "app objects deleted"),
                    Ok(Outcome::Skipped(why)) => {
                        tracing::debug!(%app, reason = %why, "app deploys nothing")
                    }
                    Err(err) => tracing::warn!(%app, error = %err, "app did not converge"),
                }
            }
        }

        // 6b. An app whose `status.build` names a build this host does not hold keeps the
        //     previous one serving, and says so on the App rather than looking healthy (AP-72).
        for name in crate::apps::static_host::build_missing(self.apps_dir.as_deref(), &self.mirror)
        {
            let Some(mut envelope) = self
                .mirror
                .find(|env| env.kind == "App" && env.metadata.name == name)
            else {
                continue;
            };
            if let Some(status) = envelope.status.as_mut() {
                status.conditions = vec![super::streams::make_condition(
                    "Ready",
                    "False",
                    "BuildMissing",
                    "status.build names a build this host does not hold; the previous one keeps \
                     serving (AP-72)",
                )];
            }
            tracing::warn!(app = %name, "the build the manifest names is not on the host");
            self.mirror.upsert(envelope);
        }

        // 6c. One writer and one reader per Organization in the artifact store, scoped to that
        //     organization's prefixes (PF-31, PF-32). Both are derived from the root credential
        //     this process holds, so the call is an upsert and re-running it changes nothing;
        //     one organization the store refuses is that organization's condition, not the
        //     run's failure, exactly like the app wave above.
        if let Some(store) = self.artifact_store.as_ref() {
            for name in self
                .mirror
                .matching(|envelope| envelope.kind == "Organization")
                .into_iter()
                .map(|envelope| envelope.metadata.name)
            {
                match store.ensure_organization(&name).await {
                    Ok(credentials) => {
                        tracing::info!(
                            organization = %name,
                            issued = credentials.len(),
                            "artifact store credentials are in place"
                        );
                        self.hand_over_reader(&name, store.as_ref()).await;
                    }
                    Err(err) => {
                        tracing::warn!(organization = %name, error = %err, "the artifact store issued no credential");
                        let Some(mut envelope) = self
                            .mirror
                            .find(|env| env.kind == "Organization" && env.metadata.name == name)
                        else {
                            continue;
                        };
                        if let Some(status) = envelope.status.as_mut() {
                            status.conditions = vec![super::streams::make_condition(
                                "ArtifactStoreCredentials",
                                "False",
                                "StoreRefused",
                                &err.to_string(),
                            )];
                        }
                        self.mirror.upsert(envelope);
                    }
                }
            }
        }

        // 7. Compile `users/` into what the forge enforces (T-0527, PF-51, PF-52, CC-41): the
        //    CODEOWNERS, the bindings as data, and the gate that reads them. Written only when
        //    the repository differs, so the commit this makes is seen once by the next run and
        //    changes nothing. A forge that refuses the write costs the run nothing but a line
        //    in the log; the Portal's own check (PF-50) holds either way.
        if let Err(err) = self.publish_roles(&repository, &default_branch).await {
            tracing::warn!(error = %err, "roles were not compiled into the repository");
        }

        // 8. Drift (CC-21): configuration cannot drift, because every component reads it from
        //    the repository (CC-72, T-0421 option B). What can is a space's seed entities, so
        //    that is what the scan compares — against the space surface every other client
        //    reads, as this Portal's own service account, so it reports what a person could
        //    see. Only the leader scans: a follower's answer would be the same read made twice.
        if leader {
            if let Some((watch, store)) = self.drift.as_ref() {
                match watch.scan(scratch.path()).await {
                    Ok(found) => {
                        let drifted: usize = found.values().map(|f| f.entities.len()).sum();
                        store.replace_all(found);
                        if drifted > 0 {
                            tracing::info!(drifted, "seed entities differ from what Git declares");
                        }
                    }
                    // A scan that failed keeps the previous answer rather than replacing it
                    // with an empty one: "nothing drifted" and "nothing was read" are not the
                    // same thing, and the second must not look like the first on the page.
                    Err(err) => tracing::warn!(error = %err, "the drift scan did not complete"),
                }
            }
        }

        Ok((loaded, revision))
    }

    /// Resolves every pipeline's references and writes them into the runner's one Secret.
    ///
    /// Returns the pipelines that must not be deployed, by `(project, name)`, with the reason
    /// for each: a reference that resolved to nothing, a reference with no `envVar`, or a
    /// variable another pipeline of this runner already claims (T-0927).
    ///
    /// Nothing configured is not a failure: a Portal with no backend refuses only the pipelines
    /// that declare a reference, and a Portal outside a cluster resolves them and has nowhere to
    /// write them, which is the same refusal for the same reason.
    async fn resolve_pipeline_secrets(
        &self,
        mirror: &Mirror,
        repository: &std::path::Path,
    ) -> BTreeMap<(String, String), String> {
        use crate::pipeline_secrets::{RunnerEnvironment, SecretError};

        let mut runner = RunnerEnvironment::default();
        for namespace in mirror.namespaces() {
            let page = mirror.list(
                &namespace,
                "Pipeline",
                &crate::store::ListOptions::default(),
            );
            for envelope in page.items {
                let references = pipeline_references(mirror, &namespace, &envelope);
                if references.is_empty() {
                    continue;
                }
                let resolved = match self.pipeline_secrets.as_ref() {
                    Some(resolver) => resolver.resolve(repository, &references).await,
                    None => Err(SecretError::NoBackend {
                        name: references[0].name.clone(),
                    }),
                };
                runner.add(&namespace, &envelope.metadata.name, resolved);
            }
        }

        if !runner.is_empty() {
            match self.credentials.as_ref() {
                Some((kube, namespace)) => self.write_runner_secret(kube, namespace, &runner).await,
                None => tracing::info!(
                    "no cluster: a pipeline's credentials resolve and reach no runner"
                ),
            }
        }

        runner.refused().clone()
    }

    /// Writes the runner's Secret and rolls the runner when its content changed.
    ///
    /// An environment variable is read once, when the pod starts, so a rotated credential
    /// reaches a running pipeline only with a restart. The annotation carries the fingerprint of
    /// the values, so an unchanged environment patches the same bytes and rolls nothing.
    async fn write_runner_secret(
        &self,
        kube: &crate::apps::kube::KubeClient,
        namespace: &str,
        runner: &crate::pipeline_secrets::RunnerEnvironment,
    ) {
        if let Err(err) = kube.apply(&runner.secret(namespace)).await {
            tracing::warn!(error = %err, "the pipeline runner's secrets were not written");
            return;
        }
        tracing::info!(
            variables = runner.variables().count(),
            "the pipeline runner's secrets are in place"
        );
        let rollout = serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": PIPELINE_RUNNER_DEPLOYMENT, "namespace": namespace },
            "spec": { "template": { "metadata": { "annotations": {
                "joinedcontext.com/pipeline-secrets": runner.fingerprint(),
            }}}},
        });
        if let Err(err) = kube.apply(&rollout).await {
            tracing::warn!(error = %err, "the pipeline runner was not rolled, so a rotated credential is not in its environment yet");
        }
    }

    /// Writes one organization's reader credential into the namespace its serving workloads
    /// read, so a credential the reconciler minted actually reaches them (T-0925, PF-32).
    ///
    /// A cluster that refuses the write costs the run a line in the log and nothing else, the
    /// way one app's failure is not the run's: the credential is derived, so the next sync
    /// writes exactly the same bytes and the one after that too.
    async fn hand_over_reader(&self, organization: &str, store: &crate::artifact_store::Client) {
        let Some((kube, namespace)) = self.credentials.as_ref() else {
            return;
        };
        let reader = store.credential(organization, crate::artifact_store::Role::Reader);
        let secret = crate::artifact_store::reader_secret(namespace, organization, &reader);
        match kube.apply(&secret).await {
            Ok(()) => tracing::info!(
                organization = %organization,
                secret = %crate::artifact_store::reader_secret_name(organization),
                "the artifact store reader is in the namespace that serves"
            ),
            Err(err) => tracing::warn!(
                organization = %organization,
                error = %err,
                "the reader credential was minted and not handed over"
            ),
        }
    }

    /// Writes every managed roles file whose content differs from the branch (T-0527).
    async fn publish_roles(
        &self,
        repository: &jcctl::loader::Repository,
        branch: &str,
    ) -> Result<(), SyncError> {
        let Some(files) =
            jcctl::roles::files(repository).map_err(|e| SyncError::Roles(e.to_string()))?
        else {
            return Ok(());
        };
        for (path, content) in files {
            let existing = self.gitea.get_file(path, branch).await?;
            if existing.as_ref().map(|f| f.content.as_str()) == Some(content.as_str()) {
                continue;
            }
            let message = format!("roles: compile users/ into {path} (T-0527)");
            self.gitea
                .put_file(&FileWrite {
                    path,
                    branch,
                    message: &message,
                    content: &content,
                    sha: existing.as_ref().map(|f| f.sha.as_str()),
                    author: Author {
                        name: crate::sync::proposal::AUTHOR_NAME,
                        email: crate::sync::proposal::AUTHOR_EMAIL,
                    },
                })
                .await?;
            tracing::info!(path, "roles compiled into the repository");
        }
        Ok(())
    }

    /// Spawns the background periodic sync task.
    ///
    /// When `interval` is `Duration::ZERO`, periodic sync is disabled and
    /// the returned handle completes immediately.
    pub fn spawn_periodic(self: Arc<Self>, interval: Duration) -> tokio::task::JoinHandle<()> {
        if interval.is_zero() {
            return tokio::spawn(async {});
        }

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // A tick that fell due while a run waited on an approval's sync is not owed.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                if let Err(err) = self.sync_once().await {
                    tracing::warn!(error = %err, "periodic mirror sync failed");
                }
            }
        })
    }
}

/// Every stream pipeline in `mirror` is `pending` with `StreamDeployed` false for `reason`: this
/// run deployed none of them.
fn mark_streams_pending(mirror: &Mirror, reason: &str, message: &str) {
    for ns in mirror.namespaces() {
        let page = mirror.list(&ns, "Pipeline", &crate::store::ListOptions::default());
        for mut envelope in page.items {
            let Ok(spec) = serde_json::from_value::<jc_core::kinds::pipeline::PipelineSpec>(
                envelope.spec.clone(),
            ) else {
                continue;
            };
            if !eligible(&spec) {
                continue;
            }
            if let Some(status) = envelope.status.as_mut() {
                status.phase = crate::resource::Phase::Pending;
                status.conditions =
                    vec![make_condition("StreamDeployed", "False", reason, message)];
            }
            mirror.upsert(envelope);
        }
    }
}

/// Identifies files that are candidates for ResourceEnvelope manifests.
///
/// The repository holds YAML that is not a manifest at all: the Portal theme, the navigation
/// tree, the locale bundles, the LinkML models and the native pipeline configurations. The
/// loader is strict on purpose, so those are never handed to it; what is handed to it are the
/// directories the manifest layout claims (CC-08).
pub(crate) fn is_candidate_manifest(path: &str) -> bool {
    let clean = path.trim_start_matches('/');
    if clean
        .split('/')
        .any(|segment| segment == ".." || segment.is_empty())
    {
        // The tree comes from the forge, so it is input: nothing that could climb out of the
        // staging directory is a candidate (CC-08).
        return false;
    }
    if clean == "org.yaml" || clean == "bundle.yaml" {
        return true;
    }
    if !clean.ends_with(".yaml") && !clean.ends_with(".yml") {
        return false;
    }
    // A blueprint is an organization-level manifest with a path of its own
    // (`blueprints/{name}/blueprint.yaml`, CC-23), and the flow gallery reads it from the
    // mirror like any other resource (CC-26). Only that one name: the notes and the fixtures
    // beside it are not manifests.
    if clean.starts_with("blueprints/") {
        return clean.ends_with("/blueprint.yaml");
    }
    // Roles and their bindings live beside the projects (T-0525): the Portal reads them for
    // its own checks and compiles them for the forge (T-0527).
    // The builder profiles are organization-level too (AG-26): `agentprofiles/{name}.yaml`.
    clean.starts_with("projects/")
        || clean.starts_with("users/roles/")
        || clean.starts_with("users/assignments/")
        || clean.starts_with("agentprofiles/")
}

/// Whether a staged file is an encrypted secrets file rather than a manifest (CC-06).
///
/// The same names `jcctl` skips when it loads a repository and reads when it resolves a
/// `secretRef`, so one file is never both.
fn is_encrypted_secrets_file(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(jcctl::secrets::sops::is_encrypted_file)
}

/// Prepares one fetched file for the loader, or leaves it out (MF-04, MF-05).
///
/// Two judgements are made here and nowhere else, because the loader is right to refuse both
/// and the Portal is right to survive them. A `status:` block somebody committed is dropped:
/// status is computed by the server and never read from Git, so a manifest carrying one is
/// sanitised rather than refused. A document of a kind the Portal does not serve is left out:
/// one unknown kind in the repository must not cost every other resource its place in the
/// mirror. Everything past this point is the loader's judgement, including which two files
/// claim one identity and which path a kind belongs at.
///
/// `Ok(None)` means the file held nothing to load.
fn stageable(content: &str) -> Result<Option<String>, serde_yaml_ng::Error> {
    let mut kept: Vec<String> = Vec::new();

    for document in serde_yaml_ng::Deserializer::from_str(content) {
        let mut value = serde_yaml_ng::Value::deserialize(document)?;
        let Some(mapping) = value.as_mapping_mut() else {
            continue;
        };
        mapping.remove("status");
        let kind = mapping
            .get("kind")
            .and_then(serde_yaml_ng::Value::as_str)
            .unwrap_or_default();
        if crate::resource::by_kind(kind).is_none() {
            tracing::warn!(kind = %kind, "manifest of an unknown kind, skipped");
            continue;
        }
        kept.push(serde_yaml_ng::to_string(&value)?);
    }

    if kept.is_empty() {
        return Ok(None);
    }
    Ok(Some(kept.join("---\n")))
}

/// The project a manifest without a namespace belongs to, taken from where it lies (CC-08).
fn namespace_of(path: &str) -> String {
    let clean = path.trim_start_matches('/');
    match clean.split('/').collect::<Vec<_>>().as_slice() {
        ["projects", project, ..] if !project.is_empty() => (*project).to_owned(),
        _ => "org".to_owned(),
    }
}

/// The directory one run stages the fetched manifests in.
///
/// `jcctl` loads a repository from a path, and the Portal reads its repository over the
/// forge API, so the two meet on disk. The directory belongs to one run and is removed when
/// that run ends: nothing from the repository outlives the sync that fetched it.
/// The repository at one revision, staged as files the loader can read (CC-08).
///
/// Its own function because two schedules need it: the mirror the Portal serves from, and the
/// foreign-model mirror that proposes a peer's schema. A second copy of this loop would be a
/// second answer to "which files in the repository are manifests".
pub(crate) async fn stage(gitea: &GiteaClient, revision: &str) -> Result<Scratch, SyncError> {
    let tree_paths = gitea.list_tree(revision).await?;
    let candidate_paths: Vec<String> = tree_paths
        .into_iter()
        .filter(|p| is_candidate_manifest(p))
        .collect();

    if candidate_paths.is_empty() {
        return Err(SyncError::Empty(
            "no manifests found in repository".to_string(),
        ));
    }

    // The staging directory is this run's own and is removed when it ends, whichever way it
    // ends.
    let scratch = Scratch::new(revision)?;
    let mut staged = 0usize;
    for path in &candidate_paths {
        let file = match gitea.get_file(path, revision).await {
            Ok(Some(f)) => f,
            Ok(None) => {
                // The tree listed it and the contents call does not have it: a race with a
                // force-push, not a broken manifest. The next run reads a consistent tree.
                tracing::warn!(path = %path, "candidate manifest listed in git tree not found");
                continue;
            }
            Err(err) => return Err(SyncError::Git(err)),
        };
        if path.ends_with("/bento.yaml") {
            // Not a manifest: the author's Bento mapping beside a Pipeline (PL-03), staged as
            // written so the streams can render it; the loader skips it by name.
            scratch.write(path, &file.content)?;
            continue;
        }
        if is_encrypted_secrets_file(path) {
            // Not a manifest either: the repository's own `*.enc.yaml`, which the SOPS backend
            // decrypts a Pipeline's `secretRef` from (CC-06, T-0935). It is staged as written
            // because the resolver reads this tree and nothing else; dropping it here is how
            // `demo-feed` came back as "not declared in any encrypted secrets file" while the
            // file sat in the repository. The loader skips it by name.
            scratch.write(path, &file.content)?;
            continue;
        }
        match stageable(&file.content) {
            Ok(Some(text)) => {
                scratch.write(path, &text)?;
                staged += 1;
            }
            Ok(None) => {
                tracing::warn!(path = %path, "no document of a kind the Portal serves, skipped")
            }
            Err(err) => {
                tracing::warn!(path = %path, error = %err, "not YAML, skipped")
            }
        }
    }

    if staged == 0 {
        return Err(SyncError::Empty(format!(
            "none of the {} candidate files holds a manifest",
            candidate_paths.len()
        )));
    }
    Ok(scratch)
}

pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new(revision: &str) -> Result<Self, SyncError> {
        // One directory per run, never per revision: two runs staging into one directory would
        // read each other's files, and the first to finish would delete the other's.
        static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jc-portal-sync-{}-{}-{run}",
            std::process::id(),
            revision.get(..12).unwrap_or(revision)
        ));
        std::fs::create_dir_all(&path).map_err(|source| SyncError::Scratch {
            path: path.clone(),
            source,
        })?;
        Ok(Self(path))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, content: &str) -> Result<(), SyncError> {
        let target = self.0.join(relative.trim_start_matches('/'));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SyncError::Scratch {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&target, content).map_err(|source| SyncError::Scratch {
            path: target,
            source,
        })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!(path = %self.0.display(), error = %err, "could not remove the sync staging directory");
        }
    }
}

/// What a Live stream's counters say when it is not writing (T-0914).
///
/// Errors and nothing sent is a stream that runs and never lands: a source that refuses the
/// runner's token, a mapping that throws on every message. Errors beside writes are the ordinary
/// weather of a stream — a page that failed and was retried — and say nothing on their own.
fn failing(metrics: &str, pipeline: &str) -> Option<String> {
    let counters = crate::api::pipelines::scrape(metrics, pipeline, String::new());
    let errors = counters.errors?;
    if errors == 0 || counters.sent.unwrap_or(0) > 0 {
        return None;
    }
    Some(format!(
        "the stream is running and has written nothing: {errors} error(s) and no message sent \
         since it started; the runner's log names the reason"
    ))
}

/// Every `secretRef` one Pipeline needs: its own, and those of the `DataSource` it reads.
///
/// PL-50 puts a connector's credentials on the `DataSource` (`spec.secrets`) and PL-15 puts the
/// pipeline's own on the Pipeline (`spec.secretRefs`); the runner has one environment and reads
/// both from it, so they are resolved together.
fn pipeline_references(
    mirror: &Mirror,
    namespace: &str,
    envelope: &ResourceEnvelope,
) -> Vec<jc_core::envelope::SecretRef> {
    let list = |value: Option<&serde_json::Value>| -> Vec<jc_core::envelope::SecretRef> {
        value
            .cloned()
            .map(serde_json::from_value)
            .and_then(Result::ok)
            .unwrap_or_default()
    };

    let mut references = list(envelope.spec.get("secretRefs"));
    let data_source = envelope
        .spec
        .pointer("/source/dataSourceRef")
        .and_then(crate::api::assistant::ref_name);
    if let Some(name) = data_source {
        if let Some(source) = mirror.get(namespace, "DataSource", &name) {
            references.extend(list(source.spec.get("secrets")));
        }
    }
    references
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING_STREAM: &str = r#"
input_received{stream="aq"} 42
output_sent{stream="aq"} 40
output_error{stream="aq"} 2
input_received{stream="kpi"} 6
output_error{stream="kpi"} 6
"#;

    #[test]
    fn a_stream_that_writes_nothing_and_only_errors_is_said_to_be_failing() {
        // T-0914: the KPI stream on dev was Live for an hour, reading a source that refused its
        // token; the counters were the only witness.
        let said = failing(RUNNING_STREAM, "kpi").expect("a stream that never wrote");
        assert!(said.contains("written nothing"), "{said}");
        assert!(said.contains("6 error"), "{said}");
    }

    #[test]
    fn errors_beside_writes_are_the_weather_and_say_nothing() {
        assert_eq!(failing(RUNNING_STREAM, "aq"), None);
        // A runner that exports no error counter for a stream says nothing about it either.
        assert_eq!(failing(RUNNING_STREAM, "nothing-of-that-name"), None);
    }

    use base64::Engine as _;
    use serde_json::json;
    use wiremock::matchers::path as path_matcher;
    use wiremock::matchers::{method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// T-0935: the SOPS backend decrypts a Pipeline's `secretRef` from the repository's own
    /// `*.enc.yaml`, and it reads the tree this function stages. While the stager dropped every
    /// file that held no manifest, the resolver answered "not declared in any encrypted secrets
    /// file" for a secret that sat in the repository — with the backend configured, the
    /// identity mounted and the file committed.
    #[tokio::test]
    async fn an_encrypted_secrets_file_is_staged_beside_the_manifests() {
        const ENCRYPTED: &str = "demo-feed:\n    password: ENC[AES256_GCM,data:aaaa,iv:bbbb,tag:cccc,type:str]\nsops:\n    age: []\n";
        let space = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: helsinki\nspec:\n  isSandbox: true\n";
        let files = [
            ("projects/helsinki/spaces/mobility/space.yaml", space),
            ("projects/helsinki/secrets/demo-feed.enc.yaml", ENCRYPTED),
        ];

        let server = MockServer::start().await;
        let tree: Vec<serde_json::Value> = files
            .iter()
            .map(|(p, _)| json!({"path": p, "type": "blob"}))
            .collect();
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/git/trees/rev-1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"truncated": false, "tree": tree})),
            )
            .mount(&server)
            .await;
        for (file, content) in files {
            Mock::given(method("GET"))
                .and(path(format!(
                    "/api/v1/repos/test-owner/test-repo/contents/{file}"
                )))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "sha": "blob-sha",
                    "content": base64::engine::general_purpose::STANDARD.encode(content),
                })))
                .mount(&server)
                .await;
        }
        let gitea = GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token",
        )
        .expect("client");

        let scratch = stage(&gitea, "rev-1").await.expect("the run stages");
        let staged = scratch
            .path()
            .join("projects/helsinki/secrets/demo-feed.enc.yaml");
        assert!(staged.is_file(), "the encrypted file is staged");
        // Byte for byte: a SOPS file whose bytes changed no longer authenticates.
        assert_eq!(
            std::fs::read_to_string(&staged).expect("read the staged file"),
            ENCRYPTED
        );
        assert!(
            scratch
                .path()
                .join("projects/helsinki/spaces/mobility/space.yaml")
                .is_file(),
            "the manifest beside it is staged as before"
        );
    }

    #[test]
    fn only_the_encrypted_names_are_kept_for_the_secret_backend() {
        assert!(is_encrypted_secrets_file("projects/hel/secrets/a.enc.yaml"));
        assert!(is_encrypted_secrets_file("a.enc.yml"));
        // A manifest whose name merely mentions the word is a manifest.
        assert!(!is_encrypted_secrets_file("projects/hel/secrets.yaml"));
        assert!(!is_encrypted_secrets_file("projects/hel/enc.yaml"));
    }

    #[test]
    fn status_defaults() {
        let status = SyncStatus::default();
        assert_eq!(status.last_sync, None);
        assert_eq!(status.revision, None);
        assert_eq!(status.manifests, 0);
        assert_eq!(status.last_error, None);
    }

    #[tokio::test]
    async fn zero_duration_does_not_loop() {
        let gitea = Arc::new(
            GiteaClient::new(
                "http://localhost:3000".parse().unwrap(),
                "test-owner",
                "test-repo",
                "token",
            )
            .unwrap(),
        );
        let mirror = Arc::new(Mirror::new());
        let syncer = Arc::new(Syncer::new(gitea, mirror));
        let handle = syncer.spawn_periodic(Duration::ZERO);
        let res = tokio::time::timeout(Duration::from_millis(100), handle).await;
        assert!(
            res.is_ok(),
            "handle should finish immediately on zero duration"
        );
    }

    #[tokio::test]
    async fn a_sync_asked_for_during_a_sync_runs_after_it() {
        let gitea = Arc::new(
            GiteaClient::new(
                "http://127.0.0.1:9".parse().unwrap(),
                "test-owner",
                "test-repo",
                "token",
            )
            .unwrap(),
        );
        let mirror = Arc::new(Mirror::new());
        let syncer = Arc::new(Syncer::new(gitea, mirror));

        let guard = syncer.running.lock().await;
        let waiting = tokio::spawn({
            let syncer = syncer.clone();
            async move { syncer.sync_once().await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !waiting.is_finished(),
            "the second run waits for the first instead of answering Ok(0)"
        );
        drop(guard);
        let res = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the waiting run proceeds once the first releases the guard")
            .expect("the task joins");
        assert!(
            res.is_err(),
            "the waiting run reached the forge (unreachable here) instead of skipping"
        );
    }

    #[test]
    fn candidate_manifest_path_filtering() {
        assert!(is_candidate_manifest("org.yaml"));
        assert!(is_candidate_manifest("bundle.yaml"));
        assert!(is_candidate_manifest(
            "projects/ovzdusie/spaces/mobility/space.yaml"
        ));
        assert!(is_candidate_manifest(
            "/projects/ovzdusie/spaces/mobility/space.yaml"
        ));
        assert!(is_candidate_manifest("projects/ovzdusie/endpoints/air.yml"));

        // A Blueprint is a manifest kind of its own, so the gallery finds it in the mirror.
        assert!(is_candidate_manifest(
            "blueprints/threshold-alert/blueprint.yaml"
        ));

        assert!(!is_candidate_manifest("README.md"));
        assert!(!is_candidate_manifest("portal/theme.yaml"));
        assert!(!is_candidate_manifest("users/groups.yaml"));
        assert!(is_candidate_manifest("users/roles/steward.yaml"));
        assert!(is_candidate_manifest("users/assignments/stewards.yaml"));
        assert!(is_candidate_manifest("agentprofiles/app-builder.yaml"));
        assert!(!is_candidate_manifest("agentprofiles/README.md"));
        assert!(!is_candidate_manifest("platform-settings.yaml"));
        // Only the blueprint manifest itself, not the notes or fixtures beside it.
        assert!(!is_candidate_manifest(
            "blueprints/threshold-alert/README.md"
        ));
        assert!(!is_candidate_manifest(
            "blueprints/threshold-alert/example.yaml"
        ));
        assert!(
            !is_candidate_manifest("projects/../../etc/passwd.yaml"),
            "the tree comes from the forge: nothing may climb out of the staging directory"
        );
        assert!(!is_candidate_manifest(
            "projects/ovzdusie/spaces/mobility/space.json"
        ));
    }

    #[tokio::test]
    async fn a_run_that_lands_a_new_revision_says_so_once() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());
        let activity = crate::activity::ActivityStore::new(None);
        let syncer = Arc::new(
            Syncer::new(Arc::clone(&client), Arc::clone(&mirror)).with_activity(activity.clone()),
        );

        mount_repository(&server).await;

        syncer.sync_once().await.expect("sync should succeed");
        let filter = crate::activity::ActivityFilter {
            limit: 50,
            ..Default::default()
        };
        let page = activity.list("ovzdusie", &filter).await.expect("list");
        assert_eq!(page.items.len(), 1, "{:?}", page.items);
        assert_eq!(page.items[0].kind, "config.applied");
        assert_eq!(page.items[0].source, "reconciler");
        assert!(
            page.items[0].summary.contains("commit"),
            "{}",
            page.items[0].summary
        );

        // The loop runs every tick; the same revision is not news twice.
        syncer
            .sync_once()
            .await
            .expect("second sync should succeed");
        let page = activity.list("ovzdusie", &filter).await.expect("list");
        assert_eq!(page.items.len(), 1, "{:?}", page.items);
    }

    #[tokio::test]
    async fn a_repository_that_will_not_load_reaches_the_feed() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());
        let activity = crate::activity::ActivityStore::new(None);
        let syncer = Arc::new(
            Syncer::new(Arc::clone(&client), Arc::clone(&mirror)).with_activity(activity.clone()),
        );

        mount_repository(&server).await;
        syncer.sync_once().await.expect("the first sync loads");

        // The forge goes away: the mirror keeps what it has and the feed says why.
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        syncer.sync_once().await.expect_err("the second sync fails");

        let page = activity
            .list(
                "ovzdusie",
                &crate::activity::ActivityFilter {
                    kinds: vec!["config.drifted".to_string()],
                    limit: 50,
                    ..Default::default()
                },
            )
            .await
            .expect("list");
        assert_eq!(page.items.len(), 1, "{:?}", page.items);
        assert_eq!(page.items[0].severity, "error");
    }

    /// The one-space repository both the success test and the activity tests read.
    async fn mount_repository(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "default_branch": "main"
            })))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "main",
                "commit": { "id": "commit-rev-123" }
            })))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/git/trees/commit-rev-123",
            ))
            .and(query_param("recursive", "true"))
            .and(query_param("per_page", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "tree-sha-1",
                "truncated": false,
                "tree": [
                    {
                        "path": "projects/ovzdusie/spaces/mobility/space.yaml",
                        "type": "blob"
                    }
                ]
            })))
            .mount(server)
            .await;

        let manifest_content = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
        let b64 = base64::engine::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            manifest_content.as_bytes(),
        );

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "commit-rev-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-sha-1",
                "content": b64
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn sync_once_success() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());
        let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

        mount_repository(&server).await;

        let count = syncer.sync_once().await.expect("sync should succeed");
        assert_eq!(count, 1);

        let status = syncer.status();
        assert_eq!(status.manifests, 1);
        assert_eq!(status.revision.as_deref(), Some("commit-rev-123"));
        assert!(status.last_sync.is_some());
        assert_eq!(status.last_error, None);

        let env = mirror
            .get("ovzdusie", "ContextSpace", "mobility")
            .expect("in mirror");
        assert_eq!(
            env.status.as_ref().map(|s| s.phase),
            Some(crate::resource::Phase::Live)
        );
        assert_eq!(
            env.status
                .as_ref()
                .and_then(|s| s.observed_revision.as_deref()),
            Some("commit-rev-123")
        );
    }

    fn b64(text: &str) -> String {
        base64::engine::Engine::encode(&base64::engine::general_purpose::STANDARD, text.as_bytes())
    }

    async fn mount_file(server: &MockServer, path: &str, git_ref: &str, sha: &str, text: &str) {
        Mock::given(method("GET"))
            .and(path_matcher(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{path}"
            )))
            .and(query_param("ref", git_ref))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": sha,
                "content": b64(text)
            })))
            .mount(server)
            .await;
    }

    const ORG_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\nspec:\n  domain: hel.fi\n  locales: [en]\n  defaultLocale: en\n";
    const ROLE_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Role\nmetadata:\n  name: steward\n  namespace: org\nspec:\n  rules:\n    - kinds: [\"*\"]\n      verbs: [propose, approve, delete]\n";
    const BINDING_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: RoleBinding\nmetadata:\n  name: stewards\n  namespace: org\nspec:\n  subjects:\n    - group: stewards\n  role: steward\n  scope:\n    organization: hel\n";

    /// T-0527: `users/` is compiled into the five managed files, and only the ones whose
    /// content differs from the branch are written.
    #[tokio::test]
    async fn bindings_are_compiled_into_the_forge() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());
        let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

        Mock::given(method("GET"))
            .and(path_matcher("/api/v1/repos/test-owner/test-repo"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_matcher(
                "/api/v1/repos/test-owner/test-repo/branches/main",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "main", "commit": { "id": "rev-1" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_matcher(
                "/api/v1/repos/test-owner/test-repo/git/trees/rev-1",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "tree-1",
                "truncated": false,
                "tree": [
                    { "path": "org.yaml", "type": "blob" },
                    { "path": "users/roles/steward.yaml", "type": "blob" },
                    { "path": "users/assignments/stewards.yaml", "type": "blob" }
                ]
            })))
            .mount(&server)
            .await;
        mount_file(&server, "org.yaml", "rev-1", "b-org", ORG_YAML).await;
        mount_file(
            &server,
            "users/roles/steward.yaml",
            "rev-1",
            "b-role",
            ROLE_YAML,
        )
        .await;
        mount_file(
            &server,
            "users/assignments/stewards.yaml",
            "rev-1",
            "b-binding",
            BINDING_YAML,
        )
        .await;

        // The branch already holds the gate as jcctl renders it: not written again.
        let repo_dir = std::env::temp_dir().join(format!("jc-portal-roles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo_dir);
        for (rel, text) in [
            ("org.yaml", ORG_YAML),
            ("users/roles/steward.yaml", ROLE_YAML),
            ("users/assignments/stewards.yaml", BINDING_YAML),
        ] {
            let full = repo_dir.join(rel);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        }
        let expected = jcctl::roles::files(&jcctl::loader::Repository::load(&repo_dir).unwrap())
            .unwrap()
            .expect("a repository with users/ compiles");
        let rego = expected
            .iter()
            .find(|(p, _)| *p == jcctl::roles::ROLES_REGO)
            .map(|(_, c)| c.clone())
            .unwrap();
        let _ = std::fs::remove_dir_all(&repo_dir);
        mount_file(&server, jcctl::roles::ROLES_REGO, "main", "b-rego", &rego).await;
        // A stale CODEOWNERS is replaced with its sha; the others do not exist yet.
        mount_file(&server, "CODEOWNERS", "main", "b-old", "* @nobody\n").await;
        Mock::given(method("PUT"))
            .and(path_regex("^/api/v1/repos/test-owner/test-repo/contents/"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "commit": { "sha": "rev-2" }
            })))
            .expect(4)
            .mount(&server)
            .await;

        syncer.sync_once().await.expect("sync should succeed");

        let puts: Vec<_> = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method == "PUT")
            .collect();
        let codeowners = puts
            .iter()
            .find(|r| r.url.path().ends_with("/contents/CODEOWNERS"))
            .expect("CODEOWNERS is written");
        let body: serde_json::Value = serde_json::from_slice(&codeowners.body).unwrap();
        assert_eq!(body["sha"], "b-old");
        assert_eq!(body["branch"], "main");
        let text = String::from_utf8(
            base64::engine::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                body["content"].as_str().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(text.contains("@hel/stewards"), "{text}");
        assert!(!puts
            .iter()
            .any(|r| r.url.path().ends_with("/contents/policies/roles.rego")));
    }

    #[tokio::test]
    async fn a_repository_of_nothing_loadable_leaves_the_mirror_alone() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());

        // Pre-seed mirror to prove it is NOT emptied on failure
        mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: crate::resource::ObjectMeta {
                name: "existing".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({}),
            status: None,
        });

        let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "default_branch": "main"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "main",
                "commit": { "id": "bad-rev" }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/git/trees/bad-rev"))
            .and(query_param("recursive", "true"))
            .and(query_param("per_page", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "tree-sha-2",
                "truncated": false,
                "tree": [
                    {
                        "path": "projects/ovzdusie/spaces/invalid/space.yaml",
                        "type": "blob"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let bad_content = "::: not yaml at all :::";
        let b64 = base64::engine::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            bad_content.as_bytes(),
        );

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/invalid/space.yaml",
            ))
            .and(query_param("ref", "bad-rev"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-sha-2",
                "content": b64
            })))
            .mount(&server)
            .await;

        let err = syncer.sync_once().await.unwrap_err();
        match err {
            SyncError::Empty(message) => {
                assert!(
                    message.contains("none of the 1 candidate files"),
                    "{message}"
                );
            }
            other => panic!("expected an empty-repository error, got {other:?}"),
        }

        // Mirror still holds the pre-seeded resource: a repository that does not load leaves
        // the last one that did in place.
        assert_eq!(mirror.len(), 1);
        assert!(mirror.get("ovzdusie", "ContextSpace", "existing").is_some());

        // Status recorded the error
        let status = syncer.status();
        assert!(status
            .last_error
            .unwrap()
            .contains("none of the 1 candidate files"));
    }
}
