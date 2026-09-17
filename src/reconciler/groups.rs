//! The Keycloak groups the repository owns (PF-62, PF-63, T-0866).
//!
//! A `Group` manifest is the membership; Keycloak is where it takes effect. This wave is the
//! only writer of the groups it manages: each one carries the attribute `managed-by:
//! joinedcontext`, and a group without that attribute — the bootstrap administrators' group
//! included — is never read for drift, never written and never removed.
//!
//! What somebody changed in the console between two runs is overwritten and reported, so the
//! repository stays the answer to "who is in this group" and the Access page can show that the
//! two disagreed (PF-63, PF-51).

use std::collections::BTreeSet;
use std::time::Duration;

use jc_core::kinds::GroupSpec;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::permissions::ORG_NAMESPACE;
use crate::store::{ListOptions, Mirror};

/// The attribute that says a group is this platform's to write (PF-63).
pub const MANAGED_BY: &str = "managed-by";
pub const MANAGED_VALUE: &str = "joinedcontext";

const TIMEOUT: Duration = Duration::from_secs(20);

/// What one run did with one group, in the shape the mirror and the feed both carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupOutcome {
    pub name: String,
    /// Empty when the realm already matched the manifest.
    pub drift: Vec<String>,
    /// A member the realm has no user for: the binding takes effect at their first login
    /// (PF-04), so this is a warning and never a failure.
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

