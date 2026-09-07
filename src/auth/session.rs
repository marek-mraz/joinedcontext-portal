//! The portal session: an encrypted cookie carrying the signed-in identity (CC-40).

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::state::AppState;

/// Encrypted cookie holding the whole session; nothing about the user is kept server-side.
pub const SESSION_COOKIE: &str = "jc_session";
/// Short-lived encrypted cookie holding the in-flight PKCE verifier, state and nonce.
pub const FLOW_COOKIE: &str = "jc_oidc_flow";
/// Encrypted cookie holding the Keycloak refresh token alone. Its own cookie, not a field of the
/// session one: an id token plus a refresh token would push a single cookie past the 4 KB a
/// browser keeps, and each of them alone stays well under it.
pub const REFRESH_COOKIE: &str = "jc_refresh";
/// How long before the access token expires the next request refreshes it.
pub const REFRESH_LEEWAY_SECS: i64 = 60;

/// Who is signed in. Returned by `GET /api/v1/auth/me` and used by every protected route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Identity {
    /// Keycloak `sub`: stable, opaque, the only durable user key.
    pub subject: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Keycloak realm roles from the ID token's `realm_access.roles` (CC-42). Empty when the
    /// token asserts none. Display and enablement only — the resource API and the forge enforce
    /// the same boundaries independently.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// The session payload. `Debug` redacts the tokens so none reaches a log line.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub identity: Identity,
    /// Unix seconds; the session is over after this instant even if a refresh would still work.
    /// A cookie session sets it from the refresh token's own `exp`, which Keycloak slides with
    /// the SSO session on every refresh; a bearer session sets it from the token's `exp`.
    pub expires_at: i64,
    /// Unix seconds; a back-channel logout invalidates every session issued at or before its mark.
    pub issued_at: i64,
    /// Kept only as `id_token_hint` for RP-initiated logout.
    pub id_token: String,
    /// Unix seconds the access token stops being valid. A cookie session refreshes shortly
    /// before it and is refused past it; a cookie written before this field existed reads as
    /// expired and signs in again once.
    #[serde(default)]
    pub access_expires_at: i64,
    /// The Keycloak refresh token. Not part of the session cookie: it travels in `REFRESH_COOKIE`
    /// and `store`/`load` join the two. `None` for a bearer session.
    #[serde(skip)]
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("identity", &self.identity)
            .field("expires_at", &self.expires_at)
            .field("issued_at", &self.issued_at)
            .field("id_token", &"[redacted]")
            .field("access_expires_at", &self.access_expires_at)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl Session {
    pub fn is_expired(&self, now: i64) -> bool {
        now >= self.expires_at
    }

    /// The access token is gone; only a refresh (or a new login) makes this session usable.
    pub fn access_expired(&self, now: i64) -> bool {
        now >= self.access_expires_at
    }

    /// Inside the leeway before the access token expires, with a refresh token to use.
    pub fn needs_refresh(&self, now: i64) -> bool {
        self.refresh_token.is_some() && now + REFRESH_LEEWAY_SECS >= self.access_expires_at
    }
}

pub fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Builds a cookie with the flags every portal cookie carries: `Secure`, `HttpOnly`,
/// `SameSite=Lax` (the OIDC callback is a top-level GET navigation and must keep its cookie).
pub fn secure_cookie(name: &'static str, value: String, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(max_age_secs.max(0)))
        .build()
}

/// Writes the session into the encrypted jar: the identity cookie and, when the session holds
/// one, the refresh-token cookie beside it, both living as long as the session.
pub fn store(jar: PrivateCookieJar, session: &Session) -> Result<PrivateCookieJar, ApiError> {
    let value = serde_json::to_string(session)
        .map_err(|e| ApiError::Internal(format!("serialize session: {e}")))?;
    let ttl = (session.expires_at - now_unix()).max(0);
    let jar = jar.add(secure_cookie(SESSION_COOKIE, value, ttl));
    Ok(match &session.refresh_token {
        Some(token) => jar.add(secure_cookie(REFRESH_COOKIE, token.clone(), ttl)),
        None => jar.remove(Cookie::build(REFRESH_COOKIE).path("/").build()),
    })
}

/// Reads the session, ignoring an expired or unparseable one. The access token may already be
/// past its time: that is for the refresh middleware and `CurrentUser` to decide.
pub fn load(jar: &PrivateCookieJar) -> Option<Session> {
    let cookie = jar.get(SESSION_COOKIE)?;
    let mut session: Session = serde_json::from_str(cookie.value()).ok()?;
    session.refresh_token = jar
        .get(REFRESH_COOKIE)
        .map(|c| c.value().to_string())
        .filter(|t| !t.is_empty());
    (!session.is_expired(now_unix())).then_some(session)
}

/// Removes the session cookies (sign-out and every failure path).
pub fn clear(jar: PrivateCookieJar) -> PrivateCookieJar {
    jar.remove(Cookie::build(SESSION_COOKIE).path("/").build())
        .remove(Cookie::build(REFRESH_COOKIE).path("/").build())
}

