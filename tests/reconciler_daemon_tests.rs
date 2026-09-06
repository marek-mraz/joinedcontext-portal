//! The embedded reconciler: the election, the loop, and what reaches the mirror
//! (T-0191, CC-03, CC-08, CC-55, MF-04, MF-05, MF-06).
//!
//! The election tests run when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test
//! may write to (locally: `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); otherwise
//! they skip with a note, like the other database tests, so the fast lane without a service
//! container stays green. Everything that does not need an election runs everywhere.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::reconciler::{Leadership, SyncError, Syncer};
use joinedcontext_portal::store::Mirror;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";

/// A forge serving one revision of a repository, as the daemon reads it.
async fn forge(files: &[(&str, &str)]) -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"default_branch": "main"})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"name": "main", "commit": {"id": REVISION}})),
        )
        .mount(&server)
        .await;

    let tree: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, _)| json!({"path": p, "type": "blob"}))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/git/trees/{REVISION}"
        )))
        .and(query_param("recursive", "true"))
        .and(query_param("per_page", "1000"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sha": "tree-sha", "truncated": false, "tree": tree})),
        )
        .mount(&server)
        .await;

    for (file_path, content) in files {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file_path}"
            )))
            .and(query_param("ref", REVISION))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-sha",
                "content": STANDARD.encode(content.as_bytes()),
            })))
            .mount(&server)
            .await;
    }

    server
}

fn client(server: &MockServer) -> Arc<GiteaClient> {
    Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("gitea client"),
    )
}

fn space(name: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: {name}\n  namespace: ovzdusie\nspec:\n  isSandbox: false\n"
    )
}

fn database_url() -> Option<String> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(url)
}

/// A lock key of this test alone, so tests sharing one database never elect each other.
fn private_key(tag: u8) -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    ((nanos as i64) & 0x0000_ffff_ffff_ff00) | i64::from(tag)
}

#[tokio::test]
async fn the_advisory_lock_admits_one_replica_at_a_time() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect and migrate");
    let key = private_key(1);

    let first = Leadership::new(pool.clone(), key);
    let second = Leadership::new(pool.clone(), key);

    assert!(first.acquire().await.expect("first election"));
    assert!(first.is_leader());
    assert!(
        !second.acquire().await.expect("second election"),
        "a second replica must not win a lock another one holds"
    );
    assert!(!second.is_leader());

    // Asking again does not lose what is already held.
    assert!(first.acquire().await.expect("re-election"));

    first.resign().await;
    assert!(!first.is_leader());
    assert!(
        second.acquire().await.expect("election after resignation"),
        "the lock must be free the moment the leader gives it up"
    );
    second.resign().await;
}

#[tokio::test]
async fn a_replica_that_lost_the_election_leaves_the_mirror_alone() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect and migrate");
    let key = private_key(2);

    let server = forge(&[(
        "projects/ovzdusie/spaces/mobility/space.yaml",
        &space("mobility"),
    )])
    .await;
    let mirror = Arc::new(Mirror::new());

    let leader = Syncer::new(client(&server), Arc::clone(&mirror))
        .with_leadership(Arc::new(Leadership::new(pool.clone(), key)));
    let follower_lock = Arc::new(Leadership::new(pool.clone(), key));
    let follower = Syncer::new(client(&server), Arc::clone(&mirror))
        .with_leadership(Arc::clone(&follower_lock));

    assert_eq!(leader.sync_once().await.expect("the leader reconciles"), 1);
    assert_eq!(mirror.len(), 1);

    assert_eq!(
        follower
            .sync_once()
            .await
            .expect("a follower is not an error"),
        0,
        "a replica that does not hold the lock must not reconcile"
    );
    assert!(!follower.is_leader());
    assert!(!follower.status().leader);
    assert!(
        follower.status().revision.is_none(),
        "a follower reports no revision of its own; it serves the leader's mirror"
    );
    assert_eq!(mirror.len(), 1, "the mirror is still the leader's");

    follower_lock.resign().await;
}

