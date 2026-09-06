//! Portal authentication: Keycloak OIDC authorization code flow with PKCE (CC-40)
//! and encrypted cookie sessions (docs/Architecture/09-portal.md §1).
//!
//! Tokens live in an encrypted, `HttpOnly` cookie — never in `localStorage`, never in a log line.

pub mod csrf;
pub mod oidc;
pub mod session;

pub use session::{CurrentUser, Identity, Session};