impl GroupOutcome {
    fn of(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            drift: Vec::new(),
            warnings: Vec::new(),
            error: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct KcGroup {
    id: String,
    name: String,
    #[serde(default)]
    attributes: std::collections::HashMap<String, Vec<String>>,
}

impl KcGroup {
    fn managed(&self) -> bool {
        self.attributes
            .get(MANAGED_BY)
            .is_some_and(|values| values.iter().any(|v| v == MANAGED_VALUE))
    }
}

#[derive(Debug, Deserialize)]
struct KcUser {
    id: String,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KcToken {
    access_token: String,
}

/// The realm's groups, as this Portal's own client may write them.
pub struct GroupSync {
    http: reqwest::Client,
    /// `https://host/realms/{realm}`: where the token comes from.
    issuer: String,
    /// `https://host/admin/realms/{realm}`: where the groups are.
    admin: String,
    client_id: String,
    client_secret: String,
}

impl GroupSync {
    /// The admin base of an issuer URL: `…/realms/{r}` becomes `…/admin/realms/{r}`. `None`
    /// when the issuer is not a realm URL, because then there is nothing to manage.
    fn admin_base(issuer: &str) -> Option<String> {
        let trimmed = issuer.trim_end_matches('/');
        let (root, realm) = trimmed.rsplit_once("/realms/")?;
        Some(format!("{root}/admin/realms/{realm}"))
    }

    /// `None` when no admin client is configured: the groups are then read from the repository
    /// and written nowhere, which is what a Portal without the credential does.
    pub fn new(issuer: &str, client_id: String, client_secret: String) -> Option<Self> {
        Some(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build().ok()?,
            issuer: issuer.trim_end_matches('/').to_owned(),
            admin: Self::admin_base(issuer)?,
            client_id,
            client_secret,
        })
    }

    async fn token(&self) -> Result<String, String> {
        let response = self
            .http
            .post(format!("{}/protocol/openid-connect/token", self.issuer))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "the realm refused the reconciler's client: {}",
                response.status()
            ));
        }
        let token: KcToken = response.json().await.map_err(|err| err.to_string())?;
        Ok(token.access_token)
    }

    async fn get(&self, token: &str, path: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(format!("{}{path}", self.admin))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.status().is_success() {
            return Err(format!("GET {path}: {}", response.status()));
        }
        response.json().await.map_err(|err| err.to_string())
    }

    async fn write(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(), String> {
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.admin))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|err| err.to_string())?;
        if !response.status().is_success() {
            return Err(format!("{method} {path}: {}", response.status()));
        }
        Ok(())
    }

    /// Brings the realm to what the manifests say, and answers what it had to change.
    ///
    /// The mirror is the repository as this run loaded it; only `Group` manifests of the
    /// organization namespace are read, and only groups carrying the managed attribute are
    /// written or removed.
    pub async fn converge(&self, mirror: &Mirror) -> Vec<GroupOutcome> {
        let token = match self.token().await {
            Ok(token) => token,
            Err(err) => {
                return vec![GroupOutcome {
                    name: "*".to_owned(),
                    drift: Vec::new(),
                    warnings: Vec::new(),
                    error: Some(err),
                }]
            }
        };

        let existing: Vec<KcGroup> = match self
            .get(&token, "/groups?briefRepresentation=false&max=1000")
            .await
            .and_then(|value| serde_json::from_value(value).map_err(|err| err.to_string()))
        {
            Ok(groups) => groups,
            Err(err) => {
                return vec![GroupOutcome {
                    name: "*".to_owned(),
                    drift: Vec::new(),
                    warnings: Vec::new(),
                    error: Some(err),
                }]
            }
        };

        let mut outcomes = Vec::new();
        let mut wanted: BTreeSet<String> = BTreeSet::new();
        for envelope in mirror
            .list(ORG_NAMESPACE, "Group", &ListOptions::default())
            .items
        {
            let name = envelope.metadata.name.clone();
            wanted.insert(name.clone());
            let spec: GroupSpec = match serde_json::from_value(envelope.spec) {
                Ok(spec) => spec,
                Err(err) => {
                    let mut outcome = GroupOutcome::of(&name);
                    outcome.error = Some(format!("the manifest does not parse: {err}"));
                    outcomes.push(outcome);
                    continue;
                }
            };
            let held = existing.iter().find(|group| group.name == name);
            outcomes.push(self.converge_one(&token, &name, &spec, held).await);
        }

        // A managed group the repository no longer declares is this wave's to remove; one
        // without the attribute belongs to whoever made it (PF-63).
        for group in existing.iter().filter(|g| g.managed()) {
            if wanted.contains(&group.name) {
                continue;
            }
            let mut outcome = GroupOutcome::of(&group.name);
            match self
                .write(
                    &token,
                    reqwest::Method::DELETE,
                    &format!("/groups/{}", group.id),
                    None,
                )
                .await
            {
                Ok(()) => outcome.drift.push(
                    "the group was managed here and no manifest declares it any more; it was \
                     removed from the realm"
                        .to_owned(),
                ),
                Err(err) => outcome.error = Some(err),
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    async fn converge_one(
        &self,
        token: &str,
        name: &str,
        spec: &GroupSpec,
        held: Option<&KcGroup>,
    ) -> GroupOutcome {
        let mut outcome = GroupOutcome::of(name);
        let id = match held {
            Some(group) if !group.managed() => {
                // Somebody else's group under the same name: it is not ours to write, and
                // saying so is the whole of the report (PF-63).
                outcome.error = Some(format!(
                    "the realm has a group '{name}' without the {MANAGED_BY}: {MANAGED_VALUE} \
                     attribute; the platform never writes a group it does not manage"
                ));
                return outcome;
            }
            Some(group) => group.id.clone(),
            None => {
                if let Err(err) = self
                    .write(
                        token,
                        reqwest::Method::POST,
                        "/groups",
                        Some(json!({
                            "name": name,
                            "attributes": { MANAGED_BY: [MANAGED_VALUE] }
                        })),
                    )
                    .await
                {
                    outcome.error = Some(err);
                    return outcome;
                }
                outcome
                    .drift
                    .push(format!("the realm had no group '{name}'; it was created"));
                match self
                    .get(token, &format!("/groups?search={name}&exact=true"))
                    .await
                    .and_then(|v| {
                        serde_json::from_value::<Vec<KcGroup>>(v).map_err(|e| e.to_string())
                    }) {
                    Ok(found) => match found.into_iter().find(|group| group.name == name) {
                        Some(group) => group.id,
                        None => {
                            outcome.error =
                                Some("the realm did not return the group it just created".into());
                            return outcome;
                        }
                    },
                    Err(err) => {
                        outcome.error = Some(err);
                        return outcome;
                    }
                }
            }
        };

        // Who is in the group now, by e-mail, against who the manifest says.
        let members: Vec<KcUser> = match self
            .get(token, &format!("/groups/{id}/members?max=1000"))
            .await
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
        {
            Ok(members) => members,
            Err(err) => {
                outcome.error = Some(err);
                return outcome;
            }
        };
        let held_by_email: Vec<(String, String)> = members
            .into_iter()
            .filter_map(|user| Some((user.email?.to_lowercase(), user.id)))
            .collect();
        let wanted: BTreeSet<String> = spec
            .members
            .iter()
            .map(|member| member.user.trim().to_lowercase())
            .collect();

        for email in &wanted {
            if held_by_email.iter().any(|(held, _)| held == email) {
                continue;
            }
            match self.user_id(token, email).await {
                Ok(Some(user)) => {
                    match self
                        .write(
                            token,
                            reqwest::Method::PUT,
                            &format!("/users/{user}/groups/{id}"),
                            None,
                        )
                        .await
                    {
                        Ok(()) => outcome
                            .drift
                            .push(format!("{email} is in the manifest and was added")),
                        Err(err) => outcome.error = Some(err),
                    }
                }
                // Not an error: the person joins the group the moment the realm knows them.
                Ok(None) => outcome.warnings.push(format!(
                    "{email} is in the manifest and the realm has no user with that address yet"
                )),
                Err(err) => outcome.error = Some(err),
            }
        }

        for (email, user) in &held_by_email {
            if wanted.contains(email) {
                continue;
            }
            match self
                .write(
                    token,
                    reqwest::Method::DELETE,
                    &format!("/users/{user}/groups/{id}"),
                    None,
                )
                .await
            {
                Ok(()) => outcome.drift.push(format!(
                    "{email} was in the realm's group and in no manifest; the membership was \
                     removed"
                )),
                Err(err) => outcome.error = Some(err),
            }
        }
        outcome
    }

    /// The realm's user id for an address, or `None` when nobody has signed in under it yet.
    async fn user_id(&self, token: &str, email: &str) -> Result<Option<String>, String> {
        let users: Vec<KcUser> = self
            .get(
                token,
                &format!("/users?exact=true&email={}", urlencoding(email)),
            )
            .await
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))?;
        Ok(users
            .into_iter()
            .find(|user| {
                user.email
                    .as_deref()
                    .is_some_and(|held| held.eq_ignore_ascii_case(email))
            })
            .map(|user| user.id))
    }
}

