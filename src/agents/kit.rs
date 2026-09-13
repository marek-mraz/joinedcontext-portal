//! The kit: the dashboard the builder fills in (Architecture/19 §1.2, AP-56, AG-54).
//!
//! The bundle under `kit/` renders one `spec.json`; this module is the Portal's copy of what
//! that file may hold. The type is what the model is shown as a JSON Schema, what a model
//! answer is parsed into before anything reaches a browser, and what the preview document is
//! rendered from. `kit/src/spec.ts` says the same things in TypeScript so the bundle can name
//! what is wrong if it is ever handed a specification this module did not check.

use std::sync::LazyLock;

use base64::Engine;
use rust_embed::RustEmbed;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The file the model writes, and the only one a kit run may write (AP-58).
pub const SPEC_FILE: &str = "spec.json";
pub const DEFAULT_LIMIT: u32 = 1000;
pub const MAX_LIMIT: u32 = 5000;
/// Where the basemap tiles come from; the one host beside the platform a preview may reach.
pub const TILES: &str = "https://tile.openstreetmap.org";

/// `kit/dist`, built with the Portal. Empty in a plain `cargo test`, which is what the 503 of
/// [`bundle`] is for.
#[derive(RustEmbed)]
#[folder = "kit/dist"]
struct Dist;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Agg {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// One entity type read from the endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Source {
    /// A short name the views and filters refer to.
    pub name: String,
    /// The NGSI-LD entity type.
    #[serde(rename = "type")]
    pub entity_type: String,
    /// The attributes the views may use; the request asks for these and no more.
    pub attrs: Vec<String>,
    /// An NGSI-LD `q` applied on the endpoint, before any filter on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Entities read at most; default 1000, ceiling 5000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A control above the views that narrows the rows of one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase", tag = "kind")]
pub enum Filter {
    /// A text box matched against the named attributes.
    #[serde(rename = "search")]
    Search {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        attrs: Vec<String>,
    },
    /// A drop-down of the attribute's distinct values.
    #[serde(rename = "select")]
    Select {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        attr: String,
    },
    /// Two sliders over a numeric attribute.
    #[serde(rename = "range")]
    Range {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        attr: String,
    },
}

/// One tile of a `stats` view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StatItem {
    pub label: String,
    pub agg: Agg,
    /// The numeric attribute aggregated; not needed for `count`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChartType {
    Bar,
    Line,
    Pie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Sort {
    pub attr: String,
    pub dir: SortDir,
}

/// One card of the dashboard. Views are drawn in order; `stats`, `map` and `table` take the
/// whole width, the others share a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase", tag = "kind")]
pub enum View {
    /// A row of numbers over the rows the filters left.
    #[serde(rename = "stats")]
    Stats {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        items: Vec<StatItem>,
    },
    /// A map with one point per entity that has a location.
    #[serde(rename = "map")]
    Map {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// The GeoProperty drawn; `location` when not named.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<String>,
        /// The attribute shown when a point is picked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// The attribute the points are coloured by: a ramp for numbers, a hue per text value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    /// A sortable, paged table; a click on a row selects it for `detail`.
    #[serde(rename = "table")]
    Table {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        columns: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort: Option<Sort>,
    },
    /// An SVG chart: `y` aggregated per distinct `x`, the largest `top` kept.
    #[serde(rename = "chart")]
    Chart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(rename = "type")]
        chart_type: ChartType,
        x: String,
        y: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agg: Option<Agg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        top: Option<u32>,
    },
    /// Every attribute of the selected entity.
    #[serde(rename = "detail")]
    Detail {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Theme {
    /// A CSS colour for tiles, bars and points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
}

/// The whole of `spec.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Spec {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    pub views: Vec<View>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
}

/// The JSON Schema the model is shown, rendered once.
pub fn schema_json() -> &'static str {
    static SCHEMA: LazyLock<String> = LazyLock::new(|| {
        serde_json::to_string_pretty(&schemars::schema_for!(Spec)).expect("the kit schema renders")
    });
    &SCHEMA
}

/// `spec.json` as the model wrote it, or every reason the kit cannot render it (AP-59).
pub fn parse(text: &str) -> Result<Spec, Vec<String>> {
    let spec: Spec =
        serde_json::from_str(text).map_err(|error| vec![format!("spec.json: {error}")])?;
    let errors = validate(&spec);
    if errors.is_empty() {
        Ok(spec)
    } else {
        Err(errors)
    }
}