#[tokio::test]
async fn the_leader_reconciles_the_repository_and_the_status_says_so() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect and migrate");
    let leadership = Arc::new(Leadership::new(pool, private_key(3)));

    let server = forge(&[
        (
            "projects/ovzdusie/spaces/mobility/space.yaml",
            &space("mobility"),
        ),
        ("projects/ovzdusie/spaces/air/space.yaml", &space("air")),
    ])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer =
        Syncer::new(client(&server), Arc::clone(&mirror)).with_leadership(Arc::clone(&leadership));

    assert_eq!(syncer.sync_once().await.expect("reconcile"), 2);

    let status = syncer.status();
    assert!(status.leader);
    assert_eq!(status.manifests, 2);
    assert_eq!(status.revision.as_deref(), Some(REVISION));
    assert!(status.last_sync.is_some());
    assert_eq!(status.last_error, None);

    let live = mirror
        .get("ovzdusie", "ContextSpace", "mobility")
        .expect("the space is mirrored");
    assert_eq!(
        live.status
            .as_ref()
            .and_then(|s| s.observed_revision.as_deref()),
        Some(REVISION)
    );

    leadership.resign().await;
}

#[tokio::test]
async fn without_a_database_the_only_replica_leads() {
    let server = forge(&[(
        "projects/ovzdusie/spaces/mobility/space.yaml",
        &space("mobility"),
    )])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    assert!(
        syncer.is_leader(),
        "a Portal with no database has no election to run and reconciles alone"
    );
    assert_eq!(syncer.sync_once().await.expect("reconcile"), 1);
    assert!(syncer.status().leader);
}

#[tokio::test]
async fn a_file_of_several_documents_contributes_every_one_of_them() {
    let both = format!("{}---\n{}", space("mobility"), space("air"));
    let server = forge(&[("projects/ovzdusie/spaces/spaces.yaml", &both)]).await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    assert_eq!(
        syncer.sync_once().await.expect("reconcile"),
        2,
        "the loader reads multi-document YAML, so one file may declare several resources"
    );
    assert!(mirror.get("ovzdusie", "ContextSpace", "mobility").is_some());
    assert!(mirror.get("ovzdusie", "ContextSpace", "air").is_some());
}

#[tokio::test]
async fn two_files_claiming_one_identity_stop_the_run_and_keep_the_mirror() {
    let server = forge(&[
        (
            "projects/ovzdusie/spaces/mobility/space.yaml",
            &space("mobility"),
        ),
        (
            "projects/ovzdusie/spaces/copy/space.yaml",
            &space("mobility"),
        ),
    ])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    let err = syncer
        .sync_once()
        .await
        .expect_err("two manifests of one identity are not a repository");
    match err {
        SyncError::Load(jcctl::loader::LoadError::DuplicateIdentity { id, .. }) => {
            assert_eq!(id.name, "mobility");
        }
        other => panic!("expected a duplicate identity, got {other:?}"),
    }
    assert!(
        mirror.is_empty(),
        "a repository that does not load leaves the mirror as it was"
    );
    assert!(syncer.status().last_error.is_some());
}

#[tokio::test]
async fn a_blueprint_reaches_the_mirror_for_the_flow_gallery() {
    let blueprint = "apiVersion: joinedcontext.com/v1alpha1\nkind: Blueprint\nmetadata:\n  name: threshold-alert\n  namespace: org\nspec:\n  version: 1.2.0\n  riskClass: green\n  allowedRoles: [domain-editor]\n  parameterSchema:\n    type: object\n  templates:\n    - name: subscription\n      template: |\n        apiVersion: joinedcontext.com/v1alpha1\n";
    let server = forge(&[
        ("blueprints/threshold-alert/blueprint.yaml", blueprint),
        (
            "projects/ovzdusie/spaces/mobility/space.yaml",
            &space("mobility"),
        ),
    ])
    .await;
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(client(&server), Arc::clone(&mirror));

    assert_eq!(syncer.sync_once().await.expect("reconcile"), 2);
    assert!(
        mirror.get("org", "Blueprint", "threshold-alert").is_some(),
        "the gallery reads blueprints from the mirror like any other resource"
    );
}
