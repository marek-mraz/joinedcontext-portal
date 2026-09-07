//! API keys of a ServiceAccount (T-0189, PF-34, PF-36, PF-37, PF-38, PF-40).
//!
//! The manifest declares that a credential exists; it can never hold the credential itself
//! (jc-core's `Credential` has nowhere to put a secret). These routes are the other half: they
//! mint the key, hand the raw token to the caller exactly once, and keep an Argon2id hash of it
//! so that a copy of the database is not a set of working credentials.
//!
//! Everything that decides is in the manifest: who owns the account, which credentials it
//! declares, and when they expire. The database only remembers the keys behind them.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::Argon2;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::db::{self, KeyRow};
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// Default and ceiling of the window in which a rotated key and its successor both work (PF-38).
const DEFAULT_OVERLAP_HOURS: i64 = 24;
const MAX_OVERLAP_HOURS: i64 = 168;

/// Bytes of randomness behind the two halves of a token: 8 for the key id the gateway resolves
/// an account by, 32 for the secret it verifies.
const KEY_ID_BYTES: usize = 8;
const SECRET_BYTES: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MintRequest {
    /// Name of the `api-key` credential in the manifest this key belongs to.
    pub credential: String,
    /// Overrides the credential's own expiry; absent means the manifest's, or never.
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RotateRequest {
    /// How long the rotated key keeps working beside its successor, in hours (PF-38).
    #[serde(default)]
    pub overlap_hours: Option<i64>,
}

/// A minted key. The only place a raw token ever appears (PF-36).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MintedKey {
    pub key_id: String,
    /// `jc_{keyId}_{secret}`, shown once and never recoverable (PF-37).
    pub token: String,
    pub credential: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// One key as everyone else ever sees it: what an operator decides on, and nothing that opens
/// a door.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    pub key_id: String,
    pub credential: String,
    pub created_at: String,
    pub created_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct KeyList {
    pub items: Vec<KeyInfo>,
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl From<KeyRow> for KeyInfo {
    fn from(row: KeyRow) -> Self {
        Self {
            key_id: row.key_id,
            credential: row.credential,
            created_at: rfc3339(row.created_at),
            created_by: row.created_by,
            expires_at: row.expires_at.map(rfc3339),
            last_used_at: row.last_used_at.map(rfc3339),
            revoked_at: row.revoked_at.map(rfc3339),
        }
    }
}

/// The declared `api-key` credentials of one ServiceAccount manifest, with their expiries.
///
/// Read from the untyped spec rather than through jc-core's typed kind: the mirror holds
/// manifests as they are in Git, and a manifest that is malformed in some other field must not
/// stop an operator from revoking a key.
fn api_key_credential(spec: &serde_json::Value, name: &str) -> Option<CredentialInfo> {
    spec.get("credentials")?
        .as_array()?
        .iter()
        .find(|credential| {
            credential.get("name").and_then(|v| v.as_str()) == Some(name)
                && credential.get("kind").and_then(|v| v.as_str()) == Some("api-key")
        })
        .map(|credential| CredentialInfo {
            expires_at: credential
                .get("expiresAt")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
}

struct CredentialInfo {
    expires_at: Option<String>,
}

/// The account, once the caller has been shown to be allowed to manage its keys.
///
/// An account the caller may not manage answers exactly like an account that does not exist: a
/// 404 that never says which of the two it was (R20).
fn admit(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    account: &str,
) -> Result<serde_json::Value, ApiError> {
    let not_found = || {
        ApiError::NotFound(format!(
            "ServiceAccount '{account}' not found in project '{project}'"
        ))
    };
    if !is_dns1123(project) || !is_dns1123(account) {
        return Err(not_found());
    }
    let envelope = state
        .mirror
        .get(project, "ServiceAccount", account)
        .ok_or_else(not_found)?;

    let identity = &user.0.identity;
    let owner = envelope
        .spec
        .get("owner")
        .and_then(|owner| owner.get("user"))
        .and_then(|user| user.as_str())
        .unwrap_or_default();
    let is_owner = !owner.is_empty()
        && (owner == identity.username || Some(owner) == identity.email.as_deref());
    let is_approver = crate::permissions::for_request(state, identity, project)
        .check("ServiceAccount", jc_core::kinds::Verb::Propose, None)
        .is_ok();
    if !is_owner && !is_approver {
        return Err(not_found());
    }
    Ok(envelope.spec)
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state
        .db
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("no database is configured for API keys".into()))
}

fn db_error(err: sqlx::Error) -> ApiError {
    tracing::error!(error = %err, "service account key database call failed");
    ApiError::Internal("the key database did not answer".into())
}

fn parse_expiry(raw: &str) -> Result<OffsetDateTime, ApiError> {
    OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).map_err(|e| {
        ApiError::BadRequest(format!("expiresAt '{raw}' is not an RFC 3339 time: {e}"))
    })
}

/// Mints a token and the row that outlives it. The secret is returned to the caller and dropped
/// here; what stays is its Argon2id hash (PF-36).
fn mint(
    project: &str,
    account: &str,
    credential: &str,
    created_by: &str,
    expires_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Result<(MintedKey, KeyRow), ApiError> {
    let mut id_bytes = [0u8; KEY_ID_BYTES];
    let mut secret_bytes = [0u8; SECRET_BYTES];
    OsRng.fill_bytes(&mut id_bytes);
    OsRng.fill_bytes(&mut secret_bytes);

    let key_id = id_bytes.iter().fold(String::new(), |mut acc, byte| {
        use std::fmt::Write;
        let _ = write!(acc, "{byte:02x}");
        acc
    });
    let secret = URL_SAFE_NO_PAD.encode(secret_bytes);

    let salt = SaltString::generate(&mut OsRng);
    let secret_hash = Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map_err(|err| {
            tracing::error!(error = %err, "argon2 hashing failed");
            ApiError::Internal("the key could not be hashed".into())
        })?
        .to_string();

    let row = KeyRow {
        key_id: key_id.clone(),
        project: project.to_string(),
        account: account.to_string(),
        credential: credential.to_string(),
        secret_hash,
        created_at: now,
        created_by: created_by.to_string(),
        expires_at,
        last_used_at: None,
        revoked_at: None,
    };
    let minted = MintedKey {
        token: format!("jc_{key_id}_{secret}"),
        key_id,
        credential: credential.to_string(),
        expires_at: expires_at.map(rfc3339),
    };
    Ok((minted, row))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/serviceaccounts/{name}/keys",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "ServiceAccount name"),
    ),
    responses(
        (status = 200, description = "The account's keys, metadata only", body = KeyList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such account, or none the caller may manage", body = ProblemDetails),
        (status = 503, description = "No key database configured", body = ProblemDetails)
    )
)]
pub async fn list_keys(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<KeyList>, ApiError> {
    admit(&state, &user, &project, &name)?;
    let rows = db::list_keys(pool(&state)?, &project, &name)
        .await
        .map_err(db_error)?;
    Ok(Json(KeyList {
        items: rows.into_iter().map(KeyInfo::from).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/serviceaccounts/{name}/keys",
    tag = "access",
    request_body = inline(serde_json::Value),
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "ServiceAccount name"),
    ),
    responses(
        (status = 201, description = "The minted key; the only answer that carries a token", body = MintedKey),
        (status = 400, description = "No such api-key credential, or an expiry in the past", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such account, or none the caller may manage", body = ProblemDetails),
        (status = 503, description = "No key database configured", body = ProblemDetails)
    )
)]
pub async fn create_key(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    body: Bytes,
) -> Result<(StatusCode, Json<MintedKey>), ApiError> {
    let spec = admit(&state, &user, &project, &name)?;
    let request: MintRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("request body is invalid: {e}")))?;

    // The manifest is the authority on which credentials exist; a key for an undeclared
    // credential would be a credential nobody reviewed (PF-34).
    let credential = api_key_credential(&spec, &request.credential).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "'{}' is not an api-key credential of this ServiceAccount",
            request.credential
        ))
    })?;

    let now = OffsetDateTime::now_utc();
    let expires_at = match request.expires_at.or(credential.expires_at) {
        Some(raw) => {
            let at = parse_expiry(&raw)?;
            if at <= now {
                return Err(ApiError::BadRequest(format!(
                    "expiresAt '{raw}' is in the past"
                )));
            }
            Some(at)
        }
        None => None,
    };

    let (minted, row) = mint(
        &project,
        &name,
        &request.credential,
        &user.0.identity.username,
        expires_at,
        now,
    )?;
    db::insert_key(pool(&state)?, &row)
        .await
        .map_err(db_error)?;
    // The key id, never the token: a log line is not a place a credential may end up (PF-36).
    tracing::info!(project = %project, account = %name, key_id = %row.key_id, "api key minted");
    Ok((StatusCode::CREATED, Json(minted)))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/serviceaccounts/{name}/keys/{keyId}/rotate",
    tag = "access",
    request_body = inline(serde_json::Value),
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "ServiceAccount name"),
        ("keyId" = String, Path, description = "Key to rotate"),
    ),
    responses(
        (status = 201, description = "The successor key; the predecessor works until the overlap ends", body = MintedKey),
        (status = 400, description = "An overlap window outside 0…168 hours", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such account or key", body = ProblemDetails),
        (status = 503, description = "No key database configured", body = ProblemDetails)
    )
)]
pub async fn rotate_key(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name, key_id)): Path<(String, String, String)>,
    body: Bytes,
) -> Result<(StatusCode, Json<MintedKey>), ApiError> {
    admit(&state, &user, &project, &name)?;
    // An empty body is the default rotation, so it must not be a parse error.
    let request: RotateRequest = if body.is_empty() {
        RotateRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("request body is invalid: {e}")))?
    };
    let overlap = request.overlap_hours.unwrap_or(DEFAULT_OVERLAP_HOURS);
    if !(0..=MAX_OVERLAP_HOURS).contains(&overlap) {
        return Err(ApiError::BadRequest(format!(
            "overlapHours must be between 0 and {MAX_OVERLAP_HOURS}"
        )));
    }

    let pool = pool(&state)?;
    let old = db::get_key(pool, &project, &name, &key_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| ApiError::NotFound(format!("key '{key_id}' not found")))?;
    if old.revoked_at.is_some() {
        return Err(ApiError::Conflict(
            "a revoked key is not rotated; mint a new one".into(),
        ));
    }

    let now = OffsetDateTime::now_utc();
    let (minted, row) = mint(
        &project,
        &name,
        &old.credential,
        &user.0.identity.username,
        old.expires_at,
        now,
    )?;
    db::insert_key(pool, &row).await.map_err(db_error)?;
    // The successor exists before the predecessor is given an end: a failure between the two
    // leaves two working keys, never none.
    db::expire_key_at(pool, &old.key_id, now + time::Duration::hours(overlap))
        .await
        .map_err(db_error)?;
    tracing::info!(
        project = %project, account = %name, key_id = %row.key_id, replaces = %old.key_id,
        overlap_hours = overlap, "api key rotated"
    );
    Ok((StatusCode::CREATED, Json(minted)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/serviceaccounts/{name}/keys/{keyId}",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "ServiceAccount name"),
        ("keyId" = String, Path, description = "Key to revoke"),
    ),
    responses(
        (status = 204, description = "Revoked; the gateway refuses it on the next request"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such account or key", body = ProblemDetails),
        (status = 503, description = "No key database configured", body = ProblemDetails)
    )
)]
pub async fn revoke_key(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name, key_id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    admit(&state, &user, &project, &name)?;
    let revoked = db::revoke_key(
        pool(&state)?,
        &project,
        &name,
        &key_id,
        OffsetDateTime::now_utc(),
    )
    .await
    .map_err(db_error)?;
    if !revoked {
        return Err(ApiError::NotFound(format!("key '{key_id}' not found")));
    }
    tracing::info!(project = %project, account = %name, key_id = %key_id, "api key revoked");
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/serviceaccounts/{name}/keys",
            get(list_keys).post(create_key),
        )
        .route(
            "/projects/{project}/serviceaccounts/{name}/keys/{keyId}/rotate",
            post(rotate_key),
        )
        .route(
            "/projects/{project}/serviceaccounts/{name}/keys/{keyId}",
            delete(revoke_key),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::{PasswordHash, PasswordVerifier};

    fn spec() -> serde_json::Value {
        serde_json::json!({
            "owner": { "user": "jana.kovacova" },
            "purpose": "VendorX pushes ParkingSpot updates",
            "credentials": [
                { "kind": "oauth-client", "name": "main" },
                { "kind": "api-key", "name": "legacy-push", "expiresAt": "2027-03-01T00:00:00Z" }
            ]
        })
    }

    #[test]
    fn only_declared_api_key_credentials_are_found() {
        assert!(api_key_credential(&spec(), "legacy-push").is_some());
        assert!(
            api_key_credential(&spec(), "main").is_none(),
            "an oauth-client credential has no API key here"
        );
        assert!(api_key_credential(&spec(), "invented").is_none());
        assert_eq!(
            api_key_credential(&spec(), "legacy-push")
                .and_then(|c| c.expires_at)
                .as_deref(),
            Some("2027-03-01T00:00:00Z"),
            "the manifest's expiry is the default"
        );
    }

    #[test]
    fn a_minted_token_verifies_against_the_stored_hash_and_the_secret_is_not_in_the_row() {
        let now = OffsetDateTime::now_utc();
        let (minted, row) = mint("bb", "vendorx", "legacy-push", "jana", None, now).expect("mint");

        let rest = minted
            .token
            .strip_prefix("jc_")
            .expect("every token carries the platform prefix (PF-37)");
        let (key_id, secret) = rest.split_once('_').expect("jc_{keyId}_{secret}");
        assert_eq!(key_id, row.key_id);
        assert_eq!(key_id.len(), KEY_ID_BYTES * 2);
        assert!(key_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));

        let hash = PasswordHash::new(&row.secret_hash).expect("a PHC string");
        assert_eq!(hash.algorithm.as_str(), "argon2id", "PF-36 names Argon2id");
        Argon2::default()
            .verify_password(secret.as_bytes(), &hash)
            .expect("the gateway verifies this token against this row");

        assert!(
            !row.secret_hash.contains(secret),
            "the row must not carry the secret it hashes"
        );
        assert!(
            !format!("{row:?}").contains(secret),
            "and neither must its Debug output"
        );
    }

    #[test]
    fn two_mints_never_collide() {
        let now = OffsetDateTime::now_utc();
        let (first, _) = mint("bb", "a", "k", "jana", None, now).expect("mint");
        let (second, _) = mint("bb", "a", "k", "jana", None, now).expect("mint");
        assert_ne!(first.key_id, second.key_id);
        assert_ne!(first.token, second.token);
    }

    #[test]
    fn a_listed_key_has_no_token_field_at_all() {
        let now = OffsetDateTime::now_utc();
        let (_, row) = mint("bb", "a", "legacy-push", "jana", Some(now), now).expect("mint");
        let hash = row.secret_hash.clone();
        let json = serde_json::to_string(&KeyInfo::from(row)).expect("serialise");
        assert!(!json.contains("token"), "no token member: {json}");
        assert!(!json.contains(&hash), "no hash either: {json}");
        assert!(json.contains("legacy-push"));
    }
}
