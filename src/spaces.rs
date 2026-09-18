//! What a new Context Space is called, and which `{space}` segment its ids carry (PF-76,
//! PF-84, PF-44).
//!
//! A space is named locally, in its project; the `{space}` segment of every URN it holds is
//! rendered as `{project}-{name}` unless `spec.urnSegment` pins it (PF-84), and that segment is
//! what is unique across the organization. One function renders it, one gate refuses a segment
//! another space already renders, and both name the other project only to a caller who may
//! read it (PF-59): to everyone else the segment is simply "taken".

use crate::auth::Identity;
use crate::error::ApiError;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::Mirror;
use serde_json::Value;

/// What to call a new Context Space, and why (PF-76).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// The local name to use: `wanted`, because a name is only unique in its project.
    pub name: String,
    /// Whether the segment [`Proposal::name`] renders is free in the organization. `false`
    /// means another space pins that segment, and the caller has to choose another name.
    pub available: bool,
    /// Why the name is not available, in the caller's own terms. `None` when it is.
    pub reason: Option<String>,
}

/// The pin a space manifest carries, if any (PF-84).
fn pin_of(spec: &Value) -> Option<&str> {
    spec.get("urnSegment").and_then(Value::as_str)
}

/// The `{space}` segment of the Context Space `name` of `project` (PF-84).
///
/// The one function every Portal path that mints an id, names a tenant or writes a URN calls,
/// through `jc_core::kinds::urn_segment`, which the gateway and the reconciler call too. A space
/// the mirror does not hold renders without a pin.
pub fn segment(mirror: &Mirror, project: &str, name: &str) -> String {
    let space = mirror.get(project, "ContextSpace", name);
    jc_core::kinds::urn_segment(
        project,
        name,
        space.as_ref().and_then(|env| pin_of(&env.spec)),
    )
}

fn segment_of(env: &ResourceEnvelope) -> String {
    let project = env.metadata.namespace.as_deref().unwrap_or_default();
    jc_core::kinds::urn_segment(project, &env.metadata.name, pin_of(&env.spec))
}

/// The project of another Context Space that renders `wanted`, if any does.
fn holder(state: &AppState, wanted: &str, project: &str, name: &str) -> Option<String> {
    state
        .mirror
        .find(|env| {
            env.kind == "ContextSpace"
                && !(env.metadata.namespace.as_deref() == Some(project)
                    && env.metadata.name == name)
                && segment_of(env) == wanted
        })
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

/// The name a new Context Space should carry in `project` (PF-76, PF-84).
///
/// The local name as asked: two projects may each hold a space called `air`, because their
/// ids render `{project}-air`. Every door proposes through this function, so the Portal's form,
/// the assistant's drafts and `jc_space_complete` cannot disagree about a name.
pub fn propose_name(
    state: &AppState,
    identity: &Identity,
    project: &str,
    wanted: &str,
) -> Proposal {
    let rendered = jc_core::kinds::urn_segment(project, wanted, None);
    let reason = holder(state, &rendered, project, wanted)
        .map(|owner| format!("'{rendered}' is {}", taken_by(state, identity, &owner)));
    Proposal {
        name: wanted.to_owned(),
        available: reason.is_none(),
        reason,
    }
}

/// Refuses a Context Space whose `{space}` segment another space already renders (PF-44,
/// PF-76, PF-84).
///
/// Called on every write before a Change exists, so the route, an operation, the assistant, an
/// import and a dry run all answer the same refusal. The same space written again is an
/// update, not a clash.
pub fn check(
    state: &AppState,
    identity: &Identity,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    if kind != "ContextSpace" {
        return Ok(());
    }
    let rendered = jc_core::kinds::urn_segment(project, name, pin_of(spec));
    let Some(owner) = holder(state, &rendered, project, name) else {
        return Ok(());
    };
    let reason = taken_by(state, identity, &owner);
    Err(ApiError::Denied(format!(
        "the entity ids of context space '{name}' would carry '{rendered}', which is {reason}: \
         an id segment is unique in the organization (PF-84); choose another name or \
         spec.urnSegment"
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
    fn a_name_is_proposed_as_asked_and_renders_under_its_project() {
        let state = world(Vec::new());
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "mhd");
        assert_eq!(
            proposal.name, "mhd",
            "ovzdusie/mhd renders ovzdusie-mhd, so mhd is free here"
        );
        assert!(proposal.available);
        assert_eq!(proposal.reason, None);
        assert_eq!(segment(&state.mirror, "ovzdusie", "mhd"), "ovzdusie-mhd");
        assert_eq!(
            segment(&state.mirror, "doprava", "nothing-yet"),
            "doprava-nothing-yet"
        );
    }

    #[test]
    fn a_pin_wins_and_takes_its_segment_from_everyone_else() {
        let state = world(vec![manifest(
            "ContextSpace",
            "hub",
            "helsinki",
            json!({ "urnSegment": "doprava-mhd" }),
        )]);
        assert_eq!(segment(&state.mirror, "helsinki", "hub"), "doprava-mhd");
        let proposal = propose_name(&state, &who("jana@hel.fi"), "doprava", "mhd");
        assert!(!proposal.available);
        assert_eq!(proposal.reason.as_deref(), Some("'doprava-mhd' is taken"));
    }

    #[test]
    fn a_taken_segment_names_the_holder_only_to_a_reader() {
        let pinned = manifest(
            "ContextSpace",
            "old",
            "ovzdusie",
            json!({ "urnSegment": "doprava-mhd" }),
        );
        let mut extra = reader_in("ovzdusie");
        extra.push(pinned.clone());
        let reader = world(extra);
        let said = format!(
            "{:?}",
            check(
                &reader,
                &who("jana@hel.fi"),
                "doprava",
                "ContextSpace",
                "mhd",
                &json!({})
            )
            .expect_err("the segment is pinned elsewhere")
        );
        assert!(said.contains("taken by project ovzdusie"), "{said}");
        assert!(said.contains("doprava-mhd"), "{said}");

        let stranger = world(vec![pinned]);
        let said = format!(
            "{:?}",
            check(
                &stranger,
                &who("jana@hel.fi"),
                "doprava",
                "ContextSpace",
                "mhd",
                &json!({})
            )
            .expect_err("taken")
        );
        assert!(said.contains("which is taken"), "{said}");
        assert!(!said.contains("ovzdusie"), "{said}");
    }

    #[test]
    fn the_gate_lets_the_same_space_and_other_kinds_through_and_checks_a_pin() {
        let state = world(reader_in("ovzdusie"));
        let jana = who("jana@hel.fi");
        // The same local name in two projects renders two segments.
        check(&state, &jana, "doprava", "ContextSpace", "mhd", &json!({}))
            .expect("doprava-mhd is free");
        // The same space written again is an update of that space.
        check(&state, &jana, "ovzdusie", "ContextSpace", "mhd", &json!({})).expect("its own space");
        // A pin onto another space's rendered segment is refused.
        check(
            &state,
            &jana,
            "doprava",
            "ContextSpace",
            "x",
            &json!({ "urnSegment": "ovzdusie-mhd" }),
        )
        .expect_err("ovzdusie-mhd is ovzdusie/mhd's");
        // Every other kind is named inside its project and is none of this gate's business.
        check(&state, &jana, "doprava", "Pipeline", "mhd", &json!({})).expect("not a space");
    }
}
