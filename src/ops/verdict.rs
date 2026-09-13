//! One verdict for every check and validation run (AG-62, UI-48).
//!
//! A check (dry run, pipeline test, LinkML validation, schema validation) produces a Verdict:
//! `ok`, structured `findings`, optional execution `trace`, `checked_at` timestamp, and the
//! deterministic digest of the input it judged. A draft's proposal is gated on having a fresh
//! green verdict matching its manifest (PF-57).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::state::AppState;

pub use crate::branding::Validation;

/// The validation mode of this installation, from its branding file (PF-57): strict unless the
/// file says `validation: lax`.
pub fn get_validation_mode(state: &AppState) -> Validation {
    state.branding().validation
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Finding {
    pub level: Level,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Verdict {
    pub ok: bool,
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
    #[schema(value_type = String, format = DateTime)]
    pub checked_at: DateTime<Utc>,
    pub input_digest: String,
}

impl Verdict {
    pub fn new(ok: bool, findings: Vec<Finding>, trace: Option<Value>, input: &Value) -> Self {
        Self {
            ok,
            findings,
            trace,
            checked_at: Utc::now(),
            input_digest: digest_of(input),
        }
    }

    pub fn green(input: &Value, trace: Option<Value>) -> Self {
        Self::new(true, Vec::new(), trace, input)
    }

    pub fn red(input: &Value, findings: Vec<Finding>, trace: Option<Value>) -> Self {
        Self::new(false, findings, trace, input)
    }

    /// True only when the check passed and its input digest matches the candidate manifest.
    pub fn is_fresh_for(&self, manifest: &Value) -> bool {
        self.ok && self.input_digest == digest_of(manifest)
    }
}

/// Computes a stable 64-bit FNV-1a hex digest (16 lowercase hex characters) of canonical JSON.
///
/// Keys are sorted recursively so the digest is order-independent across key insertions.
/// Matches the client-side `digestOf` in `ui/src/api/drafts.ts` byte-for-byte.
pub fn digest_of(input: &Value) -> String {
    let canonical = canonicalize(input);
    let s = serde_json::to_string(&canonical).unwrap_or_default();
    let mut hash: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

fn canonicalize(val: &Value) -> Value {
    match val {
        Value::Object(map) => {
            let mut sorted = std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize(v));
            }
            let mut new_map = serde_json::Map::new();
            for (k, v) in sorted {
                new_map.insert(k, v);
            }
            Value::Object(new_map)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_is_order_independent() {
        let a = json!({ "name": "foo", "spec": { "url": "https://example.com", "port": 80 } });
        let b = json!({ "spec": { "port": 80, "url": "https://example.com" }, "name": "foo" });
        assert_eq!(digest_of(&a), digest_of(&b));
        assert_eq!(digest_of(&a).len(), 16);
    }

    #[test]
    fn digest_differs_on_content_change() {
        let a = json!({ "name": "foo", "spec": { "url": "https://example.com" } });
        let b = json!({ "name": "foo", "spec": { "url": "https://other.example.com" } });
        assert_ne!(digest_of(&a), digest_of(&b));
    }

    #[test]
    fn freshness_reflects_matching_manifest() {
        let manifest = json!({ "kind": "DataSource", "metadata": { "name": "bikes" } });
        let v_green = Verdict::green(&manifest, None);
        assert!(v_green.is_fresh_for(&manifest));

        let changed = json!({ "kind": "DataSource", "metadata": { "name": "bikes-2" } });
        assert!(!v_green.is_fresh_for(&changed));

        let v_red = Verdict::red(
            &manifest,
            vec![Finding {
                level: Level::Error,
                path: "/spec".into(),
                message: "bad spec".into(),
            }],
            None,
        );
        assert!(!v_red.is_fresh_for(&manifest));
    }
}
