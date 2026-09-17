//! Roles as code, enforced (T-0526, PF-50, PF-51): the `Role` and `RoleBinding` manifests of
//! the organization repository decide who may propose, approve and delete what, per project.
//! The token contributes identity only (`sub`, e-mail, username, groups); no permission is
//! read from it. A caller without a binding reads and proposes nothing.

use chrono::{DateTime, Utc};
use jc_core::kinds::{
    Constraint, RoleBindingSpec, RoleScope, RoleSpec, Rule, ServiceAccountSpec, Verb,
};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::state::AppState;
use crate::store::{ListOptions, Mirror};

/// The namespace the organization repository's `users/` manifests carry.
pub const ORG_NAMESPACE: &str = "org";

/// One rule in force, with the binding and role it came through, so a refusal can be traced.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Grant {
    pub role: String,
    pub binding: String,
    /// Where the binding that carries this rule applies: `organization`, `project:{name}` or
    /// `contextSpace:{name}`. A grant read here may have been inherited from the organization,
    /// and the page says so rather than making it look local (PF-60, PF-61).
    pub scope: String,
    /// Set when the binding is scoped to one context space: the rule then applies only to a
    /// manifest whose `spec.contextSpaceRef` names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[schema(value_type = Object)]
    pub rule: Rule,
}

/// Something the caller may or may not do that no rule expresses as a kind and a verb, with the
/// API's own words for the refusal: the control is rendered disabled with the reason, never
/// hidden (UI-44).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Affordance {
    pub allowed: bool,
    /// Why not; absent when the caller may.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What the organization's own settings let this caller do with projects (PF-65).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProjectAffordances {
    /// Whether `POST /api/v1/projects` would open a project for this caller.
    pub creation: Affordance,
}

/// What one caller may do in one project: `GET /api/v1/projects/{project}/permissions/me`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Effective {
    pub project: String,
    /// The caller is in the bootstrap group of `JC_PORTAL_BOOTSTRAP_ADMINS`: everything,
    /// everywhere, so the first binding can be written into an empty repository.
    pub bootstrap: bool,
    pub grants: Vec<Grant>,
    /// Filled by the route, not by the rules: opening a project is the organization's own
    /// setting and no binding expresses it (PF-65, T-0870).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<ProjectAffordances>,
}

/// The effective permissions of the signed-in caller in `project`, right now.
pub fn for_request(state: &AppState, identity: &Identity, project: &str) -> Effective {
    effective(
        &state.mirror,
        &state.config.bootstrap_admins,
        identity,
        project,
        Utc::now(),
    )
}

/// Reads every `Role` and `RoleBinding` of the mirror and keeps the rules whose binding names
/// the caller, covers the project and is in force at `now`.
pub fn effective(
    mirror: &Mirror,
    bootstrap_group: &str,
    identity: &Identity,
    project: &str,
    now: DateTime<Utc>,
) -> Effective {
    if in_group(identity, bootstrap_group) {
        return Effective {
            project: project.to_owned(),
            bootstrap: true,
            grants: Vec::new(),
            projects: None,
        };
    }
    let grants = in_force(mirror, identity, now)
        .into_iter()
        .filter_map(|(reach, mut grant)| {
            grant.space = match reach {
                Reach::Organization => None,
                Reach::Project(p) if p == project => None,
                Reach::Project(_) => return None,
                Reach::Space(space) => Some(space),
            };
            Some(grant)
        })
        .collect();
    Effective {
        project: project.to_owned(),
        bootstrap: false,
        grants,
        projects: None,
    }
}

impl Effective {
    /// Whether the caller may read anything in this project (PF-59): a binding whose scope
    /// covers it, or the bootstrap group. What is not readable is `404` and not `403`, so a
    /// project nobody bound the caller to reads like a project that is not there (R20).
    pub fn may_read_project(&self) -> bool {
        self.bootstrap || !self.grants.is_empty()
    }

