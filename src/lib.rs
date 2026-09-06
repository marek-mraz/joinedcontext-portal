//! joinedcontext Portal library: the axum application, the resource API and the embedded reconciler
//! grow here (docs/Architecture/09-portal.md). The binary in `main.rs` only starts it.

/// Name reported by `/api/v1/health` and the process banner.
pub const APP_NAME: &str = "joinedcontext-portal";

#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_stable() {
        assert_eq!(super::APP_NAME, "joinedcontext-portal");
    }
}
