//! Keycloak OIDC authorization code flow with PKCE (CC-40, I1, I4).
//!
//! The portal is a confidential client: the code exchange happens server side and only an
//! encrypted session cookie reaches the browser. No token is ever written to a log.

use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, PrivateCookieJar};
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthenticationFlow, CoreClaimName, CoreClaimType, CoreClient,
    CoreClientAuthMethod, CoreGrantType, CoreIdToken, CoreJsonWebKey,
    CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
    CoreResponseType, CoreSubjectIdentifierType,
};
use openidconnect::{
    AdditionalProviderMetadata, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, ProviderMetadata, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::auth::csrf;
use crate::auth::session::{self, secure_cookie, Identity, Session, FLOW_COOKIE};
use crate::config::OidcConfig;
use crate::error::ApiError;
use crate::state::AppState;

/// RP-initiated logout is an OIDC extension, so `end_session_endpoint` is additional metadata.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogoutMetadata {
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
}

impl AdditionalProviderMetadata for LogoutMetadata {}

type PortalProviderMetadata = ProviderMetadata<
    LogoutMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

type PortalClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// How long a login may stay in flight before the PKCE verifier cookie expires.
const FLOW_TTL_SECS: i64 = 600;
/// Session lifetime when the token response does not state one.
const DEFAULT_SESSION_TTL_SECS: i64 = 3600;

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("cannot build the OIDC http client: {0}")]
    HttpClient(String),
    #[error("invalid issuer or redirect url: {0}")]
    Url(String),
    #[error("OIDC discovery against the realm failed: {0}")]
    Discovery(String),
}

/// The discovered Keycloak realm plus the http client used for every server-to-server call.
pub struct OidcClient {
    client: PortalClient,
    http: reqwest::Client,
    end_session_endpoint: Option<Url>,
}

impl OidcClient {
    pub async fn discover(config: &OidcConfig, redirect_uri: &str) -> Result<Self, OidcError> {
        // No redirects: an authorization server must never bounce a token request elsewhere.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| OidcError::HttpClient(e.to_string()))?;

        // A trailing slash makes the discovered `issuer` claim mismatch; Keycloak publishes none.
        let issuer = IssuerUrl::new(config.issuer.as_str().trim_end_matches('/').to_string())
            .map_err(|e| OidcError::Url(e.to_string()))?;
        let metadata = PortalProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(|e| OidcError::Discovery(e.to_string()))?;
        let end_session_endpoint = metadata
            .additional_metadata()
            .end_session_endpoint
            .as_ref()
            .and_then(|raw| Url::parse(raw).ok());

        let redirect = RedirectUrl::new(redirect_uri.to_string())
            .map_err(|e| OidcError::Url(e.to_string()))?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id.clone()),
            Some(ClientSecret::new(config.client_secret().to_string())),
        )
        .set_redirect_uri(redirect);

        Ok(Self {
            client,
            http,
            end_session_endpoint,
        })
    }
}

/// What the login handler parks in an encrypted cookie until Keycloak redirects back.
#[derive(Serialize, Deserialize)]
struct FlowState {
    pkce_verifier: String,
    csrf_state: String,
    nonce: String,
    /// Where to send the browser afterwards; always a path on this portal.
    redirect_to: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Optional in-app path to return to after the login.
    #[serde(default)]
    pub redirect_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BackChannelLogoutForm {
    pub logout_token: String,
}

/// Only same-origin paths are accepted, so an open redirect cannot be smuggled through login.
fn safe_redirect(candidate: Option<String>) -> String {
    match candidate {
        Some(path) if path.starts_with('/') && !path.starts_with("//") => path,
        _ => "/".to_string(),
    }
}

fn oidc(state: &AppState) -> Result<&OidcClient, ApiError> {
    state
        .oidc
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no identity provider is configured".into()))
}

/// `GET /api/v1/auth/login` — starts the authorization code flow with PKCE.
pub async fn login(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
    jar: PrivateCookieJar,
) -> Result<Response, ApiError> {
    let client = oidc(&state)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_state, nonce) = client
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(challenge)
        .url();

    let flow = FlowState {
        pkce_verifier: verifier.into_secret(),
        csrf_state: csrf_state.secret().clone(),
        nonce: nonce.secret().clone(),
        redirect_to: safe_redirect(query.redirect_to),
    };
    let value = serde_json::to_string(&flow)
        .map_err(|e| ApiError::Internal(format!("serialize oidc flow: {e}")))?;
    let jar = jar.add(secure_cookie(FLOW_COOKIE, value, FLOW_TTL_SECS));

    Ok((jar, Redirect::to(auth_url.as_str())).into_response())
}

