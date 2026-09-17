//! Tests for the Portal Configuration MCP server at `/api/v1/mcp` (T-0637, AG-60, AG-32, CC-45, CC-47).
//!
//! Asserts that:
//! 1. `initialize` and `server/discover` return protocol version and serverInfo.
//! 2. `tools/list` returns tools filtered by caller's effective permissions.
//! 3. `tools/call jc_catalog_search` returns structured content.
//! 4. A bearer token for another audience is rejected with 401 and WWW-Authenticate.
//! 5. A request with only a session cookie (no Bearer header) is rejected with 401.
//! 6. `GET /.well-known/oauth-protected-resource/api/v1/mcp` returns RFC 9728 metadata.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p256::pkcs8::EncodePrivateKey;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const REALM_PATH: &str = "/realms/banskabystrica";
const PORTAL_AUDIENCE: &str = "joinedcontext-portal";

/// One open change and one model source on the forge, so the two resource shapes AG-60 names
/// have something to answer with (T-0847).
async fn forge_mocks(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(server)
        .await;
    let pull = json!({
        "number": 1,
        "html_url": "https://gitea.example.sk/pulls/1",
        "state": "open",
        "title": "create ContextSpace mobility",
        "head": { "ref": "portal/create-contextspace-mobility-11111111" },
        "base": { "ref": "main" },
        "created_at": "2026-09-06T09:14:22Z",
        "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová",
                  "email": "jana.kovacova@banskabystrica.sk" },
        "mergeable": true,
        "merged": false
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([pull.clone()])))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "added" }
        ])))
        .mount(server)
        .await;
    let space_yaml = concat!(
        "apiVersion: joinedcontext.com/v1alpha1\n",
        "kind: ContextSpace\n",
        "metadata:\n",
        "  name: mobility\n",
        "  namespace: ovzdusie\n",
        "spec:\n",
        "  isSandbox: true\n",
    );
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "portal/create-contextspace-mobility-11111111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": base64_of(space_yaml)
        })))
        .mount(server)
        .await;
    let linkml = concat!(
        "id: https://hel.fi/models/ovzdusie/air\n",
        "name: air\n",
        "classes:\n",
        "  AirQualityObserved:\n",
        "    attributes:\n",
        "      pm10:\n",
        "        range: float\n",
    );
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/ovzdusie/datamodels/air.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-2",
            "content": base64_of(linkml)
        })))
        .mount(server)
        .await;
    // Anything else on the forge is absent, which is what a create's base branch answers.
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            "^/api/v1/repos/test-owner/test-repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .with_priority(9)
        .mount(server)
        .await;
}

fn base64_of(text: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

fn issuer_of(server: &MockServer) -> String {
    format!("{}{REALM_PATH}", server.uri().trim_end_matches('/'))
}

fn generate_keypair(kid: &str) -> (EncodingKey, Value) {
    let secret = p256::SecretKey::random(&mut rand_core::OsRng);
    let der = secret.to_pkcs8_der().expect("der");
    let signer = EncodingKey::from_ec_der(der.as_bytes());
    let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).expect("jwk");
    jwk["kid"] = json!(kid);
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    let jwks = json!({ "keys": [jwk] });
    (signer, jwks)
}

fn sign_token(
    signer: &EncodingKey,
    kid: &str,
    issuer: &str,
    audience: &str,
    subject: &str,
    roles: &[&str],
    groups: &[&str],
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    let now = joinedcontext_portal::auth::session::now_unix();
    let claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": subject,
        "preferred_username": subject,
        "exp": now + 3600,
        "iat": now,
        "realm_access": { "roles": roles },
        "groups": groups,
    });
    encode(&header, &claims, signer).expect("sign token")
}

