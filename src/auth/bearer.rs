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
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let data =
            decode::<Claims>(token, &key, &validation).map_err(|_| ApiError::Unauthorized)?;
        let c = data.claims;
        let username = c
            .preferred_username
            .or(c.azp)
            .unwrap_or_else(|| c.sub.clone());
        Ok(Session {
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
        })
    }
}

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
