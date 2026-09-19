//! Bearer-token verification for service callers and for the edge's user token (docs
//! Architecture/12 §3, OPS-33, ADR-N-019).
//!
//! The APISIX edge forwards `Authorization` untouched and the Portal is the verifier. Tokens are
//! checked against the realm JWKS, cached in memory; an unknown `kid` triggers at most one
//! refetch per minute, so a flood of forged tokens cannot turn the Portal into a Keycloak load
//! generator.
//!
//! Two signature algorithms and no third: the realm signs ES256 (TR-03187 AR-11), and the
//! `edge` client the `openid-connect` plugin logs people in with is signed RS256 by per-client
//! override, because lua-resty-openidc verifies RS and HS only. The user's access token reaches
//! the Portal as `X-Access-Token` and is verified here like any bearer (AP-27, AP-28).

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use url::Url;

use crate::auth::session::{Identity, Session};
use crate::error::ApiError;

/// The signature algorithms the realm uses: ES256 for every client (TR-03187 AR-11) and RS256
/// for the `edge` client alone (ADR-N-019). HS256 is never one of them: a shared secret would
/// let anyone who holds it mint a token.
const ALGORITHMS: [Algorithm; 2] = [Algorithm::ES256, Algorithm::RS256];

/// Whether a JWK's `alg` (or, absent one, its key type) is one of [`ALGORITHMS`].
fn usable(jwk: &jsonwebtoken::jwk::Jwk) -> bool {
    match jwk.common.key_algorithm {
        Some(alg) => matches!(alg.to_string().as_str(), "ES256" | "RS256"),
        None => matches!(
            jwk.algorithm,
            jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(_)
                | jsonwebtoken::jwk::AlgorithmParameters::RSA(_)
        ),
    }
}
/// Minimum distance between two JWKS fetches caused by unknown key ids.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub struct BearerVerifier {
    issuer: String,
    audience: String,
    jwks_url: Url,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
    last_refresh: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    exp: i64,
    #[serde(default)]
    iat: i64,
    preferred_username: Option<String>,
    azp: Option<String>,
    email: Option<String>,
    name: Option<String>,
    #[serde(default)]
    realm_access: RealmAccess,
    #[serde(default)]
    groups: Vec<String>,
}

#[derive(Default, Deserialize)]
struct RealmAccess {
    #[serde(default)]
    roles: Vec<String>,
}

impl BearerVerifier {
    /// `issuer` is the realm URL (`…/realms/{realm}`); the JWKS lives at its `certs` endpoint.
    pub fn new(issuer: &Url, audience: &str) -> Self {
        let mut jwks_url = issuer.clone();
        let path = format!(
            "{}/protocol/openid-connect/certs",
            issuer.path().trim_end_matches('/')
        );
        jwks_url.set_path(&path);
        Self::with_jwks_url(issuer, audience, jwks_url)
    }

    pub fn with_jwks_url(issuer: &Url, audience: &str, jwks_url: Url) -> Self {
        Self {
            issuer: issuer.as_str().trim_end_matches('/').to_string(),
            audience: audience.to_string(),
            jwks_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("static reqwest client configuration"),
            keys: RwLock::new(HashMap::new()),
            last_refresh: Mutex::new(None),
        }
    }

    /// Replaces the cached keys with the ES256 and RS256 keys of `set`; returns how many were
    /// usable.
    pub fn install(&self, set: &JwkSet) -> usize {
        let keys: HashMap<String, DecodingKey> = set
            .keys
            .iter()
            .filter(|jwk| usable(jwk))
            .filter_map(|jwk| {
                let kid = jwk.common.key_id.clone()?;
                DecodingKey::from_jwk(jwk).ok().map(|key| (kid, key))
            })
            .collect();
        let n = keys.len();
        if let Ok(mut cache) = self.keys.write() {
            *cache = keys;
        }
        n
    }

