//! Shared application state: configuration, the discovered OIDC client and the
//! back-channel logout marks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;

use crate::auth::oidc::OidcClient;
use crate::auth::session::Session;
use crate::config::Config;
use crate::git::GiteaClient;
use crate::store::Mirror;
use crate::sync::Syncer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// `None` when no Keycloak realm is configured: login answers 503, every protected
    /// route answers 401. Fail closed.
    pub oidc: Option<Arc<OidcClient>>,
    pub mirror: Arc<Mirror>,
    /// `None` when no forge is configured: every write answers 503. A Portal that cannot
    /// open a merge request must not fall back to a local write (CC-03).
    pub gitea: Option<Arc<GiteaClient>>,
    pub syncer: Option<Arc<Syncer>>,
    /// `sub` → unix second of the last back-channel logout for that user. Sessions issued
    /// at or before the mark are refused.
    /// ponytail: per-replica map; move it to the preferences database when the portal
    /// runs more than one replica (the reconciler is leader-elected, the UI is not).
    revocations: Arc<RwLock<HashMap<String, i64>>>,
}

impl AppState {
    pub fn new(config: Config, oidc: Option<OidcClient>) -> Self {
        Self {
            config: Arc::new(config),
            oidc: oidc.map(Arc::new),
            mirror: Arc::new(Mirror::new()),
            gitea: None,
            syncer: None,
            revocations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_mirror(mut self, mirror: Arc<Mirror>) -> Self {
        self.mirror = mirror;
        self
    }

    pub fn with_gitea(mut self, gitea: Arc<GiteaClient>) -> Self {
        self.gitea = Some(gitea);
        self
    }

    pub fn with_syncer(mut self, syncer: Arc<Syncer>) -> Self {
        self.syncer = Some(syncer);
        self
    }

    /// Builds the state for a configuration, discovering the Keycloak realm when one is set and
    /// picking up the forge from the environment. Both failures are fatal at startup: the caller
    /// only prints them, so one boxed error is enough for the two kinds.
    pub async fn from_config(
        config: Config,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let oidc = match config.oidc.as_ref() {
            Some(oidc_config) => {
                Some(OidcClient::discover(oidc_config, &config.redirect_uri()).await?)
            }
            None => None,
        };
        // A half-configured forge is a configuration error, not a reason to run without one:
        // `from_env` answers `Ok(None)` only when all four variables are absent.
        let gitea = GiteaClient::from_env(|key| std::env::var(key).ok())?;
        let mut state = Self::new(config, oidc);
        if let Some(client) = gitea {
            let client = Arc::new(client);
            let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&state.mirror)));
            state.gitea = Some(client);
            state.syncer = Some(syncer);
        }
        Ok(state)
    }

    /// Marks every session of a subject as logged out (OIDC back-channel logout).
    pub fn revoke_subject(&self, subject: &str, at: i64) {
        if let Ok(mut marks) = self.revocations.write() {
            let mark = marks.entry(subject.to_string()).or_insert(at);
            *mark = (*mark).max(at);
        }
    }

    pub fn is_revoked(&self, session: &Session) -> bool {
        self.revocations
            .read()
            .ok()
            .and_then(|marks| marks.get(&session.identity.subject).copied())
            .is_some_and(|mark| session.issued_at <= mark)
    }
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Key {
        state.config.cookie_key.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::{now_unix, Identity};

    fn session(subject: &str, issued_at: i64) -> Session {
        Session {
            identity: Identity {
                subject: subject.into(),
                username: "demo.steward".into(),
                email: None,
                name: None,
                roles: Vec::new(),
            },
            expires_at: issued_at + 600,
            issued_at,
            id_token: String::new(),
        }
    }

    #[test]
    fn back_channel_logout_revokes_older_sessions_only() {
        let state = AppState::new(Config::for_tests(), None);
        let now = now_unix();
        let old = session("sub-1", now - 10);
        let fresh = session("sub-1", now + 10);
        assert!(!state.is_revoked(&old));

        state.revoke_subject("sub-1", now);
        assert!(
            state.is_revoked(&old),
            "session issued before the logout must be refused"
        );
        assert!(!state.is_revoked(&fresh), "a later login must still work");
        assert!(
            !state.is_revoked(&session("sub-2", now - 10)),
            "other users are untouched"
        );
    }
}