/// `GET /api/v1/auth/callback` — validates state and nonce, exchanges the code, mints the session.
pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
    jar: PrivateCookieJar,
    cookies: CookieJar,
) -> Result<Response, ApiError> {
    let client = oidc(&state)?;

    if let Some(error) = query.error {
        // The description is provider text, not a token; safe to echo back.
        let detail = query.error_description.unwrap_or_else(|| error.clone());
        return Err(ApiError::BadRequest(format!(
            "identity provider rejected the login: {detail}"
        )));
    }

    let flow_cookie = jar
        .get(FLOW_COOKIE)
        .ok_or_else(|| ApiError::BadRequest("no login is in flight".into()))?;
    let flow: FlowState = serde_json::from_str(flow_cookie.value())
        .map_err(|_| ApiError::BadRequest("the login cookie is unreadable".into()))?;
    let jar = jar.remove(Cookie::build(FLOW_COOKIE).path("/").build());

    let presented_state = query
        .state
        .ok_or_else(|| ApiError::BadRequest("missing state".into()))?;
    if presented_state != flow.csrf_state {
        return Err(ApiError::BadRequest(
            "state does not match the login that was started".into(),
        ));
    }
    let code = query
        .code
        .ok_or_else(|| ApiError::BadRequest("missing code".into()))?;

    let token_response = client
        .client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| ApiError::Internal(format!("token endpoint is not configured: {e}")))?
        .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier))
        .request_async(&client.http)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "code exchange failed");
            ApiError::BadRequest("the authorization code could not be exchanged".into())
        })?;

    let id_token = token_response
        .id_token()
        .ok_or_else(|| ApiError::BadRequest("the identity provider returned no id_token".into()))?;
    let nonce = Nonce::new(flow.nonce);
    let claims = id_token
        .claims(&client.client.id_token_verifier(), &nonce)
        .map_err(|e| {
            tracing::warn!(error = %e, "id_token verification failed");
            ApiError::BadRequest("the id_token did not verify".into())
        })?;

    let issued_at = session::now_unix();
    let ttl = token_response
        .expires_in()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(DEFAULT_SESSION_TTL_SECS);
    let identity = Identity {
        subject: claims.subject().to_string(),
        username: claims
            .preferred_username()
            .map(|u| u.to_string())
            .unwrap_or_else(|| claims.subject().to_string()),
        email: claims.email().map(|e| e.to_string()),
        name: claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.to_string()),
    };
    let session = Session {
        identity,
        expires_at: issued_at + ttl,
        issued_at,
        id_token: id_token.to_string(),
    };
    tracing::info!(subject = %session.identity.subject, "portal login");

    let jar = session::store(jar, &session)?;
    let (cookies, _token) = csrf::issue(cookies);
    Ok((jar, cookies, Redirect::to(&flow.redirect_to)).into_response())
}

/// `GET /api/v1/auth/me` — who is signed in.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "The signed-in identity", body = Identity),
        (status = 401, description = "No live session", body = crate::error::ProblemDetails)
    )
)]
pub async fn me(user: crate::auth::CurrentUser) -> Json<Identity> {
    Json(user.0.identity)
}

/// `POST /api/v1/auth/logout` — clears the session and hands the browser to Keycloak.
pub async fn logout(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
    cookies: CookieJar,
) -> Result<Response, ApiError> {
    let session = session::load(&jar);
    let jar = session::clear(jar);
    let cookies = cookies.remove(Cookie::build(csrf::CSRF_COOKIE).path("/").build());

    let target = state
        .oidc
        .as_deref()
        .and_then(|client| client.end_session_endpoint.clone())
        .map(|mut url| {
            {
                let mut params = url.query_pairs_mut();
                params.append_pair(
                    "post_logout_redirect_uri",
                    state.config.public_base_url.as_str(),
                );
                if let Some(ref session) = session {
                    params.append_pair("id_token_hint", &session.id_token);
                }
                if let Some(oidc) = state.config.oidc.as_ref() {
                    params.append_pair("client_id", &oidc.client_id);
                }
            }
            url.to_string()
        })
        .unwrap_or_else(|| state.config.public_base_url.to_string());

    Ok((jar, cookies, Redirect::to(&target)).into_response())
}

/// `POST /api/v1/auth/backchannel-logout` — the Keycloak back-channel logout endpoint.
///
/// The logout token is verified against the realm keys before anything is revoked.
pub async fn backchannel_logout(
    State(state): State<AppState>,
    Form(form): Form<BackChannelLogoutForm>,
) -> Result<Response, ApiError> {
    let client = oidc(&state)?;
    let token: CoreIdToken = form
        .logout_token
        .parse()
        .map_err(|_| ApiError::BadRequest("the logout token is not a JWT".into()))?;
    let claims = token
        .claims(&client.client.id_token_verifier(), |_: Option<&Nonce>| {
            Ok(())
        })
        .map_err(|e| {
            tracing::warn!(error = %e, "logout token verification failed");
            ApiError::BadRequest("the logout token did not verify".into())
        })?;

    state.revoke_subject(claims.subject().as_str(), session::now_unix());
    tracing::info!(subject = %claims.subject().as_str(), "back-channel logout");
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/me", get(me))
        .route("/auth/logout", post(logout))
        .route("/auth/backchannel-logout", post(backchannel_logout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_same_origin_paths_are_accepted_as_a_return_target() {
        assert_eq!(
            safe_redirect(Some("/projects/ovzdusie".into())),
            "/projects/ovzdusie"
        );
        assert_eq!(safe_redirect(Some("//evil.example".into())), "/");
        assert_eq!(safe_redirect(Some("https://evil.example".into())), "/");
        assert_eq!(safe_redirect(None), "/");
    }
}
