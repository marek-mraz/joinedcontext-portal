use axum_extra::extract::cookie::Key;
use std::net::SocketAddr;
use std::time::Duration;
use url::Url;

/// Portal server configuration. Secrets are redacted in `Debug` so a config dump
/// never puts a client secret or a cookie key into the log (CC-40).
#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub public_base_url: Url,
    /// `None` disables login: no session can be minted, so every protected route
    /// answers 401. Configuration is fail-closed, never fail-open.
    pub oidc: Option<OidcConfig>,
    /// Whether an `X-Access-Token` header is believed to come from the APISIX edge and is
    /// verified as if it were `Authorization: Bearer` (ADR-N-019, AP-28). The deployment sets
    /// it behind the edge, which strips the header from every client request first; a Portal
    /// without an edge in front leaves it off and the header is ignored. Default `false`.
    pub trust_edge_token: bool,
    pub cookie_key: Key,
    pub sync_interval: Duration,
    pub gitea_webhook_secret: Option<String>,
    /// Base URL of a project's Bento pipeline runner with `{project}` still in it, e.g.
    /// `http://pipeline-runner.{project}-pipeline-runner.svc.cluster.local:4195`. `None`
    /// leaves the metrics route answering 503 instead of guessing a service name.
    pub pipeline_runner_url: Option<String>,
    /// Base URL of the stateless Model Tools service, e.g.
    /// `http://model-tools.tools.svc.cluster.local:8080`. `None` leaves the LinkML preview
    /// routes answering 503 instead of guessing a service name (DM-18).
    pub model_tools_url: Option<String>,
    /// Root of the built app bundles, one directory per app. `None` leaves every
    /// `/apps/{name}/` path answering 404 rather than reading a guessed directory (AP-14).
    pub apps_dir: Option<String>,
    /// The file the deployment renders `global.branding` into (UI-30, OPS-46). `None` serves
    /// neutral joinedcontext defaults, which is what an installation without branding looks
    /// like; it is never an error.
    pub branding_file: Option<String>,
    /// PostgreSQL connection string of the preferences tier (UI-09). Carries a password, so it
    /// is redacted in `Debug`. `None` runs the Portal without preferences: those routes answer
    /// 503, everything else works.
    pub database_url: Option<String>,
    /// The group (or realm role) whose members may do everything everywhere, so the first
    /// `RoleBinding` can be written into an empty repository (T-0526, PF-50).
    pub bootstrap_admins: String,
    /// Where an App's four Kubernetes objects are applied (AP-13, AP-18, T-0411). `None` leaves
    /// the reconciler reading apps and applying nothing, which is what a Portal outside a
    /// cluster does; it is never a guess, because guessing a namespace here would mean writing
    /// a Deployment into somebody else's.
    pub app_settings: Option<crate::apps::reconciler::Settings>,
    /// Where a builder run's workspace is scheduled and how the proxy reaches this Portal
    /// (AG-33, AG-40). `None` leaves every agent-run route answering 503: without a namespace
    /// to schedule into and a proxy for the workspace to speak to, a run has nowhere to happen.
    pub agent_settings: Option<AgentSettings>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("public_base_url", &self.public_base_url.as_str())
            .field("oidc", &self.oidc)
            .field("trust_edge_token", &self.trust_edge_token)
            .field("cookie_key", &"[redacted]")
            .field("sync_interval", &self.sync_interval)
            .field(
                "gitea_webhook_secret",
                &self.gitea_webhook_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("pipeline_runner_url", &self.pipeline_runner_url)
            .field("model_tools_url", &self.model_tools_url)
            .field("apps_dir", &self.apps_dir)
            .field("branding_file", &self.branding_file)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "[redacted]"),
            )
            .field("app_settings", &self.app_settings)
            .field("agent_settings", &self.agent_settings)
            .finish()
    }
}