    /// Fetches the JWKS, at most once a minute; inside the interval it is a
    /// no-op that reports the current key count. A failed fetch also starts the interval.
    pub async fn refresh(&self) -> Result<usize, ApiError> {
        {
            let mut last = self
                .last_refresh
                .lock()
                .map_err(|_| ApiError::Internal("jwks lock".into()))?;
            if last.is_some_and(|t| t.elapsed() < REFRESH_INTERVAL) {
                return Ok(self.keys.read().map(|k| k.len()).unwrap_or(0));
            }
            *last = Some(Instant::now());
        }
        let set: JwkSet = self
            .http
            .get(self.jwks_url.clone())
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| ApiError::Unavailable(format!("JWKS fetch failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Unavailable(format!("JWKS is not a key set: {e}")))?;
        Ok(self.install(&set))
    }

    fn key(&self, kid: &str) -> Option<DecodingKey> {
        self.keys.read().ok()?.get(kid).cloned()
    }

    /// Verifies signature (ES256 or RS256, known `kid`), `iss`, `aud`, `exp` and `nbf`; anything
    /// else is `401`. The returned session carries no id token: bearer callers never log out.
    pub async fn verify(&self, token: &str) -> Result<Session, ApiError> {
        self.verify_with_client(token)
            .await
            .map(|(session, _)| session)
    }

    /// [`Self::verify`], and the Keycloak client that obtained the token (`azp`): a route that
    /// belongs to one workload asks which client it is, because a user name is not that.
    pub async fn verify_with_client(
        &self,
        token: &str,
    ) -> Result<(Session, Option<String>), ApiError> {
        self.verify_for_audience(token, &self.audience).await
    }

    /// [`Self::verify_with_client`] against an audience other than this verifier's own: the
    /// internal listener takes tokens bound to `portal-internal` rather than to the Portal's API
    /// client, and one verifier with one key cache serves both (PF-46, AG-52). The issuer, the
    /// algorithms and the required claims are the same; only what the token must be *for* differs.
    pub async fn verify_for_audience(
        &self,
        token: &str,
        audience: &str,
    ) -> Result<(Session, Option<String>), ApiError> {
        let header = decode_header(token).map_err(|_| ApiError::Unauthorized)?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(ApiError::Unauthorized);
        }
        let kid = header.kid.ok_or(ApiError::Unauthorized)?;
        let key = match self.key(&kid) {
            Some(key) => key,
            None => {
                // A rotated realm key is the only legitimate reason for an unknown kid.
                if let Err(err) = self.refresh().await {
                    tracing::warn!(error = %err, "JWKS refresh after unknown kid failed");
                }
                self.key(&kid).ok_or(ApiError::Unauthorized)?
            }
        };
        // The token's own algorithm, which the header check above narrowed to the two allowed
        // ones; `decode` refuses a key of the other family, so an RS256 header cannot be
        // verified against an EC key or the other way round.
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let data =
            decode::<Claims>(token, &key, &validation).map_err(|_| ApiError::Unauthorized)?;
        let c = data.claims;
        let client = c.azp.clone();
        let username = c
            .preferred_username
            .or(c.azp)
            .unwrap_or_else(|| c.sub.clone());
        let session = Session {
            identity: Identity {
                subject: c.sub,
                username,
                email: c.email,
                name: c.name,
                roles: c.realm_access.roles,
                groups: c
                    .groups
                    .into_iter()
                    .map(|g| g.trim_start_matches('/').to_owned())
                    .collect(),
            },
            expires_at: c.exp,
            issued_at: c.iat,
            id_token: String::new(),
            access_expires_at: c.exp,
            refresh_token: None,
        };
        Ok((session, client))
    }
}

/// The Keycloak client (`azp`) of a verified bearer token, parked in the request's extensions
/// by [`crate::auth::session::CurrentUser`]. Absent for a cookie session and for the edge's
/// token, which is a person's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenClient(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use p256::pkcs8::EncodePrivateKey;
    use serde_json::json;

    const ISSUER: &str = "https://idm.example.test/realms/dev";

    fn verifier() -> BearerVerifier {
        // Port 9 (discard) refuses connections immediately: a refresh fails fast, never hangs.
        BearerVerifier::with_jwks_url(
            &Url::parse(ISSUER).unwrap(),
            "portal-api",
            Url::parse("http://127.0.0.1:9/certs").unwrap(),
        )
    }