async fn setup_app_and_keys() -> (axum::Router, String, EncodingKey, String) {
    let mock_server = MockServer::start().await;
    let issuer = issuer_of(&mock_server);
    let (signer, jwks) = generate_keypair("key-mcp-test");

    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM_PATH}/.well-known/openid-configuration"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "end_session_endpoint": format!("{issuer}/protocol/openid-connect/logout"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
        .mount(&mock_server)
        .await;

    let mut config = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer.clone()),
        "JC_OIDC_CLIENT_ID" => Some(PORTAL_AUDIENCE.to_string()),
        "JC_OIDC_CLIENT_SECRET" => Some("secret".to_string()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        _ => None,
    })
    .expect("config");
    config.public_base_url = "https://portal.test".parse().unwrap();

    let mirror = std::sync::Arc::new(Mirror::new());
    // Seed some resources
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Endpoint".to_string(),
        metadata: ObjectMeta::new("public-air", "ovzdusie"),
        spec: json!({
            "slug": "publicair00000000000000000000",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }),
        status: None,
    });

    // A model, so its LinkML source is a resource beside its manifest (T-0847).
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta::new("air", "ovzdusie"),
        spec: json!({ "contextSpaceRef": "ovzdusie", "linkml": "./air.linkml.yaml" }),
        status: None,
    });

    // Add role for viewer
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Role".to_string(),
        metadata: ObjectMeta::new("read-only-role", "org"),
        spec: json!({
            "rules": [
                { "kinds": ["Endpoint", "ContextSpace"], "verbs": [] }
            ]
        }),
        status: None,
    });
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "RoleBinding".to_string(),
        metadata: ObjectMeta::new("viewer-binding", "org"),
        spec: json!({
            "subjects": [{ "group": "city-viewers" }],
            "role": "read-only-role",
            "scope": { "project": "ovzdusie" }
        }),
        status: None,
    });

    // The same mock server plays the forge: `/realms/...` is Keycloak, `/api/v1/repos/...` is
    // Gitea, so a change's plan and a model's source are readable over MCP (T-0847).
    forge_mocks(&mock_server).await;
    let gitea = joinedcontext_portal::git::GiteaClient::new(
        mock_server.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("gitea client");

    let state = AppState::from_config(config)
        .await
        .expect("state")
        .with_mirror(mirror)
        .with_gitea(std::sync::Arc::new(gitea));

    (
        server::app(state),
        issuer,
        signer,
        "key-mcp-test".to_string(),
    )
}

#[tokio::test]
async fn mcp_initialize_and_discover() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        // The bootstrap group (Config::DEFAULT_BOOTSTRAP_ADMINS): the steward holds every
        // operation, the viewer below only what its binding grants.
        &["platform-admins"],
    );

    // 1. initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": { "name": "claude-code", "version": "1.0.0" }
        }
    });

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&init_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let init_res: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(init_res["jsonrpc"], "2.0");
    assert_eq!(init_res["id"], 1);
    assert_eq!(init_res["result"]["protocolVersion"], "2026-07-28");
    assert_eq!(
        init_res["result"]["serverInfo"]["name"],
        "joinedcontext-portal"
    );

    // 2. server/discover
    let discover_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "server/discover"
    });

    let resp_disc = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&discover_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_disc.status(), StatusCode::OK);
    let bytes_disc = resp_disc.into_body().collect().await.unwrap().to_bytes();
    let disc_res: Value = serde_json::from_slice(&bytes_disc).unwrap();
    assert_eq!(disc_res["jsonrpc"], "2.0");
    assert_eq!(disc_res["id"], 2);
    assert_eq!(disc_res["result"]["protocolVersion"], "2026-07-28");
    assert!(disc_res["result"]["tools"]["count"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn mcp_tools_list_filtered_by_caller() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;

    // 1. Steward bearer token
    let steward_token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        // The bootstrap group (Config::DEFAULT_BOOTSTRAP_ADMINS): the steward holds every
        // operation, the viewer below only what its binding grants.
        &["platform-admins"],
    );

    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/list",
        "params": {
            "project": "ovzdusie"
        }
    });

    let resp_steward = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {steward_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&list_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_steward.status(), StatusCode::OK);
    let bytes_steward = resp_steward.into_body().collect().await.unwrap().to_bytes();
    let res_steward: Value = serde_json::from_slice(&bytes_steward).unwrap();
    let tools = res_steward["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(names.contains(&"jc_catalog_search"));
    assert!(names.contains(&"jc_datasource_propose"));
    assert!(names.contains(&"jc_change_approve"));

    // Check annotations and schemas
    let search_tool = tools
        .iter()
        .find(|t| t["name"] == "jc_catalog_search")
        .unwrap();
    assert_eq!(search_tool["annotations"]["readOnlyHint"], true);
    assert_eq!(search_tool["annotations"]["destructiveHint"], false);
    assert_eq!(search_tool["annotations"]["idempotentHint"], true);

    // 2. Viewer bearer token
    let viewer_token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "viewer.user",
        &[],
        &["city-viewers"],
    );

    let resp_viewer = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {viewer_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&list_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_viewer.status(), StatusCode::OK);
    let bytes_viewer = resp_viewer.into_body().collect().await.unwrap().to_bytes();
    let res_viewer: Value = serde_json::from_slice(&bytes_viewer).unwrap();
    let v_tools = res_viewer["result"]["tools"].as_array().unwrap();
    let v_names: Vec<&str> = v_tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(v_names.contains(&"jc_catalog_search"));
    assert!(
        !v_names.contains(&"jc_datasource_propose"),
        "viewer must not see propose tools"
    );
    assert!(
        !v_names.contains(&"jc_change_approve"),
        "viewer must not see approve tools"
    );
}

