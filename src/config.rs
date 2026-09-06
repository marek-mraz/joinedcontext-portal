use std::net::SocketAddr;
use url::Url;

/// Portal server configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub public_base_url: Url,
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

        Ok(Self {
            bind,
            public_base_url,
        })
    }

    pub fn for_tests() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            public_base_url: Url::parse("http://localhost:8080")
                .unwrap_or_else(|_| unreachable!("valid test url")),
        }
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
    fn for_tests_provides_valid_config() {
        let cfg = Config::for_tests();
        assert_eq!(cfg.bind.port(), 0);
    }
}
