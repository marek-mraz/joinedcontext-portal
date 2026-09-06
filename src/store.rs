use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::resource::selector::{FieldSelector, LabelSelector};
use crate::resource::{ResourceEnvelope, ResourceKey, API_VERSION};

#[derive(Debug, Default)]
pub struct ListOptions {
    pub label_selector: Option<LabelSelector>,
    pub field_selector: Option<FieldSelector>,
    pub limit: Option<usize>,
    pub continue_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ListPage {
    pub items: Vec<ResourceEnvelope>,
    pub continue_token: Option<String>,
    pub remaining: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse yaml at {path}: {source}")]
    Yaml {
        path: PathBuf,
        source: serde_yaml_ng::Error,
    },
    #[error("invalid resource at {path}: {reason}")]
    InvalidResource { path: PathBuf, reason: String },
}

#[derive(Default)]
pub struct Mirror {
    resources: RwLock<BTreeMap<ResourceKey, ResourceEnvelope>>,
}

impl Mirror {
    pub fn new() -> Self {
        Self {
            resources: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn upsert(&self, env: ResourceEnvelope) {
        let key = env.key();
        let mut lock = self.resources.write().unwrap_or_else(|p| p.into_inner());
        lock.insert(key, env);
    }

    pub fn remove(&self, key: &ResourceKey) -> Option<ResourceEnvelope> {
        let mut lock = self.resources.write().unwrap_or_else(|p| p.into_inner());
        lock.remove(key)
    }

    pub fn len(&self) -> usize {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        lock.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn replace_all(&self, other: &Mirror) {
        let new_resources = other
            .resources
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut lock = self.resources.write().unwrap_or_else(|p| p.into_inner());
        *lock = new_resources;
    }

    pub fn count_matching(&self, mut predicate: impl FnMut(&ResourceEnvelope) -> bool) -> usize {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        lock.values().filter(|env| predicate(env)).count()
    }

    pub fn get(&self, namespace: &str, kind: &str, name: &str) -> Option<ResourceEnvelope> {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        let key = ResourceKey {
            namespace: namespace.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
        };
        lock.get(&key).cloned()
    }

    pub fn list(&self, namespace: &str, kind: &str, opts: &ListOptions) -> ListPage {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());

        let mut filtered: Vec<ResourceEnvelope> = lock
            .values()
            .filter(|env| {
                let ns = env.metadata.namespace.as_deref().unwrap_or_default();
                if ns != namespace || env.kind != kind {
                    return false;
                }
                if let Some(ref ls) = opts.label_selector {
                    if !ls.matches(&env.metadata.labels) {
                        return false;
                    }
                }
                if let Some(ref fs) = opts.field_selector {
                    if !fs.matches(env) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        filtered.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));

        let mut items = filtered;
        if let Some(ref token) = opts.continue_token {
            items.retain(|item| item.metadata.name.as_str() > token.as_str());
        }

        let total = items.len();
        match opts.limit {
            Some(limit) if limit < total => {
                let page_items: Vec<ResourceEnvelope> = items.into_iter().take(limit).collect();
                let continue_token = page_items.last().map(|i| i.metadata.name.clone());
                let remaining = total - limit;
                ListPage {
                    items: page_items,
                    continue_token,
                    remaining,
                }
            }
            _ => ListPage {
                items,
                continue_token: None,
                remaining: 0,
            },
        }
    }

    pub fn load_dir(root: &Path) -> Result<Self, MirrorError> {
        let mirror = Self::new();
        visit_dir(root, &mirror)?;
        Ok(mirror)
    }
}

fn visit_dir(dir: &Path, mirror: &Mirror) -> Result<(), MirrorError> {
    let entries = std::fs::read_dir(dir).map_err(|e| MirrorError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| MirrorError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        if path.is_dir() {
            visit_dir(&path, mirror)?;
        } else if path.is_file() {
            let is_yaml = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
                .unwrap_or(false);

            if !is_yaml {
                continue;
            }

            let content = std::fs::read_to_string(&path).map_err(|e| MirrorError::Io {
                path: path.clone(),
                source: e,
            })?;

            if content.trim().is_empty() {
                continue;
            }

            let value: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    return Err(MirrorError::Yaml {
                        path: path.clone(),
                        source: e,
                    });
                }
            };

            let api_version = value.get("apiVersion").and_then(|v| v.as_str());
            let kind = value.get("kind").and_then(|v| v.as_str());

            if api_version.is_none() || kind.is_none() {
                continue;
            }
            if api_version != Some(API_VERSION) {
                continue;
            }

            let envelope: ResourceEnvelope =
                serde_yaml_ng::from_value(value).map_err(|e| MirrorError::Yaml {
                    path: path.clone(),
                    source: e,
                })?;

            if let Err(reason) = crate::resource::validate_meta(&envelope.metadata) {
                return Err(MirrorError::InvalidResource {
                    path: path.clone(),
                    reason,
                });
            }

            mirror.upsert(envelope);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::ObjectMeta;
    use std::collections::BTreeMap;

    fn sample(
        namespace: &str,
        kind: &str,
        name: &str,
        labels: &[(&str, &str)],
    ) -> ResourceEnvelope {
        let label_map: BTreeMap<String, String> = labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: kind.to_string(),
            metadata: ObjectMeta {
                name: name.to_string(),
                namespace: Some(namespace.to_string()),
                labels: label_map,
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        }
    }

    #[test]
    fn replace_all_replaces_resources() {
        let mirror1 = Mirror::new();
        mirror1.upsert(sample("ns1", "Endpoint", "old-a", &[]));
        assert_eq!(mirror1.len(), 1);

        let mirror2 = Mirror::new();
        mirror2.upsert(sample("ns2", "Endpoint", "new-b", &[]));
        mirror2.upsert(sample("ns2", "Endpoint", "new-c", &[]));

        mirror1.replace_all(&mirror2);
        assert_eq!(mirror1.len(), 2);
        assert!(mirror1.get("ns1", "Endpoint", "old-a").is_none());
        assert!(mirror1.get("ns2", "Endpoint", "new-b").is_some());
        assert!(mirror1.get("ns2", "Endpoint", "new-c").is_some());
    }

    #[test]
    fn namespace_and_kind_isolation() {
        let mirror = Mirror::new();
        mirror.upsert(sample("ns1", "Endpoint", "res-a", &[]));
        mirror.upsert(sample("ns2", "Endpoint", "res-a", &[]));
        mirror.upsert(sample("ns1", "ContextSpace", "res-a", &[]));

        assert_eq!(mirror.len(), 3);

        let list_ns1_endpoints = mirror.list("ns1", "Endpoint", &ListOptions::default());
        assert_eq!(list_ns1_endpoints.items.len(), 1);
        assert_eq!(list_ns1_endpoints.items[0].metadata.name, "res-a");
        assert_eq!(
            list_ns1_endpoints.items[0].metadata.namespace.as_deref(),
            Some("ns1")
        );

        let list_ns2_endpoints = mirror.list("ns2", "Endpoint", &ListOptions::default());
        assert_eq!(list_ns2_endpoints.items.len(), 1);
        assert_eq!(
            list_ns2_endpoints.items[0].metadata.namespace.as_deref(),
            Some("ns2")
        );

        let list_ns1_spaces = mirror.list("ns1", "ContextSpace", &ListOptions::default());
        assert_eq!(list_ns1_spaces.items.len(), 1);
        assert_eq!(list_ns1_spaces.items[0].kind, "ContextSpace");
    }

    #[test]
    fn selector_filtering() {
        let mirror = Mirror::new();
        mirror.upsert(sample(
            "ovzdusie",
            "Endpoint",
            "public-air",
            &[("env", "prod")],
        ));
        mirror.upsert(sample(
            "ovzdusie",
            "Endpoint",
            "test-air",
            &[("env", "dev")],
        ));

        let opts_label = ListOptions {
            label_selector: Some(LabelSelector::parse("env=prod").expect("selector")),
            ..Default::default()
        };
        let page = mirror.list("ovzdusie", "Endpoint", &opts_label);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].metadata.name, "public-air");

        let opts_field = ListOptions {
            field_selector: Some(FieldSelector::parse("metadata.name=test-air").expect("selector")),
            ..Default::default()
        };
        let page_field = mirror.list("ovzdusie", "Endpoint", &opts_field);
        assert_eq!(page_field.items.len(), 1);
        assert_eq!(page_field.items[0].metadata.name, "test-air");
    }

    #[test]
    fn two_page_walk_returns_every_item_exactly_once() {
        let mirror = Mirror::new();
        for name in &["item-1", "item-2", "item-3", "item-4", "item-5"] {
            mirror.upsert(sample("ovzdusie", "Endpoint", name, &[]));
        }

        let page1 = mirror.list(
            "ovzdusie",
            "Endpoint",
            &ListOptions {
                limit: Some(3),
                continue_token: None,
                ..Default::default()
            },
        );
        assert_eq!(page1.items.len(), 3);
        assert_eq!(page1.remaining, 2);
        assert_eq!(page1.items[0].metadata.name, "item-1");
        assert_eq!(page1.items[1].metadata.name, "item-2");
        assert_eq!(page1.items[2].metadata.name, "item-3");
        let token = page1.continue_token.expect("continue token");

        let page2 = mirror.list(
            "ovzdusie",
            "Endpoint",
            &ListOptions {
                limit: Some(3),
                continue_token: Some(token),
                ..Default::default()
            },
        );
        assert_eq!(page2.items.len(), 2);
        assert_eq!(page2.remaining, 0);
        assert_eq!(page2.items[0].metadata.name, "item-4");
        assert_eq!(page2.items[1].metadata.name, "item-5");
        assert!(page2.continue_token.is_none());
    }

    #[test]
    fn load_dir_on_temporary_tree() {
        let temp_dir = std::env::temp_dir().join(format!(
            "jc_mirror_test_{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        std::fs::create_dir_all(temp_dir.join("sub")).expect("create test dirs");

        let valid1 = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: public-air
  namespace: ovzdusie
spec:
  audience: public
"#;
        std::fs::write(temp_dir.join("endpoint.yaml"), valid1).expect("write valid1");

        let valid2 = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: ovzdusie
  namespace: ovzdusie
spec: {}
"#;
        std::fs::write(temp_dir.join("sub/space.yaml"), valid2).expect("write valid2");

        let native_bento = r#"
input:
  mqtt:
    urls: [ "mosquitto:1883" ]
"#;
        std::fs::write(temp_dir.join("bento.yaml"), native_bento).expect("write bento");

        let foreign_api = r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: ignored
"#;
        std::fs::write(temp_dir.join("k8s.yaml"), foreign_api).expect("write k8s");

        let loaded = Mirror::load_dir(&temp_dir).expect("load_dir");
        assert_eq!(loaded.len(), 2);
        assert!(loaded.get("ovzdusie", "Endpoint", "public-air").is_some());
        assert!(loaded.get("ovzdusie", "ContextSpace", "ovzdusie").is_some());

        std::fs::remove_dir_all(&temp_dir).expect("clean temp dir");
    }
}