#[tokio::test]
async fn mcp_tools_call_jc_catalog_search() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        // The bootstrap group (Config::DEFAULT_BOOTSTRAP_ADMINS): the steward holds every
        // operation, the viewer below only what its binding grants.
        &["platform-admins"],
    );

    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "tools/call",
        "params": {
            "name": "jc_catalog_search",
            "arguments": {
                "project": "ovzdusie",
                "q": "air"
            }
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&call_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let call_res: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(call_res["jsonrpc"], "2.0");
    assert_eq!(call_res["id"], 20);
    assert_eq!(call_res["result"]["isError"], false);

    let structured = &call_res["result"]["structuredContent"];
    let items = structured["items"].as_array().expect("items in catalog");
    assert!(!items.is_empty());
    assert_eq!(items[0]["name"], "public-air");

    let content = call_res["result"]["content"].as_array().expect("content");
    assert_eq!(content[0]["type"], "text");
    assert!(content[0]["text"].as_str().unwrap().contains("public-air"));
}

#[tokio::test]
async fn mcp_wrong_audience_bearer_returns_401_with_www_authenticate() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;

    // Token for a different audience
    let foreign_token = sign_token(
        &signer,
        &kid,
        &issuer,
        "context-gateway",
        "attacker.user",
        &["portal-approver"],
        &[],
    );

    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "ping"
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {foreign_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&call_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let auth_header = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate header");
    let auth_str = auth_header.to_str().unwrap();
    assert!(auth_str.starts_with("Bearer resource_metadata="));
    assert!(auth_str.contains("/.well-known/oauth-protected-resource/api/v1/mcp"));
}

#[tokio::test]
async fn mcp_session_cookie_only_returns_401() {
    let (app, _issuer, _signer, _kid) = setup_app_and_keys().await;

    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 40,
        "method": "ping"
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::COOKIE, "jc_session=some_session_value")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&call_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_oauth_protected_resource_metadata() {
    let (app, issuer, _signer, _kid) = setup_app_and_keys().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-protected-resource/api/v1/mcp")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let meta: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(meta["resource"], "https://portal.test/api/v1/mcp");
    assert_eq!(meta["authorization_servers"], json!([issuer]));
    assert_eq!(meta["bearer_methods_supported"], json!(["header"]));
    assert_eq!(meta["scopes_supported"], json!(["mcp:portal"]));
}