/// The few characters an address may carry that a query string may not (`+` above all).
fn urlencoding(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' | '@' => c.to_string(),
            other => other
                .to_string()
                .as_bytes()
                .iter()
                .map(|b| format!("%{b:02X}"))
                .collect(),
        })
        .collect()
}

/// What the Access page reads: the drift and the warnings of one run, on the Group manifest
/// itself, so a person sees the disagreement where the membership is (PF-62, UI-44).
pub fn record(mirror: &Mirror, outcomes: &[GroupOutcome]) {
    for outcome in outcomes {
        let Some(mut envelope) = mirror.get(ORG_NAMESPACE, "Group", &outcome.name) else {
            continue;
        };
        let Some(status) = envelope.status.as_mut() else {
            continue;
        };
        if let Some(error) = &outcome.error {
            status.phase = crate::resource::Phase::Error;
            status.conditions = vec![super::streams::make_condition(
                "GroupSynced",
                "False",
                "RealmRefused",
                error,
            )];
        } else if !outcome.drift.is_empty() {
            status.conditions = vec![super::streams::make_condition(
                "GroupSynced",
                "True",
                "DriftCorrected",
                &outcome.drift.join("; "),
            )];
        } else if !outcome.warnings.is_empty() {
            status.conditions = vec![super::streams::make_condition(
                "GroupSynced",
                "True",
                "MemberUnknown",
                &outcome.warnings.join("; "),
            )];
        }
        mirror.upsert(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_admin_base_is_the_realm_url_with_admin_in_front_of_realms() {
        assert_eq!(
            GroupSync::admin_base("https://id.example.sk/realms/bb"),
            Some("https://id.example.sk/admin/realms/bb".to_owned())
        );
        assert_eq!(
            GroupSync::admin_base("https://id.example.sk/realms/bb/"),
            Some("https://id.example.sk/admin/realms/bb".to_owned())
        );
        assert_eq!(GroupSync::admin_base("https://id.example.sk/"), None);
    }

    #[test]
    fn an_address_is_safe_in_a_query_string() {
        assert_eq!(urlencoding("jana+test@hel.fi"), "jana%2Btest@hel.fi");
        assert_eq!(urlencoding("jana@hel.fi"), "jana@hel.fi");
    }

    #[test]
    fn only_a_group_carrying_the_attribute_is_ours() {
        let ours = KcGroup {
            id: "1".into(),
            name: "city-leads".into(),
            attributes: [(MANAGED_BY.to_owned(), vec![MANAGED_VALUE.to_owned()])]
                .into_iter()
                .collect(),
        };
        let theirs = KcGroup {
            id: "2".into(),
            name: "platform-admins".into(),
            attributes: Default::default(),
        };
        assert!(ours.managed());
        assert!(!theirs.managed());
    }
}