/// Extractor for a protected route: 401 problem+json when there is no live session.
#[derive(Clone)]
pub struct CurrentUser(pub Session);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // A bearer token is a service or a script, verified by the Portal itself; a browser
        // carries the encrypted session cookie. Neither falls back to the other.
        if let Some(header) = parts.headers.get(axum::http::header::AUTHORIZATION) {
            let token = header
                .to_str()
                .ok()
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or(ApiError::Unauthorized)?;
            let verifier = state.bearer.as_ref().ok_or(ApiError::Unauthorized)?;
            let session = verifier.verify(token).await?;
            if state.is_revoked(&session) {
                return Err(ApiError::Unauthorized);
            }
            return Ok(CurrentUser(session));
        }
        // The refresh middleware parks the session it just refreshed in the extensions; the
        // cookie on this request still carries the one before it.
        let session = match parts.extensions.get::<Session>() {
            Some(refreshed) => refreshed.clone(),
            None => {
                let jar =
                    PrivateCookieJar::from_headers(&parts.headers, state.config.cookie_key.clone());
                load(&jar).ok_or(ApiError::Unauthorized)?
            }
        };
        if session.access_expired(now_unix()) || state.is_revoked(&session) {
            return Err(ApiError::Unauthorized);
        }
        Ok(CurrentUser(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use axum_extra::extract::cookie::Key;

    fn sample(expires_in: i64) -> Session {
        let now = now_unix();
        Session {
            identity: Identity {
                subject: "f:1:demo.steward".into(),
                username: "demo.steward".into(),
                email: Some("demo.steward@banskabystrica.sk".into()),
                name: Some("Demo Steward".into()),
                roles: Vec::new(),
            },
            expires_at: now + expires_in,
            issued_at: now,
            // Deliberately not JWT-shaped: a real-looking token literal trips gitleaks in CI.
            id_token: "opaque-id-token-for-tests".into(),
            access_expires_at: now + expires_in,
            refresh_token: Some("opaque-refresh-token-for-tests".into()),
        }
    }

    /// The jar hands out plaintext cookies; only the response carries the encrypted value,
    /// so the round trip has to go through `Set-Cookie` the way a browser does.
    fn replay_cookies(jar: PrivateCookieJar) -> HeaderMap {
        use axum::response::IntoResponse;
        let response = (jar, axum::http::StatusCode::OK).into_response();
        let mut headers = HeaderMap::new();
        for value in response.headers().get_all(axum::http::header::SET_COOKIE) {
            let raw = value.to_str().expect("set-cookie is ascii");
            let pair = raw.split(';').next().unwrap_or_default();
            headers.append(
                axum::http::header::COOKIE,
                pair.parse().expect("cookie header"),
            );
        }
        headers
    }

    #[test]
    fn round_trips_through_the_encrypted_jar() {
        let key = Key::generate();
        let jar = store(PrivateCookieJar::new(key.clone()), &sample(600)).expect("store");
        let headers = replay_cookies(jar);
        assert!(
            !headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .contains("demo.steward"),
            "the session cookie must be encrypted on the wire"
        );
        let restored = load(&PrivateCookieJar::from_headers(&headers, key)).expect("load");
        assert_eq!(restored.identity.username, "demo.steward");
        assert_eq!(
            restored.refresh_token.as_deref(),
            Some("opaque-refresh-token-for-tests"),
            "the refresh token rides in its own cookie and is joined back on load"
        );
    }

    #[test]
    fn the_refresh_token_never_enters_the_session_cookie() {
        let jar = store(PrivateCookieJar::new(Key::generate()), &sample(600)).expect("store");
        let session_cookie = jar.get(SESSION_COOKIE).expect("session cookie");
        assert!(!session_cookie
            .value()
            .contains("opaque-refresh-token-for-tests"));
        assert!(jar.get(REFRESH_COOKIE).is_some());
    }

    #[test]
    fn refresh_is_due_inside_the_leeway_only() {
        let now = now_unix();
        let mut session = sample(600);
        session.access_expires_at = now + REFRESH_LEEWAY_SECS + 5;
        assert!(!session.needs_refresh(now));
        session.access_expires_at = now + REFRESH_LEEWAY_SECS - 5;
        assert!(session.needs_refresh(now));
        assert!(!session.access_expired(now));
        session.access_expires_at = now - 1;
        assert!(session.access_expired(now));
        session.refresh_token = None;
        assert!(!session.needs_refresh(now), "nothing to refresh with");
    }

    #[test]
    fn debug_redacts_the_refresh_token() {
        let dumped = format!("{:?}", sample(60));
        assert!(
            !dumped.contains("opaque-refresh-token-for-tests"),
            "{dumped}"
        );
    }

    #[test]
    fn a_foreign_key_cannot_read_the_session() {
        let jar = store(PrivateCookieJar::new(Key::generate()), &sample(600)).expect("store");
        let headers = replay_cookies(jar);
        assert!(load(&PrivateCookieJar::from_headers(&headers, Key::generate())).is_none());
    }

    #[test]
    fn expired_session_is_not_loaded() {
        let key = Key::generate();
        let mut expired = sample(600);
        expired.expires_at = now_unix() - 1;
        let value = serde_json::to_string(&expired).expect("json");
        let jar = PrivateCookieJar::new(key).add(secure_cookie(SESSION_COOKIE, value, 600));
        assert!(load(&jar).is_none());
    }

    #[test]
    fn cookie_carries_the_required_flags() {
        let cookie = secure_cookie(SESSION_COOKIE, "v".into(), 60);
        assert!(cookie.http_only().unwrap_or(false));
        assert!(cookie.secure().unwrap_or(false));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.path(), Some("/"));
    }

    #[test]
    fn debug_redacts_the_id_token() {
        let dumped = format!("{:?}", sample(60));
        assert!(
            !dumped.contains("signature"),
            "id_token leaked into Debug: {dumped}"
        );
        assert!(dumped.contains("[redacted]"));
    }
}