/// What the schema cannot say: source names are unique, every reference names a source, every
/// attribute is one that source reads. Same rules and same wording as `kit/src/spec.ts`.
pub fn validate(spec: &Spec) -> Vec<String> {
    let mut errors = Vec::new();
    if spec.title.trim().is_empty() {
        errors.push("title: must be a non-empty string".to_owned());
    }
    if spec.sources.is_empty() {
        errors.push("sources: must list at least one entity type".to_owned());
    }
    let mut known: Vec<(&str, Vec<&str>)> = Vec::new();
    for (index, source) in spec.sources.iter().enumerate() {
        if source.name.is_empty() {
            errors.push(format!("sources[{index}].name: must be a non-empty string"));
        } else if known.iter().any(|(name, _)| *name == source.name) {
            errors.push(format!(
                "sources[{index}].name: '{}' is used twice",
                source.name
            ));
            continue;
        }
        if source.entity_type.is_empty() {
            errors.push(format!("sources[{index}].type: must be an entity type"));
        }
        if source.attrs.is_empty() {
            errors.push(format!(
                "sources[{index}].attrs: must list at least one attribute"
            ));
        }
        if source
            .limit
            .is_some_and(|limit| limit == 0 || limit > MAX_LIMIT)
        {
            errors.push(format!(
                "sources[{index}].limit: must be between 1 and {MAX_LIMIT}"
            ));
        }
        let mut attrs = vec!["id", "type"];
        attrs.extend(source.attrs.iter().map(String::as_str));
        known.push((source.name.as_str(), attrs));
    }
    let first = known.first().map(|(name, _)| *name).unwrap_or("");
    let resolve = |errors: &mut Vec<String>, path: &str, name: Option<&str>| -> Option<Vec<&str>> {
        let key = name.unwrap_or(first);
        match known.iter().find(|(known, _)| *known == key) {
            Some((_, attrs)) => Some(attrs.clone()),
            None => {
                errors.push(format!("{path}.source: '{key}' is not a source"));
                None
            }
        }
    };
    let check = |errors: &mut Vec<String>, path: &str, attrs: &Option<Vec<&str>>, attr: &str| {
        if let Some(attrs) = attrs {
            if !attrs.contains(&attr) {
                errors.push(format!(
                    "{path}: '{attr}' is not among the source's attributes"
                ));
            }
        }
    };

    for (index, filter) in spec.filters.iter().enumerate() {
        let path = format!("filters[{index}]");
        match filter {
            Filter::Search { source, attrs, .. } => {
                let known = resolve(&mut errors, &path, source.as_deref());
                if attrs.is_empty() {
                    errors.push(format!("{path}.attrs: must list the attributes to search"));
                }
                for (i, attr) in attrs.iter().enumerate() {
                    check(&mut errors, &format!("{path}.attrs[{i}]"), &known, attr);
                }
            }
            Filter::Select { source, attr, .. } | Filter::Range { source, attr, .. } => {
                let known = resolve(&mut errors, &path, source.as_deref());
                check(&mut errors, &format!("{path}.attr"), &known, attr);
            }
        }
    }

    if spec.views.is_empty() {
        errors.push("views: must list at least one view".to_owned());
    }
    for (index, view) in spec.views.iter().enumerate() {
        let path = format!("views[{index}]");
        match view {
            View::Stats { source, items, .. } => {
                let known = resolve(&mut errors, &path, source.as_deref());
                if items.is_empty() {
                    errors.push(format!("{path}.items: must list at least one tile"));
                }
                for (i, item) in items.iter().enumerate() {
                    if item.agg != Agg::Count {
                        match &item.attr {
                            Some(attr) => check(
                                &mut errors,
                                &format!("{path}.items[{i}].attr"),
                                &known,
                                attr,
                            ),
                            None => errors
                                .push(format!("{path}.items[{i}].attr: must name an attribute")),
                        }
                    }
                }
            }
            View::Map {
                source,
                location,
                label,
                color,
                ..
            } => {
                let known = resolve(&mut errors, &path, source.as_deref());
                for (field, value) in [("location", location), ("label", label), ("color", color)] {
                    if let Some(attr) = value {
                        check(&mut errors, &format!("{path}.{field}"), &known, attr);
                    }
                }
            }
            View::Table {
                source,
                columns,
                sort,
                ..
            } => {
                let known = resolve(&mut errors, &path, source.as_deref());
                if columns.is_empty() {
                    errors.push(format!("{path}.columns: must list at least one column"));
                }
                for (i, column) in columns.iter().enumerate() {
                    check(&mut errors, &format!("{path}.columns[{i}]"), &known, column);
                }
                if let Some(sort) = sort {
                    check(
                        &mut errors,
                        &format!("{path}.sort.attr"),
                        &known,
                        &sort.attr,
                    );
                }
            }
            View::Chart { source, x, y, .. } => {
                let known = resolve(&mut errors, &path, source.as_deref());
                check(&mut errors, &format!("{path}.x"), &known, x);
                check(&mut errors, &format!("{path}.y"), &known, y);
            }
            View::Detail { source, .. } => {
                resolve(&mut errors, &path, source.as_deref());
            }
        }
    }
    errors
}

/// The built bundle, or nothing when the Portal was compiled without one.
pub fn bundle() -> Option<(String, String)> {
    let js = Dist::get("kit.js")?;
    let css = Dist::get("kit.css")?;
    Some((
        String::from_utf8_lossy(&js.data).into_owned(),
        String::from_utf8_lossy(&css.data).into_owned(),
    ))
}

