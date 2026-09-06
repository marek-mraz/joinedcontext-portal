//! T-0190: Gitea REST API client integration tests with wiremock.

use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::git::{
    Author, FileDelete, FileWrite, GitError, GiteaClient, MergeStyle, RepoFile, ReviewEvent,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn default_branch_and_branch_head() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .and(header("authorization", "token secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .and(header("authorization", "token secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "commit": {
                "id": "c0ffee1234567890abcdef"
            }
        })))
        .mount(&server)
        .await;

    let branch = client.default_branch().await.unwrap();
    assert_eq!(branch, "main");

    let head = client.branch_head("main").await.unwrap();
    assert_eq!(head, "c0ffee1234567890abcdef");
}

#[tokio::test]
async fn create_branch_success_and_conflict() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "new_branch_name": "feature-1",
            "old_branch_name": "main"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "name": "feature-1"
        })))
        .mount(&server)
        .await;

    client.create_branch("feature-1", "main").await.unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "new_branch_name": "already-exists",
            "old_branch_name": "main"
        })))
        .respond_with(ResponseTemplate::new(409).set_body_string("branch already exists"))
        .mount(&server)
        .await;

    let err = client
        .create_branch("already-exists", "main")
        .await
        .unwrap_err();
    match err {
        GitError::Conflict(msg) => assert!(msg.contains("branch already exists")),
        other => panic!("expected GitError::Conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn get_file_decodes_base64_and_handles_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    let raw_text = "apiVersion: joinedcontext.com/v1alpha1\nkind: Dashboard\n";
    // Gitea wraps the base64 payload, so the fixture is built with the line breaks in it
    // rather than pasted — a hand-typed constant is one typo away from invalid padding.
    let encoded = base64::engine::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        raw_text.as_bytes(),
    );
    let b64_with_newlines = format!("{}\n{}\n", &encoded[..16], &encoded[16..]);

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/dashboards/dash.yaml",
        ))
        .and(header("authorization", "token secret-token"))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-file-123",
            "content": b64_with_newlines.clone()
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/missing.yaml",
        ))
        .and(header("authorization", "token secret-token"))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "not found"
        })))
        .mount(&server)
        .await;

    let file = client
        .get_file("dashboards/dash.yaml", "main")
        .await
        .unwrap();
    assert_eq!(
        file,
        Some(RepoFile {
            sha: "sha-file-123".to_string(),
            content: raw_text.to_string(),
        })
    );

    let missing = client.get_file("missing.yaml", "main").await.unwrap();
    assert_eq!(missing, None);
}

#[tokio::test]
async fn put_file_creates_and_replaces() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    // Create (sha is None -> omitted from JSON)
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/dashboards/dash.yaml",
        ))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "content": "aGVsbG8gd29ybGQ=",
            "message": "create dashboard",
            "branch": "feature-1",
            "author": { "name": "Alice Developer", "email": "alice@example.com" },
            "committer": { "name": "Alice Developer", "email": "alice@example.com" }
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;

    let write_create = FileWrite {
        path: "dashboards/dash.yaml",
        branch: "feature-1",
        message: "create dashboard",
        content: "hello world",
        sha: None,
        author: Author {
            name: "Alice Developer",
            email: "alice@example.com",
        },
    };
    let sha_created = client.put_file(&write_create).await.unwrap();
    assert_eq!(sha_created, "commit-sha-created");

    // Replace (sha is Some -> present in JSON)
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/dashboards/dash.yaml",
        ))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "content": "aGVsbG8gd29ybGQgdjI=",
            "message": "update dashboard",
            "branch": "feature-1",
            "sha": "old-sha-111",
            "author": { "name": "Alice Developer", "email": "alice@example.com" },
            "committer": { "name": "Alice Developer", "email": "alice@example.com" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "commit": { "sha": "commit-sha-updated" }
        })))
        .mount(&server)
        .await;

    let write_replace = FileWrite {
        path: "dashboards/dash.yaml",
        branch: "feature-1",
        message: "update dashboard",
        content: "hello world v2",
        sha: Some("old-sha-111"),
        author: Author {
            name: "Alice Developer",
            email: "alice@example.com",
        },
    };
    let sha_updated = client.put_file(&write_replace).await.unwrap();
    assert_eq!(sha_updated, "commit-sha-updated");
}

#[tokio::test]
async fn delete_file_sends_sha() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/dashboards/dash.yaml",
        ))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "message": "remove dashboard",
            "branch": "feature-1",
            "sha": "delete-sha-222",
            "author": { "name": "Bob Admin", "email": "bob@example.com" },
            "committer": { "name": "Bob Admin", "email": "bob@example.com" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "commit": { "sha": "commit-sha-deleted" }
        })))
        .mount(&server)
        .await;

    let req = FileDelete {
        path: "dashboards/dash.yaml",
        branch: "feature-1",
        message: "remove dashboard",
        sha: "delete-sha-222",
        author: Author {
            name: "Bob Admin",
            email: "bob@example.com",
        },
    };
    let sha = client.delete_file(&req).await.unwrap();
    assert_eq!(sha, "commit-sha-deleted");
}

