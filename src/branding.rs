//! The branding of one installation (UI-30, OPS-46, Deployment/12).
//!
//! One image serves every city. What differs is a file: the deployment renders
//! `global.branding` into a ConfigMap, mounts it and names it in `JC_BRANDING_FILE`. Nothing
//! here is a secret, which is why the endpoint that serves it is public and cacheable: the
//! login page needs the instance name and the logo before anyone has signed in.
//!
//! A missing, unreadable or invalid file is not an error. An installation whose ConfigMap has
//! not been rendered yet looks plain rather than failing to load, and the reason is logged.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Everything the Portal shows that names or themes an installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Branding {
    /// Full name: page titles and the login page.
    pub instance_name: String,
    /// Short name: sidebars, tabs, e-mail subjects. A block that omits it gets the full name,
    /// so the field default is empty rather than the struct's.
    #[serde(default)]
    pub short_name: String,
    /// The city or region this installation serves.
    pub city: String,
    /// The legal publisher, which is also the DCAT-AP `dcterms:publisher`.
    pub organisation: String,
    /// The organization's domain: URN segment, realm display name, `did:web`.
    pub org_domain: String,
    /// The platform host.
    pub domain: String,
    /// The DCAT-AP contact point and the footer's address.
    pub contact_email: String,
    /// The default dataset licence.
    pub license_default: String,
    /// Logo file, served from the platform's own origin.
    pub logo: String,
    /// Favicon file, served from the platform's own origin.
    pub favicon: String,
    /// The colour tokens the UI writes as CSS custom properties.
    pub colours: Colours,
    /// The font stacks the UI writes as CSS custom properties.
    pub fonts: Fonts,
    /// The locales the language switcher offers.
    pub languages: Languages,
    /// Readable text on top of the primary colour. Always computed from that colour, never
    /// taken from the file: the block names a primary colour but no foreground, and white on
    /// a light primary is unreadable (WCAG 1.4.3). A value in the file is overwritten.
    pub primary_foreground: String,
}

/// The five colours a page is built from. Each is validated as a hex triplet or sextet before
/// it is served, because the UI writes it into a CSS custom property and a value that is not a
/// colour is a way into the page (OPS-46).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Colours {
    /// Primary action colour.
    pub primary: String,
    /// Secondary accent used for links and focus.
    pub secondary: String,
    /// Highlight colour.
    pub accent: String,
    /// Page background.
    pub background: String,
    /// Body text.
    pub text: String,
}

/// Heading and body font stacks. Self-hosted or system families only: nothing on a page
/// fetches from a third-party origin at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Fonts {
    /// Font stack for headings.
    pub heading: String,
    /// Font stack for body text.
    pub body: String,
}

/// The locale the UI starts in and the ones it offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Languages {
    /// The locale a first-time visitor gets.
    pub default: String,
    /// Every locale the switcher lists; always contains the default.
    pub offered: Vec<String>,
}

impl Default for Branding {
    fn default() -> Self {
        Self {
            instance_name: "joinedcontext".into(),
            short_name: "joinedcontext".into(),
            city: String::new(),
            organisation: String::new(),
            org_domain: String::new(),
            domain: String::new(),
            contact_email: String::new(),
            license_default: "CC-BY-4.0".into(),
            logo: String::new(),
            favicon: String::new(),
            colours: Colours::default(),
            fonts: Fonts::default(),
            languages: Languages::default(),
            primary_foreground: "#ffffff".into(),
        }
    }
}

impl Default for Colours {
    fn default() -> Self {
        Self {
            primary: "#1d4ed8".into(),
            secondary: "#0f766e".into(),
            accent: "#f59e0b".into(),
            background: "#ffffff".into(),
            text: "#0f172a".into(),
        }
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            heading: "system-ui, sans-serif".into(),
            body: "system-ui, sans-serif".into(),
        }
    }
}

impl Default for Languages {
    fn default() -> Self {
        Self {
            default: "en".into(),
            offered: vec!["en".into()],
        }
    }
}

