//! Who may call the Portal's internal listener (PF-46, AG-52; docs Architecture/13 §6).
//!
//! The listener is not on the public URL scheme and a NetworkPolicy admits one workload to its
//! port. That is the second control, never the only one: a pod that reaches the port through a
//! policy mistake must still present the identity the route belongs to. CLAUDE.md's rule is the
//! whole of this module — "every workload via a `ServiceAccount` client with audience-bound
//! tokens; no static keys between cluster services".
//!
//! One check, one place, and a client per route (T-2271): the gateway reads the preview list, the
//! credential proxy answers the run callbacks, the project's pipeline runner posts a test capture.
//! A token for one of those opens that route and no other, because the check compares `azp` and not
//! only the audience — the audience says "this listener", the client says "this door".

use axum::http::{header, HeaderMap};

use crate::error::ApiError;
use crate::state::AppState;

/// What a token for this listener must be issued for. Not the Portal's API audience: a token a
/// person or an app holds for `/api/v1` must not open an internal route as well.
pub const INTERNAL_AUDIENCE: &str = "portal-internal";

/// The bearer of this request, without its scheme.
fn presented(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.trim().is_empty())
        .ok_or(ApiError::Unauthorized)
}

/// Verifies that this call carries the ServiceAccount token of the workload this route belongs to:
/// signed by the realm, issued for [`INTERNAL_AUDIENCE`], and obtained by the client the
/// configuration names for that route.
///
/// Fail closed twice over: a Portal with no realm and a Portal that was not told which client owns
/// the route both refuse every call, rather than answering because the network rule let it in.
pub async fn authenticate_workload(
    state: &AppState,
    headers: &HeaderMap,
    expected: Option<&str>,
) -> Result<(), ApiError> {
    let expected = expected
        .map(str::trim)
        .filter(|client| !client.is_empty())
        .ok_or(ApiError::Unauthorized)?;
    let verifier = state.bearer.as_ref().ok_or(ApiError::Unauthorized)?;
    let (_, azp) = verifier
        .verify_for_audience(presented(headers)?, INTERNAL_AUDIENCE)
        .await?;
    // The audience says the token is for this listener; `azp` says which client asked for it, and
    // one route belongs to one client. Another workload of the same realm is refused here, holding
    // a token that is otherwise perfectly valid.
    if azp.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

/// The Context Gateway, which reads the list of running workspace previews (PF-46).
pub async fn authenticate_gateway(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    authenticate_workload(state, headers, state.config.gateway_client_id.as_deref()).await
}

/// The credential proxy, which relays a run's own calls back to the Portal (AG-52). It held a
/// static bearer shared with the Portal until T-2271; a shared string is a key that never rotates
/// and that either side can leak, which CLAUDE.md rules out between cluster services.
pub async fn authenticate_agent_proxy(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    authenticate_workload(
        state,
        headers,
        state.config.agent_proxy_client_id.as_deref(),
    )
    .await
}

/// The project's pipeline runner, which posts back what a pipeline test produced. The test's id is
/// 130 random bits and therefore a capability of its own, but a caller with no identity now gets
/// the 401 it deserves instead of the 404 that says only "no such test".
pub async fn authenticate_pipeline_runner(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    authenticate_workload(
        state,
        headers,
        state.config.pipeline_runner_client_id.as_deref(),
    )
    .await
}