    /// A fresh P-256 key pair: the signing key for `encode` and its public half as a JWK set.
    fn keypair(kid: &str) -> (EncodingKey, JwkSet) {
        let secret = p256::SecretKey::random(&mut rand_core::OsRng);
        let der = secret.to_pkcs8_der().unwrap();
        let signer = EncodingKey::from_ec_der(der.as_bytes());
        let mut jwk: serde_json::Value =
            serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
        jwk["kid"] = json!(kid);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");
        let set: JwkSet = serde_json::from_value(json!({ "keys": [jwk] })).unwrap();
        (signer, set)
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn claims(exp: i64) -> serde_json::Value {
        json!({
            "iss": ISSUER, "aud": "portal-api", "sub": "svc:smoke", "azp": "apisix-gateway",
            "exp": exp, "iat": now(), "realm_access": { "roles": ["viewer"] }
        })
    }

    fn sign(signer: &EncodingKey, kid: &str, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.into());
        encode(&header, claims, signer).unwrap()
    }

    // ---------------------------------------------------------------------------------------
    // T-2091 `refresh`: the interval, and what a failure says and to whom.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_failed_fetch_starts_the_interval_so_the_next_call_does_not_hammer_the_realm() {
        let v = verifier(); // port 9: the connection is refused at once
        let (_signer, set) = keypair("k1");
        assert_eq!(v.install(&set), 1);

        // The first call tries and fails; it must not throw the installed keys away.
        let first = v.refresh().await;
        assert!(matches!(first, Err(ApiError::Unavailable(_))), "{first:?}");
        assert!(v.key("k1").is_some(), "a failed refresh emptied the cache");

