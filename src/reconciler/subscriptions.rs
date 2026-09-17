//! What a `Subscription` manifest declares, written into the space it names (CC-72, DS-16).
//!
//! The manifest is the truth and the broker holds the effect, so this wave is the one place
//! where the repository reaches into live broker state. Three rules keep that from turning into
//! ownership of the broker (Architecture/06):
//!
//! - the id is the manifest's, `urn:ngsi-ld:Subscription:{orgDomain}:{space}:{name}`, so a
//!   second pass updates instead of duplicating;
//! - the credential is resolved at apply time and sent as the `Authorization` header of
//!   `receiverInfo`, so it is in no manifest, no status and no log line (MF-31, PL-17);
//! - only a subscription this reconciler declared in the previous run is ever deleted, so the
//!   ones an application created through the API are left alone even when they are written in
//!   the same URN namespace.
//!
//! Everything goes through the space surface (`/cs/{space}/ngsi-ld/v1/subscriptions`) with the
//! Portal's own service-account token, never straight at the broker: the space's policy decides
//! this write like any other, and the gateway injects the tenant (SP-06, SP-07).

use crate::pipeline_secrets::Resolver;
use crate::store::Mirror;
use jc_core::kinds::subscription::SubscriptionSpec;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

/// A broker call that has not answered in this long is a failed sync, not a slow one.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The header a resolved `secretRef` becomes on the notification hop.
const CREDENTIAL_HEADER: &str = "Authorization";

/// What happened to one `Subscription` in a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionOutcome {
    /// The space holds what the manifest declares.
    Written,
    /// Nothing was written and why: a reference that does not resolve, a refusal from the
    /// space, or a gateway that did not answer.
    Error(String),
}

/// One realm token, one HTTP client, and the base of the space surface.
pub struct SubscriptionSync {
    http: reqwest::Client,
    /// The platform host the spaces are served on, without a trailing slash.
    base: String,
    /// `https://host/realms/{realm}`: where the service-account token comes from.
    issuer: String,
    client_id: String,
    client_secret: String,
    /// The organization's domain, the third segment of every URN this instance writes.
    org_domain: String,
}

