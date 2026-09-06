//! Live schema preview for the LinkML editor, compiled by Model Tools.
//!
//! Model Tools is a stateless, versioned image (`linkml`, `schema-automator`,
//! `pysmartdatamodels`, the `ngsi_ld_kind` post-processor) that holds no credentials, reads no
//! platform state and may fetch only from the Smart Data Models organisation (DM-10, DM-18).
//! The Portal calls it so the editor never fetches a third-party schema itself, and so the
//! container needs no ingress. CI compiles with the same image version as this preview (DM-19).

use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// How long a compilation may take. The editor asks on every pause in typing, so a slow answer
/// is worse than no answer: the view shows the source without a preview and asks again.
const COMPILE_TIMEOUT: Duration = Duration::from_secs(10);

/// Largest LinkML source the Portal forwards. Model Tools is stateless and shared, so the
/// Portal caps the payload before it reaches it rather than after (DM-18). A schema this size
/// is already far past what an editor session produces.
pub const MAX_REQUEST_BYTES: usize = 512 * 1024;

/// A LinkML source to compile.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct GenerateRequest {
    /// LinkML YAML, exactly as the editor holds it.
    pub source: String,
}

/// A model to import from the Smart Data Models catalogue.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ImportSdmRequest {
    /// Catalogue identifier, `dataModel.Environment/AirQualityObserved`. Never a URL: the
    /// allowlist that limits fetching to the Smart Data Models organisation lives in Model
    /// Tools (DM-10), and the Portal refuses anything a caller could steer.
    pub model: String,
}

/// What one Model Tools run rendered. Every artifact is optional: a version that does not
/// render one omits it, and a source that does not compile yet answers with `errors` filled in
/// and the artifacts absent. A half-written model is the normal state of an editor.
#[derive(Debug, Default, Deserialize, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shacl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    /// The generator version that produced these artifacts, so a preview and a committed
    /// artifact set can be compared (DM-19).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_version: Option<String>,
    /// Compilation messages. Non-empty with no artifacts means the source does not compile.
    #[serde(default)]
    pub errors: Vec<String>,
}

/// A catalogue identifier is `dataModel.<Subject>/<Model>`: two segments of the characters the
/// catalogue itself uses. Anything else — a URL, a scheme, a traversal, a query — is refused
/// before a request is built, so no caller can point the fetch at a host of their choosing.
fn is_catalogue_id(model: &str) -> bool {
    let Some((subject, name)) = model.split_once('/') else {
        return false;
    };
    let segment_ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
            && !s.starts_with('.')
    };
    segment_ok(subject) && segment_ok(name)
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(COMPILE_TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

/// Posts one body to a Model Tools route and reads the artifacts back.
async fn compile(
    state: &AppState,
    route: &str,
    body: &impl Serialize,
) -> Result<Json<Artifacts>, ApiError> {
    let base = state
        .config
        .model_tools_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no model tools service is configured".into()))?;
    let url = format!("{}/{route}", base.trim_end_matches('/'));

    // The URL names an internal service, so the reason is logged and the caller learns only
    // that the compiler is unreachable.
    let unavailable = |what: &str, err: &dyn std::fmt::Display| {
        tracing::warn!(route = %route, error = %err, "model tools {what}");
        ApiError::Unavailable("the model tools service did not answer".into())
    };

    let response = http()
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|err| unavailable("unreachable", &err))?;
    if !response.status().is_success() {
        return Err(unavailable("refused", &response.status()));
    }
    let artifacts = response
        .json::<Artifacts>()
        .await
        .map_err(|err| unavailable("answered unreadably", &err))?;
    Ok(Json(artifacts))
}

#[utoipa::path(
    post,
    path = "/api/v1/tools/generate",
    tag = "tools",
    request_body = GenerateRequest,
    responses(
        (status = 200, description = "Compiled artifacts, or the messages of a source that does not compile", body = Artifacts),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 413, description = "Source larger than the payload limit", body = ProblemDetails),
        (status = 503, description = "No model tools service configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn generate(
    _user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<GenerateRequest>,
) -> Result<Json<Artifacts>, ApiError> {
    compile(&state, "generate", &request).await
}

#[utoipa::path(
    post,
    path = "/api/v1/tools/import-sdm",
    tag = "tools",
    request_body = ImportSdmRequest,
    responses(
        (status = 200, description = "Compiled artifacts of the catalogue model", body = Artifacts),
        (status = 400, description = "Not a Smart Data Models catalogue identifier", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "No model tools service configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn import_sdm(
    _user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<ImportSdmRequest>,
) -> Result<Json<Artifacts>, ApiError> {
    if !is_catalogue_id(&request.model) {
        return Err(ApiError::BadRequest(
            "model must be a Smart Data Models catalogue identifier such as \
             'dataModel.Environment/AirQualityObserved', not a URL"
                .into(),
        ));
    }
    compile(&state, "import-sdm", &request).await
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/generate", post(generate))
        .route("/tools/import-sdm", post(import_sdm))
        // Model Tools is stateless and shared: an oversized source is refused here, before it
        // is read into memory or forwarded (DM-18).
        .layer(axum::extract::DefaultBodyLimit::max(MAX_REQUEST_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalogue_identifier_is_two_plain_segments() {
        assert!(is_catalogue_id("dataModel.Environment/AirQualityObserved"));
        assert!(is_catalogue_id("dataModel.Transportation/Vehicle"));
    }

    #[test]
    fn nothing_a_caller_could_steer_is_an_identifier() {
        for steered in [
            "https://example.org/evil.json",
            "http://169.254.169.254/latest/meta-data",
            "//example.org/x",
            "dataModel.Environment/../../etc/passwd",
            "../secrets/x",
            "dataModel.Environment/Air Quality",
            "dataModel.Environment",
            "",
            "/",
            "dataModel.Environment/",
        ] {
            assert!(
                !is_catalogue_id(steered),
                "{steered} must not pass as a catalogue identifier"
            );
        }
    }

    #[test]
    fn artifacts_of_an_older_tool_version_still_parse() {
        // Every field is optional on purpose: a Model Tools version that renders no OWL must
        // not turn a working preview into a 503.
        let artifacts: Artifacts = serde_json::from_str(r#"{"shacl":"@prefix sh: <> ."}"#)
            .expect("a partial artifact set parses");
        assert_eq!(artifacts.shacl.as_deref(), Some("@prefix sh: <> ."));
        assert!(artifacts.json_schema.is_none());
        assert!(artifacts.errors.is_empty(), "absent errors is not an error");
    }

    #[test]
    fn artifacts_serialize_in_the_documented_camel_case() {
        let artifacts = Artifacts {
            generator_version: Some("linkml-1.8.0".into()),
            ..Artifacts::default()
        };
        let json = serde_json::to_string(&artifacts).expect("serialize");
        assert!(
            json.contains("\"generatorVersion\":\"linkml-1.8.0\""),
            "{json}"
        );
        assert!(!json.contains("jsonSchema"), "absent artifacts are omitted");
    }
}
