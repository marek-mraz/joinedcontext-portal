use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::change::Lane;
use crate::error::ApiError;
use crate::plan::PlanDiff;

#[derive(Debug, Default, Deserialize)]
pub struct DryRunQuery {
    #[serde(default, rename = "dryRun")]
    pub dry_run: Option<String>,
    /// Write into this workspace's branch instead of opening a Change (API/01 §22, CC-76).
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Evaluates the `?dryRun` query parameter.
///
/// `None` evaluates to `false` (mutations execute normally).
/// `Some("All")` evaluates to `true` (dry run: plan is computed without mutating state).
/// Any other value is rejected with 400 Bad Request naming `All` as the only accepted value.
pub fn is_dry_run(q: &DryRunQuery) -> Result<bool, ApiError> {
    match q.dry_run.as_deref() {
        None => Ok(false),
        Some("All") => Ok(true),
        Some(other) => Err(ApiError::BadRequest(format!(
            "invalid dryRun value '{other}'; 'All' is the only accepted value"
        ))),
    }
}

/// Reviewer-facing validation and diff result returned on `?dryRun=All` (MF-13, R17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DryRunResult {
    pub valid: bool,
    pub lane: Lane,
    pub plan: PlanDiff,
    /// Whether applying this makes the runner restart the pipeline's stream, so a person sees
    /// it before approving rather than afterwards (T-1056, PL-45). Absent for every other kind.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restarts_stream: bool,
    /// What one fetch of an `http` DataSource returned (MF-39); absent for every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
    /// The verdict this check recorded for the manifest, which its proposal needs (PF-57, T-0956).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<crate::ops::verdict::Verdict>,
    /// Values the manifest writes out where the loader renders them (CC-83): each names the
    /// path and what to write instead. Nothing here blocks the proposal; a copy of the
    /// manifest into another organization would carry the literal with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
}

/// Every string under `value` that writes `org_domain` out, as a finding naming its JSON
/// path and `{orgDomain}` as the replacement (CC-74, CC-83).
pub fn literal_domain_findings(value: &serde_json::Value, org_domain: &str) -> Vec<String> {
    fn walk(value: &serde_json::Value, path: &str, domain: &str, found: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) if domain.contains('.') && text.contains(domain) => {
                found.push(format!(
                    "{path} writes the organization's domain out: `{text}`; write `{}` for \
                     `{domain}`, so a copy renders the organization it lands in (CC-83)",
                    text.replace(domain, "{orgDomain}")
                ));
            }
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}[{index}]"), domain, found);
                }
            }
            serde_json::Value::Object(map) => {
                for (key, item) in map {
                    walk(item, &format!("{path}.{key}"), domain, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(value, "spec", org_domain, &mut found);
    found
}

/// One fetch of a DataSource on the project's runner, or why there was none (MF-39).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// Messages after the format split; one for a JSON document, one per element of an array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    /// The first record as data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<serde_json::Value>,
    /// Why the source was not fetched: a credential, no runner, a feed that did not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

impl Probe {
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            skipped: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// Executes a dry run for a candidate manifest (MF-13, R17).
pub async fn execute_dry_run(
    identity: &crate::auth::session::Identity,
    state: &crate::state::AppState,
    project: &str,
    manifest: serde_json::Value,
) -> Result<DryRunResult, ApiError> {
    let kind = manifest
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("manifest must specify 'kind'".to_string()))?;
    let kind_info = crate::resource::by_kind(kind)
        .ok_or_else(|| ApiError::NotFound(format!("kind '{kind}' is unknown")))?;

    let name = manifest
        .pointer("/metadata/name")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let op = match name
        .as_deref()
        .and_then(|n| state.mirror.get(project, kind, n))
    {
        Some(_) => crate::change::Operation::Update,
        None => crate::change::Operation::Create,
    };

    let user = crate::auth::CurrentUser(crate::auth::Session {
        identity: identity.clone(),
        expires_at: i64::MAX,
        issued_at: 0,
        id_token: String::new(),
        access_expires_at: i64::MAX,
        refresh_token: None,
    });

    let response = crate::api::mutate::propose(
        &user,
        state,
        project,
        kind_info.plural,
        name.as_deref(),
        op,
        true,
        manifest,
    )
    .await?;

    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to read dry-run response: {e}")))?;
    serde_json::from_slice::<DryRunResult>(&bytes)
        .map_err(|e| ApiError::Internal(format!("failed to deserialize dry-run result: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_dry_run_plans_sandbox_space() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let identity = crate::auth::session::Identity {
            subject: "sub-123".into(),
            username: "demo.developer".into(),
            email: Some("demo@example.com".into()),
            name: Some("Demo Developer".into()),
            roles: vec![],
            groups: vec!["portal-approver".into()],
        };
        let payload = serde_json::json!({
            "apiVersion": crate::resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility",
                "namespace": "ovzdusie"
            },
            "spec": {
                "isSandbox": true
            }
        });
        let res = execute_dry_run(&identity, &state, "ovzdusie", payload)
            .await
            .expect("dry run executes");
        assert!(res.valid);
        assert_eq!(res.lane, Lane::Green);
    }

    #[test]
    fn is_dry_run_none_is_false() {
        let q = DryRunQuery {
            workspace: None,
            dry_run: None,
        };
        assert!(!is_dry_run(&q).expect("query should succeed"));
    }

    #[test]
    fn is_dry_run_all_is_true() {
        let q = DryRunQuery {
            workspace: None,
            dry_run: Some("All".to_string()),
        };
        assert!(is_dry_run(&q).expect("query should succeed"));
    }

    #[test]
    fn is_dry_run_other_value_is_rejected_with_bad_request() {
        for candidate in ["true", "all", "1", ""] {
            let q = DryRunQuery {
                workspace: None,
                dry_run: Some(candidate.to_string()),
            };
            let err = is_dry_run(&q).expect_err("should reject unsupported values");
            match err {
                ApiError::BadRequest(msg) => {
                    assert!(
                        msg.contains("All"),
                        "error message should name 'All' as accepted value, got: {msg}"
                    );
                }
                other => panic!("expected ApiError::BadRequest, got {other:?}"),
            }
        }
    }
}