impl SubscriptionSync {
    /// A sync against one platform host, authenticating as the Portal's own client.
    pub fn new(
        base: impl Into<String>,
        issuer: &str,
        client_id: String,
        client_secret: String,
        org_domain: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: base.into().trim_end_matches('/').to_owned(),
            issuer: issuer.trim_end_matches('/').to_owned(),
            client_id,
            client_secret,
            org_domain: org_domain.into(),
        }
    }

    /// The id a manifest's subscription carries in the broker.
    fn urn(&self, space: &str, name: &str) -> String {
        format!(
            "urn:ngsi-ld:Subscription:{}:{space}:{name}",
            self.org_domain
        )
    }

    async fn token(&self) -> Result<String, String> {
        super::realm::token(
            &self.http,
            &self.issuer,
            &self.client_id,
            &self.client_secret,
        )
        .await
    }

    /// Brings every space's subscriptions to what the repository declares.
    ///
    /// `repository` is the staged tree of this sync, which is where a SOPS `secretRef` reads.
    /// Returns one outcome per declared manifest, by `(project, name)`.
    pub async fn converge(
        &self,
        mirror: &Mirror,
        previous: &Mirror,
        repository: &Path,
        secrets: Option<&Resolver>,
    ) -> Vec<(String, String, SubscriptionOutcome)> {
        let mut outcomes = Vec::new();
        let token = match self.token().await {
            Ok(token) => token,
            Err(reason) => {
                // Without a token nothing can be written, and saying so once per run beats
                // saying it once per subscription.
                tracing::warn!(%reason, "no token: subscriptions are read, not written");
                for (namespace, name, _, _) in declared(mirror) {
                    outcomes.push((namespace, name, SubscriptionOutcome::Error(reason.clone())));
                }
                return outcomes;
            }
        };

        // What each space should hold when this run is over, so the sweep below knows what is
        // left over rather than what merely failed today.
        let mut declared_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (namespace, name, space, spec) in declared(mirror) {
            let id = self.urn(&space, &name);
            declared_ids
                .entry(space.clone())
                .or_default()
                .insert(id.clone());
            let body = match self.body(&id, &spec, repository, secrets).await {
                Ok(body) => body,
                Err(reason) => {
                    outcomes.push((namespace, name, SubscriptionOutcome::Error(reason)));
                    continue;
                }
            };
            let outcome = match self.write(&token, &space, &id, &body).await {
                Ok(()) => SubscriptionOutcome::Written,
                Err(reason) => SubscriptionOutcome::Error(reason),
            };
            outcomes.push((namespace, name, outcome));
        }

        // A manifest that is gone takes its subscription with it: what the run before declared
        // and this one does not. Nothing else is deleted — the space's other subscriptions were
        // written by applications through the API, and this reconciler does not own the space.
        // A manifest removed while the Portal was down leaves its subscription behind, which is
        // the failure to have: an orphan a person can delete, not someone else's deleted.
        for (space, name) in declared(previous)
            .into_iter()
            .map(|(_, name, space, _)| (space, name))
        {
            let id = self.urn(&space, &name);
            if declared_ids
                .get(&space)
                .is_some_and(|declared| declared.contains(&id))
            {
                continue;
            }
            match self.delete(&token, &space, &id).await {
                Ok(()) => tracing::info!(%space, %id, "subscription removed with its manifest"),
                Err(reason) => tracing::warn!(%space, %id, %reason, "subscription not removed"),
            }
        }
        outcomes
    }

    /// The NGSI-LD subscription one manifest asks for, with the credential resolved into it.
    async fn body(
        &self,
        id: &str,
        spec: &SubscriptionSpec,
        repository: &Path,
        secrets: Option<&Resolver>,
    ) -> Result<Value, String> {
        let mut endpoint = Map::new();
        endpoint.insert("uri".to_owned(), json!(spec.notification.endpoint.uri));
        if let Some(accept) = &spec.notification.endpoint.accept {
            endpoint.insert("accept".to_owned(), json!(accept));
        }
        let mut receiver_info: Vec<Value> = spec
            .notification
            .endpoint
            .receiver_info
            .iter()
            .map(|pair| json!({ &pair.key: pair.value }))
            .collect();
        if let Some(reference) = &spec.notification.endpoint.secret_ref {
            let resolver = secrets.ok_or_else(|| {
                format!(
                    "the notification credential {} cannot be resolved: no secret backend is \
                     configured",
                    reference.name
                )
            })?;
            let value = resolver
                .one(repository, reference)
                .await
                .map_err(|err| err.to_string())?;
            receiver_info.push(json!({ CREDENTIAL_HEADER: value }));
        }
        if !receiver_info.is_empty() {
            endpoint.insert("receiverInfo".to_owned(), Value::Array(receiver_info));
        }

        let mut notification = Map::new();
        notification.insert("endpoint".to_owned(), Value::Object(endpoint));
        if !spec.notification.attributes.is_empty() {
            notification.insert("attributes".to_owned(), json!(spec.notification.attributes));
        }
        if let Some(format) = &spec.notification.format {
            notification.insert("format".to_owned(), json!(format));
        }

        let mut body = Map::new();
        body.insert("id".to_owned(), json!(id));
        body.insert("type".to_owned(), json!("Subscription"));
        body.insert("notification".to_owned(), Value::Object(notification));
        if let Some(title) = &spec.subscription_name {
            body.insert("subscriptionName".to_owned(), json!(title));
        }
        if let Some(description) = &spec.description {
            body.insert("description".to_owned(), json!(description));
        }
        if !spec.entities.is_empty() {
            body.insert(
                "entities".to_owned(),
                serde_json::to_value(&spec.entities).map_err(|err| err.to_string())?,
            );
        }
        if !spec.watched_attributes.is_empty() {
            body.insert(
                "watchedAttributes".to_owned(),
                json!(spec.watched_attributes),
            );
        }
        if let Some(q) = &spec.q {
            body.insert("q".to_owned(), json!(q));
        }
        if let Some(geo_q) = &spec.geo_q {
            body.insert("geoQ".to_owned(), geo_query(geo_q)?);
        }
        if let Some(throttling) = spec.throttling {
            body.insert("throttling".to_owned(), json!(throttling));
        }
        if let Some(expires) = spec.expires_at {
            body.insert(
                "expiresAt".to_owned(),
                json!(expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            );
        }
        body.insert("isActive".to_owned(), json!(spec.is_active));
        Ok(Value::Object(body))
    }

    /// Creates the subscription, or patches the one that is already there.
    async fn write(&self, token: &str, space: &str, id: &str, body: &Value) -> Result<(), String> {
        let created = self
            .http
            .post(format!("{}/cs/{space}/ngsi-ld/v1/subscriptions", self.base))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if created.status().is_success() {
            return Ok(());
        }
        if created.status() != reqwest::StatusCode::CONFLICT {
            return Err(refusal("the space refused the subscription", created).await);
        }
        // Already there: the manifest is still the truth, so it is patched onto it. The id is
        // not sent again — a PATCH that renames the resource is not what this is.
        let mut patch = body.clone();
        if let Some(object) = patch.as_object_mut() {
            object.remove("id");
        }
        let updated = self
            .http
            .patch(format!(
                "{}/cs/{space}/ngsi-ld/v1/subscriptions/{id}",
                self.base
            ))
            .bearer_auth(token)
            .json(&patch)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match updated.status().is_success() {
            true => Ok(()),
            false => Err(refusal("the space refused the update", updated).await),
        }
    }

    async fn delete(&self, token: &str, space: &str, id: &str) -> Result<(), String> {
        let response = self
            .http
            .delete(format!(
                "{}/cs/{space}/ngsi-ld/v1/subscriptions/{id}",
                self.base
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND
        {
            true => Ok(()),
            false => Err(refusal("the space refused the delete", response).await),
        }
    }
}

/// `what: status body`, with the body cut short — a problem document is a sentence, an HTML
/// error page is not, and neither belongs in a status field whole.
async fn refusal(what: &str, response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let mut said = body.trim().replace('\n', " ");
    if said.len() > 300 {
        let mut end = 300;
        while !said.is_char_boundary(end) {
            end -= 1;
        }
        said.truncate(end);
    }
    match said.is_empty() {
        true => format!("{what}: {status}"),
        false => format!("{what}: {status}: {said}"),
    }
}

/// Every declared subscription of the mirror: `(project, name, space, spec)`.
fn declared(mirror: &Mirror) -> Vec<(String, String, String, SubscriptionSpec)> {
    let mut found = Vec::new();
    for namespace in mirror.namespaces() {
        let page = mirror.list(
            &namespace,
            "Subscription",
            &crate::store::ListOptions::default(),
        );
        for envelope in page.items {
            let name = envelope.metadata.name.clone();
            match serde_json::from_value::<SubscriptionSpec>(envelope.spec.clone()) {
                Ok(spec) => {
                    let space = spec.context_space_ref.name().to_owned();
                    found.push((namespace.clone(), name, space, spec));
                }
                Err(err) => tracing::warn!(
                    namespace = %namespace,
                    subscription = %name,
                    error = %err,
                    "a Subscription manifest does not parse and is not written"
                ),
            }
        }
    }
    found
}

/// `georel=near;maxDistance==2000&geometry=Point&coordinates=[13.4,52.5]` as the object CIM 009
/// asks for in a body. A `geoQ` that names none of the three is refused rather than sent half.
fn geo_query(written: &str) -> Result<Value, String> {
    let mut object = Map::new();
    for pair in written.split('&').filter(|p| !p.is_empty()) {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("geoQ '{written}' is not key=value pairs"));
        };
        let value = match value.trim_start().starts_with('[') {
            true => serde_json::from_str(value)
                .map_err(|err| format!("geoQ coordinates '{value}' are not JSON: {err}"))?,
            false => json!(value),
        };
        object.insert(key.trim().to_owned(), value);
    }
    for required in ["georel", "geometry", "coordinates"] {
        if !object.contains_key(required) {
            return Err(format!("geoQ '{written}' names no {required}"));
        }
    }
    Ok(Value::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_geo_query_becomes_the_object_the_broker_takes() {
        let query =
            geo_query("georel=near;maxDistance==2000&geometry=Point&coordinates=[13.4,52.5]")
                .expect("a full geoQ");
        assert_eq!(query["georel"], json!("near;maxDistance==2000"));
        assert_eq!(query["geometry"], json!("Point"));
        assert_eq!(query["coordinates"], json!([13.4, 52.5]));
    }

    #[test]
    fn half_a_geo_query_is_refused_rather_than_sent() {
        let reason = geo_query("georel=near&geometry=Point").expect_err("no coordinates");
        assert!(reason.contains("coordinates"), "{reason}");
    }

    #[test]
    fn the_id_is_the_manifests_and_names_the_space() {
        let sync = SubscriptionSync::new(
            "https://city.example/",
            "https://idm.example/realms/dev",
            "portal-reconciler".to_owned(),
            "s3cr3t".to_owned(),
            "hel.fi",
        );
        assert_eq!(
            sync.urn("air-quality", "alerts"),
            "urn:ngsi-ld:Subscription:hel.fi:air-quality:alerts"
        );
        // The same manifest in another space is another subscription; the space is in the id.
        assert_ne!(
            sync.urn("traffic", "alerts"),
            sync.urn("air-quality", "alerts")
        );
        // The base keeps no trailing slash, so no path is built with a double one.
        assert_eq!(sync.base, "https://city.example");
    }
}