        // Inside the interval the next call is a no-op that reports what is cached, however many
        // times it is called: a route that verifies a token with an unknown kid cannot turn one
        // request into one realm fetch each (REFRESH_INTERVAL).
        for _ in 0..5 {
            assert!(matches!(v.refresh().await, Ok(1)));
        }
    }

    #[tokio::test]
    async fn a_refresh_failure_reaches_the_log_and_never_a_caller() {
        // The message carries the reqwest error, which names the JWKS URL — an internal realm
        // address. That is a log line, never a body: both callers swallow it
        // (`src/state.rs:207` and `verify_for_audience` at bearer.rs:187, each `if let Err(err)`),
        // and the verification itself answers a bare 401. Asserted here so a caller that starts
        // propagating it is noticed.
        let v = verifier();
        let Err(ApiError::Unavailable(message)) = v.refresh().await else {
            panic!("a refused connection should be Unavailable");
        };
        assert!(message.starts_with("JWKS fetch failed"));

        // The door a caller actually knocks on: an unknown kid with the realm unreachable is 401
        // with nothing about the realm in it.
        let (signer, _set) = keypair("k1");
        let refused = v.verify(&sign(&signer, "k1", &claims(now() + 60))).await;
        assert!(
            matches!(refused, Err(ApiError::Unauthorized)),
            "{refused:?}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // T-2092 `verify_with_client`: what it accepts, what it refuses, and what it hands back.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_client_comes_back_with_the_session_and_is_absent_when_the_token_has_no_azp() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);

        let (session, client) = v
            .verify_with_client(&sign(&signer, "k1", &claims(now() + 60)))
            .await
            .unwrap();
        assert_eq!(client.as_deref(), Some("apisix-gateway"));
        assert_eq!(session.identity.username, "apisix-gateway");

        // No `azp`: no client, and the name falls back to the subject rather than to nothing.
        let mut anonymous_client = claims(now() + 60);
        anonymous_client["azp"] = json!(null);
        let (session, client) = v
            .verify_with_client(&sign(&signer, "k1", &anonymous_client))
            .await
            .unwrap();
        assert_eq!(client, None);
        assert_eq!(session.identity.username, "svc:smoke");

        // A person's token: the username is theirs, and the client is still named.
        let mut human = claims(now() + 60);
        human["preferred_username"] = json!("jana.kovacova");
        let (session, client) = v
            .verify_with_client(&sign(&signer, "k1", &human))
            .await
            .unwrap();
        assert_eq!(session.identity.username, "jana.kovacova");
        assert_eq!(client.as_deref(), Some("apisix-gateway"));
    }

    #[tokio::test]
    async fn a_token_that_is_not_one_is_refused_without_a_key_lookup() {
        let v = verifier();
        let (_signer, set) = keypair("k1");
        v.install(&set);
        for token in [
            "",
            "   ",
            "not.a.token",
            "onlyonesegment",
            "two.segments",
            "a.b.c.d",
            // A header that is valid base64 JSON but names no kid.
            &jsonwebtoken::encode(
                &Header::new(Algorithm::ES256),
                &claims(now() + 60),
                &keypair("k1").0,
            )
            .unwrap(),
        ] {
            assert!(
                matches!(
                    v.verify_with_client(token).await,
                    Err(ApiError::Unauthorized)
                ),
                "{token:?} was not refused",
            );
        }
    }

    #[tokio::test]
    async fn a_missing_required_claim_is_refused_and_the_leeway_is_thirty_seconds() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);

        // `exp`, `iss`, `aud` and `sub` are required: a token without one of them is 401, whatever
        // else it carries.
        for missing in ["exp", "iss", "aud", "sub"] {
            let mut without = claims(now() + 60);
            without[missing] = json!(null);
            assert!(
                matches!(
                    v.verify_with_client(&sign(&signer, "k1", &without)).await,
                    Err(ApiError::Unauthorized)
                ),
                "a token without {missing} passed",
            );
        }

        // `nbf` is validated: a token not yet valid is refused, and one inside the 30 s leeway is
        // taken, because two clocks in a cluster are never the same (`validation.leeway = 30`).
        let mut future = claims(now() + 600);
        future["nbf"] = json!(now() + 3600);
        assert!(matches!(
            v.verify_with_client(&sign(&signer, "k1", &future)).await,
            Err(ApiError::Unauthorized)
        ));
        let mut just_now = claims(now() + 600);
        just_now["nbf"] = json!(now() + 10);
        assert!(v
            .verify_with_client(&sign(&signer, "k1", &just_now))
            .await
            .is_ok());

        // An expiry a second inside the leeway is still taken; one well outside it is not.
        assert!(v
            .verify_with_client(&sign(&signer, "k1", &claims(now() - 10)))
            .await
            .is_ok());
        assert!(matches!(
            v.verify_with_client(&sign(&signer, "k1", &claims(now() - 3600)))
                .await,
            Err(ApiError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn an_audience_list_is_read_and_a_group_keeps_no_leading_slash() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);

        // Keycloak sends `aud` as a list when a token is for more than one client: ours has to be
        // in it, and a list that does not name it is refused.
        let mut many = claims(now() + 60);
        many["aud"] = json!(["context-gateway", "portal-api"]);
        assert!(v
            .verify_with_client(&sign(&signer, "k1", &many))
            .await
            .is_ok());
        let mut others = claims(now() + 60);
        others["aud"] = json!(["context-gateway", "portal-internal"]);
        assert!(matches!(
            v.verify_with_client(&sign(&signer, "k1", &others)).await,
            Err(ApiError::Unauthorized)
        ));

        // A realm group is `/editors` on the wire and `editors` in a grant, so the slash goes.
        let mut grouped = claims(now() + 60);
        grouped["groups"] = json!(["/editors", "nested/one", "/a/b"]);
        let (session, _) = v
            .verify_with_client(&sign(&signer, "k1", &grouped))
            .await
            .unwrap();
        assert_eq!(
            session.identity.groups,
            vec!["editors", "nested/one", "a/b"]
        );
    }

    #[tokio::test]
    async fn a_verified_bearer_carries_no_id_token_and_no_refresh_token() {
        // A bearer caller has nothing to log out of and nothing to refresh with; a session that
        // carried either would hand a script a credential it never presented (AG-59).
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let (session, _) = v
            .verify_with_client(&sign(&signer, "k1", &claims(now() + 60)))
            .await
            .unwrap();
        assert!(session.id_token.is_empty());
        assert_eq!(session.refresh_token, None);
        assert_eq!(session.access_expires_at, session.expires_at);
    }

    #[tokio::test]
    async fn a_valid_es256_token_yields_the_service_identity() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        assert_eq!(v.install(&set), 1);
        let session = v
            .verify(&sign(&signer, "k1", &claims(now() + 60)))
            .await
            .unwrap();
        assert_eq!(session.identity.subject, "svc:smoke");
        assert_eq!(session.identity.username, "apisix-gateway");
        assert_eq!(session.identity.roles, vec!["viewer"]);
        assert!(!session.is_expired(now()));
    }

    #[tokio::test]
    async fn expired_wrong_audience_wrong_issuer_and_other_key_are_refused() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let (other, _) = keypair("k1");
        let mut foreign_aud = claims(now() + 60);
        foreign_aud["aud"] = json!("context-gateway");
        let mut foreign_iss = claims(now() + 60);
        foreign_iss["iss"] = json!("https://idm.example.test/realms/other");
        for token in [
            sign(&signer, "k1", &claims(now() - 120)),
            sign(&signer, "k1", &foreign_aud),
            sign(&signer, "k1", &foreign_iss),
            sign(&other, "k1", &claims(now() + 60)),
        ] {
            assert!(matches!(
                v.verify(&token).await,
                Err(ApiError::Unauthorized)
            ));
        }
    }

    #[tokio::test]
    async fn a_tampered_signature_and_an_unsigned_expired_token_are_refused() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let good = sign(&signer, "k1", &claims(now() + 60));
        let tampered = format!("{}.forged", good.rsplit_once('.').unwrap().0);
        assert!(matches!(
            v.verify(&tampered).await,
            Err(ApiError::Unauthorized)
        ));
        // The same token scripts/smoke.sh in joinedcontext-deployment sends (built here, not a
        // literal: a JWT-shaped string in the repo trips gitleaks).
        use base64::Engine;
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let unsigned = format!(
            "{}.{}.AAAA",
            b64(r#"{"alg":"ES256","typ":"JWT"}"#),
            b64(r#"{"exp":1}"#)
        );
        assert!(matches!(
            v.verify(&unsigned).await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            v.verify("not-a-jwt").await,
            Err(ApiError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn a_shared_secret_algorithm_is_refused_even_with_the_right_claims() {
        let v = verifier();
        let (_, set) = keypair("k1");
        v.install(&set);
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("k1".into());
        let hs = encode(
            &header,
            &claims(now() + 60),
            &EncodingKey::from_secret(b"shared"),
        )
        .unwrap();
        assert!(matches!(v.verify(&hs).await, Err(ApiError::Unauthorized)));
    }

    #[test]
    fn a_jwk_of_another_algorithm_is_not_installed() {
        let v = verifier();
        let (_, mut set) = keypair("k1");
        // Keycloak lists the realm's RSA keys with `alg: RS256`; an HMAC or an encryption key
        // in the same set must not become a verification key.
        let mut oct: serde_json::Value = serde_json::to_value(&set.keys[0]).unwrap();
        oct["kid"] = json!("k-enc");
        oct["alg"] = json!("RSA-OAEP");
        set.keys.push(serde_json::from_value(oct).unwrap());
        assert_eq!(v.install(&set), 1, "only the ES256 key is usable");
    }

    #[tokio::test]
    async fn an_unknown_kid_fetches_once_per_minute_and_is_refused() {
        let v = verifier();
        let (signer, _) = keypair("k9");
        let token = sign(&signer, "k9", &claims(now() + 60));
        assert!(matches!(
            v.verify(&token).await,
            Err(ApiError::Unauthorized)
        ));
        // The failed fetch started the interval: the next refresh is a no-op, not a request.
        assert_eq!(v.refresh().await.unwrap(), 0);
    }

    #[test]
    fn jwks_url_is_the_realm_certs_endpoint() {
        let v = BearerVerifier::new(
            &Url::parse("https://idm.example.test/realms/dev/").unwrap(),
            "portal-api",
        );
        assert_eq!(
            v.jwks_url.as_str(),
            "https://idm.example.test/realms/dev/protocol/openid-connect/certs"
        );
        assert_eq!(v.issuer, ISSUER);
    }
}
