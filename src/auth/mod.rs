//! Portal authentication: Keycloak OIDC authorization code flow with PKCE (CC-40)
//! and encrypted cookie sessions (docs/Architecture/09-portal.md §1).
//!
//! Tokens live in an encrypted, `HttpOnly` cookie — never in `localStorage`, never in a log line.
//! Services call the API with `Authorization: Bearer`; the Portal verifies that token itself
//! (ES256 against the realm JWKS), the APISIX edge verifies nothing (docs Deployment/10 §4).

pub mod bearer;
pub mod csrf;
pub mod oidc;
pub mod session;

pub use session::{CurrentUser, Identity, Session};
