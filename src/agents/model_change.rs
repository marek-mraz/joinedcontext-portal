//! `change_resource` on a DataModel (AG-77, DM-13): the model editor's operations applied to the
//! model's LinkML source as data, so the platform checks what the editor will do before its page
//! opens. The page applies the same operations to the source's text, which keeps its comments and
//! order; the rules here are the editor's (`ui/src/pages/models/operations.ts`).

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// One operation of the model editor the conversation may send.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Operation {
    AddClass {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// The parent class; `Entity` makes it an NGSI-LD entity type (DM-09).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_a: Option<String>,
    },
    RemoveClass {
        name: String,
    },
    RenameClass {
        name: String,
        to: String,
    },
    AddSlot {
        name: String,
        /// The class the slot is attached to; a slot without one is declared and left loose.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#class: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
    RemoveSlot {
        name: String,
    },
    RenameSlot {
        name: String,
        to: String,
    },
    AttachSlot {
        r#class: String,
        slot: String,
    },
    DetachSlot {
        r#class: String,
        slot: String,
    },
    SetSlot {
        name: String,
        field: String,
        value: Value,
    },
    /// The class a class specialises, as LinkML `is_a`; an empty value removes it (DM-13).
    SetClassParent {
        name: String,
        parent: String,
    },
    /// The classes a class mixes in; an empty list removes the key.
    SetClassMixins {
        name: String,
        mixins: Vec<String>,
    },
    /// The profiles a slot belongs to; an empty list removes the key.
    SetSlotSubsets {
        name: String,
        subsets: Vec<String>,
    },
}

/// LinkML element names: what the generators accept as a class or slot name.
static NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new("^[A-Za-z_][A-Za-z0-9_]*$").expect("a literal pattern"));

/// The ranges a slot takes besides the model's own classes and enums (`RANGES` in `linkml.ts`).
const RANGES: [&str; 10] = [
    "string",
    "integer",
    "float",
    "double",
    "decimal",
    "boolean",
    "date",
    "datetime",
    "uri",
    "uriorcurie",
];

/// The NGSI-LD kinds a slot declares (DM-05); a Property stays unwritten.
const KINDS: [&str; 7] = [
    "Property",
    "GeoProperty",
    "Relationship",
    "LanguageProperty",
    "ListProperty",
    "JsonProperty",
    "VocabProperty",
];

/// The slot fields a conversation sets; units, IRIs and bounds are set in the editor.
const SET_FIELDS: [&str; 4] = ["range", "required", "multivalued", "description"];

/// The model with the operations applied in order, all or nothing. A refusal names the operation
/// by its index, so the model corrects that one.
pub fn apply(model: &Value, operations: &[Operation]) -> Result<Value, String> {
    let mut changed = model.clone();
    for (index, operation) in operations.iter().enumerate() {
        mutate(&mut changed, operation).map_err(|reason| format!("operation {index}: {reason}"))?;
    }
    Ok(changed)
}

/// The model's classes with their slots, one line each, for the model to name real ones.
pub fn outline(model: &Value) -> String {
    let lines: Vec<String> = section(model, "classes")
        .map(|classes| {
            classes
                .iter()
                .map(|(name, class)| format!("{name}: {}", slots_of(class).join(", ")))
                .collect()
        })
        .unwrap_or_default();
    if lines.is_empty() {
        "the model has no class".to_owned()
    } else {
        lines.join("; ")
    }
}

fn section<'a>(model: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    model.get(key).and_then(Value::as_object)
}

/// The model's own prefix for a term added without an IRI (DM-04, T-0893): the prefix named
/// after the model, `{name}: {id}/`, declared on first use; a model without a name or an id
/// has none, and the term is left for the check to name.
fn own_prefix(model: &mut Value) -> Option<String> {
    let name = model.get("name")?.as_str()?.trim().to_owned();
    let id = model
        .get("id")?
        .as_str()?
        .trim()
        .trim_end_matches('/')
        .to_owned();
    if name.is_empty() || id.is_empty() {
        return None;
    }
    let prefixes = section_mut(model, "prefixes");
    if !prefixes.contains_key(&name) {
        prefixes.insert(name.clone(), json!(format!("{id}/")));
    }
    Some(name)
}