async fn rpc(app: axum::Router, token: &str, body: Value) -> Value {
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn mcp_resources_list_and_read_manifests_drafts_and_schemas() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );

    // A draft filed through the registry shows up as a resource too.
    let put = rpc(
        app.clone(),
        &token,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "jc_draft_put", "arguments": {
                "project": "ovzdusie", "kind": "DataSource", "name": "hsy-air",
                "manifest": { "apiVersion": "joinedcontext.com/v1", "kind": "DataSource",
                    "metadata": { "name": "hsy-air", "namespace": "ovzdusie" },
                    "spec": { "protocol": "http", "url": "https://example.org/air" } }
            } }
        }),
    )
    .await;
    assert_eq!(put["result"]["isError"], false, "{put}");

    let listed = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/list", "params": { "project": "ovzdusie" } }),
    )
    .await;
    let uris: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(
        uris.contains(&"jc://ovzdusie/endpoints/public-air"),
        "{uris:?}"
    );
    assert!(
        uris.contains(&"jc://ovzdusie/drafts/DataSource/hsy-air"),
        "{uris:?}"
    );
    assert!(uris.contains(&"jc://schemas/Endpoint"), "{uris:?}");

    for (uri, needle) in [
        (
            "jc://ovzdusie/endpoints/public-air",
            "publicair00000000000000000000",
        ),
        (
            "jc://ovzdusie/drafts/DataSource/hsy-air",
            "https://example.org/air",
        ),
        ("jc://schemas/Endpoint", "\"properties\""),
    ] {
        let read = rpc(
            app.clone(),
            &token,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/read", "params": { "uri": uri } }),
        )
        .await;
        let text = read["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{uri}: {read}"));
        assert!(text.contains(needle), "{uri}: {text}");
    }

    let missing = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/read", "params": { "uri": "jc://ovzdusie/endpoints/nope" } }),
    )
    .await;
    assert_eq!(missing["error"]["code"], -32002);

    // A caller without a grant on the project sees nothing and reads nothing.
    let stranger = sign_token(&signer, &kid, &issuer, PORTAL_AUDIENCE, "nobody", &[], &[]);
    let listed = rpc(
        app.clone(),
        &stranger,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list", "params": { "project": "ovzdusie" } }),
    )
    .await;
    assert_eq!(
        listed["result"]["resources"].as_array().map(Vec::len),
        Some(0)
    );
    let read = rpc(
        app,
        &stranger,
        json!({ "jsonrpc": "2.0", "id": 6, "method": "resources/read", "params": { "uri": "jc://ovzdusie/endpoints/public-air" } }),
    )
    .await;
    assert_eq!(read["error"]["code"], -32002);
}

#[tokio::test]
async fn mcp_prompts_name_the_units_and_their_operations() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );
    let listed = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "prompts/list" }),
    )
    .await;
    let names: Vec<&str> = listed["result"]["prompts"]
        .as_array()
        .expect("prompts")
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names, ["load", "share", "analyse", "model", "change"]);

    let got = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "prompts/get",
            "params": { "name": "share", "arguments": { "project": "ovzdusie" } } }),
    )
    .await;
    let text = got["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("Project: ovzdusie"), "{text}");
    assert!(text.contains("jc_endpoint_propose"), "{text}");

    let unknown = rpc(
        app,
        &token,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "prompts/get", "params": { "name": "dance" } }),
    )
    .await;
    assert_eq!(unknown["error"]["code"], -32602);
}

