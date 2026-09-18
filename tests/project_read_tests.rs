//! Every read of a project answers `404` to a caller who may not read it, the one answer for
//! "missing" and "not yours" (PF-59, R20): the catalogue status, the federation graph, the
//! assistant's catalog search, a sync source's status, the agent-run list and the activity
//! operation (T-1401, T-1361, T-1368, T-1340). A reader of the project is answered.

mod common;

use axum::http::StatusCode;
use serde_json::json;

use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const READS: &[(&str, &str)] = &[
    ("GET", "/api/v1/projects/ovzdusie/ckan/status"),
    ("GET", "/api/v1/projects/ovzdusie/federation-graph"),
    ("GET", "/api/v1/projects/ovzdusie/assistant/catalog?q=air"),
    (
        "GET",
        "/api/v1/projects/ovzdusie/syncsources/upstream/status",
    ),
    ("GET", "/api/v1/projects/ovzdusie/agent-runs"),
    ("POST", "/api/v1/projects/ovzdusie/ops/jc_activity_list"),
];

/// `ovzdusie` holds a sync source, and `reader@hel.fi` reads the kinds these routes read there.
async fn state() -> AppState {
    let state = common::state_on(&common::forge().await);
    state.mirror.upsert(common::envelope(
        "SyncSource",
        "upstream",
        "ovzdusie",
        json!({ "url": "https://git.example.org/city/config.git" }),
    ));
    state.mirror.upsert(common::envelope(
        "Role",
        "reads-everything",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["CkanInstance", "SyncSource", "Pipeline"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "reader-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "reads-everything",
            "scope": { "project": "ovzdusie" },
        }),
    ));
    state
}

fn body(http: &str) -> Option<serde_json::Value> {
    (http == "POST").then(|| json!({}))
}

#[tokio::test]
async fn a_project_the_caller_may_not_read_is_not_there() {
    let state = state().await;
    for (http, uri) in READS {
        let answer = common::send(&state, common::person("stranger"), http, uri, body(http)).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{http} {uri}: {}",
            answer.text
        );
        let absent = common::send(
            &state,
            common::person("stranger"),
            http,
            &uri.replace("ovzdusie", "no-such-project"),
            body(http),
        )
        .await;
        assert_eq!(
            (
                answer.status,
                answer.text.replace("ovzdusie", "no-such-project")
            ),
            (absent.status, absent.text),
            "{http} {uri}: a project the caller may not read reads like one that is not there"
        );
    }
}

#[tokio::test]
async fn a_reader_of_the_project_is_answered() {
    let state = state().await;
    for (http, uri) in READS {
        let answer = common::send(&state, common::person("reader"), http, uri, body(http)).await;
        assert_ne!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{http} {uri}: {}",
            answer.text
        );
        assert_ne!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{http} {uri}: {}",
            answer.text
        );
    }
}
