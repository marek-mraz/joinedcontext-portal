//! Validates application generation dataNeeds against endpoint projection and user grants (AP-44).

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::store::Mirror;

pub fn validate_data_needs(
    mirror: &Mirror,
    project: &str,
    endpoint_name: &str,
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

    let endpoint_envelope = mirror
        .get(project, "Endpoint", endpoint_name)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "endpoint '{endpoint_name}' not found in project '{project}'"
            ))
        })?;

    let mut violations = Vec::new();
    let mut allows_write = false;

    let hidden_attrs: Vec<String> = endpoint_envelope
        .spec
        .get("projection")
        .and_then(|p| p.get("hiddenAttributes"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

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

        for attr in attrs {
            if hidden_attrs.iter().any(|h| h == attr) {
                violations.push(format!(
                    "attribute '{attr}' is hidden by endpoint '{endpoint_name}'"
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
