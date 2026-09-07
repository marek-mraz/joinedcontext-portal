//! Shared application state: configuration, the discovered OIDC client and the
//! back-channel logout marks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;

use crate::auth::bearer::BearerVerifier;
use crate::auth::oidc::OidcClient;
use crate::auth::session::Session;
use crate::config::Config;
use crate::git::GiteaClient;
use crate::reconciler::{Leadership, Syncer};
use crate::store::Mirror;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// `None` when no Keycloak realm is configured: login answers 503, every protected
    /// route answers 401. Fail closed.
    pub oidc: Option<Arc<OidcClient>>,
    /// Verifies `Authorization: Bearer` tokens against the realm JWKS. `None` without a realm:
    /// every bearer call answers 401. Fail closed.
    pub bearer: Option<Arc<BearerVerifier>>,
    pub mirror: Arc<Mirror>,
    /// `None` when no forge is configured: every write answers 503. A Portal that cannot
    /// open a merge request must not fall back to a local write (CC-03).
    pub gitea: Option<Arc<GiteaClient>>,
    pub syncer: Option<Arc<Syncer>>,
    /// The `SyncSource` loop (MF-27…MF-32). `None` without a forge, or when the outbound HTTP
    /// client could not be built: the sync routes answer 503 and nothing syncs, rather than a
    /// loop that quietly reaches nothing.
    pub sync: Option<Arc<crate::sync::driver::Driver>>,
    /// The preferences tier (UI-09). `None` without a database: the preferences routes answer
    /// 503 and nothing else notices.
    pub db: Option<sqlx::PgPool>,
    /// `sub` → unix second of the last back-channel logout for that user. Sessions issued
    /// at or before the mark are refused.
    /// ponytail: per-replica map; move it to the preferences database when the portal
    /// runs more than one replica (the reconciler is leader-elected, the UI is not).
    revocations: Arc<RwLock<HashMap<String, i64>>>,
}

impl AppState {
    pub fn new(config: Config, oidc: Option<OidcClient>) -> Self {
        let bearer = config
            .oidc
            .as_ref()
            .map(|o| Arc::new(BearerVerifier::new(&o.issuer, &o.client_id)));
        Self {
            bearer,
            config: Arc::new(config),
            oidc: oidc.map(Arc::new),
            mirror: Arc::new(Mirror::new()),
            gitea: None,
            syncer: None,
            sync: None,
            db: None,
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

    pub fn with_db(mut self, db: sqlx::PgPool) -> Self {
        self.db = Some(db);
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
        // A configured database that cannot be reached or migrated is fatal, like a half-configured
        // forge: better one clear startup error than a Portal that silently forgets preferences.
        let db = match config.database_url.as_deref() {
            Some(url) => Some(crate::db::connect(url).await?),
            None => None,
        };
        let mut state = Self::new(config, oidc);
        state.db = db;
        // Warm the key cache so the first bearer call does not pay for the fetch; a realm that
        // is down at startup only costs a warning, the next unknown `kid` fetches again.
        if let Some(bearer) = state.bearer.as_ref() {
            if let Err(err) = bearer.refresh().await {
                tracing::warn!(error = %err, "JWKS not loaded at startup");
            }
        }
        if let Some(client) = gitea {
            let client = Arc::new(client);
            let mut syncer = Syncer::new(Arc::clone(&client), Arc::clone(&state.mirror));
            // With a database the replicas elect one reconciler; without one there is nothing
            // to elect with, and a Portal that runs alone reconciles alone (T-0191, CC-03).
            if let Some(pool) = state.db.as_ref() {
                syncer = syncer.with_leadership(Arc::new(Leadership::reconciler(pool.clone())));
            }
            // Applying an app's objects needs three halves, not two: a cluster to write into,
            // the settings that say where, and the realm the sidecar logs users in against.
            // Any one missing leaves the reconciler reading apps and deploying nothing, which
            // is what a Portal on a laptop does (T-0411, AP-18, AP-27).
            match (
                state.config.app_settings.clone(),
                crate::apps::kube::KubeClient::in_cluster(),
                state.config.oidc.as_ref(),
            ) {
                (Some(settings), Ok(Some(kube)), Some(oidc)) => {
                    // The Portal's own confidential client; its service account is what the
                    // realm grants `manage-clients` to (AP-27).
                    match crate::apps::keycloak::AdminClient::new(
                        reqwest::Client::new(),
                        &oidc.issuer,
                        &oidc.client_id,
                        oidc.client_secret(),
                    ) {
                        Ok(keycloak) => {
                            syncer = syncer.with_converger(Arc::new(
                                crate::apps::converge::Converger::new(kube, keycloak, settings),
                            ));
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "the issuer names no realm, so no app is deployed")
                        }
                    }
                }
                (_, Err(err), _) => {
                    tracing::warn!(error = %err, "the ServiceAccount mount is unreadable, so no app is deployed")
                }
                _ => tracing::info!("no app settings or no cluster: apps are read, not deployed"),
            }
            // The `SyncSource` loop needs the forge and a way out to the origins. Without the
            // second there is no loop at all: a driver that cannot fetch would report every
            // source as failing every minute (MF-27).
            match crate::sync::remote::HttpRemote::new() {
                Ok(remote) => {
                    let states = Arc::new(crate::sync::state::States::new(state.db.clone()));
                    if !states.is_durable() {
                        tracing::info!(
                            "no database: sync sources remember their runs only until this \
                             process ends"
                        );
                    }
                    state.sync = Some(Arc::new(crate::sync::driver::Driver::new(
                        Arc::clone(&client),
                        states,
                        Arc::new(remote),
                    )));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "no HTTP client for sync origins; sync sources do not run")
                }
            }
            state.gitea = Some(client);
            state.syncer = Some(Arc::new(syncer));
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
