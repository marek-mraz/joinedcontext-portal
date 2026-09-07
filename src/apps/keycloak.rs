//! Creating the confidential OIDC client an app's sidecar logs users in against (T-0411, AP-27).
//!
//! The Portal talks to the Keycloak Admin API as the service account of its own `portal-api`
//! client, which the realm grants `realm-management: manage-clients`. There is no second
//! credential and no administrator password anywhere in the platform.
//!
//! Two decisions keep the surface small. The representation carries the `client-secret` the
//! reconciler has just written into the Kubernetes Secret, so Keycloak never mints one and this
//! module never reads one back: the Secret stays the single source of truth for both halves of
//! the sidecar's configuration. And the client is looked up by `clientId` and then created or
//! updated, so the same call converges a realm that has the client, a realm that lost it and a
//! realm that never had it (AP-21).
//!
//! `manage-clients` is realm-wide because Keycloak scopes client management per realm and not
//! per name prefix. What keeps it honest is that the only client id this module is ever given is
//! `app-{name}` built from an App manifest's name, which is a DNS label.

use reqwest::{Client, StatusCode, Url};
use serde_json::{json, Value};

/// Why the Admin API could not be reached or refused what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum KeycloakError {
    /// The request never produced an answer.
    #[error("keycloak did not answer: {0}")]
    Transport(#[from] reqwest::Error),
    /// The answer was an error status.
    #[error("keycloak answered {status} while {what}")]
    Status {
        /// The HTTP status.
        status: u16,
        /// What was being attempted, in a form that fits after "while".
        what: String,
    },
    /// The issuer is not a realm URL, so no admin URL can be built from it.
    #[error("the issuer {0} does not end in /realms/{{realm}}, so it names no realm to manage")]
    Issuer(String),
    /// The token endpoint answered without an access token.
    #[error("the client-credentials grant returned no access_token")]
    NoToken,
}

/// The Keycloak Admin API, scoped to one realm.
pub struct AdminClient {
    http: Client,
    /// `https://idm.example.sk/realms/{realm}`, the issuer the Portal already logs people in at.
    issuer: Url,
    /// `https://idm.example.sk/admin/realms/{realm}/clients`.
    clients: Url,
    client_id: String,
    client_secret: String,
}

impl std::fmt::Debug for AdminClient {
    /// Prints no secret: a converge error carries this struct's context into the log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminClient")
            .field("issuer", &self.issuer.as_str())
            .field("client_id", &self.client_id)
            .field("client_secret", &"redacted")
            .finish()
    }
}

impl AdminClient {
    /// An admin client for the realm the issuer names.
    ///
    /// The admin URL is the issuer with `realms/{realm}` replaced by `admin/realms/{realm}`, so a
    /// Keycloak served under a path prefix keeps it.
    pub fn new(
        http: Client,
        issuer: &Url,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Self, KeycloakError> {
        let path = issuer.path().trim_end_matches('/');
        let (root, realm) = path
            .rsplit_once("/realms/")
            .filter(|(_, realm)| !realm.is_empty() && !realm.contains('/'))
            .ok_or_else(|| KeycloakError::Issuer(issuer.to_string()))?;

        let mut clients = issuer.clone();
        clients.set_path(&format!("{root}/admin/realms/{realm}/clients"));
        clients.set_query(None);
        clients.set_fragment(None);

        Ok(Self {
            http,
            issuer: issuer.clone(),
            clients,
            client_id: client_id.into(),
            client_secret: client_secret.into(),
        })
    }

    /// The realm holds this client with this redirect URI and this secret, whatever it held
    /// before.
    pub async fn ensure_client(
        &self,
        client_id: &str,
        redirect_uri: &str,
        client_secret: &str,
    ) -> Result<(), KeycloakError> {
        let token = self.token().await?;
        let representation = representation(client_id, redirect_uri, client_secret);

        match self.find(&token, client_id).await? {
            // `id` is Keycloak's own uuid for the client and the only way to address it.
            Some(uuid) => {
                let response = self
                    .http
                    .put(self.client_url(&uuid))
                    .bearer_auth(&token)
                    .json(&representation)
                    .send()
                    .await?;
                status(response.status(), || {
                    format!("updating the client {client_id}")
                })
            }
            None => {
                let response = self
                    .http
                    .post(self.clients.clone())
                    .bearer_auth(&token)
                    .json(&representation)
                    .send()
                    .await?;
                status(response.status(), || {
                    format!("creating the client {client_id}")
                })
            }
        }
    }

    /// The realm no longer holds this client; deleting one that is not there succeeds (CC-18).
    pub async fn delete_client(&self, client_id: &str) -> Result<(), KeycloakError> {
        let token = self.token().await?;
        let Some(uuid) = self.find(&token, client_id).await? else {
            return Ok(());
        };
        let response = self
            .http
            .delete(self.client_url(&uuid))
            .bearer_auth(&token)
            .send()
            .await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(()),
            other => status(other, || format!("deleting the client {client_id}")),
        }
    }

    /// `…/clients/{uuid}`, the only way to address one client.
    ///
    /// The segment is pushed rather than joined, so it is percent-encoded and an answer from a
    /// compromised Keycloak cannot walk the path back up to another realm.
    fn client_url(&self, uuid: &str) -> Url {
        let mut url = self.clients.clone();
        url.path_segments_mut()
            .expect("the admin url has a path")
            .push(uuid);
        url
    }

