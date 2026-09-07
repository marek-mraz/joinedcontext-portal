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
//! Only the elected leader reconciles ([`super::leader`]); the other replicas serve the UI
//! from the mirror the leader fills and answer `sync_once` with `Ok(0)`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use utoipa::ToSchema;

use super::leader::Leadership;
use crate::apps::converge::{Converger, Outcome};
use crate::git::{Author, FileWrite, GitError, GiteaClient};
use crate::resource::ResourceEnvelope;
use crate::store::Mirror;

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
        }
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
    /// Answers `Ok(0)` without touching the repository in the two cases where running would
    /// be wrong rather than merely redundant: another run of this replica is still in
    /// flight, or this replica is not the leader (CC-03). Both are ordinary states, so
    /// neither is an error; the webhook that triggered it is already recorded in Git and the
    /// leader picks it up on its next tick.
    pub async fn sync_once(&self) -> Result<usize, SyncError> {
        let _guard = match self.running.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::info!("mirror sync is already in flight, skipping concurrent execution");
                return Ok(0);
            }
        };

        if !self.claim_leadership().await {
            tracing::debug!("another replica holds the reconciler lock, skipping this run");
            return Ok(0);
        }

        match self.do_sync().await {
            Ok((count, revision)) => {
                let now = crate::auth::session::now_unix();
                let mut status = self.status.write().unwrap_or_else(|p| p.into_inner());
                status.last_sync = Some(now);
                status.revision = Some(revision);
                status.manifests = count;
                status.last_error = None;
                status.leader = true;
                Ok(count)
            }
            Err(err) => {
                let mut status = self.status.write().unwrap_or_else(|p| p.into_inner());
                status.last_error = Some(err.to_string());
                Err(err)
            }
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

    async fn do_sync(&self) -> Result<(usize, String), SyncError> {
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

            envelope.strip_status();
            envelope.status = Some(crate::resource::Status {
                phase: crate::resource::Phase::Live,
                observed_revision: Some(revision.clone()),
                // The branch, not the revision: a Source link should keep working after the
                // next commit, and the observed revision is right there beside it.
                source_url: Some(self.gitea.browse_url(&path, &default_branch)),
                conditions: Vec::new(),
            });

            fresh_mirror.upsert(envelope);
            loaded += 1;
        }

        if loaded == 0 {
            return Err(SyncError::Empty(
                "no resource of a known kind in the staged manifests".to_string(),
            ));
        }

        self.mirror.replace_all(&fresh_mirror);

        // 6. Converge what an App compiles into (T-0411, AP-18, AP-21). The mirror is already
        //    swapped, so a cluster that refuses one object leaves the Portal serving the
        //    repository correctly and says why in the log; one app's failure is not the run's.
        if let Some(converger) = self.converger.as_ref() {
            for (app, outcome) in converger.converge(&repository).await {
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

        // 7. Compile `users/` into what the forge enforces (T-0527, PF-51, PF-52, CC-41): the
        //    CODEOWNERS, the bindings as data, and the gate that reads them. Written only when
        //    the repository differs, so the commit this makes is seen once by the next run and
        //    changes nothing. A forge that refuses the write costs the run nothing but a line
        //    in the log; the Portal's own check (PF-50) holds either way.
        if let Err(err) = self.publish_roles(&repository, &default_branch).await {
            tracing::warn!(error = %err, "roles were not compiled into the repository");
        }

        Ok((loaded, revision))
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
            loop {
                ticker.tick().await;
                if let Err(err) = self.sync_once().await {
                    tracing::warn!(error = %err, "periodic mirror sync failed");
                }
            }
        })
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
    clean.starts_with("projects/")
        || clean.starts_with("users/roles/")
        || clean.starts_with("users/assignments/")
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::path as path_matcher;
    use wiremock::matchers::{method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
    async fn concurrency_guard_returns_ok_zero() {
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

        let _guard = syncer.running.lock().await;
        let res = syncer
            .sync_once()
            .await
            .expect("sync_once should return Ok(0)");
        assert_eq!(res, 0);
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
    async fn sync_once_success() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());
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
                "commit": { "id": "commit-rev-123" }
            })))
            .mount(&server)
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
            .mount(&server)
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
            .mount(&server)
            .await;

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