impl Branding {
    /// Reads the branding file, falling back to neutral defaults for anything unusable.
    ///
    /// The file is read on every call rather than cached, so applying a new ConfigMap changes
    /// the Portal without a restart (Deployment/12 section 4).
    pub fn load(path: Option<&str>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!(%path, error = %err, "branding file unreadable, serving defaults");
                return Self::default();
            }
        };
        match serde_yaml_ng::from_str::<Self>(&text) {
            Ok(branding) => branding.sanitised(),
            Err(err) => {
                tracing::warn!(%path, error = %err, "branding file is not valid YAML, serving defaults");
                Self::default()
            }
        }
    }

    /// Replaces every value the UI must not be handed with its neutral default.
    pub fn sanitised(mut self) -> Self {
        let fallback = Self::default();
        self.colours = self.colours.sanitised(&fallback.colours);
        self.logo = same_origin(&self.logo, "logo");
        self.favicon = same_origin(&self.favicon, "favicon");
        self.languages = self.languages.sanitised(&fallback.languages);
        self.primary_foreground = self.primary_foreground().to_owned();
        if self.instance_name.trim().is_empty() {
            self.instance_name = fallback.instance_name.clone();
        }
        if self.short_name.trim().is_empty() {
            self.short_name = self.instance_name.clone();
        }
        self
    }

    /// Whether the primary colour is dark enough for white text on top of it (WCAG 1.4.3).
    ///
    /// The branding block names a primary colour but no foreground for it, and a light
    /// primary with white text is unreadable, so the readable one is computed.
    pub fn primary_foreground(&self) -> &'static str {
        match luminance(&self.colours.primary) {
            Some(l) if l > 0.5 => "#0f172a",
            _ => "#ffffff",
        }
    }
}

impl Colours {
    fn sanitised(self, fallback: &Self) -> Self {
        Self {
            primary: hex_or(self.primary, &fallback.primary, "primary"),
            secondary: hex_or(self.secondary, &fallback.secondary, "secondary"),
            accent: hex_or(self.accent, &fallback.accent, "accent"),
            background: hex_or(self.background, &fallback.background, "background"),
            text: hex_or(self.text, &fallback.text, "text"),
        }
    }
}

impl Languages {
    fn sanitised(mut self, fallback: &Self) -> Self {
        self.offered.retain(|locale| is_locale(locale));
        if !is_locale(&self.default) {
            tracing::warn!(locale = %self.default, "branding default locale is not a locale tag");
            self.default = fallback.default.clone();
        }
        // A switcher that cannot reach the locale the page starts in would strand the visitor.
        if !self.offered.iter().any(|locale| locale == &self.default) {
            self.offered.insert(0, self.default.clone());
        }
        self
    }
}

/// A hex triplet or sextet, or the fallback with the reason logged.
fn hex_or(value: String, fallback: &str, field: &'static str) -> String {
    if is_hex_colour(&value) {
        return value;
    }
    tracing::warn!(field, %value, "branding colour is not a hex colour, using the default");
    fallback.to_owned()
}

/// `#rgb` or `#rrggbb`, which is the whole of what may reach a CSS custom property.
pub fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 6) && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A file name served from the platform's own origin: no scheme, no host, no traversal.
fn same_origin(value: &str, field: &'static str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let bad = trimmed.contains("://")
        || trimmed.starts_with("//")
        || trimmed.starts_with('/')
        || trimmed.contains("..")
        || trimmed.contains(char::is_whitespace);
    if bad {
        tracing::warn!(field, value = %trimmed, "branding asset is not a same-origin file name");
        return String::new();
    }
    trimmed.to_owned()
}

/// A BCP 47 tag as far as the switcher needs it: letters, digits and hyphens.
fn is_locale(value: &str) -> bool {
    (2..=12).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic())
}