/// The CSP source of one inline script: its SHA-256, so the policy needs no `'unsafe-inline'`.
pub fn script_hash(js: &str) -> String {
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(js.as_bytes()))
    )
}

/// The policy of the preview document: the kit's own script, the platform origin and the
/// tiles, nothing else (AP-49, AP-50). `'unsafe-inline'` styles because the bundle's
/// stylesheet is inlined and MapLibre writes style attributes.
pub fn content_security_policy(origin: &str, hash: &str) -> String {
    format!(
        "default-src 'none'; base-uri 'none'; form-action 'none'; script-src {hash}; \
         style-src 'unsafe-inline'; img-src data: blob: {TILES}; font-src data:; \
         connect-src {origin} {TILES}; worker-src blob:; child-src blob:; frame-ancestors 'self'"
    )
}

/// One document: the stylesheet, the specification as data, the script. Nothing in it is
/// fetched later, because a frame without `allow-same-origin` has no session to fetch with.
pub fn document(title: &str, slug: &str, spec: &Spec, js: &str, css: &str) -> String {
    let payload = serde_json::to_string(&serde_json::json!({ "slug": slug, "spec": spec }))
        .unwrap_or_default()
        // `</script>` inside the data would end the element; `\u003c` is the same character
        // to a JSON parser and nothing to the HTML one.
        .replace('<', "\\u003c");
    let title = escape(title);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n<style>{css}</style>\n</head>\n<body>\n<div id=\"root\"></div>\n\
         <script id=\"kit-spec\" type=\"application/json\">{payload}</script>\n\
         <script type=\"module\">{js}</script>\n</body>\n</html>\n"
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../kit/spec.example.json");

    #[test]
    fn the_example_the_bundle_ships_with_parses_clean() {
        let spec = parse(EXAMPLE).expect("the example is valid");
        assert_eq!(spec.title, "Helsinki city bikes");
        assert_eq!(spec.views.len(), 5);
        assert_eq!(spec.filters.len(), 3);
    }

    #[test]
    fn the_schema_names_every_view_kind_and_refuses_unknown_fields() {
        let schema = schema_json();
        for kind in ["stats", "map", "table", "chart", "detail"] {
            assert!(
                schema.contains(&format!("\"{kind}\"")),
                "{kind} missing from the schema"
            );
        }
        let error = parse(r#"{"title":"x","sources":[{"name":"s","type":"T","attrs":["a"],"secret":1}],"views":[]}"#)
            .expect_err("unknown field");
        assert!(error[0].contains("unknown field `secret`"), "{error:?}");
    }

    #[test]
    fn every_problem_is_named_at_once_with_its_path() {
        let errors = parse(
            r#"{
              "title": " ",
              "sources": [{"name":"s","type":"T","attrs":["a"]},{"name":"s","type":"T","attrs":[]}],
              "filters": [{"kind":"select","attr":"zzz"}],
              "views": [
                {"kind":"chart","type":"bar","x":"a","y":"b"},
                {"kind":"table","source":"ghost","columns":[]},
                {"kind":"stats","items":[{"label":"n","agg":"sum"}]},
                {"kind":"map","color":"nope"}
              ]
            }"#,
        )
        .expect_err("invalid");
        assert_eq!(
            errors,
            vec![
                "title: must be a non-empty string",
                "sources[1].name: 's' is used twice",
                "filters[0].attr: 'zzz' is not among the source's attributes",
                "views[0].y: 'b' is not among the source's attributes",
                "views[1].source: 'ghost' is not a source",
                "views[1].columns: must list at least one column",
                "views[2].items[0].attr: must name an attribute",
                "views[3].color: 'nope' is not among the source's attributes",
            ]
        );
        assert!(parse("{").expect_err("json")[0].starts_with("spec.json: "));
    }

    #[test]
    fn the_document_inlines_everything_and_cannot_be_broken_out_of() {
        let spec = parse(EXAMPLE).expect("valid");
        let mut spec = spec;
        spec.title = "</script><script>alert(1)</script>".to_owned();
        let html = document("A & <B>", "s1ug", &spec, "console.log(1)", "body{}");
        assert!(html.contains("<title>A &amp; &lt;B&gt;</title>"));
        assert!(html.contains("<style>body{}</style>"));
        assert!(html.contains("<script type=\"module\">console.log(1)</script>"));
        assert!(html.contains("\"slug\":\"s1ug\""));
        assert!(
            !html.contains("</script><script>alert"),
            "the data ended the element"
        );
        assert!(html.contains("\\u003c/script>"));
        let csp = content_security_policy("https://portal.example", &script_hash("console.log(1)"));
        assert!(csp.contains("script-src 'sha256-"));
        assert!(csp.contains("connect-src https://portal.example https://tile.openstreetmap.org"));
        assert!(!csp.contains("script-src 'unsafe-inline'"));
        assert!(csp.contains("style-src 'unsafe-inline'"));
    }
}