/// Where an App's Kubernetes objects are applied, or `None` when this Portal applies none
/// (AP-13, AP-18, T-0411).
///
/// All three values are needed together: the namespace to write into, the organization domain
/// that becomes a policy's assigner, and the host the app's endpoint is served on. The host is
/// read off configuration the Portal already has, so an installation states two variables
/// rather than three, and any missing one leaves the converger off instead of guessing a
/// namespace and deploying into somebody else's.
fn app_settings(
    lookup: &impl Fn(&str) -> Option<String>,
    public_base_url: &Url,
) -> Option<crate::apps::reconciler::Settings> {
    let namespace = lookup("JC_PORTAL_APPS_NAMESPACE").filter(|v| !v.trim().is_empty())?;
    let org_domain = lookup("JC_PORTAL_ORG_DOMAIN").filter(|v| !v.trim().is_empty())?;
    let host = public_base_url.host_str()?.to_owned();
    Some(crate::apps::reconciler::Settings {
        host,
        namespace,
        org_domain,
    })
}

/// Where a builder run happens and how the credential proxy reaches this Portal (ADR-N-020).
///
/// The namespace and the proxy are needed together: a workspace with no proxy has no way to
/// reach the model, the data or the forge, and a proxy with no namespace has nothing to serve.
/// The token is the proxy's own credential on the internal listener, and it is the reason this
/// block is all-or-nothing rather than three independent variables: two of the three set is a
/// misconfiguration, not a Portal that runs agent runs unauthenticated.
#[derive(Clone)]
pub struct AgentSettings {
    /// Namespace the workspace Jobs, their ServiceAccounts and their NetworkPolicies go into.
    pub namespace: String,
    /// Base URL of `jc-agent-proxy` as a workspace sees it, e.g.
    /// `http://jc-agent-proxy.agents.svc.cluster.local:8080`.
    pub proxy_base: String,
    /// The bearer the proxy presents on the internal listener. Carries a secret, so it is
    /// redacted in `Debug`.
    proxy_token: String,
    /// Where the internal listener binds. APISIX routes nothing to it, and a NetworkPolicy
    /// opens it to the proxy alone (AG-52).
    pub internal_bind: SocketAddr,
    /// Wall clock of one run, in seconds. The Job carries the same number as its
    /// `activeDeadlineSeconds`, so the two cannot disagree about when a run is over.
    pub run_ttl_secs: i64,
}

impl AgentSettings {
    pub fn proxy_token(&self) -> &str {
        &self.proxy_token
    }
}

impl std::fmt::Debug for AgentSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSettings")
            .field("namespace", &self.namespace)
            .field("proxy_base", &self.proxy_base)
            .field("proxy_token", &"[redacted]")
            .field("internal_bind", &self.internal_bind)
            .field("run_ttl_secs", &self.run_ttl_secs)
            .finish()
    }
}

/// The agent runner block, or `None` when this Portal runs no agent (AG-33, AG-40).
fn agent_settings(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<AgentSettings>, ConfigError> {
    let namespace = lookup("JC_AGENTS_NAMESPACE").filter(|v| !v.trim().is_empty());
    let proxy_base = lookup("JC_AGENT_PROXY_BASE").filter(|v| !v.trim().is_empty());
    let proxy_token = lookup("JC_AGENT_PROXY_TOKEN").filter(|v| !v.trim().is_empty());
    let (namespace, proxy_base, proxy_token) = match (namespace, proxy_base, proxy_token) {
        (None, None, None) => return Ok(None),
        (Some(namespace), Some(proxy_base), Some(proxy_token)) => {
            (namespace, proxy_base, proxy_token)
        }
        _ => {
            return Err(ConfigError::Invalid {
                var: "JC_AGENTS_NAMESPACE",
                reason: "JC_AGENTS_NAMESPACE, JC_AGENT_PROXY_BASE and JC_AGENT_PROXY_TOKEN must \
                         be set together"
                    .to_string(),
            })
        }
    };

    let probe: Url = proxy_base
        .parse()
        .map_err(|e: url::ParseError| ConfigError::Invalid {
            var: "JC_AGENT_PROXY_BASE",
            reason: e.to_string(),
        })?;
    if probe.scheme() != "http" && probe.scheme() != "https" {
        return Err(ConfigError::Invalid {
            var: "JC_AGENT_PROXY_BASE",
            reason: format!("scheme '{}' is not http or https", probe.scheme()),
        });
    }

    let internal_bind: SocketAddr = lookup("JC_INTERNAL_BIND")
        .unwrap_or_else(|| Config::DEFAULT_INTERNAL_BIND.to_string())
        .parse()
        .map_err(|e: std::net::AddrParseError| ConfigError::Invalid {
            var: "JC_INTERNAL_BIND",
            reason: e.to_string(),
        })?;

    let run_ttl_secs = match lookup("JC_AGENT_RUN_TTL") {
        Some(value) => value.parse::<i64>().map_err(|e| ConfigError::Invalid {
            var: "JC_AGENT_RUN_TTL",
            reason: e.to_string(),
        })?,
        None => Config::DEFAULT_RUN_TTL_SECS,
    };
    if run_ttl_secs <= 0 {
        return Err(ConfigError::Invalid {
            var: "JC_AGENT_RUN_TTL",
            reason: "a run needs a positive wall clock".to_string(),
        });
    }

    Ok(Some(AgentSettings {
        namespace,
        proxy_base: proxy_base.trim_end_matches('/').to_owned(),
        proxy_token,
        internal_bind,
        run_ttl_secs,
    }))
}

/// Keycloak realm the portal authenticates humans against (CC-40).
#[derive(Clone)]
pub struct OidcConfig {
    pub issuer: Url,
    pub client_id: String,
    client_secret: String,
    /// PEM of an extra root the discovery client trusts, on top of the compiled-in Mozilla
    /// bundle. An instance whose issuer is served by a private CA (a self-signed cluster
    /// issuer, an internal PKI) is unreachable without it: the binary carries `webpki-roots`
    /// alone, so no mounted file or `SSL_CERT_FILE` is consulted. Verification stays on;
    /// this only widens what a valid chain may end in.
    pub extra_ca_pem: Option<Vec<u8>>,
}

impl OidcConfig {
    pub fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

impl std::fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer.as_str())
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .finish()
    }
}