/// Relative luminance of a hex colour, for the one contrast decision the UI cannot make.
fn luminance(colour: &str) -> Option<f32> {
    let digits = colour.strip_prefix('#')?;
    let expand = |c: u8| u8::from_str_radix(&format!("{}{}", c as char, c as char), 16).ok();
    let (r, g, b) = match digits.len() {
        3 => {
            let d = digits.as_bytes();
            (expand(d[0])?, expand(d[1])?, expand(d[2])?)
        }
        6 => (
            u8::from_str_radix(&digits[0..2], 16).ok()?,
            u8::from_str_radix(&digits[2..4], 16).ok()?,
            u8::from_str_radix(&digits[4..6], 16).ok()?,
        ),
        _ => return None,
    };
    // The sRGB coefficients, close enough for a black-or-white decision.
    Some((0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = r##"
instanceName: "Banská Bystrica Context"
shortName: "BB Context"
city: "Banská Bystrica"
organisation: "Mesto Banská Bystrica"
orgDomain: "banskabystrica.sk"
domain: "bb.example.com"
contactEmail: "opendata@banskabystrica.sk"
licenseDefault: "CC-BY-4.0"
logo: "logo.svg"
favicon: "favicon.svg"
colours:
  primary: "#0000bf"
  secondary: "#0072c6"
  accent: "#ffe977"
  background: "#ffffff"
  text: "#1a1a1a"
fonts:
  heading: "HelsinkiGrotesk, system-ui, sans-serif"
  body: "system-ui, sans-serif"
languages:
  default: "sk"
  offered: ["sk", "en"]
"##;

    fn written(contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "jc-branding-{}-{}.yaml",
            std::process::id(),
            contents.len()
        ));
        std::fs::write(&path, contents).expect("write the branding file");
        path
    }

    #[test]
    fn no_branding_file_is_neutral_defaults() {
        let branding = Branding::load(None);
        assert_eq!(branding.instance_name, "joinedcontext");
        assert_eq!(branding.colours.primary, "#1d4ed8");
        assert_eq!(branding.languages.offered, vec!["en".to_string()]);
    }

    #[test]
    fn an_unreadable_file_is_neutral_defaults() {
        assert_eq!(
            Branding::load(Some("/nonexistent/jc/branding.yaml")),
            Branding::default()
        );
    }

    #[test]
    fn an_invalid_file_is_neutral_defaults() {
        let path = written("instanceName: [unclosed\n");
        let branding = Branding::load(Some(&path.display().to_string()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(branding, Branding::default());
    }

    #[test]
    fn the_documented_block_is_served_as_written() {
        let path = written(BLOCK);
        let branding = Branding::load(Some(&path.display().to_string()));
        let _ = std::fs::remove_file(&path);

        assert_eq!(branding.instance_name, "Banská Bystrica Context");
        assert_eq!(branding.organisation, "Mesto Banská Bystrica");
        assert_eq!(branding.colours.primary, "#0000bf");
        assert_eq!(
            branding.fonts.heading,
            "HelsinkiGrotesk, system-ui, sans-serif"
        );
        assert_eq!(branding.languages.default, "sk");
        assert_eq!(
            branding.languages.offered,
            vec!["sk".to_string(), "en".into()]
        );
        assert_eq!(branding.logo, "logo.svg");
    }

    #[test]
    fn a_partial_block_keeps_the_defaults_for_what_it_omits() {
        let path = written("instanceName: \"Helsinki Region Context\"\n");
        let branding = Branding::load(Some(&path.display().to_string()));
        let _ = std::fs::remove_file(&path);

        assert_eq!(branding.instance_name, "Helsinki Region Context");
        // A block that names no short name gets the full one rather than "joinedcontext".
        assert_eq!(branding.short_name, "Helsinki Region Context");
        assert_eq!(branding.colours, Colours::default());
    }

    /// OPS-46: a custom property is a value the browser evaluates.
    #[test]
    fn a_colour_that_is_not_a_colour_is_replaced_by_the_default() {
        for attempt in [
            "red; background: url(https://evil.example/x)",
            "var(--anything)",
            "#12",
            "#1234567",
            "#gggggg",
            "",
        ] {
            let branding = Branding {
                colours: Colours {
                    primary: attempt.to_owned(),
                    ..Colours::default()
                },
                ..Branding::default()
            }
            .sanitised();
            assert_eq!(
                branding.colours.primary,
                Colours::default().primary,
                "`{attempt}` must not reach a style"
            );
        }
        assert!(is_hex_colour("#fff") && is_hex_colour("#0000BF"));
    }

    #[test]
    fn an_asset_from_another_origin_is_dropped() {
        for attempt in [
            "https://evil.example/logo.svg",
            "//evil.example/logo.svg",
            "../../etc/passwd",
            "/etc/passwd",
        ] {
            let branding = Branding {
                logo: attempt.to_owned(),
                ..Branding::default()
            }
            .sanitised();
            assert!(branding.logo.is_empty(), "`{attempt}` must not be served");
        }
    }

    #[test]
    fn the_switcher_always_reaches_the_starting_locale() {
        let branding = Branding {
            languages: Languages {
                default: "fi".into(),
                offered: vec!["sv".into(), "en".into()],
            },
            ..Branding::default()
        }
        .sanitised();

        assert_eq!(branding.languages.offered, vec!["fi", "sv", "en"]);
    }

    #[test]
    fn a_light_primary_gets_dark_text_on_it() {
        let light = Branding {
            colours: Colours {
                primary: "#ffe977".into(),
                ..Colours::default()
            },
            ..Branding::default()
        };
        assert_eq!(light.primary_foreground(), "#0f172a");
        assert_eq!(Branding::default().primary_foreground(), "#ffffff");
    }
}
