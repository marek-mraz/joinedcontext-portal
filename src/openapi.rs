use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use utoipa::openapi::schema::{Array, Ref, Schema};
use utoipa::openapi::RefOr;
use utoipa::{Modify, OpenApi};

use crate::api::blueprints::FlowRequest;
use crate::api::changes::{ChangeAuthor, ChangeList, ChangeProposal, ChangeSummary};
use crate::api::ckan::{
    CkanStatus, DataStoreStatus, InstanceSummary, PublicationStatus, ResourceLink,
};
use crate::api::dry_run::DryRunResult;
use crate::api::export::{Revision, RevisionList};
use crate::api::federation::{Edge, EdgeKind, FederationGraph, Node, NodeHealth, RegistrationCard};
use crate::api::health::Health;
use crate::api::pipelines::PipelineMetrics;
use crate::api::preferences::Preferences;
use crate::api::resources::{ListMeta, ResourceList};
use crate::api::service_accounts::{KeyInfo, KeyList, MintedKey};
use crate::auth::oidc::LogoutTarget;
use crate::auth::Identity;
use crate::branding::{Branding, Colours, Fonts, Languages};
use crate::change::{Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, PlanSummary};
use crate::error::ProblemDetails;
use crate::plan::{FieldChange, PlanDiff};
use crate::reconciler::SyncStatus;
use crate::resource::{ResourceEnvelope, Status};
use crate::state::AppState;
use crate::tools::model_tools::{
    Artifacts, Catalogue, CatalogueModel, CatalogueSubject, GenerateRequest, ImportSdmRequest,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::api::health::health,
        crate::api::branding::get_branding,
        crate::api::branding::get_asset,
        crate::auth::oidc::me,
        crate::auth::oidc::logout,
        crate::api::resources::list,
        crate::api::blueprints::list_blueprints,
        crate::api::blueprints::start_flow,
        crate::api::resources::get_resource,
        crate::api::pipelines::get_metrics,
        crate::api::export::export,
        crate::api::import::import,
        crate::api::export::revisions,
        crate::api::service_accounts::list_keys,
        crate::api::service_accounts::create_key,
        crate::api::service_accounts::rotate_key,
        crate::api::service_accounts::revoke_key,
        crate::api::preferences::get_preferences,
        crate::api::preferences::put_preferences,
        crate::api::mutate::create,
        crate::api::mutate::replace,
        crate::api::mutate::patch,
        crate::api::changes::list_changes,
        crate::api::ckan::get_status,
        crate::api::federation::get_graph,
        crate::api::changes::get_change,
        crate::api::changes::approve_change,
        crate::api::changes::reject_change,
        crate::api::delete::delete_resource,
        crate::api::sync::get_sync_status,
        crate::api::sync_sources::status,
        crate::api::sync_sources::sync_now,
        crate::api::sync_sources::pause,
        crate::api::sync_sources::detach,
        crate::api::sync_sources::webhook,
        crate::api::webhook::gitea_webhook,
        crate::tools::model_tools::sdm_catalog,
        crate::tools::model_tools::generate,
        crate::tools::model_tools::import_sdm,
    ),
    components(schemas(
        Health,
        Branding,
        Colours,
        Fonts,
        Languages,
        CkanStatus,
        InstanceSummary,
        PublicationStatus,
        ResourceLink,
        DataStoreStatus,
        FederationGraph,
        Node,
        NodeHealth,
        Edge,
        EdgeKind,
        RegistrationCard,
        Identity,
        LogoutTarget,
        ProblemDetails,
        ResourceEnvelope,
        Status,
        ResourceList,
        ListMeta,
        Change,
        ChangeMeta,
        ChangeStatus,
        ChangePhase,
        crate::api::sync_sources::SyncSourceStatus,
        crate::api::sync_sources::SyncRunReport,
        crate::api::sync_sources::PauseRequest,
        crate::api::import::ImportReport,
        crate::api::import::ConflictPolicy,
        Lane,
        PlanSummary,
        PlanDiff,
        FieldChange,
        DryRunResult,
        ChangeProposal,
        ChangeList,
        ChangeSummary,
        ChangeAuthor,
        SyncStatus,
        PipelineMetrics,
        GenerateRequest,
        ImportSdmRequest,
        Artifacts,
        Catalogue,
        CatalogueSubject,
        CatalogueModel,
        Preferences,
        KeyInfo,
        KeyList,
        MintedKey,
        Revision,
        RevisionList,
        FlowRequest,
    )),
    info(
        title = "joinedcontext Portal API",
        version = "0.1.0",
        description = "Administrative and platform management REST API for joinedcontext Portal"
    ),
    tags(
        (name = "system", description = "System operations"),
        (name = "auth", description = "Sign-in, sign-out and the current identity"),
        (name = "resources", description = "Resource operations"),
        (name = "tools", description = "Model Tools schema generation and preview"),
        (name = "preferences", description = "The signed-in person's own UI preferences"),
        (name = "access", description = "ServiceAccounts, their API keys and effective grants")
    ),
    modifiers(&JcCoreSchemas)
)]
pub struct ApiDoc;

/// jc-core's types describe themselves with schemars, the Portal's with utoipa. The fields typed
/// by jc-core point at named components (so the generated TypeScript keeps `ObjectMeta`, `Phase`
/// and `Condition` as it always had them) and this modifier fills those components in from the
/// crate's own JSON Schema, so the document can never drift from the tagged contract.
struct JcCoreSchemas;

impl Modify for JcCoreSchemas {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.schemas.insert(
            "ObjectMeta".into(),
            schemars_schema::<jc_core::ObjectMeta>(),
        );
        components
            .schemas
            .insert("Phase".into(), schemars_schema::<jc_core::Phase>());
        components
            .schemas
            .insert("Condition".into(), schemars_schema::<jc_core::Condition>());
    }
}

/// A jc-core type's draft-07 schema as a utoipa schema. Subschemas are inlined so no
/// `#/definitions/` reference is left pointing outside the OpenAPI components.
fn schemars_schema<T: schemars::JsonSchema>() -> RefOr<Schema> {
    let mut settings = schemars::gen::SchemaSettings::draft07();
    settings.inline_subschemas = true;
    let root = schemars::gen::SchemaGenerator::new(settings).into_root_schema_for::<T>();
    let value = serde_json::to_value(root.schema).unwrap_or_default();
    serde_json::from_value(value).unwrap_or_else(|err| {
        // A schema this crate cannot express is a build defect, not a runtime condition: the
        // openapi test catches it before it ships. Documented as a free object until then.
        tracing::error!(error = %err, "jc-core schema is not a valid OpenAPI schema");
        RefOr::T(Schema::Object(Default::default()))
    })
}

pub fn object_meta_ref() -> Ref {
    Ref::from_schema_name("ObjectMeta")
}

pub fn phase_ref() -> Ref {
    Ref::from_schema_name("Phase")
}

pub fn conditions_ref() -> Array {
    Array::new(Ref::from_schema_name("Condition"))
}

pub async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/openapi.json", get(openapi_json))
}