/// Errors raised when parsing configuration parameters.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("invalid configuration for {var}: {reason}")]
    Invalid { var: &'static str, reason: String },
}

impl Config {
    pub const DEFAULT_BIND: &'static str = "0.0.0.0:8080";
    const DEFAULT_BOOTSTRAP_ADMINS: &'static str = "platform-admins";
    pub const DEFAULT_PUBLIC_URL: &'static str = "http://localhost:8080";
    /// The internal listener's default. Port 9090, which the edge does not route (AG-52).
    pub const DEFAULT_INTERNAL_BIND: &'static str = "0.0.0.0:9090";
    /// Wall clock of one builder run when the deployment names none: twenty minutes, the
    /// window AG-43 gives a run before it expires.
    pub const DEFAULT_RUN_TTL_SECS: i64 = 1_200;
    /// `Key::from` panics below 64 bytes, so the length is checked before it is called.
    pub const MIN_COOKIE_KEY_LEN: usize = 64;

    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind_str = lookup("JC_PORTAL_BIND").unwrap_or_else(|| Self::DEFAULT_BIND.to_string());
        let bind: SocketAddr =
            bind_str
                .parse()
                .map_err(|e: std::net::AddrParseError| ConfigError::Invalid {
                    var: "JC_PORTAL_BIND",
                    reason: e.to_string(),
                })?;

        let url_str =
            lookup("JC_PORTAL_PUBLIC_URL").unwrap_or_else(|| Self::DEFAULT_PUBLIC_URL.to_string());
        let public_base_url: Url =
            url_str
                .parse()
                .map_err(|e: url::ParseError| ConfigError::Invalid {
                    var: "JC_PORTAL_PUBLIC_URL",
                    reason: e.to_string(),
                })?;

        let oidc = match (
            lookup("JC_OIDC_ISSUER"),
            lookup("JC_OIDC_CLIENT_ID"),
            lookup("JC_OIDC_CLIENT_SECRET"),
        ) {
            (None, None, None) => None,
            (Some(issuer), Some(client_id), Some(client_secret)) => Some(OidcConfig {
                issuer: issuer
                    .parse()
                    .map_err(|e: url::ParseError| ConfigError::Invalid {
                        var: "JC_OIDC_ISSUER",
                        reason: e.to_string(),
                    })?,
                client_id,
                client_secret,
                // Unreadable means misconfigured, not "carry on with fewer roots": a start-up
                // error names the file, a silent fallback would be a confusing 500 at login.
                extra_ca_pem: match lookup("JC_OIDC_CA_FILE") {
                    None => None,
                    Some(path) => Some(std::fs::read(&path).map_err(|e| ConfigError::Invalid {
                        var: "JC_OIDC_CA_FILE",
                        reason: format!("{path}: {e}"),
                    })?),
                },
            }),
            _ => {
                return Err(ConfigError::Invalid {
                    var: "JC_OIDC_ISSUER",
                    reason: "JC_OIDC_ISSUER, JC_OIDC_CLIENT_ID and JC_OIDC_CLIENT_SECRET must be \
                             set together"
                        .to_string(),
                })
            }
        };

