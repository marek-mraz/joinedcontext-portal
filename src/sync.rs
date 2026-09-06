//! Background mirror synchronization against the Git repository (MF-04, CC-08).
//!
//! Re-reads declared manifests from Gitea at the default branch HEAD, compiles
//! their live status, and swaps them into the in-memory mirror atomically.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use utoipa::ToSchema;

use crate::git::{GitError, GiteaClient};
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
}

/// Errors returned during repository synchronization.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("{0}")]
    Empty(String),
}

/// Background reconciler synchronizing repository manifests into the in-memory mirror.
pub struct Syncer {
    gitea: Arc<GiteaClient>,
    mirror: Arc<Mirror>,
    status: Arc<RwLock<SyncStatus>>,
    running: Arc<Mutex<()>>,
}

impl Syncer {
    pub fn new(gitea: Arc<GiteaClient>, mirror: Arc<Mirror>) -> Self {
        Self {
            gitea,
            mirror,
            status: Arc::new(RwLock::new(SyncStatus::default())),
            running: Arc::new(Mutex::new(())),
        }
    }

    pub fn status(&self) -> SyncStatus {
        self.status
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Synchronizes the mirror once against the Git repository.
    ///
    /// Guarded against concurrent runs: if another sync is currently active,
    /// returns `Ok(0)` immediately instead of queueing.
    pub async fn sync_once(&self) -> Result<usize, SyncError> {
        let _guard = match self.running.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::info!("mirror sync is already in flight, skipping concurrent execution");
                return Ok(0);
            }
        };

        match self.do_sync().await {
            Ok((count, revision)) => {
                let now = crate::auth::session::now_unix();
                let mut status = self.status.write().unwrap_or_else(|p| p.into_inner());
                status.last_sync = Some(now);
                status.revision = Some(revision);
                status.manifests = count;
                status.last_error = None;
                Ok(count)
            }
            Err(err) => {
                let mut status = self.status.write().unwrap_or_else(|p| p.into_inner());
                status.last_error = Some(err.to_string());
                Err(err)
            }
        }
    }

    async fn do_sync(&self) -> Result<(usize, String), SyncError> {
        // 1. Resolve default branch and its commit revision
        let default_branch = self.gitea.default_branch().await?;
        let revision = self.gitea.branch_head(&default_branch).await?;

        // 2. List all files in the Git tree and filter candidate manifests
        let tree_paths = self.gitea.list_tree(&revision).await?;
        let candidate_paths: Vec<String> = tree_paths
            .into_iter()
            .filter(|p| is_candidate_manifest(p))
            .collect();

        // 3. Read and parse each candidate manifest
        let mut skipped = 0usize;
        let mut loaded = 0usize;
        let fresh_mirror = Mirror::new();

        for path in &candidate_paths {
            let file = match self.gitea.get_file(path, &revision).await {
                Ok(Some(f)) => f,
                Ok(None) => {
                    tracing::warn!(path = %path, "candidate manifest listed in git tree not found");
                    skipped += 1;
                    continue;
                }
                Err(err) => return Err(SyncError::Git(err)),
            };

            let mut envelope: ResourceEnvelope = match serde_yaml_ng::from_str(&file.content) {
                Ok(env) => env,
                Err(err) => {
                    tracing::warn!(path = %path, error = %err, "failed to parse manifest as ResourceEnvelope");
                    skipped += 1;
                    continue;
                }
            };

            if crate::resource::by_kind(&envelope.kind).is_none() {
                tracing::warn!(path = %path, kind = %envelope.kind, "skipping manifest with unknown kind");
                skipped += 1;
                continue;
            }

            // Fill in namespace if missing from manifest
            if envelope
                .metadata
                .namespace
                .as_deref()
                .unwrap_or("")
                .is_empty()
            {
                if path == "org.yaml" || path == "bundle.yaml" {
                    envelope.metadata.namespace = Some("org".to_string());
                } else if let Some(proj) = path.split('/').nth(1) {
                    envelope.metadata.namespace = Some(proj.to_string());
                }
            }

            // 4. Compute status on server (MF-04)
            envelope.strip_status();
            envelope.status = Some(crate::resource::Status {
                phase: crate::resource::Phase::Live,
                observed_revision: Some(revision.clone()),
                conditions: Vec::new(),
            });

            fresh_mirror.upsert(envelope);
            loaded += 1;
        }

        if !candidate_paths.is_empty() && loaded == 0 {
            let msg = format!("every manifest failed to parse ({skipped} skipped)");
            return Err(SyncError::Empty(msg));
        }

        if candidate_paths.is_empty() {
            return Err(SyncError::Empty(
                "no manifests found in repository".to_string(),
            ));
        }

        // 5. Swap contents into shared mirror only after all files are processed
        self.mirror.replace_all(&fresh_mirror);

        Ok((loaded, revision))
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
pub(crate) fn is_candidate_manifest(path: &str) -> bool {
    let clean = path.trim_start_matches('/');
    if clean == "org.yaml" || clean == "bundle.yaml" {
        return true;
    }
    clean.starts_with("projects/") && (clean.ends_with(".yaml") || clean.ends_with(".yml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
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

        assert!(!is_candidate_manifest("README.md"));
        assert!(!is_candidate_manifest("blueprints/air/blueprint.yaml"));
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

    #[tokio::test]
    async fn sync_once_all_files_fail_returns_empty_error() {
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
            SyncError::Empty(msg) => {
                assert!(msg.contains("every manifest failed to parse (1 skipped)"));
            }
            other => panic!("expected SyncError::Empty, got {other:?}"),
        }

        // Mirror still holds the pre-seeded resource
        assert_eq!(mirror.len(), 1);
        assert!(mirror.get("ovzdusie", "ContextSpace", "existing").is_some());

        // Status recorded the error
        let status = syncer.status();
        assert!(status
            .last_error
            .unwrap()
            .contains("every manifest failed to parse (1 skipped)"));
    }
}