/// AG-60, T-0839: the route counts and bounds for itself, so a caller inside the cluster —
/// where the edge's bucket does not exist — is bounded too.
#[tokio::test]
async fn the_mcp_route_bounds_the_calls_and_the_bytes_of_one_token() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "loop.bot",
        &["portal-approver"],
        &["platform-admins"],
    );
    let post = |body: Vec<u8>| {
        Request::builder()
            .method("POST")
            .uri("/api/v1/mcp")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let ping = serde_json::to_vec(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).unwrap();

    // A request larger than the route reads is refused before it is parsed, naming what to do.
    let huge = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "jc_catalog_search", "arguments": { "q": "x".repeat(1024 * 1024 + 16) } }
    }))
    .unwrap();
    let resp = app.clone().oneshot(post(huge)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("draft"),
        "{body}"
    );

    // 120 calls a minute, and the one past it answers 429 with the minute to wait.
    let mut last = StatusCode::OK;
    for _ in 0..121 {
        last = app
            .clone()
            .oneshot(post(ping.clone()))
            .await
            .unwrap()
            .status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    let resp = app.clone().oneshot(post(ping.clone())).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get(header::RETRY_AFTER).unwrap(), "60");

    // The budget is the subject's own: another token still answers.
    let other = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "second.bot",
        &["portal-approver"],
        &["platform-admins"],
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {other}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(ping))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// AG-60, ADR-N-021, T-0843: a call that waits on something else answers a task, and the client
/// polls it instead of holding the request open. A short call still answers inline.
#[tokio::test]
async fn a_long_call_answers_a_task_the_caller_polls_and_a_short_one_answers_inline() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );
    let send = |body: Value| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/mcp")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }
    };

    // The catalogue says which tools are served as tasks.
    let listed = send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "project": "ovzdusie" }
    }))
    .await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    let support = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .map(|tool| tool["execution"]["taskSupport"].clone())
    };
    assert_eq!(support("jc_pipeline_test"), Some(json!("required")));
    assert_eq!(support("jc_catalog_search"), Some(json!("forbidden")));

    // A pipeline test waits on the project's runner: the call answers a task at once.
    let started = send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "jc_pipeline_test", "project": "ovzdusie",
                    "arguments": { "project": "ovzdusie", "pipeline": { "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Pipeline", "metadata": { "name": "bikes" }, "spec": {} }, "sample": "{}" } }
    }))
    .await;
    let task_id = started["result"]["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("a task id: {started}"))
        .to_owned();
    assert_eq!(started["result"]["task"]["status"], json!("working"));

    // It is polled: working, then whatever the call answered.
    let mut status = json!("working");
    for _ in 0..100 {
        let polled = send(json!({
            "jsonrpc": "2.0", "id": 3, "method": "tasks/get", "params": { "taskId": task_id }
        }))
        .await;
        status = polled["result"]["status"].clone();
        if status != json!("working") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_ne!(status, json!("working"), "the task ends");
    let read = send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "tasks/result", "params": { "taskId": task_id }
    }))
    .await;
    assert!(
        read["result"]["content"].is_array(),
        "the result is the tool's own answer: {read}"
    );

    // A task of another subject is answered as one that never existed.
    let other = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "second.user",
        &["portal-approver"],
        &["platform-admins"],
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {other}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "jsonrpc": "2.0", "id": 5, "method": "tasks/get",
                        "params": { "taskId": task_id }
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let refused: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(refused["error"]["message"], json!("unknown task"));

    // A short call is unchanged: the answer is the answer.
    let inline = send(json!({
        "jsonrpc": "2.0", "id": 6, "method": "tools/call",
        "params": { "name": "jc_catalog_search", "project": "ovzdusie", "arguments": { "q": "air" } }
    }))
    .await;
    assert!(
        inline["result"]["structuredContent"].is_object(),
        "{inline}"
    );
    assert!(inline["result"].get("task").is_none());
}

