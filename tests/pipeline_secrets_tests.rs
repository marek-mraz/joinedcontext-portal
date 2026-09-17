//! A pipeline's `secretRef`s reach the runner's environment, or the pipeline does not run
//! (T-0927, PL-15…PL-17).
//!
//! The whole path, with the three things it talks to mocked: the forge that serves the
//! repository, the secret backend that holds the value, and the API server the Secret is written
//! to. OpenBao is the backend here because it speaks HTTP and a test can answer it; the SOPS
//! backend is the same resolution over a decrypted file and is covered where the store is
//! (`jcctl`'s `sops_tests`).

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::pipeline_secrets::{Backend, Resolver};
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";
const PASSWORD: &str = "the-mqtt-password-nobody-else-has";

const ORGANIZATION: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\n  namespace: org\nspec:\n  domain: hel.fi\n  locales: [en]\n  defaultLocale: en\n";

const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: helsinki\nspec:\n  isSandbox: true\n";

/// A DataSource whose password is an interpolation naming its own `spec.secrets` (PL-50).
const DATA_SOURCE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: DataSource
metadata:
  name: mesto-mqtt
  namespace: helsinki
spec:
  contextSpaceRef: { kind: ContextSpace, name: mobility }
  class: resident
  type: mqtt
  input:
    urls: ["tcp://broker.example.fi:1883"]
    topics: ["vehicles/#"]
    password: "${MQTT_PASSWORD}"
  secrets:
    - { name: mqtt-mesto, key: password, envVar: MQTT_PASSWORD }
"#;

const PIPELINE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: vehicles
  namespace: helsinki
spec:
  class: resident
  enabled: true
  targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:mobility:vehicles-in"
  quotas: { maxMemoryMb: 128, cpuMillicores: 250 }
  source:
    dataSourceRef: { kind: DataSource, name: mesto-mqtt }
  compute:
    kind: bloblang
    mapping: "root = this"
"#;

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
    let tree: Vec<Value> = files
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
    for (file, content) in files {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file}"
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

fn repository() -> Vec<(&'static str, &'static str)> {
    vec![
        ("org.yaml", ORGANIZATION),
        ("projects/helsinki/spaces/mobility/space.yaml", SPACE),
        ("projects/helsinki/datasources/mesto-mqtt.yaml", DATA_SOURCE),
        (
            "projects/helsinki/pipelines/vehicles/pipeline.yaml",
            PIPELINE,
        ),
    ]
}

fn client(server: &MockServer) -> Arc<GiteaClient> {
    Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("a forge client"),
    )
}

/// An OpenBao that answers the login and one KV v2 read.
async fn openbao() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/kubernetes/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": { "client_token": "a-session-token", "lease_duration": 3600, "renewable": true }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/mqtt-mesto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "data": { "password": PASSWORD }, "metadata": { "version": 1 } }
        })))
        .mount(&server)
        .await;
    server
}

/// An API server that accepts every apply and remembers what it was sent.
async fn cluster() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"kind": "Secret"})))
        .mount(&server)
        .await;
    server
}

fn jwt_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("jc-portal-{name}-token"));
    std::fs::write(&path, "a.service.account.token").expect("write the token");
    path
}

async fn applied(cluster: &MockServer, ends_with: &str) -> Option<Value> {
    cluster
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|request| request.url.path().ends_with(ends_with))
        .map(|request| serde_json::from_slice(&request.body).expect("a json body"))
}

#[tokio::test]
async fn the_runner_gets_the_value_and_the_pipeline_is_not_refused() {
    let forge = forge(&repository()).await;
    let bao = openbao().await;
    let cluster = cluster().await;

    let syncer = Syncer::new(client(&forge), Arc::new(Mirror::new()))
        .with_pipeline_secrets(Resolver::new(Backend::OpenBao {
            address: bao.uri(),
            role: "portal".to_owned(),
            jwt_path: jwt_file("runner-gets-the-value"),
        }))
        .with_credential_secrets(
            Arc::new(KubeClient::with_token(&cluster.uri(), "token").expect("a kube client")),
            "dev",
        );
    syncer.sync_once().await.expect("the run loads");

    let secret = applied(&cluster, "/secrets/pipeline-secrets")
        .await
        .expect("the runner's Secret was written");
    assert_eq!(secret["stringData"]["MQTT_PASSWORD"], PASSWORD);
    assert_eq!(secret["kind"], "Secret");

    // The value is read once, when the pod starts, so the runner is rolled when it changes.
    let rollout = applied(&cluster, "/deployments/pipeline-runner")
        .await
        .expect("the runner was rolled");
    let annotation = rollout["spec"]["template"]["metadata"]["annotations"]
        ["joinedcontext.com/pipeline-secrets"]
        .as_str()
        .expect("the fingerprint is stamped");
    assert_eq!(annotation.len(), 64, "a sha-256 of the values");
    assert!(!annotation.contains(PASSWORD));
}

#[tokio::test]
async fn without_a_backend_the_pipeline_is_refused_and_nothing_is_written() {
    let forge = forge(&repository()).await;
    let cluster = cluster().await;
    let mirror = Arc::new(Mirror::new());

    let syncer = Syncer::new(client(&forge), Arc::clone(&mirror)).with_credential_secrets(
        Arc::new(KubeClient::with_token(&cluster.uri(), "token").expect("a kube client")),
        "dev",
    );
    syncer.sync_once().await.expect("the run loads");

    assert!(
        applied(&cluster, "/secrets/pipeline-secrets")
            .await
            .is_none(),
        "a Portal with no backend wrote a Secret"
    );
    let pipeline = mirror
        .get("helsinki", "Pipeline", "vehicles")
        .expect("the pipeline is in the mirror");
    let condition = serde_json::to_string(&pipeline.status).expect("a status");
    assert!(
        condition.contains("mqtt-mesto"),
        "the refusal names the secret: {condition}"
    );
    assert!(
        !condition.contains(PASSWORD),
        "a value reached the status: {condition}"
    );
}
