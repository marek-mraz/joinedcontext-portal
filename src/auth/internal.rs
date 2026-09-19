//! Who may call the Portal's internal listener (PF-46, AG-52; docs Architecture/13 §6).
//!
//! The listener is not on the public URL scheme and a NetworkPolicy admits one workload to its
//! port. That is the second control, never the only one: a pod that reaches the port through a
//! policy mistake must still present the identity the route belongs to. CLAUDE.md's rule is the
//! whole of this module — "every workload via a `ServiceAccount` client with audience-bound
//! tokens; no static keys between cluster services".
//!
//! One function, one place. `served_previews` uses it today; `capture` and the run callbacks
//! follow with T-2271, which also retires the one static bearer left between the Portal and the
//! credential proxy.

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

/// Verifies that this call carries the Context Gateway's own ServiceAccount token: signed by the
/// realm, issued for [`INTERNAL_AUDIENCE`], and obtained by the client the configuration names.
///
/// Fail closed twice over: a Portal with no realm and a Portal that was not told which client the
/// gateway is both refuse every call, rather than answering because the network rule let it in.
pub async fn authenticate_gateway(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let expected = state
        .config
        .gateway_client_id
        .as_deref()
        .filter(|client| !client.trim().is_empty())
        .ok_or(ApiError::Unauthorized)?;
    let verifier = state.bearer.as_ref().ok_or(ApiError::Unauthorized)?;
    let (_, azp) = verifier
        .verify_for_audience(presented(headers)?, INTERNAL_AUDIENCE)
        .await?;
    // The audience says the token is for this listener; `azp` says which client asked for it, and
    // only one client may read the previews. Another workload of the same realm is refused here.
    if azp.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}
