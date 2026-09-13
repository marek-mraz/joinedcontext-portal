use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::change::Lane;
use crate::error::ApiError;
use crate::plan::PlanDiff;

#[derive(Debug, Default, Deserialize)]
pub struct DryRunQuery {
    #[serde(default, rename = "dryRun")]
    pub dry_run: Option<String>,
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
    /// What one fetch of an `http` DataSource returned (MF-39); absent for every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_dry_run_none_is_false() {
        let q = DryRunQuery { dry_run: None };
        assert!(!is_dry_run(&q).expect("query should succeed"));
    }

    #[test]
    fn is_dry_run_all_is_true() {
        let q = DryRunQuery {
            dry_run: Some("All".to_string()),
        };
        assert!(is_dry_run(&q).expect("query should succeed"));
    }

    #[test]
    fn is_dry_run_other_value_is_rejected_with_bad_request() {
        for candidate in ["true", "all", "1", ""] {
            let q = DryRunQuery {
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