        let cookie_key = match lookup("JC_PORTAL_COOKIE_KEY") {
            Some(material) if material.len() >= Self::MIN_COOKIE_KEY_LEN => {
                Key::from(material.as_bytes())
            }
            Some(_) => {
                return Err(ConfigError::Invalid {
                    var: "JC_PORTAL_COOKIE_KEY",
                    reason: format!("at least {} bytes required", Self::MIN_COOKIE_KEY_LEN),
                })
            }
            None => {
                tracing::warn!(
                    "JC_PORTAL_COOKIE_KEY is unset: using an ephemeral key, sessions do not \
                     survive a restart"
                );
                Key::generate()
            }
        };

        let sync_interval_secs = match lookup("JC_PORTAL_SYNC_INTERVAL") {
            Some(val) => val.parse::<u64>().map_err(|e| ConfigError::Invalid {
                var: "JC_PORTAL_SYNC_INTERVAL",
                reason: e.to_string(),
            })?,
            None => 60,
        };
        let sync_interval = Duration::from_secs(sync_interval_secs);

        let gitea_webhook_secret = lookup("JC_GITEA_WEBHOOK_SECRET");

        // The template is not a URL until `{project}` is filled in, so it is checked against a
        // stand-in: an operator learns about a typo at startup, not on the first scrape.
        let pipeline_runner_url = match lookup("JC_PORTAL_PIPELINE_RUNNER_URL") {
            Some(template) => {
                let probe: Url = template.replace("{project}", "project").parse().map_err(
                    |e: url::ParseError| ConfigError::Invalid {
                        var: "JC_PORTAL_PIPELINE_RUNNER_URL",
                        reason: e.to_string(),
                    },
                )?;
                if probe.scheme() != "http" && probe.scheme() != "https" {
                    return Err(ConfigError::Invalid {
                        var: "JC_PORTAL_PIPELINE_RUNNER_URL",
                        reason: format!("scheme '{}' is not http or https", probe.scheme()),
                    });
                }
                Some(template)
            }
            None => None,
        };

        let model_tools_url = match lookup("JC_PORTAL_MODEL_TOOLS_URL") {
            Some(raw) => {
                let url: Url = raw
                    .parse()
                    .map_err(|e: url::ParseError| ConfigError::Invalid {
                        var: "JC_PORTAL_MODEL_TOOLS_URL",
                        reason: e.to_string(),
                    })?;
                if url.scheme() != "http" && url.scheme() != "https" {
                    return Err(ConfigError::Invalid {
                        var: "JC_PORTAL_MODEL_TOOLS_URL",
                        reason: format!("scheme '{}' is not http or https", url.scheme()),
                    });
                }
                Some(raw)
            }
            None => None,
        };

        // Only the literal `true` turns it on: a misspelling must not open the door (ADR-N-019).
        let trust_edge_token = lookup("JC_TRUST_EDGE_TOKEN").is_some_and(|v| v.trim() == "true");