/// AG-63, T-0836: a Red call runs nothing on the agent's word. The first call answers the
/// question, the second carries the person's answer, and the answer is on the activity.
#[tokio::test]
async fn a_red_call_asks_the_person_before_it_runs_and_a_refusal_runs_nothing() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );
    let call = |params: Value| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let body =
                json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": params });
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/mcp")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }
    };
    let arguments = json!({
        "project": "ovzdusie",
        "kind": "Endpoint",
        "name": "public-air",
        "confirm": "public-air"
    });

    // 1. The call the model makes alone: a question, and nothing removed.
    let asked = call(json!({ "name": "jc_resource_delete", "arguments": arguments })).await;
    let elicitation = &asked["result"]["structuredContent"]["elicitation"];
    let elicitation_id = elicitation["elicitationId"]
        .as_str()
        .unwrap_or_else(|| panic!("a question: {asked}"))
        .to_owned();
    assert_eq!(asked["result"]["status"], json!("input_required"));
    assert_eq!(elicitation["mode"], json!("url"));
    assert!(
        elicitation["url"]
            .as_str()
            .unwrap_or_default()
            .contains("/projects/ovzdusie/endpoints"),
        "{asked}"
    );

    // 2. An answer to a question nobody asked is refused, and still nothing runs.
    let forged = call(json!({
        "name": "jc_resource_delete",
        "arguments": arguments,
        "elicitation": { "elicitationId": "eli-0000000000000000", "action": "accept" }
    }))
    .await;
    assert_eq!(forged["result"]["isError"], json!(true), "{forged}");

    // 3. The person declines: the call is refused by name, nothing runs.
    let declined = call(json!({
        "name": "jc_resource_delete",
        "arguments": arguments,
        "elicitation": { "elicitationId": elicitation_id, "action": "decline" }
    }))
    .await;
    assert_eq!(declined["result"]["isError"], json!(true));
    assert!(
        declined["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("declined"),
        "{declined}"
    );

    // 4. The same id is spent: it cannot be answered a second time.
    let again = call(json!({
        "name": "jc_resource_delete",
        "arguments": arguments,
        "elicitation": { "elicitationId": elicitation_id, "action": "accept" }
    }))
    .await;
    assert_eq!(again["result"]["isError"], json!(true), "{again}");

    // 5. A green read is untouched by any of this.
    let read = call(json!({
        "name": "jc_catalog_search",
        "project": "ovzdusie",
        "arguments": { "q": "air" }
    }))
    .await;
    assert!(read["result"]["structuredContent"].is_object(), "{read}");
    assert!(read["result"]["structuredContent"]
        .get("elicitation")
        .is_none());
}

/// PF-57, AG-62, T-0947: a proposal whose check has not run is refused on the MCP door with
/// the same three fields the REST route answers, so a client reads `verdict_required`
/// wherever it knocks (ADR-N-021). The question is still asked: the URL is where the person
/// runs the check.
#[tokio::test]
async fn a_proposal_without_a_verdict_names_verdict_required_on_the_mcp_door() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );

    let put = rpc(
        app.clone(),
        &token,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "jc_draft_put", "arguments": {
                "project": "ovzdusie", "kind": "DataSource", "name": "unchecked",
                "manifest": { "apiVersion": "joinedcontext.com/v1", "kind": "DataSource",
                    "metadata": { "name": "unchecked", "namespace": "ovzdusie" },
                    "spec": { "protocol": "http", "url": "https://example.org/air" } }
            } }
        }),
    )
    .await;
    assert_eq!(put["result"]["isError"], false, "{put}");

    let proposal = rpc(
        app.clone(),
        &token,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "jc_datasource_propose", "arguments": {
                "project": "ovzdusie", "draft": { "kind": "DataSource", "name": "unchecked" }
            } }
        }),
    )
    .await;
    let structured = &proposal["result"]["structuredContent"];
    assert_eq!(
        proposal["result"]["status"],
        json!("input_required"),
        "{proposal}"
    );
    assert_eq!(structured["error"], json!("verdict_required"), "{proposal}");
    assert_eq!(
        structured["check"],
        json!("jc_datasource_check"),
        "{proposal}"
    );
    assert_eq!(structured["reason"], json!("verdict_absent"), "{proposal}");
    assert!(
        structured["detail"].as_str().is_some_and(|d| !d.is_empty()),
        "the refusal says it in one sentence a page shows as it is: {proposal}"
    );
    // The question is still there: the person needs somewhere to run the check.
    assert!(
        structured["elicitation"]["elicitationId"]
            .as_str()
            .is_some_and(|id| id.starts_with("eli-")),
        "{proposal}"
    );
    assert!(
        proposal["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("jc_datasource_check"),
        "the sentence a model reads does not name the check to run: {proposal}"
    );

    // Nothing was proposed: the draft is still a draft.
    let listed = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list", "params": { "project": "ovzdusie" } }),
    )
    .await;
    let uris: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(
        uris.contains(&"jc://ovzdusie/drafts/DataSource/unchecked"),
        "the gate let the proposal through: {uris:?}"
    );
}