fn section_mut<'a>(model: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !model.is_object() {
        *model = json!({});
    }
    let entry = model
        .as_object_mut()
        .expect("an object")
        .entry(key)
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().expect("an object")
}

fn has(model: &Value, key: &str, name: &str) -> bool {
    section(model, key).is_some_and(|entries| entries.contains_key(name))
}

fn slots_of(class: &Value) -> Vec<String> {
    class
        .get("slots")
        .and_then(Value::as_array)
        .map(|slots| {
            slots
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn set_slots(class: &mut Value, slots: Vec<String>) {
    if let Some(fields) = class.as_object_mut() {
        fields.insert("slots".to_owned(), json!(slots));
    }
}

fn valid_name(name: &str, what: &str) -> Result<(), String> {
    if NAME.is_match(name) {
        Ok(())
    } else {
        Err(format!("'{name}' is not a valid {what} name"))
    }
}

fn existing(model: &Value, key: &str, name: &str, what: &str) -> Result<(), String> {
    if has(model, key, name) {
        Ok(())
    } else {
        Err(format!("unknown {what} '{name}'"))
    }
}

fn new_name(model: &Value, key: &str, name: &str, what: &str) -> Result<(), String> {
    valid_name(name, what)?;
    if has(model, key, name) {
        Err(format!("{what} '{name}' already exists"))
    } else {
        Ok(())
    }
}

fn mutate(model: &mut Value, operation: &Operation) -> Result<(), String> {
    match operation {
        Operation::AddClass {
            name,
            class_uri,
            description,
            is_a,
        } => {
            new_name(model, "classes", name, "class")?;
            let mut class = Map::new();
            let minted = class_uri
                .clone()
                .or_else(|| own_prefix(model).map(|prefix| format!("{prefix}:{name}")));
            for (field, value) in [
                ("class_uri", &minted),
                ("description", description),
                ("is_a", is_a),
            ] {
                if let Some(value) = value {
                    class.insert(field.to_owned(), json!(value));
                }
            }
            class.insert("slots".to_owned(), json!([]));
            section_mut(model, "classes").insert(name.clone(), Value::Object(class));
        }
        Operation::RemoveClass { name } => {
            existing(model, "classes", name, "class")?;
            section_mut(model, "classes").remove(name);
        }
        Operation::RenameClass { name, to } => {
            existing(model, "classes", name, "class")?;
            new_name(model, "classes", to, "class")?;
            let classes = section_mut(model, "classes");
            let class = classes.remove(name).unwrap_or(Value::Null);
            classes.insert(to.clone(), class);
            for class in classes.values_mut() {
                if class.get("is_a").and_then(Value::as_str) == Some(name) {
                    class["is_a"] = json!(to);
                }
            }
            for slot in section_mut(model, "slots").values_mut() {
                if slot.get("range").and_then(Value::as_str) == Some(name) {
                    slot["range"] = json!(to);
                }
            }
        }
        Operation::AddSlot {
            name,
            r#class,
            range,
            slot_uri,
            kind,
        } => {
            new_name(model, "slots", name, "slot")?;
            if let Some(owner) = r#class {
                existing(model, "classes", owner, "class")?;
            }
            let mut slot = json!({ "range": range.as_deref().unwrap_or("string") });
            if let Some(uri) = slot_uri {
                slot["slot_uri"] = json!(uri.trim());
            } else if let Some(prefix) = own_prefix(model) {
                slot["slot_uri"] = json!(format!("{prefix}:{name}"));
            }
            if let Some(kind) = kind.as_deref().map(str::trim) {
                if !KINDS.contains(&kind) {
                    return Err(format!(
                        "kind '{kind}' of slot '{name}' is not one of {}",
                        KINDS.join(", ")
                    ));
                }
                if kind != "Property" {
                    slot["annotations"] = json!({ "ngsi_ld_kind": kind });
                }
            }
            section_mut(model, "slots").insert(name.clone(), slot);
            if let Some(owner) = r#class {
                let class = section_mut(model, "classes")
                    .get_mut(owner)
                    .expect("checked above");
                let mut slots = slots_of(class);
                slots.push(name.clone());
                set_slots(class, slots);
            }
        }
        Operation::RemoveSlot { name } => {
            existing(model, "slots", name, "slot")?;
            section_mut(model, "slots").remove(name);
            for class in section_mut(model, "classes").values_mut() {
                let slots = slots_of(class);
                if slots.contains(name) {
                    set_slots(class, slots.into_iter().filter(|s| s != name).collect());
                }
            }
        }
        Operation::RenameSlot { name, to } => {
            existing(model, "slots", name, "slot")?;
            new_name(model, "slots", to, "slot")?;
            let slots = section_mut(model, "slots");
            let slot = slots.remove(name).unwrap_or(Value::Null);
            slots.insert(to.clone(), slot);
            for class in section_mut(model, "classes").values_mut() {
                let slots = slots_of(class);
                if slots.contains(name) {
                    let renamed = slots
                        .into_iter()
                        .map(|s| if &s == name { to.clone() } else { s })
                        .collect();
                    set_slots(class, renamed);
                }
            }
        }
        Operation::AttachSlot { r#class, slot } => {
            existing(model, "classes", r#class, "class")?;
            existing(model, "slots", slot, "slot")?;
            let owner = section_mut(model, "classes")
                .get_mut(r#class)
                .expect("checked above");
            let mut slots = slots_of(owner);
            if !slots.contains(slot) {
                slots.push(slot.clone());
                set_slots(owner, slots);
            }
        }
        Operation::DetachSlot { r#class, slot } => {
            existing(model, "classes", r#class, "class")?;
            let owner = section_mut(model, "classes")
                .get_mut(r#class)
                .expect("checked above");
            let slots = slots_of(owner);
            if !slots.contains(slot) {
                return Err(format!("class '{class}' does not use slot '{slot}'"));
            }
            set_slots(owner, slots.into_iter().filter(|s| s != slot).collect());
        }
        Operation::SetClassParent { name, parent } => {
            existing(model, "classes", name, "class")?;
            let parent = parent.trim();
            if !parent.is_empty() {
                if parent == name {
                    return Err(format!("class '{name}' cannot specialise itself"));
                }
                existing(model, "classes", parent, "class")?;
            }
            set_class_field(
                model,
                name,
                "is_a",
                (!parent.is_empty()).then(|| json!(parent)),
            );
        }
        Operation::SetClassMixins { name, mixins } => {
            existing(model, "classes", name, "class")?;
            let named = named_list(mixins);
            for mixin in &named {
                if mixin == name {
                    return Err(format!("class '{name}' cannot mix itself in"));
                }
                existing(model, "classes", mixin, "class")?;
            }
            set_class_field(
                model,
                name,
                "mixins",
                (!named.is_empty()).then(|| json!(named)),
            );
        }
        Operation::SetSlotSubsets { name, subsets } => {
            existing(model, "slots", name, "slot")?;
            let named = named_list(subsets);
            for subset in &named {
                if !NAME.is_match(subset) {
                    return Err(format!("'{subset}' is not a valid subset name"));
                }
            }
            let slot = section_mut(model, "slots")
                .get_mut(name)
                .expect("checked above");
            if !slot.is_object() {
                *slot = json!({});
            }
            let fields = slot.as_object_mut().expect("an object");
            match named.is_empty() {
                true => fields.remove("subsets"),
                false => fields.insert("subsets".to_owned(), json!(named)),
            };
        }
        Operation::SetSlot { name, field, value } => {
            existing(model, "slots", name, "slot")?;
            let value = set_value(model, name, field, value)?;
            let slot = section_mut(model, "slots")
                .get_mut(name)
                .expect("checked above");
            if !slot.is_object() {
                *slot = json!({});
            }
            let fields = slot.as_object_mut().expect("an object");
            match value {
                Some(value) => fields.insert(field.clone(), value),
                None => fields.remove(field),
            };
        }
    }
    Ok(())
}

/// The value a `setSlot` writes, `None` when it clears the field, as the editor's `setOrDelete`.
/// The names of a list as the metamodel writes them: trimmed, and the empty ones dropped.
fn named_list(names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|one| one.trim().to_owned())
        .filter(|one| !one.is_empty())
        .collect()
}

/// Sets one field of a class, or removes it when there is nothing to set.
fn set_class_field(model: &mut Value, name: &str, field: &str, value: Option<Value>) {
    let class = section_mut(model, "classes")
        .get_mut(name)
        .expect("checked by the caller");
    if !class.is_object() {
        *class = json!({});
    }
    let fields = class.as_object_mut().expect("an object");
    match value {
        Some(value) => fields.insert(field.to_owned(), value),
        None => fields.remove(field),
    };
}

fn set_value(
    model: &Value,
    name: &str,
    field: &str,
    value: &Value,
) -> Result<Option<Value>, String> {
    if !SET_FIELDS.contains(&field) {
        return Err(format!(
            "'{field}' of slot '{name}' is set in the model editor; a conversation sets {}",
            SET_FIELDS.join(", ")
        ));
    }
    match (field, value) {
        (_, Value::Null) | ("required" | "multivalued", Value::Bool(false)) => Ok(None),
        ("required" | "multivalued", Value::Bool(_)) => Ok(Some(value.clone())),
        ("required" | "multivalued", _) => {
            Err(format!("'{field}' of slot '{name}' takes true or false"))
        }
        (_, Value::String(text)) if text.trim().is_empty() => Ok(None),
        ("description", Value::String(_)) => Ok(Some(value.clone())),
        ("range", Value::String(range)) => {
            let range = range.trim();
            if RANGES.contains(&range) || has(model, "classes", range) || has(model, "enums", range)
            {
                Ok(Some(json!(range)))
            } else {
                Err(format!(
                    "range '{range}' of slot '{name}' is neither a type, an enum nor a class of this model"
                ))
            }
        }
        _ => Err(format!("'{field}' of slot '{name}' takes text")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bikes() -> Value {
        serde_yaml_ng::from_str(
            "classes:\n  Entity:\n    abstract: true\n  BikeHireDockingStation:\n    is_a: Entity\n    slots: [name, availableBikeNumber, status]\n  Vehicle:\n    is_a: Entity\n    slots: [name, station]\nslots:\n  name:\n    range: string\n  availableBikeNumber:\n    range: integer\n  status:\n    range: string\n  station:\n    range: BikeHireDockingStation\n",
        )
        .expect("yaml")
    }

    fn ops(json: Value) -> Vec<Operation> {
        serde_json::from_value(json).expect("operations")
    }

    #[test]
    fn a_term_added_without_an_iri_is_minted_under_the_models_own_prefix() {
        // The helsinki model on dev: its default prefix is an imported vocabulary, so a term
        // left without an IRI would be minted there and refused (DM-04, DM-16, T-0893).
        let mut model = bikes();
        model["name"] = json!("helsinki");
        model["id"] = json!("https://hel.fi/models/helsinki/helsinki");
        model["default_prefix"] = json!("sdm");
        model["prefixes"] = json!({ "sdm": "https://smartdatamodels.org/" });
        let changed = apply(
            &model,
            &ops(json!([
                { "op": "addSlot", "name": "bikeType", "class": "BikeHireDockingStation", "range": "string" },
                { "op": "addSlot", "name": "colour", "slot_uri": "schema:color" },
                { "op": "addClass", "name": "Dock", "is_a": "Entity" }
            ])),
        )
        .expect("applied");
        assert_eq!(
            changed["slots"]["bikeType"]["slot_uri"],
            json!("helsinki:bikeType")
        );
        assert_eq!(
            changed["slots"]["colour"]["slot_uri"],
            json!("schema:color")
        );
        assert_eq!(
            changed["classes"]["Dock"]["class_uri"],
            json!("helsinki:Dock")
        );
        assert_eq!(
            changed["prefixes"]["helsinki"],
            json!("https://hel.fi/models/helsinki/helsinki/")
        );
        assert_eq!(
            changed["prefixes"]["sdm"],
            json!("https://smartdatamodels.org/")
        );
        assert_eq!(changed["default_prefix"], json!("sdm"));

        // A prefix the model already declares under its name is kept as it is.
        model["prefixes"]["helsinki"] = json!("https://hel.fi/ns/");
        let changed =
            apply(&model, &ops(json!([{ "op": "addSlot", "name": "x" }]))).expect("applied");
        assert_eq!(changed["prefixes"]["helsinki"], json!("https://hel.fi/ns/"));
        assert_eq!(changed["slots"]["x"]["slot_uri"], json!("helsinki:x"));
    }

    #[test]
    fn an_attribute_added_to_a_class_is_declared_and_listed_on_it() {
        let changed = apply(
            &bikes(),
            &ops(json!([{ "op": "addSlot", "name": "bikeType", "class": "BikeHireDockingStation", "range": "string" }])),
        )
        .expect("applied");
        assert_eq!(changed["slots"]["bikeType"], json!({ "range": "string" }));
        assert_eq!(
            changed["classes"]["BikeHireDockingStation"]["slots"],
            json!(["name", "availableBikeNumber", "status", "bikeType"])
        );
    }

    #[test]
    fn a_renamed_attribute_keeps_its_place_in_every_class_and_its_definition() {
        let changed = apply(
            &bikes(),
            &ops(json!([{ "op": "renameSlot", "name": "name", "to": "title" }])),
        )
        .expect("applied");
        assert!(changed["slots"].get("name").is_none());
        assert_eq!(changed["slots"]["title"], json!({ "range": "string" }));
        assert_eq!(
            changed["classes"]["BikeHireDockingStation"]["slots"],
            json!(["title", "availableBikeNumber", "status"])
        );
        assert_eq!(
            changed["classes"]["Vehicle"]["slots"],
            json!(["title", "station"])
        );
    }

    #[test]
    fn a_removed_attribute_leaves_every_class_and_a_renamed_class_every_reference() {
        let changed = apply(
            &bikes(),
            &ops(json!([
                { "op": "removeSlot", "name": "status" },
                { "op": "renameClass", "name": "BikeHireDockingStation", "to": "BikeStation" }
            ])),
        )
        .expect("applied");
        assert!(changed["slots"].get("status").is_none());
        assert!(changed["classes"].get("BikeHireDockingStation").is_none());
        assert_eq!(
            changed["classes"]["BikeStation"]["slots"],
            json!(["name", "availableBikeNumber"])
        );
        assert_eq!(changed["slots"]["station"]["range"], "BikeStation");
    }

    #[test]
    fn a_class_added_removed_or_detached_changes_only_what_it_names() {
        let changed = apply(
            &bikes(),
            &ops(json!([
                { "op": "addClass", "name": "Dock", "is_a": "Entity" },
                { "op": "attachSlot", "class": "Dock", "slot": "status" },
                { "op": "detachSlot", "class": "Vehicle", "slot": "station" },
                { "op": "setSlot", "name": "status", "field": "required", "value": true },
                { "op": "removeClass", "name": "Vehicle" }
            ])),
        )
        .expect("applied");
        assert_eq!(
            changed["classes"]["Dock"],
            json!({ "is_a": "Entity", "slots": ["status"] })
        );
        assert!(changed["classes"].get("Vehicle").is_none());
        assert_eq!(changed["slots"]["status"]["required"], true);
        assert_eq!(
            changed["slots"]["station"]["range"],
            "BikeHireDockingStation"
        );
    }

    #[test]
    fn an_unknown_class_or_slot_a_taken_name_or_a_wrong_value_is_refused_by_index() {
        let model = bikes();
        for (operations, reason) in [
            (
                json!([{ "op": "addSlot", "name": "bikeType", "class": "BikeStation" }]),
                "operation 0: unknown class 'BikeStation'",
            ),
            (
                json!([{ "op": "addSlot", "name": "x" }, { "op": "removeSlot", "name": "bikeType" }]),
                "operation 1: unknown slot 'bikeType'",
            ),
            (
                json!([{ "op": "renameSlot", "name": "status", "to": "name" }]),
                "operation 0: slot 'name' already exists",
            ),
            (
                json!([{ "op": "addClass", "name": "bike station" }]),
                "operation 0: 'bike station' is not a valid class name",
            ),
            (
                json!([{ "op": "setSlot", "name": "status", "field": "range", "value": "text" }]),
                "operation 0: range 'text' of slot 'status' is neither a type, an enum nor a class of this model",
            ),
            (
                json!([{ "op": "setSlot", "name": "status", "field": "unit", "value": "GQ" }]),
                "operation 0: 'unit' of slot 'status' is set in the model editor; a conversation sets range, required, multivalued, description",
            ),
            (
                json!([{ "op": "detachSlot", "class": "Vehicle", "slot": "status" }]),
                "operation 0: class 'Vehicle' does not use slot 'status'",
            ),
        ] {
            assert_eq!(apply(&model, &ops(operations)), Err(reason.to_owned()));
        }
    }

    #[test]
    fn an_operation_the_editor_does_not_know_or_a_field_it_does_not_take_does_not_parse() {
        assert!(
            serde_json::from_value::<Operation>(json!({ "op": "dropTable", "name": "x" })).is_err()
        );
        assert!(serde_json::from_value::<Operation>(
            json!({ "op": "removeSlot", "name": "x", "cascade": true })
        )
        .is_err());
    }

    #[test]
    fn the_outline_names_each_class_with_its_slots() {
        assert_eq!(
            outline(&bikes()),
            "BikeHireDockingStation: name, availableBikeNumber, status; Entity: ; Vehicle: name, station"
        );
        assert_eq!(outline(&json!({})), "the model has no class");
    }

    /// T-1085, DM-13: the assistant edits the hierarchy the model declares, not only its slots.
    /// A class that specialises nothing this model has, or itself, is refused rather than
    /// written, so the model never names a parent the generators cannot resolve.
    #[test]
    fn a_class_hierarchy_is_set_and_cleared_through_the_operations() {
        let changed = apply(
            &bikes(),
            &ops(json!([
                { "op": "setClassParent", "name": "Vehicle", "parent": "Entity" },
                { "op": "setClassMixins", "name": "Vehicle", "mixins": ["Entity"] },
                { "op": "setSlotSubsets", "name": "name", "subsets": ["public", "steward"] }
            ])),
        )
        .expect("the hierarchy lands");
        assert_eq!(changed["classes"]["Vehicle"]["is_a"], json!("Entity"));
        assert_eq!(changed["classes"]["Vehicle"]["mixins"], json!(["Entity"]));
        assert_eq!(
            changed["slots"]["name"]["subsets"],
            json!(["public", "steward"])
        );

        // Emptied, the keys go rather than staying behind as null or an empty list.
        let cleared = apply(
            &changed,
            &ops(json!([
                { "op": "setClassParent", "name": "Vehicle", "parent": "  " },
                { "op": "setClassMixins", "name": "Vehicle", "mixins": [] },
                { "op": "setSlotSubsets", "name": "name", "subsets": [] }
            ])),
        )
        .expect("the hierarchy goes");
        assert!(cleared["classes"]["Vehicle"].get("is_a").is_none());
        assert!(cleared["classes"]["Vehicle"].get("mixins").is_none());
        assert!(cleared["slots"]["name"].get("subsets").is_none());
    }

    #[test]
    fn a_hierarchy_that_names_nothing_or_names_itself_is_refused() {
        for operation in [
            json!({ "op": "setClassParent", "name": "Vehicle", "parent": "Nowhere" }),
            json!({ "op": "setClassParent", "name": "Vehicle", "parent": "Vehicle" }),
            json!({ "op": "setClassMixins", "name": "Vehicle", "mixins": ["Vehicle"] }),
            json!({ "op": "setClassMixins", "name": "Vehicle", "mixins": ["Nowhere"] }),
            json!({ "op": "setClassParent", "name": "Nowhere", "parent": "Entity" }),
            json!({ "op": "setSlotSubsets", "name": "name", "subsets": ["not a name"] }),
        ] {
            let refused = apply(&bikes(), &ops(json!([operation.clone()])));
            assert!(refused.is_err(), "{operation} was accepted");
        }
    }
}
