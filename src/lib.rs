//! joinedcontext Portal library: the axum application, the resource API, and the embedded reconciler
//! (see docs/Architecture/09-portal.md). The binary in `main.rs` only starts it.

pub mod api;
pub mod apps;
pub mod assets;
pub mod auth;
pub mod change;
pub mod config;
pub mod db;
pub mod error;
pub mod git;
pub mod openapi;
pub mod plan;
pub mod resource;
pub mod server;
pub mod state;
pub mod store;
pub mod sync;
pub mod tools;

/// Name reported by `/api/v1/health` and the process banner.
pub const APP_NAME: &str = "joinedcontext-portal";

/// Version reported by `/api/v1/health` and OpenAPI documentation.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_stable() {
        assert_eq!(super::APP_NAME, "joinedcontext-portal");
    }
}