/// T-0846, CC-47: the handshake answers a revision this server implements. Echoing an unknown
/// one tells the client the server speaks it, and the disagreement then surfaces mid-session as
/// `method not found` instead of at negotiation, the one moment a client can still choose.
#[tokio::test]
async fn the_handshake_answers_a_revision_this_server_implements() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );

    for method_name in ["initialize", "server/discover"] {
        // Every revision the server speaks is echoed, the client's own string.
        for known in ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"] {
            let answer = rpc(
                app.clone(),
                &token,
                json!({ "jsonrpc": "2.0", "id": 1, "method": method_name,
                        "params": { "protocolVersion": known } }),
            )
            .await;
            assert_eq!(
                answer["result"]["protocolVersion"], known,
                "{method_name} did not agree to {known}: {answer}"
            );
        }
        // Anything else is answered with what this server does speak.
        for unknown in ["2027-99-01", "", "1.0", "2025-06-18-draft"] {
            let answer = rpc(
                app.clone(),
                &token,
                json!({ "jsonrpc": "2.0", "id": 2, "method": method_name,
                        "params": { "protocolVersion": unknown } }),
            )
            .await;
            assert_eq!(
                answer["result"]["protocolVersion"], "2026-07-28",
                "{method_name} told the client it speaks `{unknown}`: {answer}"
            );
        }
        // No version at all is the newest, as before.
        let bare = rpc(
            app.clone(),
            &token,
            json!({ "jsonrpc": "2.0", "id": 3, "method": method_name }),
        )
        .await;
        assert_eq!(bare["result"]["protocolVersion"], "2026-07-28", "{bare}");
    }
}

/// T-0847, AG-60: a change's plan and a model's source are resources, so a model reads them
/// without spending a tool call, and both are advertised in `resources/list`.
#[tokio::test]
async fn a_change_plan_and_a_model_source_are_readable_resources() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );

    let listed = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list",
                "params": { "project": "ovzdusie" } }),
    )
    .await;
    let uris: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(
        uris.contains(&"jc://ovzdusie/datamodels/air/linkml"),
        "the model's source is not advertised: {uris:?}"
    );
    assert!(
        uris.contains(&"jc://ovzdusie/changes/chg-00000001"),
        "the open change is not advertised: {uris:?}"
    );

    let plan = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/read",
                "params": { "uri": "jc://ovzdusie/changes/chg-00000001" } }),
    )
    .await;
    let text = plan["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the change's plan is not readable: {plan}"));
    assert!(text.contains("chg-00000001"), "{text}");
    assert!(text.contains("mobility"), "{text}");

    let source = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/read",
                "params": { "uri": "jc://ovzdusie/datamodels/air/linkml" } }),
    )
    .await;
    assert_eq!(
        source["result"]["contents"][0]["mimeType"],
        json!("application/yaml"),
        "{source}"
    );
    assert!(
        source["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("AirQualityObserved"),
        "{source}"
    );

    // A model nobody declared and a change nobody opened answer as an unknown resource does.
    for missing in [
        "jc://ovzdusie/datamodels/nope/linkml",
        "jc://ovzdusie/changes/chg-00000099",
    ] {
        let answer = rpc(
            app.clone(),
            &token,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/read",
                    "params": { "uri": missing } }),
        )
        .await;
        assert_eq!(
            answer["error"]["code"], -32002,
            "{missing} answered something: {answer}"
        );
    }
}
