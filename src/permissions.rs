//! Roles as code, enforced (T-0526, PF-50, PF-51): the `Role` and `RoleBinding` manifests of
//! the organization repository decide who may propose, approve and delete what, per project.
//! The token contributes identity only (`sub`, e-mail, username, groups); no permission is
//! read from it. A caller without a binding reads and proposes nothing.

use chrono::{DateTime, Utc};
use jc_core::kinds::{Constraint, RoleBindingSpec, RoleSpec, Rule, Verb};
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
    /// Set when the binding is scoped to one context space: the rule then applies only to a
    /// manifest whose `spec.contextSpaceRef` names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[schema(value_type = Object)]
    pub rule: Rule,
}

/// What one caller may do in one project: `GET /api/v1/projects/{project}/permissions/me`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Effective {
    pub project: String,
    /// The caller is in the bootstrap group of `JC_PORTAL_BOOTSTRAP_ADMINS`: everything,
    /// everywhere, so the first binding can be written into an empty repository.
    pub bootstrap: bool,
    pub grants: Vec<Grant>,
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
        };
    }
    let opts = ListOptions::default();
    let roles: Vec<(String, RoleSpec)> = mirror
        .list(ORG_NAMESPACE, "Role", &opts)
        .items
        .into_iter()
        .filter_map(|env| match serde_json::from_value::<RoleSpec>(env.spec) {
            Ok(spec) => Some((env.metadata.name, spec)),
            Err(e) => {
                tracing::warn!(role = %env.metadata.name, error = %e, "Role in the mirror does not parse; it grants nothing");
                None
            }
        })
        .collect();

    let mut grants = Vec::new();
    for env in mirror.list(ORG_NAMESPACE, "RoleBinding", &opts).items {
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
        let space = match (
            &binding.scope.organization,
            &binding.scope.project,
            &binding.scope.context_space,
        ) {
            (Some(_), _, _) => None,
            (_, Some(p), _) if p == project => None,
            (_, _, Some(space)) => Some(space.clone()),
            _ => continue,
        };
        let Some((role_name, role)) = roles.iter().find(|(name, _)| *name == binding.role) else {
            tracing::warn!(binding = %env.metadata.name, role = %binding.role, "RoleBinding names a Role the repository lacks");
            continue;
        };
        for rule in &role.rules {
            grants.push(Grant {
                role: role_name.clone(),
                binding: env.metadata.name.clone(),
                space: space.clone(),
                rule: rule.clone(),
            });
        }
    }
    Effective {
        project: project.to_owned(),
        bootstrap: false,
        grants,
    }
}

impl Effective {
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
        Err(ApiError::Denied(violation.unwrap_or_else(|| {
            format!(
                "no role grants {} on {kind} in project {} (PF-50)",
                verb_name(verb),
                self.project
            )
        })))
    }
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
        Verb::Propose => "propose",
        Verb::Approve => "approve",
        Verb::Delete => "delete",
    }
}