    /// Whether the caller may read `kind` here (PF-59). `propose` on a kind implies `read` on
    /// it, which is what keeps a role written before the verb working (jc-core `Rule::grants`).
    pub fn may_read(&self, kind: &str) -> bool {
        self.bootstrap
            || self
                .grants
                .iter()
                .any(|grant| grant.rule.grants(kind, Verb::Read))
    }

    /// Whether the caller may read this one manifest (PF-59, PF-60): a grant that reads the
    /// kind, and — when the binding is scoped to one context space — a manifest of that space.
    /// This is what an organization-level list filters with, item by item.
    pub fn may_read_manifest(&self, kind: &str, manifest: &Value) -> bool {
        if self.bootstrap {
            return true;
        }
        let space = space_ref(manifest);
        self.grants.iter().any(|grant| {
            grant.rule.grants(kind, Verb::Read)
                && match &grant.space {
                    None => true,
                    Some(bound) => space.as_deref() == Some(bound.as_str()),
                }
        })
    }

    /// `Ok` when a grant allows `verb` on `kind` for `target` (the whole manifest as JSON, when
    /// there is one); a 403 that names the missing verb or the violated constraint otherwise.
    pub fn check(&self, kind: &str, verb: Verb, target: Option<&Value>) -> Result<(), ApiError> {
        if self.bootstrap {
            return Ok(());
        }
        let space_of_target = target.and_then(space_ref);
        let mut violation: Option<String> = None;
        for grant in &self.grants {
            let rule = &grant.rule;
            if !rule.kinds.iter().any(|k| k == kind) || !rule.verbs.contains(&verb) {
                continue;
            }
            if let Some(space) = &grant.space {
                if space_of_target.as_deref() != Some(space.as_str()) {
                    continue;
                }
            }
            match rule.constraints.iter().find(|c| !satisfied(c, target)) {
                None => return Ok(()),
                Some(c) => {
                    violation.get_or_insert_with(|| {
                        format!(
                            "{} does not satisfy role {} ({})",
                            c.field,
                            grant.role,
                            describe(c)
                        )
                    });
                }
            }
        }
        // Letting data out to the public is a right of its own, and the one refusal that says
        // which role holds it: every door goes through this check, so the Approvals page, the
        // operations registry and the assistant all say the same sentence (EP-76, PF-71, PF-72).
        if kind == "Endpoint" && verb == Verb::Approve && audience_of(target) == Some("public") {
            return Err(ApiError::Denied(
                "approving a public Endpoint needs publisher, the role whose approve is \
                 constrained to a public audience; org-admin holds it too (EP-76, PF-71)"
                    .to_owned(),
            ));
        }
        Err(ApiError::Denied(violation.unwrap_or_else(|| {
            format!(
                "no role grants {} on {kind} in project {} (PF-50)",
                verb_name(verb),
                self.project
            )
        })))
    }
}

/// Where a binding applies.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reach {
    Organization,
    Project(String),
    Space(String),
}

impl Reach {
    /// What `permissions/me` calls this scope (PF-61).
    fn name(&self) -> String {
        match self {
            Self::Organization => "organization".to_owned(),
            Self::Project(project) => format!("project:{project}"),
            Self::Space(space) => format!("contextSpace:{space}"),
        }
    }

    fn of(scope: &RoleScope) -> Option<Self> {
        match (&scope.organization, &scope.project, &scope.context_space) {
            (Some(_), _, _) => Some(Self::Organization),
            (_, Some(project), _) => Some(Self::Project(project.clone())),
            (_, _, Some(space)) => Some(Self::Space(space.clone())),
            _ => None,
        }
    }

    /// Whether a grant held here also holds at `target`: the organization covers everything, a
    /// project its own context spaces.
    fn covers(&self, target: &Self, mirror: &Mirror) -> bool {
        match (self, target) {
            (Self::Organization, _) => true,
            (Self::Project(held), Self::Project(wanted)) => held == wanted,
            (Self::Project(held), Self::Space(space)) => {
                mirror.get(held, "ContextSpace", space).is_some()
            }
            (Self::Space(held), Self::Space(wanted)) => held == wanted,
            _ => false,
        }
    }
}

