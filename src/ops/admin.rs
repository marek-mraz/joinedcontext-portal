//! Moving a project and its credentials through the registry (AG-59, CC-48, T-0840).
//!
//! Exporting a project, importing a bundle, writing a data model's LinkML and minting, rotating
//! or revoking a service account's key were reachable through the Portal's routes alone. Each
//! operation here calls the route's own function, so the grants, the refusals and the answers
//! are the route's; nothing here decides anything of its own.
//!
//! A key is a credential, so an agent asks and a person acts: the three key operations refuse a
//! run outright (AG-11), and through MCP the Red lane asks the person first (AG-63).

use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::runs::{as_user, refuse_agent};
use super::{change_schema, dry_run_output_schema, parse_input, Annotations, OpError, Operation};
use crate::change::Lane;
use crate::error::ApiError;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportInput {
    /// `yaml` or `json`; the `zip` a browser downloads is a file, not an answer to a call.
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub kinds: Option<String>,
    #[serde(default)]
    pub names: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImportInput {
    /// The bundle itself: the document an export answered.
    pub bundle: String,
    #[serde(default)]
    pub org_domain: Option<String>,
    #[serde(default)]
    pub conflict_policy: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourcePutInput {
    pub name: String,
    /// The LinkML document, as the editor writes it.
    pub source: String,
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KeyMintInput {
    /// The ServiceAccount the key belongs to.
    pub account: String,
    /// The `api-key` credential of that account the key is minted for.
    pub credential: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KeyRotateInput {
    pub account: String,
    pub key_id: String,
    #[serde(default)]
    pub overlap_hours: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KeyRevokeInput {
    pub account: String,
    pub key_id: String,
}

fn export_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "format": { "type": "string", "enum": ["yaml", "json"] },
            "kinds": { "type": "string", "description": "Comma-separated plurals, e.g. endpoints,pipelines" },
            "names": { "type": "string", "description": "Comma-separated names" },
            "revision": { "type": "string", "description": "A commit of the configuration repository" }
        },
        "additionalProperties": false
    })
}

fn export_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "format": { "type": "string" },
            "document": { "type": "string", "description": "The bundle, as the download holds it" }
        },
        "required": ["format", "document"]
    })
}

fn import_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "bundle": { "type": "string", "description": "The bundle document an export answered" },
            "orgDomain": { "type": "string", "description": "The organisation every imported id is rewritten to; this one when absent" },
            "conflictPolicy": {
                "type": "string",
                "enum": ["fail", "skip", "replace", "rename"],
                "description": "What to do with a resource the project already has; refusing the whole import when absent (MF-23)"
            },
            "dryRun": { "type": "boolean", "description": "Answer the plan and write nothing" }
        },
        "required": ["bundle"],
        "additionalProperties": false
    })
}

fn import_output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "description": "The plan alone, when `dryRun` is true",
                "properties": {
                    "created": { "type": "array", "items": { "type": "object" } },
                    "updated": { "type": "array", "items": { "type": "object" } },
                    "conflicts": { "type": "array", "items": { "type": "object" } }
                }
            },
            change_schema()
        ]
    })
}

fn source_put_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": "The DataModel's name" },
            "source": { "type": "string", "description": "The LinkML document" },
            "space": { "type": "string", "description": "The space a model the project does not hold yet is created in (DM-57)" },
            "version": { "type": "string", "description": "The version to write; the next one the check computes when absent" },
            "dryRun": { "type": "boolean", "description": "Answer the check and write nothing" }
        },
        "required": ["name", "source"],
        "additionalProperties": false
    })
}

fn source_put_output_schema() -> Value {
    json!({ "oneOf": [dry_run_output_schema(), change_schema()] })
}

fn key_mint_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "account": { "type": "string", "description": "The ServiceAccount's name" },
            "credential": { "type": "string", "description": "The api-key credential of that account the key belongs to (PF-34)" },
            "expiresAt": { "type": "string", "format": "date-time", "description": "Overrides the manifest's expiry" }
        },
        "required": ["account", "credential"],
        "additionalProperties": false
    })
}

fn key_rotate_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "account": { "type": "string" },
            "keyId": { "type": "string" },
            "overlapHours": { "type": "integer", "description": "How long the old key keeps working beside its successor (PF-38)" }
        },
        "required": ["account", "keyId"],
        "additionalProperties": false
    })
}

fn key_revoke_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "account": { "type": "string" },
            "keyId": { "type": "string" }
        },
        "required": ["account", "keyId"],
        "additionalProperties": false
    })
}

fn minted_key_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The only answer that ever carries the token itself; it is never recoverable (PF-36, PF-37)",
        "properties": {
            "keyId": { "type": "string" },
            "token": { "type": "string" },
            "credential": { "type": "string" },
            "expiresAt": { "type": "string", "format": "date-time" }
        },
        "required": ["keyId", "token", "credential"]
    })
}

fn revoked_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "revoked": { "type": "boolean" } },
        "required": ["revoked"]
    })
}

