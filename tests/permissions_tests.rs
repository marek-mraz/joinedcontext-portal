//! T-0526: roles as code, enforced (PF-50). Who may propose, delete and approve comes from the
//! `Role` and `RoleBinding` manifests in the mirror, never from the token.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use jc_core::kinds::Verb;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::{effective, ORG_NAMESPACE};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "test-csrf-token-12345";

fn identity(email: &str, groups: &[&str]) -> Identity {
    Identity {
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: groups.iter().map(|g| (*g).to_owned()).collect(),
    }
}

fn cookies(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let s = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// The docs example: a developer proposes pipelines and non-public endpoints (Architecture/12 §2a).
fn developer_role() -> ResourceEnvelope {
    org(
        "Role",
        "pipeline-developer",
        json!({ "rules": [
            { "kinds": ["Pipeline", "DataSource", "Mapping"], "verbs": ["propose"] },
            { "kinds": ["Endpoint"], "verbs": ["propose"], "constraints": [{ "field": "spec.audience", "notIn": ["public"] }] }
        ]}),
    )
}

fn binding(
    name: &str,
    role: &str,
    subjects: Value,
    scope: Value,
    validity: Option<Value>,
) -> ResourceEnvelope {
    let mut spec = json!({ "subjects": subjects, "role": role, "scope": scope });
    if let Some(v) = validity {
        spec["validity"] = v;
    }
    org("RoleBinding", name, spec)
}

fn mirror_with(envelopes: Vec<ResourceEnvelope>) -> Mirror {
    let mirror = Mirror::new();
    for env in envelopes {
        mirror.upsert(env);
    }
    mirror
}

fn pipeline(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } },
            "compute": { "kind": "bloblang", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
        }
    })
}

fn endpoint(audience: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": { "name": "air", "namespace": "ovzdusie" },
        "spec": { "contextSpaceRef": "ovzdusie", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "audience": audience, "enabledRepresentations": ["ngsi-ld"] }
    })
}

async fn post(
    app: axum::Router,
    config: &Config,
    who: Identity,
    uri: &str,
    body: &Value,
) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookies(config, who))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn app_with(config: &Config, envelopes: Vec<ResourceEnvelope>) -> axum::Router {
    let state = AppState::new(config.clone(), None);
    for env in envelopes {
        state.mirror.upsert(env);
    }
    server::app(state)
}

