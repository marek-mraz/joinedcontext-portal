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
    /// PostgreSQL connection string of the preferences tier (UI-09). Carries a password, so it
    /// is redacted in `Debug`. `None` runs the Portal without preferences: those routes answer
    /// 503, everything else works.
    pub database_url: Option<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("public_base_url", &self.public_base_url.as_str())
            .field("oidc", &self.oidc)
            .field("cookie_key", &"[redacted]")
            .field("sync_interval", &self.sync_interval)
            .field(
                "gitea_webhook_secret",
                &self.gitea_webhook_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("pipeline_runner_url", &self.pipeline_runner_url)
            .field("model_tools_url", &self.model_tools_url)
            .field("apps_dir", &self.apps_dir)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
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
    pub const DEFAULT_PUBLIC_URL: &'static str = "http://localhost:8080";
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

        let apps_dir = lookup("JC_PORTAL_APPS_DIR");
        let database_url = lookup("JC_PORTAL_DATABASE_URL").filter(|url| !url.trim().is_empty());

        Ok(Self {
            bind,
            public_base_url,
            oidc,
            cookie_key,
            sync_interval,
            gitea_webhook_secret,
            pipeline_runner_url,
            model_tools_url,
            apps_dir,
            database_url,
        })
    }

    pub fn for_tests() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            public_base_url: Url::parse("http://localhost:8080")
                .unwrap_or_else(|_| unreachable!("valid test url")),
            oidc: None,
            cookie_key: Key::generate(),
            sync_interval: Duration::ZERO,
            gitea_webhook_secret: None,
            pipeline_runner_url: None,
            model_tools_url: None,
            apps_dir: None,
            database_url: None,
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
}