/// A route that answers a `Change` as its response, read back as an operation answers it: the id
/// and the lane beside the document, like every other propose (T-0838).
pub(super) async fn proposed(response: axum::response::Response) -> Result<Value, OpError> {
    let accepted = response.status() == axum::http::StatusCode::ACCEPTED;
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|err| OpError::Api(ApiError::Internal(err.to_string())))?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !accepted {
        // A dry run answers the check, not a change.
        return Ok(body);
    }
    let change: crate::change::Change = serde_json::from_value(body)
        .map_err(|err| OpError::Api(ApiError::Internal(err.to_string())))?;
    Ok(crate::api::mutate::ProposeOutcome::Change(change).into_value())
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_project_export",
            title: "Export Project",
            description: "The project's manifests as one bundle, narrowed to the kinds and names asked for",
            input: export_input_schema,
            output: export_output_schema,
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ExportInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ExportInput = parse_input(val)?;
                    let format = input.format.unwrap_or_else(|| "yaml".to_owned());
                    if !matches!(format.as_str(), "yaml" | "json") {
                        return Err(OpError::InvalidInput {
                            path: "/format".into(),
                            message: "an export through an operation is yaml or json; the zip is \
                                      a download of the Portal"
                                .into(),
                        });
                    }
                    let response = crate::api::export::export(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                        Query(crate::api::export::ExportQuery {
                            format: Some(format.clone()),
                            revision: input.revision,
                            kinds: input.kinds,
                            names: input.names,
                        }),
                    )
                    .await?;
                    let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
                        .await
                        .map_err(|err| OpError::Api(ApiError::Internal(err.to_string())))?;
                    Ok(json!({
                        "format": format,
                        "document": String::from_utf8_lossy(&bytes),
                    }))
                })
            },
        },
        Operation {
            name: "jc_project_import",
            title: "Import A Bundle",
            description: "Imports a bundle into the project as one change a person approves, or answers the plan alone",
            input: import_input_schema,
            output: import_output_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Yellow,
            validate: |val| parse_input::<ImportInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ImportInput = parse_input(val)?;
                    let conflict_policy = match input.conflict_policy.as_deref() {
                        None => crate::api::import::ConflictPolicy::default(),
                        Some("fail") => crate::api::import::ConflictPolicy::Fail,
                        Some("skip") => crate::api::import::ConflictPolicy::Skip,
                        Some("replace") => crate::api::import::ConflictPolicy::Replace,
                        Some("rename") => crate::api::import::ConflictPolicy::Rename,
                        Some(other) => {
                            return Err(OpError::InvalidInput {
                                path: "/conflictPolicy".into(),
                                message: format!(
                                    "conflictPolicy '{other}' is not fail, skip, replace or \
                                     rename (MF-23)"
                                ),
                            })
                        }
                    };
                    let options = crate::api::import::ImportOptions {
                        target_namespace: None,
                        org_domain: input.org_domain,
                        conflict_policy,
                        dry_run: input.dry_run,
                        url: None,
                        manifests: None,
                    };
                    let (_, body) = crate::api::import::import_bundle(
                        state,
                        &caller.identity,
                        project,
                        input.bundle.as_bytes(),
                        options,
                    )
                    .await?;
                    Ok(body)
                })
            },
        },
        Operation {
            name: "jc_model_source_put",
            title: "Write Model Source",
            description: "Writes one DataModel's LinkML and its generated artifacts as a change a person approves",
            input: source_put_input_schema,
            output: source_put_output_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataModel",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<SourcePutInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: SourcePutInput = parse_input(val)?;
                    let response = crate::api::datamodels::put_source(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.name)),
                        Query(crate::api::datamodels::SourcePutQuery {
                            version: input.version,
                            dry_run: input.dry_run.then(|| "all".to_owned()),
                            space: input.space,
                        }),
                        input.source.into_bytes().into(),
                    )
                    .await?;
                    proposed(response).await
                })
            },
        },
        Operation {
            name: "jc_service_account_key_mint",
            title: "Mint A Service Account Key",
            description: "Mints one api-key credential of a ServiceAccount; the token is in this answer and nowhere else",
            input: key_mint_input_schema,
            output: minted_key_output_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "ServiceAccount",
            verb: None,
            lane: Lane::Red,
            validate: |val| parse_input::<KeyMintInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller)?;
                    let input: KeyMintInput = parse_input(val)?;
                    let body = json!({
                        "credential": input.credential,
                        "expiresAt": input.expires_at,
                    });
                    let (_, axum::Json(minted)) = crate::api::service_accounts::create_key(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.account)),
                        serde_json::to_vec(&body).unwrap_or_default().into(),
                    )
                    .await?;
                    Ok(serde_json::to_value(minted)?)
                })
            },
        },
        Operation {
            name: "jc_service_account_key_rotate",
            title: "Rotate A Service Account Key",
            description: "Replaces one key with a successor; the old one stops working when its overlap ends",
            input: key_rotate_input_schema,
            output: minted_key_output_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
            },
            kind: "ServiceAccount",
            verb: None,
            lane: Lane::Red,
            validate: |val| parse_input::<KeyRotateInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller)?;
                    let input: KeyRotateInput = parse_input(val)?;
                    let body = json!({ "overlapHours": input.overlap_hours });
                    let (_, axum::Json(minted)) = crate::api::service_accounts::rotate_key(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.account, input.key_id)),
                        serde_json::to_vec(&body).unwrap_or_default().into(),
                    )
                    .await?;
                    Ok(serde_json::to_value(minted)?)
                })
            },
        },
        Operation {
            name: "jc_service_account_key_revoke",
            title: "Revoke A Service Account Key",
            description: "Stops one key at once; nothing that used it works afterwards",
            input: key_revoke_input_schema,
            output: revoked_output_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: true,
            },
            kind: "ServiceAccount",
            verb: None,
            lane: Lane::Red,
            validate: |val| parse_input::<KeyRevokeInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller)?;
                    let input: KeyRevokeInput = parse_input(val)?;
                    crate::api::service_accounts::revoke_key(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.account, input.key_id)),
                    )
                    .await?;
                    Ok(json!({ "revoked": true }))
                })
            },
        },
    ]
}
