//! What a new Context Space is called (PF-76, PF-44).
//!
//! A space name is the `{space}` segment of every URN it holds, so it is unique across the
//! organization and not merely inside one project. One function answers what to call a new
//! space, one gate refuses a name another project already holds, and both name that project
//! only to a caller who may read it (PF-59): to everyone else the name is simply "taken".

use crate::auth::Identity;
use crate::error::ApiError;
use crate::state::AppState;

/// What to call a new Context Space, and why (PF-76).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// The name to use: `wanted` when it is free across the organization, `{project}-{wanted}`
    /// otherwise.
    pub name: String,
    /// Whether [`Proposal::name`] is free. `false` means the prefixed name is taken too, and
    /// the caller has to choose another.
    pub available: bool,
    /// Why the bare name was not proposed, in the caller's own terms. `None` when `wanted` was
    /// free and is the proposal.
    pub reason: Option<String>,
}

/// The project holding the Context Space called `name`, if any project does.
fn owner(state: &AppState, name: &str) -> Option<String> {
    state
        .mirror
        .find(|env| env.kind == "ContextSpace" && env.metadata.name == name)
        .and_then(|env| env.metadata.namespace)
}

/// How a collision is described to this caller (PF-59, PF-76): the project that holds the name
/// when the caller may read Context Spaces there, and nothing but "taken" when they may not.
fn taken_by(state: &AppState, identity: &Identity, owner: &str) -> String {
    if crate::permissions::for_request(state, identity, owner).may_read("ContextSpace") {
        format!("taken by project {owner}")
    } else {
        "taken".to_owned()
    }
}

/// The name a new Context Space should carry in `project` (PF-76).
///
/// `{project}-{wanted}` by default, the bare `wanted` when no project in the organization holds
/// it. Every door proposes through this function, so the Portal's form, the assistant's drafts
/// and `jc_space_complete` cannot disagree about a name.
pub fn propose_name(
    state: &AppState,
    identity: &Identity,
    project: &str,
    wanted: &str,
) -> Proposal {
    let Some(holder) = owner(state, wanted) else {
        return Proposal {
            name: wanted.to_owned(),
            available: true,
            reason: None,
        };
    };
    let reason = taken_by(state, identity, &holder);
    let prefixed = format!("{project}-{wanted}");
    let available = owner(state, &prefixed).is_none();
    Proposal {
        name: prefixed,
        available,
        reason: Some(reason),
    }
}

/// Refuses a Context Space whose name another project already holds (PF-44, PF-76).
///
/// Called on every write before a Change exists, so the route, an operation, the assistant, an
/// import and a dry run all answer the same refusal — and it carries the name to use instead.
/// A space of this name in this project is this project's own space: an update, not a clash.
pub fn check(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    name: &str,
) -> Result<(), ApiError> {
    if kind != "ContextSpace" {
        return Ok(());
    }
    let Some(holder) = owner(state, name) else {
        return Ok(());
    };
    if holder == project {
        return Ok(());
    }
    let reason = taken_by(state, identity, &holder);
    let proposal = propose_name(state, identity, project, name);
    let instead = if proposal.available {
        format!("; propose '{}' instead", proposal.name)
    } else {
        String::new()
    };
    Err(ApiError::Denied(format!(
        "context space name '{name}' is {reason}: a space name is unique in the organization \
         (PF-44){instead}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::permissions::ORG_NAMESPACE;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::{json, Value};

    fn who(email: &str) -> Identity {
        Identity {
            subject: format!("f:1:{email}"),
            username: email.split('@').next().unwrap_or(email).to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
        }
    }

    fn manifest(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    /// A world where `ovzdusie` already holds the space `mhd`, plus whatever the test adds.
    fn world(extra: Vec<ResourceEnvelope>) -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(manifest(
            "ContextSpace",
            "mhd",
            "ovzdusie",
            json!({ "isSandbox": false }),
        ));
        for env in extra {
            state.mirror.upsert(env);
        }
        state
    }

    /// The binding that lets `jana@hel.fi` read Context Spaces in `project`.
    fn reader_in(project: &str) -> Vec<ResourceEnvelope> {
        vec![
            manifest(
                "Role",
                "space-reader",
                ORG_NAMESPACE,
                json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
            ),
            manifest(
                "RoleBinding",
                "jana-reads",
                ORG_NAMESPACE,
                json!({
                    "subjects": [{ "user": "jana@hel.fi" }],
                    "role": "space-reader",
                    "scope": { "project": project },
                }),
            ),
        ]
    }

    #[test]
    fn a_free_name_is_proposed_bare() {
        let state = world(Vec::new());
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "parkovanie");
        assert_eq!(proposal.name, "parkovanie");
        assert!(proposal.available);
        assert_eq!(proposal.reason, None);
    }

    #[test]
    fn a_taken_name_is_proposed_with_the_project_in_front_and_names_the_holder_to_a_reader() {
        let state = world(reader_in("ovzdusie"));
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "mhd");
        assert_eq!(proposal.name, "doprava-mhd");
        assert!(proposal.available);
        assert_eq!(
            proposal.reason.as_deref(),
            Some("taken by project ovzdusie")
        );
    }

    #[test]
    fn a_taken_name_says_only_taken_to_someone_who_may_not_read_the_project_that_holds_it() {
        // Bound in a project of their own, so they are not a stranger to the Portal — only to
        // the project that holds the name (PF-59, R20).
        let state = world(reader_in("doprava"));
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "mhd");
        assert_eq!(proposal.name, "doprava-mhd");
        assert_eq!(proposal.reason.as_deref(), Some("taken"));
    }

    #[test]
    fn both_names_taken_is_answered_and_not_proposed() {
        let state = world(vec![manifest(
            "ContextSpace",
            "doprava-mhd",
            "helsinki",
            json!({ "isSandbox": false }),
        )]);
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "mhd");
        assert_eq!(proposal.name, "doprava-mhd");
        assert!(!proposal.available);
    }

    #[test]
    fn the_gate_refuses_another_projects_name_and_lets_the_projects_own_space_through() {
        let state = world(reader_in("ovzdusie"));
        let jana = who("jana@hel.fi");
        let refused = check(&state, &jana, "doprava", "ContextSpace", "mhd")
            .expect_err("another project holds it");
        let said = format!("{refused:?}");
        assert!(said.contains("taken by project ovzdusie"), "{said}");
        assert!(said.contains("doprava-mhd"), "{said}");

        // The same name in the project that holds it is an update of that space.
        check(&state, &jana, "ovzdusie", "ContextSpace", "mhd").expect("its own space");
        // Every other kind is named inside its project and is none of this gate's business.
        check(&state, &jana, "doprava", "Pipeline", "mhd").expect("not a space");
    }
}
