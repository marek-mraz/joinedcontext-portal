//! `GET`/`PUT /api/v1/preferences`: how the signed-in person likes the UI (UI-09, UI-10).
//!
//! Scoped by the session's Keycloak `sub` (CC-40) and nothing else: there is no path parameter,
//! so no caller can name another person's row.

use axum::body::Bytes;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::db;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The browser's saved dashboard state, per dashboard name. Opaque to the Portal, so it is capped
/// rather than understood: a saved layout is small, anything bigger is not a layout.
const MAX_LAYOUTS_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Preferences {
    /// `light`, `dark` or `system`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// ISO 639-1 language code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// The project the shell opens on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project: Option<String>,
    /// Free JSON per dashboard name, the browser's own saved state.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub dashboard_layouts: BTreeMap<String, serde_json::Value>,
}

impl Preferences {
    /// The few fields the Portal understands are checked; the rest is only bounded.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(theme) = &self.theme {
            if !matches!(theme.as_str(), "light" | "dark" | "system") {
                return Err(format!("theme '{theme}' is not light, dark or system"));
            }
        }
        if let Some(locale) = &self.locale {
            jc_core::names::validate_locale(locale).map_err(|e| e.to_string())?;
        }
        if let Some(project) = &self.default_project {
            if !is_dns1123(project) {
                return Err(format!("defaultProject '{project}' is not a DNS-1123 name"));
            }
        }
        for name in self.dashboard_layouts.keys() {
            if !is_dns1123(name) {
                return Err(format!(
                    "dashboardLayouts key '{name}' is not a dashboard name"
                ));
            }
        }
        let layouts_len = serde_json::to_vec(&self.dashboard_layouts)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if layouts_len > MAX_LAYOUTS_BYTES {
            return Err(format!(
                "dashboardLayouts is {layouts_len} bytes, the limit is {MAX_LAYOUTS_BYTES}"
            ));
        }
        Ok(())
    }
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state
        .db
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("no preferences database is configured".into()))
}

fn db_error(err: sqlx::Error) -> ApiError {
    // The DSN, the SQL and the driver's wording stay in the log (TS-09).
    tracing::error!(error = %err, "preferences database call failed");
    ApiError::Internal("the preferences database did not answer".into())
}

#[utoipa::path(
    get,
    path = "/api/v1/preferences",
    tag = "preferences",
    responses(
        (status = 200, description = "The caller's preferences, empty before the first save", body = Preferences),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "No preferences database configured", body = ProblemDetails)
    )
)]
pub async fn get_preferences(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Preferences>, ApiError> {
    let stored = db::load_preferences(pool(&state)?, &user.0.identity.subject)
        .await
        .map_err(db_error)?;
    // A row written by an older Portal may hold a field this one no longer knows; it is dropped
    // rather than turned into a 500 on the caller's own settings.
    let preferences = stored
        .and_then(|value| serde_json::from_value::<Preferences>(value).ok())
        .unwrap_or_default();
    Ok(Json(preferences))
}

#[utoipa::path(
    put,
    path = "/api/v1/preferences",
    tag = "preferences",
    request_body = Preferences,
    responses(
        (status = 200, description = "Stored; the body is what is now saved", body = Preferences),
        (status = 400, description = "A field the Portal understands is invalid", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Missing or mismatched CSRF token", body = ProblemDetails),
        (status = 503, description = "No preferences database configured", body = ProblemDetails)
    )
)]
pub async fn put_preferences(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Preferences>, ApiError> {
    // Parsed by hand, as every write handler here is: axum's `Json` extractor answers a bad body
    // with a plain-text 422, and the contract says problem+json 400 (API/01 sections 2 and 8).
    let preferences: Preferences = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("preferences body is invalid: {e}")))?;
    preferences.validate().map_err(ApiError::BadRequest)?;
    let value = serde_json::to_value(&preferences)
        .map_err(|e| ApiError::Internal(format!("preferences did not serialize: {e}")))?;
    db::save_preferences(pool(&state)?, &user.0.identity.subject, &value)
        .await
        .map_err(db_error)?;
    Ok(Json(preferences))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/preferences", get(get_preferences).put(put_preferences))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(json: serde_json::Value) -> Preferences {
        serde_json::from_value(json).expect("valid shape")
    }

    #[test]
    fn accepts_the_documented_example() {
        let p = prefs(serde_json::json!({
            "theme": "system",
            "locale": "sk",
            "defaultProject": "ovzdusie",
            "dashboardLayouts": { "ovzdusie-prehlad": { "collapsedLegend": true } }
        }));
        assert_eq!(p.validate(), Ok(()));
        assert!(
            Preferences::default().validate().is_ok(),
            "nothing set is fine"
        );
    }

    #[test]
    fn refuses_what_it_understands_and_is_wrong() {
        assert!(prefs(serde_json::json!({ "theme": "sepia" }))
            .validate()
            .is_err());
        assert!(prefs(serde_json::json!({ "locale": "slovak" }))
            .validate()
            .is_err());
        assert!(prefs(serde_json::json!({ "defaultProject": "Ovzdusie" }))
            .validate()
            .is_err());
        assert!(
            prefs(serde_json::json!({ "dashboardLayouts": { "../x": {} } }))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn unknown_fields_are_rejected_at_the_door() {
        let result = serde_json::from_value::<Preferences>(serde_json::json!({ "colour": "red" }));
        assert!(
            result.is_err(),
            "deny_unknown_fields keeps the contract honest"
        );
    }

    #[test]
    fn layouts_are_bounded() {
        let big = "x".repeat(MAX_LAYOUTS_BYTES);
        let p = prefs(serde_json::json!({ "dashboardLayouts": { "d": big } }));
        assert!(
            p.validate().is_err(),
            "a layout larger than the cap is not a layout"
        );
    }
}