/// The roles of one namespace: the organization's `users/roles/`, or one project's own (PF-68).
fn roles(mirror: &Mirror, namespace: &str) -> Vec<(String, RoleSpec)> {
    mirror
        .list(namespace, "Role", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|env| match serde_json::from_value::<RoleSpec>(env.spec) {
            Ok(spec) => Some((env.metadata.name, spec)),
            Err(e) => {
                tracing::warn!(role = %env.metadata.name, error = %e, "Role in the mirror does not parse; it grants nothing");
                None
            }
        })
        .collect()
}

/// The project a context space belongs to, which is the project whose roles a binding scoped to
/// that space may reach (PF-69).
fn project_of_space(mirror: &Mirror, space: &str) -> Option<String> {
    mirror
        .find(|env| env.kind == "ContextSpace" && env.metadata.name == space)
        .and_then(|env| env.metadata.namespace)
}

/// The role a binding names, looked up where the binding reaches it: the organization's roles
/// first, then the roles of the project its scope names (PF-68, PF-69).
///
/// A name in both places is refused by `jcctl validate` before the manifest ever lands, so the
/// organization's copy winning here is a tie that cannot happen, not a precedence rule.
fn role_of(
    mirror: &Mirror,
    organization: &[(String, RoleSpec)],
    reach: &Reach,
    name: &str,
) -> Option<(String, RoleSpec)> {
    if let Some((found, spec)) = organization.iter().find(|(role, _)| role == name) {
        return Some((found.clone(), spec.clone()));
    }
    let project = match reach {
        Reach::Organization => return None,
        Reach::Project(project) => project.clone(),
        Reach::Space(space) => project_of_space(mirror, space)?,
    };
    roles(mirror, &project)
        .into_iter()
        .find(|(role, _)| role == name)
}

/// Every rule a binding in force at `now` gives the caller, with where the binding applies.
fn in_force(mirror: &Mirror, identity: &Identity, now: DateTime<Utc>) -> Vec<(Reach, Grant)> {
    let organization = roles(mirror, ORG_NAMESPACE);
    let mut grants = Vec::new();
    for env in mirror
        .list(ORG_NAMESPACE, "RoleBinding", &ListOptions::default())
        .items
    {
        let binding = match serde_json::from_value::<RoleBindingSpec>(env.spec) {
            Ok(spec) => spec,
            Err(e) => {
                tracing::warn!(binding = %env.metadata.name, error = %e, "RoleBinding in the mirror does not parse; it grants nothing");
                continue;
            }
        };
        if !binding.subjects.iter().any(|s| is_subject(identity, s)) {
            continue;
        }
        if binding.validity.as_ref().is_some_and(|v| !v.contains(now)) {
            continue;
        }
        let Some(reach) = Reach::of(&binding.scope) else {
            continue;
        };
        let Some((role_name, role)) = role_of(mirror, &organization, &reach, &binding.role) else {
            tracing::warn!(binding = %env.metadata.name, role = %binding.role, "RoleBinding names a Role it does not reach");
            continue;
        };
        for rule in &role.rules {
            grants.push((
                reach.clone(),
                Grant {
                    role: role_name.clone(),
                    binding: env.metadata.name.clone(),
                    scope: reach.name(),
                    space: None,
                    rule: rule.clone(),
                },
            ));
        }
    }
    grants
}

