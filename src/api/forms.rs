//! `GET /api/v1/forms`: the `kind: UiSchema` manifests that arrange the Portal's forms
//! (T-0452, UI-01, UI-02, CC-31).
//!
//! One request for the whole of `portal/forms/`, not one per kind: a form dialog opens on a
//! click, the directory is a few kilobytes, and a per-kind fetch would put a round trip in front
//! of every dialog. The manifests are organization-scoped and already in the mirror like every
//! other kind (T-0451), so this is a filter over the store rather than a second way of reading
//! the configuration repository.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::api::blueprints::ORG_NAMESPACE;
use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::session::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::API_VERSION;
use crate::state::AppState;
use crate::store::ListOptions;

/// The kind whose manifests arrange a form.
const UI_SCHEMA_KIND: &str = "UiSchema";

#[utoipa::path(
    get,
    path = "/api/v1/forms",
    tag = "forms",
    responses(
        (status = 200, description = "Every UiSchema manifest in portal/forms/", body = ResourceList),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_forms(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ResourceList>, ApiError> {
    // Unpaged on purpose: an organisation arranges tens of kinds, not thousands, and a form
    // arrangement the caller did not receive is a dialog rendered wrong with no way to know it.
    let page = state
        .mirror
        .list(ORG_NAMESPACE, UI_SCHEMA_KIND, &ListOptions::default());

    Ok(Json(ResourceList {
        api_version: API_VERSION.to_string(),
        kind: "List".to_string(),
        metadata: ListMeta {
            continue_token: None,
            remaining_item_count: None,
        },
        items: page.items,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/forms", get(list_forms))
}
