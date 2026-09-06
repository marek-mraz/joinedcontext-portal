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

/// `Display` of a `DiscoveryError` is "Request failed" and the reason lives in its source, so a
/// TLS failure reads exactly like a 404 in the log. This walks the chain and joins it.
fn source_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut cause = error.source();
    while let Some(current) = cause {
        parts.push(current.to_string());
        cause = current.source();
    }
    parts.join(": ")
}

impl OidcClient {
    pub async fn discover(config: &OidcConfig, redirect_uri: &str) -> Result<Self, OidcError> {
        // No redirects: an authorization server must never bounce a token request elsewhere.
        let mut builder = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10));
        // The binary trusts `webpki-roots` and nothing else, so an issuer served by a private
        // CA needs its root handed over explicitly (JC_OIDC_CA_FILE). Added, never swapped in:
        // the public roots stay, and certificate verification is untouched.
        if let Some(pem) = &config.extra_ca_pem {
            // Whole bundle, not `from_pem`: a private CA is usually a chain and the first block
            // alone would not verify. Empty is an error rather than "carry on with the public
            // roots" — rustls drops roots it cannot parse without a word, so a wrong mount would
            // otherwise fail later as a plain handshake error with nothing pointing at the file.
            let roots = reqwest::Certificate::from_pem_bundle(pem)
                .map_err(|e| OidcError::HttpClient(format!("JC_OIDC_CA_FILE: {e}")))?;
            if roots.is_empty() {
                return Err(OidcError::HttpClient(
                    "JC_OIDC_CA_FILE: no PEM certificate in the file".to_string(),
                ));
            }
            for root in roots {
                builder = builder.add_root_certificate(root);
            }
        }
        let http = builder
            .build()
            .map_err(|e| OidcError::HttpClient(e.to_string()))?;

        // A trailing slash makes the discovered `issuer` claim mismatch; Keycloak publishes none.
        let issuer = IssuerUrl::new(config.issuer.as_str().trim_end_matches('/').to_string())
            .map_err(|e| OidcError::Url(e.to_string()))?;
        let metadata = PortalProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(|e| OidcError::Discovery(source_chain(&e)))?;
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
        roles: realm_roles(id_token.to_string().as_str()),
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

/// Keycloak puts realm roles in `realm_access.roles`, which `openidconnect`'s core claim set does
/// not model. Reading them back out of the token's payload is safe here and only here: the caller
/// has already verified this exact token's signature and nonce, so the bytes are trusted. A
/// malformed or role-less token yields an empty list, never an error — roles are display and
/// enablement only (CC-42), and the resource API enforces the real boundary.
fn realm_roles(id_token: &str) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct RealmAccess {
        #[serde(default)]
        roles: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Payload {
        realm_access: Option<RealmAccess>,
    }

    let Some(payload) = id_token.split('.').nth(1) else {
        return Vec::new();
    };
    let Ok(bytes) =
        base64::engine::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload)
    else {
        return Vec::new();
    };
    serde_json::from_slice::<Payload>(&bytes)
        .ok()
        .and_then(|p| p.realm_access)
        .map(|r| r.roles)
        .unwrap_or_default()
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

/// The URL the browser must visit to finish an RP-initiated logout at Keycloak.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LogoutTarget {
    /// Absolute URL: Keycloak's `end_session_endpoint`, or the portal itself when no realm is
    /// configured.
    pub end_session_url: String,
}

/// `POST /api/v1/auth/logout` — clears the session and tells the SPA where to send the browser.
///
/// It answers with JSON rather than a 302 on purpose: the SPA calls this with `fetch`, and a
/// cross-origin redirect to Keycloak is unreadable to it. The browser navigation is the caller's.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 200, description = "Session cleared; navigate to endSessionUrl", body = LogoutTarget),
        (status = 403, description = "Missing or mismatched CSRF token", body = crate::error::ProblemDetails)
    )
)]
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

    Ok((
        jar,
        cookies,
        Json(LogoutTarget {
            end_session_url: target,
        }),
    )
        .into_response())
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

    /// Shaped like `openidconnect::DiscoveryError`: the reason is in the source, not in `Display`.
    #[derive(Debug, thiserror::Error)]
    #[error("Request failed")]
    struct Opaque(#[source] std::io::Error);

    #[test]
    fn the_source_chain_survives_into_the_message() {
        let err = Opaque(std::io::Error::other(
            "invalid peer certificate: UnknownIssuer",
        ));
        assert_eq!(
            source_chain(&err),
            "Request failed: invalid peer certificate: UnknownIssuer"
        );
    }

    fn oidc_config(ca_file: Option<&str>) -> crate::config::OidcConfig {
        crate::config::Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.test/realms/bb".to_string()),
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            "JC_OIDC_CA_FILE" => ca_file.map(str::to_string),
            _ => None,
        })
        .expect("oidc config")
        .oidc
        .expect("oidc present")
    }

    /// The private root has to reach the http client, and the only failure that proves it does so
    /// without a network is an unusable one: a PEM the builder refuses stops discovery before the
    /// first request. Configured but ignored is exactly the bug this guards (T-0350).
    #[tokio::test]
    async fn an_unusable_extra_root_is_refused_by_name() {
        let path =
            std::env::temp_dir().join(format!("jc-portal-bad-ca-{}.pem", std::process::id()));
        std::fs::write(&path, b"not a certificate\n").expect("write pem");
        let config = oidc_config(Some(&path.display().to_string()));
        let err = OidcClient::discover(&config, "https://portal.example.test/api/v1/auth/callback")
            .await
            .map(|_| ())
            .expect_err("a root that cannot be parsed must stop start-up");
        let _ = std::fs::remove_file(&path);
        match err {
            OidcError::HttpClient(reason) => assert!(
                reason.contains("JC_OIDC_CA_FILE"),
                "the operator needs the variable in the message: {reason}"
            ),
            other => panic!("expected the ca file to be refused, got {other}"),
        }
    }
}

#[cfg(test)]
mod realm_roles_tests {
    use super::realm_roles;
    use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine};

    /// Builds a JWT-shaped string around `payload`. Nothing here is signed: `realm_roles` runs
    /// only on a token the caller already verified, so the test exercises the parsing alone.
    fn token(payload: &str) -> String {
        format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#),
            URL_SAFE_NO_PAD.encode(payload.as_bytes()),
            URL_SAFE_NO_PAD.encode(b"not-a-signature"),
        )
    }

    #[test]
    fn reads_realm_access_roles_in_order() {
        let t = token(r#"{"sub":"u1","realm_access":{"roles":["space-editor","portal-viewer"]}}"#);
        assert_eq!(realm_roles(&t), vec!["space-editor", "portal-viewer"]);
    }

    #[test]
    fn missing_or_empty_realm_access_is_no_roles() {
        assert!(realm_roles(&token(r#"{"sub":"u1"}"#)).is_empty());
        assert!(realm_roles(&token(r#"{"sub":"u1","realm_access":{}}"#)).is_empty());
        assert!(realm_roles(&token(r#"{"sub":"u1","realm_access":{"roles":[]}}"#)).is_empty());
    }

    #[test]
    fn a_malformed_token_yields_no_roles_instead_of_an_error() {
        assert!(realm_roles("").is_empty());
        assert!(realm_roles("only-one-segment").is_empty());
        assert!(realm_roles("header.!!!not-base64!!!.sig").is_empty());
        assert!(realm_roles(&token("not json at all")).is_empty());
    }
}
