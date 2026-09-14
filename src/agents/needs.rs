//! Validates application generation dataNeeds against endpoint projection and user grants (AP-44).

use crate::agents::endpoints::{self, RunEndpoint};
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::store::Mirror;

/// Every need checked against the endpoint it belongs to (AP-44): the first of `run_endpoints`
/// whose context space is the need's, the primary otherwise.
pub fn validate_data_needs(
    mirror: &Mirror,
    project: &str,
    run_endpoints: &[RunEndpoint],
    visibility: &str,
    data_needs: &[serde_json::Value],
    _user: &CurrentUser,
) -> Result<bool, ApiError> {
    if visibility == "public" {
        return Err(ApiError::BadRequest(
            "public visibility is refused for agent-generated applications (AP-42)".to_string(),
        ));
    }

    if data_needs.is_empty() {
        return Err(ApiError::BadRequest(
            "dataNeeds must not be empty (AP-44)".to_string(),
        ));
    }

    // The hidden attributes of each endpoint, aligned with `run_endpoints`.
    let mut hidden: Vec<Vec<String>> = Vec::new();
    for endpoint in run_endpoints {
        let envelope = mirror
            .get(project, "Endpoint", &endpoint.name)
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "endpoint '{}' not found in project '{project}'",
                    endpoint.name
                ))
            })?;
        hidden.push(
            envelope
                .spec
                .get("projection")
                .and_then(|p| p.get("hiddenAttributes"))
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        );
    }

    let mut violations = Vec::new();
    let mut allows_write = false;

    for (idx, need) in data_needs.iter().enumerate() {
        // The shape first: `spec.dataNeeds` of the App this run publishes is this value
        // verbatim, so a need the `App` kind cannot parse is a run that can never be published.
        // Failing here names the field while the person is still on the form (AP-44, CC-24).
        if let Err(error) = serde_json::from_value::<jc_core::kinds::DataNeed>(need.clone()) {
            violations.push(format!("dataNeeds[{idx}]: {error}"));
            continue;
        }

        let types = need
            .get("types")
            .and_then(|t| t.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        if types.is_empty() {
            violations.push(format!(
                "dataNeeds[{idx}].types: at least one type required"
            ));
        }

        let attrs = need
            .get("attrs")
            .and_then(|t| t.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        let at = endpoints::of_need(run_endpoints, need);
        let (Some(endpoint), Some(hidden_attrs)) = (run_endpoints.get(at), hidden.get(at)) else {
            continue;
        };
        for attr in attrs {
            if hidden_attrs.iter().any(|h| h == attr) {
                violations.push(format!(
                    "attribute '{attr}' is hidden by endpoint '{}'",
                    endpoint.name
                ));
            }
        }

        let operations = need
            .get("operations")
            .and_then(|t| t.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        for op in operations {
            if matches!(
                op,
                "createEntity" | "updateAttrs" | "updateEntity" | "deleteEntity" | "upsertBatch"
            ) {
                allows_write = true;
            }
        }
    }

    if !violations.is_empty() {
        return Err(ApiError::Invalid {
            detail: "declared dataNeeds exceed what endpoint publishes (AP-44)".to_string(),
            errors: violations,
        });
    }

    Ok(allows_write)
}