    /// Keycloak's uuid for a client id, or `None` when the realm does not have it.
    async fn find(&self, token: &str, client_id: &str) -> Result<Option<String>, KeycloakError> {
        let response = self
            .http
            .get(self.clients.clone())
            .query(&[("clientId", client_id)])
            .bearer_auth(token)
            .send()
            .await?;
        status(response.status(), || format!("looking up {client_id}"))?;
        let found: Vec<Value> = response.json().await?;
        Ok(found
            .first()
            .and_then(|client| client.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// An access token of the Portal's own service account.
    ///
    /// ponytail: fetched per operation rather than cached. One reconcile of one app costs one
    /// extra round trip to a service that is already on the critical path of every login; cache
    /// it with its `expires_in` if a realm ever holds enough apps for that to show.
    async fn token(&self) -> Result<String, KeycloakError> {
        let mut url = self.issuer.clone();
        url.set_path(&format!(
            "{}/protocol/openid-connect/token",
            self.issuer.path().trim_end_matches('/')
        ));
        let response = self
            .http
            .post(url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await?;
        status(response.status(), || {
            "asking for a service-account token".to_owned()
        })?;
        let body: Value = response.json().await?;
        body.get("access_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(KeycloakError::NoToken)
    }
}

/// An error status becomes an error; the body is never read, because a Keycloak error body
/// echoes the request and the request carries a secret.
fn status(status: StatusCode, what: impl FnOnce() -> String) -> Result<(), KeycloakError> {
    match status.is_success() {
        true => Ok(()),
        false => Err(KeycloakError::Status {
            status: status.as_u16(),
            what: what(),
        }),
    }
}

/// What the realm should hold for one app.
///
/// Every flow the sidecar does not use is off: no implicit grant, no password grant and no
/// service account, so the client can do exactly one thing, log a browser in and come back to
/// the one redirect URI the reconciler rendered into the sidecar's arguments.
fn representation(client_id: &str, redirect_uri: &str, client_secret: &str) -> Value {
    // `https://{host}/apps/{name}/oauth2/callback` → `https://{host}`, the app's own origin.
    let origin = redirect_uri
        .split_once("/apps/")
        .map(|(origin, _)| origin.to_owned())
        .unwrap_or_default();
    json!({
        "clientId": client_id,
        "name": client_id,
        "description": "Login front of an app on demand, reconciled by the Portal (AP-27)",
        "enabled": true,
        "protocol": "openid-connect",
        "publicClient": false,
        "bearerOnly": false,
        "secret": client_secret,
        "standardFlowEnabled": true,
        "implicitFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": false,
        "authorizationServicesEnabled": false,
        "frontchannelLogout": true,
        "rootUrl": origin,
        "baseUrl": origin,
        "redirectUris": [redirect_uri],
        "webOrigins": [origin],
        "defaultClientScopes": ["basic", "email", "profile", "roles", "web-origins"],
        "optionalClientScopes": [],
        // The sidecar runs with --code-challenge-method=S256, so the realm may insist on it.
        "attributes": { "pkce.code.challenge.method": "S256" }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin(issuer: &str) -> Result<AdminClient, KeycloakError> {
        AdminClient::new(
            Client::new(),
            &Url::parse(issuer).expect("a url"),
            "portal-api",
            "s3cr3t",
        )
    }

    #[test]
    fn the_admin_url_keeps_whatever_path_the_issuer_had() {
        let plain = admin("https://idm.example.sk/realms/bb").expect("a realm issuer");
        assert_eq!(
            plain.clients.as_str(),
            "https://idm.example.sk/admin/realms/bb/clients"
        );
        // Keycloak behind a path prefix, as older installations still are.
        let prefixed = admin("https://example.sk/auth/realms/bb").expect("a realm issuer");
        assert_eq!(
            prefixed.clients.as_str(),
            "https://example.sk/auth/admin/realms/bb/clients"
        );
    }

    #[test]
    fn an_issuer_that_names_no_realm_is_refused_rather_than_guessed() {
        for wrong in [
            "https://idm.example.sk/",
            "https://idm.example.sk/realms/",
            "https://idm.example.sk/realms/bb/extra",
        ] {
            assert!(
                matches!(admin(wrong), Err(KeycloakError::Issuer(_))),
                "{wrong} was accepted"
            );
        }
    }

    #[test]
    fn the_debug_of_an_admin_client_carries_no_secret() {
        let printed = format!(
            "{:?}",
            admin("https://idm.example.sk/realms/bb").expect("valid")
        );
        assert!(!printed.contains("s3cr3t"), "{printed}");
        assert!(printed.contains("portal-api"), "{printed}");
    }

    #[test]
    fn the_representation_grants_the_one_flow_the_sidecar_uses() {
        let client = representation(
            "app-air-quality",
            "https://bb.example.sk/apps/air-quality/oauth2/callback",
            "the-secret",
        );
        assert_eq!(client["publicClient"], json!(false));
        assert_eq!(client["standardFlowEnabled"], json!(true));
        for off in [
            "implicitFlowEnabled",
            "directAccessGrantsEnabled",
            "serviceAccountsEnabled",
        ] {
            assert_eq!(client[off], json!(false), "{off} is on");
        }
        assert_eq!(
            client["redirectUris"],
            json!(["https://bb.example.sk/apps/air-quality/oauth2/callback"]),
            "one redirect uri and no wildcard"
        );
        assert_eq!(client["webOrigins"], json!(["https://bb.example.sk"]));
        assert_eq!(client["secret"], json!("the-secret"));
    }
}