#[tokio::test]
async fn create_and_get_pull_request() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "head": "feature-1",
            "base": "main",
            "title": "Add feature",
            "body": "PR description"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 17,
            "html_url": "https://gitea.example.com/test-owner/test-repo/pulls/17",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/17"))
        .and(header("authorization", "token secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 17,
            "html_url": "https://gitea.example.com/test-owner/test-repo/pulls/17",
            "state": "closed",
            "mergeable": null,
            "merged": true
        })))
        .mount(&server)
        .await;

    let pr = client
        .create_pull_request("feature-1", "main", "Add feature", "PR description")
        .await
        .unwrap();
    assert_eq!(pr.number, 17);
    assert_eq!(
        pr.url,
        "https://gitea.example.com/test-owner/test-repo/pulls/17"
    );
    assert_eq!(pr.state, "open");
    assert_eq!(pr.mergeable, Some(true));
    assert!(!pr.merged);

    let fetched = client.pull_request(17).await.unwrap();
    assert_eq!(fetched.number, 17);
    assert_eq!(fetched.state, "closed");
    assert_eq!(fetched.mergeable, None);
    assert!(fetched.merged);
}

#[tokio::test]
async fn review_and_merge() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "test-owner", "test-repo", "secret-token").unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/17/reviews"))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "event": "APPROVED",
            "body": "Approved by security steward"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    client
        .review(17, ReviewEvent::Approve, "Approved by security steward")
        .await
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/17/merge"))
        .and(header("authorization", "token secret-token"))
        .and(body_json(json!({
            "Do": "merge",
            "merge_message_field": "Merge PR #17"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    client
        .merge(17, MergeStyle::Merge, "Merge PR #17")
        .await
        .unwrap();
}

#[test]
fn gitea_client_redacts_token_in_debug() {
    let url = "https://gitea.example.com".parse().unwrap();
    let client = GiteaClient::new(url, "owner", "repo", "my-secret-token").unwrap();
    let debug = format!("{client:?}");
    assert!(!debug.contains("my-secret-token"));
    assert!(debug.contains("[redacted]"));
}

#[test]
fn from_env_fail_closed_behavior() {
    let empty = GiteaClient::from_env(|_| None).unwrap();
    assert!(empty.is_none());

    let err1 = GiteaClient::from_env(|k| match k {
        "JC_GITEA_URL" => Some("https://gitea.example.com".to_string()),
        _ => None,
    })
    .unwrap_err();
    match err1 {
        GitError::Config(msg) => assert!(msg.contains("must be set together")),
        other => panic!("expected Config error, got {other:?}"),
    }

    let err2 = GiteaClient::from_env(|k| match k {
        "JC_GITEA_URL" => Some("https://gitea.example.com".to_string()),
        "JC_GITEA_OWNER" => Some("owner".to_string()),
        "JC_GITEA_REPO" => Some("repo".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(matches!(err2, GitError::Config(_)));

    let err_url = GiteaClient::from_env(|k| match k {
        "JC_GITEA_URL" => Some("not-a-valid-url".to_string()),
        "JC_GITEA_OWNER" => Some("owner".to_string()),
        "JC_GITEA_REPO" => Some("repo".to_string()),
        "JC_GITEA_TOKEN" => Some("token".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(matches!(err_url, GitError::Config(_)));

    let complete = GiteaClient::from_env(|k| match k {
        "JC_GITEA_URL" => Some("https://gitea.example.com".to_string()),
        "JC_GITEA_OWNER" => Some("my-owner".to_string()),
        "JC_GITEA_REPO" => Some("my-repo".to_string()),
        "JC_GITEA_TOKEN" => Some("my-token".to_string()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    assert_eq!(complete.owner, "my-owner");
    assert_eq!(complete.repo, "my-repo");
}

#[test]
fn git_error_converts_to_api_error() {
    let not_found = GitError::NotFound;
    assert!(matches!(ApiError::from(not_found), ApiError::NotFound(_)));

    let conflict = GitError::Conflict("branch exists".into());
    assert!(matches!(ApiError::from(conflict), ApiError::Conflict(_)));

    let transport = GitError::Transport("connection reset".into());
    assert!(matches!(ApiError::from(transport), ApiError::Internal(_)));

    let config = GitError::Config("missing var".into());
    assert!(matches!(ApiError::from(config), ApiError::Internal(_)));

    let api = GitError::Api {
        status: 500,
        message: "internal".into(),
    };
    assert!(matches!(ApiError::from(api), ApiError::Internal(_)));
}