#[tokio::test]
async fn a_user_with_no_binding_cannot_propose_and_the_403_names_the_verb() {
    let config = Config::for_tests();
    let app = app_with(&config, vec![developer_role()]);
    let (status, body) = post(
        app,
        &config,
        identity("viewer@hel.fi", &[]),
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline("aq"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("propose") && body.contains("Pipeline"),
        "{body}"
    );
}

#[tokio::test]
async fn a_developer_proposes_a_pipeline_but_not_a_public_endpoint() {
    let config = Config::for_tests();
    let dev = || identity("jana@hel.fi", &["air-quality-team"]);
    let repo = || {
        vec![
            developer_role(),
            binding(
                "ovzdusie-developers",
                "pipeline-developer",
                json!([{ "group": "air-quality-team" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
        ]
    };
    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        &pipeline("aq"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/ovzdusie/endpoints?dryRun=All",
        &endpoint("organization"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/ovzdusie/endpoints?dryRun=All",
        &endpoint("public"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("spec.audience"), "{body}");

    // The binding covers ovzdusie, not another project.
    let other = pipeline("aq");
    let mut other = other;
    other["metadata"]["namespace"] = json!("doprava");
    let (status, body) = post(
        app_with(&config, repo()),
        &config,
        dev(),
        "/api/v1/projects/doprava/pipelines?dryRun=All",
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn deletion_needs_delete_and_the_bootstrap_group_needs_no_repository() {
    let config = Config::for_tests();
    let live = || {
        let mut env = serde_json::from_value::<ResourceEnvelope>(pipeline("aq")).expect("envelope");
        env.metadata.namespace = Some("ovzdusie".into());
        env
    };
    let dev = identity("jana@hel.fi", &["air-quality-team"]);
    let repo = vec![
        live(),
        developer_role(),
        binding(
            "ovzdusie-developers",
            "pipeline-developer",
            json!([{ "user": "jana@hel.fi" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
    ];
    let delete = |app: axum::Router, who: Identity, config: Config| async move {
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/projects/ovzdusie/pipelines/aq?dryRun=All")
                    .header(header::COOKIE, cookies(&config, who))
                    .header(CSRF_HEADER, CSRF)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        response.status()
    };
    assert_eq!(
        delete(app_with(&config, repo), dev, config.clone()).await,
        StatusCode::FORBIDDEN
    );

    // `portal-approver` is the bootstrap group of `Config::for_tests()`: everything, with an empty repository.
    let admin = identity("admin@hel.fi", &["portal-approver"]);
    assert_eq!(
        delete(app_with(&config, vec![live()]), admin, config.clone()).await,
        StatusCode::OK
    );
}

#[test]
fn approve_expiry_and_organization_scope_are_decided_by_the_bindings() {
    let now = Utc.with_ymd_and_hms(2026, 9, 7, 12, 0, 0).unwrap();
    let approver_role = org(
        "Role",
        "approver",
        json!({ "rules": [{ "kinds": ["Pipeline", "Endpoint"], "verbs": ["approve", "delete"] }] }),
    );
    let mirror = mirror_with(vec![
        developer_role(),
        approver_role,
        binding(
            "org-approvers",
            "approver",
            json!([{ "user": "boss@hel.fi" }]),
            json!({ "organization": "hel" }),
            None,
        ),
        binding(
            "gone",
            "approver",
            json!([{ "user": "former@hel.fi" }]),
            json!({ "organization": "hel" }),
            Some(json!({ "notAfter": "2026-01-01T00:00:00Z" })),
        ),
        binding(
            "devs",
            "pipeline-developer",
            json!([{ "group": "air-quality-team" }]),
            json!({ "project": "ovzdusie" }),
            None,
        ),
    ]);
    let target = pipeline("aq");

    let boss = effective(
        &mirror,
        "platform-admins",
        &identity("boss@hel.fi", &[]),
        "any-project",
        now,
    );
    assert!(
        boss.check("Pipeline", Verb::Approve, Some(&target)).is_ok(),
        "organization scope covers every project"
    );
    assert!(
        boss.check("Pipeline", Verb::Propose, Some(&target))
            .is_err(),
        "approve is not propose"
    );

    let former = effective(
        &mirror,
        "platform-admins",
        &identity("former@hel.fi", &[]),
        "ovzdusie",
        now,
    );
    assert!(
        former.grants.is_empty(),
        "an expired binding grants nothing"
    );

    let dev = effective(
        &mirror,
        "platform-admins",
        &identity("jana@hel.fi", &["air-quality-team"]),
        "ovzdusie",
        now,
    );
    assert!(dev.check("Pipeline", Verb::Propose, Some(&target)).is_ok());
    let err = dev
        .check("Pipeline", Verb::Approve, Some(&target))
        .expect_err("a developer does not approve");
    assert!(err.to_string().contains("approve"), "{err}");
    assert_eq!(dev.grants.len(), 2, "one grant per rule of the role");
}

#[tokio::test]
async fn permissions_me_lists_the_grants_of_the_caller() {
    let config = Config::for_tests();
    let app = app_with(
        &config,
        vec![
            developer_role(),
            binding(
                "ovzdusie-developers",
                "pipeline-developer",
                json!([{ "group": "air-quality-team" }]),
                json!({ "project": "ovzdusie" }),
                None,
            ),
        ],
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/permissions/me")
                .header(
                    header::COOKIE,
                    cookies(&config, identity("jana@hel.fi", &["air-quality-team"])),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let doc: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(doc["project"], "ovzdusie");
    assert_eq!(doc["bootstrap"], false);
    assert_eq!(doc["grants"].as_array().map(Vec::len), Some(2));
    assert_eq!(doc["grants"][0]["role"], "pipeline-developer");
    assert_eq!(
        doc["grants"][1]["rule"]["constraints"][0]["field"],
        "spec.audience"
    );
}
