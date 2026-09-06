//! Every reference app in the repository is a manifest the reconciler can actually read
//! (T-0310, AP-34, AP-37, AP-39).
//!
//! The app crates deliberately do not depend on `jc-core`: an app has to build and run
//! outside this platform. That leaves nothing checking that `apps/*/app.yaml` still parses
//! after a change to the kind, which is exactly the kind of drift that is found on a cluster
//! instead of in CI. The Portal already owns the reconciler and the contract, so the check
//! lives here.

use jc_core::kinds::App;

/// Reads every `apps/*/app.yaml` beside the Portal, in path order so a failure names the same
/// app on every machine.
fn reference_apps() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("apps");
    let mut found: Vec<_> = std::fs::read_dir(&root)
        .expect("the apps folder exists")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("app.yaml"))
        .filter(|manifest| manifest.is_file())
        .map(|manifest| {
            let name = manifest
                .parent()
                .and_then(|dir| dir.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (
                name,
                std::fs::read_to_string(&manifest).expect("a readable manifest"),
            )
        })
        .collect();
    found.sort();
    found
}

#[test]
fn every_reference_app_manifest_parses_as_the_kind_the_reconciler_reads() {
    let apps = reference_apps();
    assert!(!apps.is_empty(), "no apps/*/app.yaml found");
    for (name, yaml) in apps {
        let app: App = serde_yaml_ng::from_str(&yaml)
            .unwrap_or_else(|error| panic!("apps/{name}/app.yaml does not parse: {error}"));
        assert_eq!(app.metadata.name, name, "the folder names the app");
        assert!(
            !app.spec.data_needs.is_empty(),
            "apps/{name} declares no data needs, so it has nothing to reach (AP-04)"
        );
    }
}

/// AP-35. The annotations are an agent's attribution; hand-written source claiming them would
/// make the provenance trail say something untrue.
#[test]
fn a_hand_written_app_claims_no_agent_attribution() {
    for (name, yaml) in reference_apps() {
        let app: App = serde_yaml_ng::from_str(&yaml).expect("a manifest");
        for annotation in [
            "joinedcontext.com/generated-by",
            "joinedcontext.com/prompt-digest",
        ] {
            assert!(
                !app.metadata.annotations.contains_key(annotation),
                "apps/{name} claims {annotation} but is written by hand"
            );
        }
    }
}

/// AP-39. A write is the one thing that puts an app's publication in the red lane, so the
/// list of apps that can write is worth stating out loud rather than discovering later.
#[test]
fn air_quality_writes_exactly_one_attribute_and_nothing_else_writes_at_all() {
    let writes = [
        "createEntity",
        "updateEntity",
        "updateAttrs",
        "appendAttrs",
        "deleteAttrs",
        "deleteEntity",
        "mergeEntity",
        "replaceEntity",
        "replaceAttrs",
    ];
    for (name, yaml) in reference_apps() {
        let app: App = serde_yaml_ng::from_str(&yaml).expect("a manifest");
        let writing: Vec<_> = app
            .spec
            .data_needs
            .iter()
            .flat_map(|need| need.operations.iter())
            .map(|operation| operation.as_str())
            .filter(|operation| writes.contains(operation))
            .collect();
        match name.as_str() {
            "air-quality" => {
                assert_eq!(
                    writing,
                    vec!["updateAttrs"],
                    "air-quality writes one way only"
                );
                let attrs: Vec<_> = app
                    .spec
                    .data_needs
                    .iter()
                    .flat_map(|need| need.attrs.iter())
                    .collect();
                assert!(
                    attrs.iter().any(|attr| *attr == "stewardNote"),
                    "the note attribute has to be in the grant"
                );
            }
            other => assert!(
                writing.is_empty(),
                "apps/{other} declares writes {writing:?}; add it to this test on purpose"
            ),
        }
    }
}