        let apps_dir = lookup("JC_PORTAL_APPS_DIR");
        let app_settings = app_settings(&lookup, &public_base_url);
        let agent_settings = agent_settings(&lookup)?;
        let branding_file = lookup("JC_BRANDING_FILE").filter(|path| !path.trim().is_empty());
        let database_url = lookup("JC_PORTAL_DATABASE_URL").filter(|url| !url.trim().is_empty());
        let bootstrap_admins = lookup("JC_PORTAL_BOOTSTRAP_ADMINS")
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| Self::DEFAULT_BOOTSTRAP_ADMINS.to_owned());

        Ok(Self {
            bind,
            public_base_url,
            oidc,
            trust_edge_token,
            cookie_key,
            sync_interval,
            gitea_webhook_secret,
            pipeline_runner_url,
            model_tools_url,
            apps_dir,
            branding_file,
            database_url,
            bootstrap_admins,
            app_settings,
            agent_settings,
        })
    }

    pub fn for_tests() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            public_base_url: Url::parse("http://localhost:8080")
                .unwrap_or_else(|_| unreachable!("valid test url")),
            oidc: None,
            trust_edge_token: false,
            cookie_key: Key::generate(),
            sync_interval: Duration::ZERO,
            gitea_webhook_secret: None,
            pipeline_runner_url: None,
            model_tools_url: None,
            app_settings: None,
            agent_settings: None,
            apps_dir: None,
            branding_file: None,
            database_url: None,
            // The dev realm's approver role: a test session that carries it may do everything,
            // one that does not is bound by whatever Role/RoleBinding the test puts in the mirror.
            bootstrap_admins: "portal-approver".to_owned(),
        }
    }

    /// Redirect URI registered for this portal in the Keycloak client (CC-40).
    pub fn redirect_uri(&self) -> String {
        format!(
            "{}/api/v1/auth/callback",
            self.public_base_url.as_str().trim_end_matches('/')
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config = Config::from_vars(|_| None).expect("default config");
        assert_eq!(
            config.bind,
            "0.0.0.0:8080".parse().expect("parse default bind")
        );
        assert_eq!(config.public_base_url.as_str(), "http://localhost:8080/");
    }

    #[test]
    fn custom_valid_values() {
        let config = Config::from_vars(|k| match k {
            "JC_PORTAL_BIND" => Some("127.0.0.1:9090".to_string()),
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.com".to_string()),
            _ => None,
        })
        .expect("custom config");
        assert_eq!(
            config.bind,
            "127.0.0.1:9090".parse().expect("parse custom bind")
        );
        assert_eq!(
            config.public_base_url.as_str(),
            "https://portal.example.com/"
        );
    }

    #[test]
    fn invalid_bind_produces_error() {
        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_BIND" => Some("invalid-bind".to_string()),
            _ => None,
        })
        .expect_err("should fail with invalid bind");

        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_BIND"),
        }
    }

    #[test]
    fn invalid_public_url_produces_error() {
        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_PUBLIC_URL" => Some("not a valid url".to_string()),
            _ => None,
        })
        .expect_err("should fail with invalid url");

        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_PUBLIC_URL"),
        }
    }

    #[test]
    fn partial_oidc_configuration_is_rejected() {
        let err = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            _ => None,
        })
        .expect_err("partial oidc config must fail closed");
        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_OIDC_ISSUER"),
        }
    }

    #[test]
    fn complete_oidc_configuration_is_accepted_and_redacts_the_secret() {
        let config = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            _ => None,
        })
        .expect("oidc config");
        let oidc = config.oidc.as_ref().expect("oidc present");
        assert_eq!(oidc.client_id, "portal");
        assert_eq!(oidc.client_secret(), "s3cr3t");
        let dumped = format!("{config:?}");
        assert!(
            !dumped.contains("s3cr3t"),
            "client secret leaked into Debug: {dumped}"
        );
        assert!(dumped.contains("[redacted]"));
    }

    #[test]
    fn oidc_ca_file_is_read_from_disk() {
        let path = std::env::temp_dir().join(format!("jc-portal-ca-{}.pem", std::process::id()));
        std::fs::write(&path, b"-----BEGIN CERTIFICATE-----\nnot-a-real-one\n").expect("write pem");
        let config = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            "JC_OIDC_CA_FILE" => Some(path.display().to_string()),
            _ => None,
        })
        .expect("oidc config with a ca file");
        let _ = std::fs::remove_file(&path);
        let pem = config
            .oidc
            .as_ref()
            .expect("oidc present")
            .extra_ca_pem
            .as_ref()
            .expect("ca pem loaded");
        assert!(pem.starts_with(b"-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn oidc_configuration_without_a_ca_file_carries_no_extra_root() {
        let config = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            _ => None,
        })
        .expect("oidc config");
        assert!(config.oidc.expect("oidc present").extra_ca_pem.is_none());
    }

    #[test]
    fn unreadable_oidc_ca_file_stops_start_up() {
        let err = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            "JC_OIDC_CA_FILE" => Some("/nonexistent/jc-portal/ca.crt".to_string()),
            _ => None,
        })
        .expect_err("a mount that is not there must fail closed, not lose the root silently");
        match err {
            ConfigError::Invalid { var, reason } => {
                assert_eq!(var, "JC_OIDC_CA_FILE");
                assert!(
                    reason.contains("/nonexistent/jc-portal/ca.crt"),
                    "the operator needs the path in the message: {reason}"
                );
            }
        }
    }

    #[test]
    fn short_cookie_key_is_rejected() {
        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_COOKIE_KEY" => Some("too-short".to_string()),
            _ => None,
        })
        .expect_err("short cookie key must fail");
        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_COOKIE_KEY"),
        }
    }

    #[test]
    fn redirect_uri_is_derived_from_the_public_base_url() {
        let config = Config::from_vars(|k| match k {
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.sk".to_string()),
            _ => None,
        })
        .expect("config");
        assert_eq!(
            config.redirect_uri(),
            "https://portal.example.sk/api/v1/auth/callback"
        );
    }

    #[test]
    fn for_tests_provides_valid_config() {
        let cfg = Config::for_tests();
        assert_eq!(cfg.bind.port(), 0);
    }

    #[test]
    fn sync_interval_and_webhook_secret_configuration() {
        let config = Config::from_vars(|k| match k {
            "JC_PORTAL_SYNC_INTERVAL" => Some("30".to_string()),
            "JC_GITEA_WEBHOOK_SECRET" => Some("my-secret".to_string()),
            _ => None,
        })
        .expect("config");
        assert_eq!(config.sync_interval, Duration::from_secs(30));
        assert_eq!(config.gitea_webhook_secret.as_deref(), Some("my-secret"));

        let debug = format!("{config:?}");
        assert!(!debug.contains("my-secret"));
        assert!(debug.contains("[redacted]"));

        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_SYNC_INTERVAL" => Some("not-a-number".to_string()),
            _ => None,
        })
        .expect_err("should reject invalid sync interval");
        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_SYNC_INTERVAL"),
        }
    }

    /// T-0411, AP-18: a Portal deploys an app only when it is told where. Guessing a namespace
    /// would mean writing a Deployment into somebody else's.
    #[test]
    fn app_settings_need_every_part_and_derive_the_one_they_can() {
        let complete = |k: &str| match k {
            "JC_PORTAL_PUBLIC_URL" => Some("https://bb.example.sk".to_string()),
            "JC_PORTAL_APPS_NAMESPACE" => Some("joinedcontext".to_string()),
            "JC_PORTAL_ORG_DOMAIN" => Some("banskabystrica.sk".to_string()),
            _ => None,
        };
        let settings = Config::from_vars(complete)
            .expect("a complete configuration")
            .app_settings
            .expect("every part is there");
        assert_eq!(settings.namespace, "joinedcontext");
        assert_eq!(settings.org_domain, "banskabystrica.sk");
        // Not a variable of its own: the host is the Portal's public URL. No realm and no
        // sidecar image any more: the login front is the edge's one `edge` client (ADR-N-019).
        assert_eq!(settings.host, "bb.example.sk");

        for missing in ["JC_PORTAL_APPS_NAMESPACE", "JC_PORTAL_ORG_DOMAIN"] {
            let config = Config::from_vars(|k| match k == missing {
                true => None,
                false => complete(k),
            })
            .expect("a Portal without apps is still a Portal");
            assert!(config.app_settings.is_none(), "{missing} was not needed");
        }
    }

    /// ADR-N-019: the edge token is trusted on the literal `true` and on nothing else.
    #[test]
    fn the_edge_token_is_trusted_only_when_asked_for_in_so_many_words() {
        assert!(!Config::from_vars(|_| None).unwrap().trust_edge_token);
        for value in ["1", "yes", "TRUE", ""] {
            let config = Config::from_vars(|k| match k {
                "JC_TRUST_EDGE_TOKEN" => Some(value.to_string()),
                _ => None,
            })
            .unwrap();
            assert!(!config.trust_edge_token, "{value:?} opened the door");
        }
        let config = Config::from_vars(|k| match k {
            "JC_TRUST_EDGE_TOKEN" => Some("true".to_string()),
            _ => None,
        })
        .unwrap();
        assert!(config.trust_edge_token);
    }
}