/// Every `group` subject of a `RoleBinding` or `ServiceAccount` names a `Group` manifest of the
/// organization (PF-62, PF-64). A binding to a group nobody declared matches nobody and says
/// nothing about it, which is the silence this refusal replaces. The bootstrap administrators
/// are not a subject — they are the platform setting of PF-52 — so nothing here touches them.
fn subjects_name_a_group(mirror: &Mirror, manifest: &Value) -> Result<(), ApiError> {
    let kind = manifest.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind != "RoleBinding" && kind != "ServiceAccount" {
        return Ok(());
    }
    let named = manifest
        .pointer("/spec/subjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|subject| subject.get("group").and_then(Value::as_str));
    for group in named {
        if mirror.get(ORG_NAMESPACE, "Group", group).is_none() {
            let declared: Vec<String> = mirror
                .list(ORG_NAMESPACE, "Group", &ListOptions::default())
                .items
                .into_iter()
                .map(|env| env.metadata.name)
                .collect();
            return Err(ApiError::BadRequest(format!(
                "spec.subjects names the group '{group}', and no Group manifest declares it;                  propose the group first, or a binding to it matches nobody (PF-62, PF-64).                  Declared: {}",
                if declared.is_empty() {
                    "none".to_owned()
                } else {
                    declared.join(", ")
                }
            )));
        }
    }
    Ok(())
}

/// Nobody grants above their own rights (PF-52, AG-77): every verb on every kind a proposed
/// `Role` (on the organization), `RoleBinding` (on its scope) or `ServiceAccount` (through each of
/// its roles the organization defines, on that role's scope) would grant must be one `identity`
/// holds there, under no constraint the new rule drops. `who` is "proposer" or "approver"; any
/// other kind and the bootstrap group pass.
pub fn within_own_rights(
    state: &AppState,
    identity: &Identity,
    manifest: &Value,
    who: &str,
) -> Result<(), ApiError> {
    let mirror = &state.mirror;
    // PF-64: a subject that names a group names a `Group` manifest. Checked here because this
    // is the gate every door to a `users/` manifest passes through — the resource route, an
    // import, a blueprint and an approval.
    subjects_name_a_group(mirror, manifest)?;
    let spec = manifest.get("spec").cloned().unwrap_or(Value::Null);
    let unreadable = |e: serde_json::Error| ApiError::BadRequest(format!("spec: {e}"));
    let no_scope = || {
        ApiError::BadRequest("spec.scope names no organization, project or context space".into())
    };
    let organization = roles(mirror, ORG_NAMESPACE);
    // The role a manifest names, read where that manifest reaches it: a binding at project
    // scope may name the project's own role, an organization one may not (PF-69).
    let rules_at = |name: &str, target: &Reach| {
        role_of(mirror, &organization, target, name).map(|(_, spec)| spec.rules)
    };
    let (noun, grants): (&str, Vec<(Vec<Rule>, Reach)>) =
        match manifest.get("kind").and_then(Value::as_str) {
            Some("Role") => {
                let role: RoleSpec = serde_json::from_value(spec).map_err(unreadable)?;
                // A role of a project is measured against what its proposer holds in that project,
                // an organization role against what they hold organization-wide (PF-68).
                let namespace = manifest
                    .pointer("/metadata/namespace")
                    .and_then(Value::as_str)
                    .unwrap_or(ORG_NAMESPACE);
                let at = match namespace {
                    ORG_NAMESPACE | "" => Reach::Organization,
                    project => Reach::Project(project.to_owned()),
                };
                ("role", vec![(role.rules, at)])
            }
            Some("RoleBinding") => {
                let binding: RoleBindingSpec = serde_json::from_value(spec).map_err(unreadable)?;
                let target = Reach::of(&binding.scope).ok_or_else(no_scope)?;
                let rules = rules_at(&binding.role, &target).ok_or_else(|| {
                    ApiError::Denied(format!(
                        "no role {} is defined where this binding applies; propose the role \
                     before a binding to it (PF-52, PF-69)",
                        binding.role
                    ))
                })?;
                ("binding", vec![(rules, target)])
            }
            // A service account's other roles are the gateway's role templates, which grant data
            // access through Policies and nothing here (CC-60).
            Some("ServiceAccount") => {
                let account: ServiceAccountSpec =
                    serde_json::from_value(spec).map_err(unreadable)?;
                let mut grants = Vec::new();
                for granted in &account.roles {
                    let target = Reach::of(&granted.scope).ok_or_else(no_scope)?;
                    if let Some(rules) = rules_at(&granted.role, &target) {
                        grants.push((rules, target));
                    }
                }
                ("service account", grants)
            }
            _ => return Ok(()),
        };
    if in_group(identity, &state.config.bootstrap_admins) {
        return Ok(());
    }
    let held = in_force(mirror, identity, Utc::now());
    let mut missing: Vec<String> = Vec::new();
    for (rules, target) in &grants {
        for rule in rules {
            for kind in &rule.kinds {
                for verb in &rule.verbs {
                    let holds = held.iter().any(|(reach, grant)| {
                        reach.covers(target, mirror)
                            && grant.rule.kinds.contains(kind)
                            && grant.rule.verbs.contains(verb)
                            && grant
                                .rule
                                .constraints
                                .iter()
                                .all(|c| rule.constraints.contains(c))
                    });
                    let item = format!("{} on {kind}", verb_name(*verb));
                    if !holds && !missing.contains(&item) {
                        missing.push(item);
                    }
                }
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(ApiError::Denied(format!(
        "a {noun} may not grant more than its {who} holds: missing {} (PF-52)",
        missing.join(", ")
    )))
}

fn in_group(identity: &Identity, group: &str) -> bool {
    // Keycloak realm roles count as groups here: the dev realm assigns people through them
    // and a realm role, like a group, says who somebody is, not what they may do (PF-50).
    identity.groups.iter().any(|g| g == group) || identity.roles.iter().any(|r| r == group)
}

fn is_subject(identity: &Identity, subject: &jc_core::kinds::Subject) -> bool {
    if let Some(user) = &subject.user {
        return identity
            .email
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case(user))
            || identity.username.eq_ignore_ascii_case(user)
            || identity.subject == *user;
    }
    subject
        .group
        .as_deref()
        .is_some_and(|g| in_group(identity, g))
}

/// `spec.contextSpaceRef` of a manifest, written bare or as `{ name }`.
/// The audience of the manifest under approval, when it has one (EP-14).
fn audience_of(target: Option<&Value>) -> Option<&str> {
    target?.pointer("/spec/audience")?.as_str()
}

fn space_ref(target: &Value) -> Option<String> {
    let value = target.pointer("/spec/contextSpaceRef")?;
    value
        .as_str()
        .or_else(|| value.get("name").and_then(Value::as_str))
        .map(str::to_owned)
}

/// The value at a dotted path of the manifest, as text; `None` when absent or structured.
fn field_text(target: Option<&Value>, path: &str) -> Option<String> {
    let mut cursor = target?;
    for segment in path.split('.') {
        cursor = cursor.get(segment)?;
    }
    match cursor {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn satisfied(constraint: &Constraint, target: Option<&Value>) -> bool {
    let value = field_text(target, &constraint.field);
    if let Some(expected) = &constraint.equals {
        return value.as_deref() == Some(expected.as_str());
    }
    if !constraint.one_of.is_empty() {
        return value.is_some_and(|v| constraint.one_of.contains(&v));
    }
    !value.is_some_and(|v| constraint.not_in.contains(&v))
}

fn describe(constraint: &Constraint) -> String {
    if let Some(expected) = &constraint.equals {
        format!("must equal {expected}")
    } else if !constraint.one_of.is_empty() {
        format!("must be one of {}", constraint.one_of.join(", "))
    } else {
        format!("must not be one of {}", constraint.not_in.join(", "))
    }
}

pub fn verb_name(verb: Verb) -> &'static str {
    match verb {
        Verb::Read => "read",
        Verb::Propose => "propose",
        Verb::Approve => "approve",
        Verb::Delete => "delete",
    }
}
