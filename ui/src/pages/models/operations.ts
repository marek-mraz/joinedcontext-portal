import { isMap, isScalar } from "yaml";
import type { Document } from "yaml";
import {
  DEFAULT_KIND,
  EMPTY_MODEL,
  NGSI_LD_KINDS,
  RANGES,
  UNIT_CODES,
  edit,
  parseModel,
  reservedNamespace,
  setOrDelete,
} from "./linkml";
import type { LinkmlModel, NgsiLdKind } from "./linkml";

/**
 * The operations the editor can perform on a model, as plain data (DM-13, DM-31, DM-32).
 *
 * The structured view, a test and an assistant all edit the document through this one set: a
 * person clicks and the view emits an operation; an assistant answers with a list of them and
 * the Portal applies it the same way. Every operation is refused by name when it points at a
 * class, slot or enum the model does not have, so a typo can never quietly edit nothing, and a
 * slot IRI under a reserved namespace is refused here rather than only flagged later (DM-16).
 */
export type Operation =
  | {
      op: "addClass";
      name: string;
      class_uri?: string;
      description?: string;
      /** The parent class; `Entity` makes it an NGSI-LD entity type (DM-09). */
      is_a?: string;
    }
  | { op: "removeClass"; name: string }
  /** Renames the class in every `is_a` and every slot `range` that names it. */
  | { op: "renameClass"; name: string; to: string }
  | { op: "setClass"; name: string; field: "class_uri" | "description" | "is_a"; value: string }
  /** The classes a class mixes in; an empty list removes the key (DM-13). */
  | { op: "setClassMixins"; name: string; mixins: string[] }
  /** The profiles a slot belongs to; an empty list removes the key. */
  | { op: "setSlotSubsets"; name: string; subsets: string[] }
  | {
      op: "addSlot";
      name: string;
      /** The class the slot is attached to; a slot without one is declared and left loose. */
      class?: string;
      range?: string;
      slot_uri?: string;
      kind?: NgsiLdKind;
    }
  | { op: "removeSlot"; name: string }
  /** Renames the slot in every class that lists it, keeping its place there. */
  | { op: "renameSlot"; name: string; to: string }
  | { op: "attachSlot"; class: string; slot: string }
  | { op: "detachSlot"; class: string; slot: string }
  | { op: "setSlot"; name: string; field: SlotField; value: unknown }
  | { op: "setTitle"; target: "class" | "slot"; name: string; locale: string; value: string }
  | { op: "addEnum"; name: string }
  | { op: "addEnumValue"; enum: string; value: string; description?: string };

/** `unit` takes a UN/CEFACT common code (DM-06); `kind` an NGSI-LD kind; the rest their YAML value. */
export type SlotField =
  | "range"
  | "required"
  | "multivalued"
  | "deprecated"
  | "description"
  | "slot_uri"
  | "kind"
  | "unit"
  | "pattern"
  | "minimum_value"
  | "maximum_value";

export interface Refusal {
  /** Which operation of the list, so an assistant can correct that one. */
  index: number;
  reason: string;
}

export interface Applied {
  /** The new source, or the source that came in when anything was refused. */
  source: string;
  refused: Refusal[];
}

/**
 * LinkML element names: what the generators accept as a class, slot or enum name.
 *
 * ASCII, because a class name becomes the entity type in every URN the space mints
 * (`urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}`) and the gateway accepts a type matching
 * `^[A-Za-z][A-Za-z0-9_-]*$`. A model whose class is "Ovzduší" would mint ids its own endpoint
 * refuses. The human-readable name is the element's `title`, which is a LanguageProperty and
 * says "Ovzdušie" in as many languages as the space needs (T-1091).
 */
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

class Refused extends Error {}

function refuse(reason: string): never {
  throw new Refused(reason);
}

function requireName(name: string, what: string): void {
  if (!NAME.test(name)) {
    // The reason, not only the refusal: a person typing an accented name is told where the
    // accented name belongs rather than left guessing which character offended (T-1091).
    refuse(
      `'${name}' is not a valid ${what} name: letters, digits and _ only, starting with a letter, ` +
        `because the name becomes the entity type in every URN. Put the readable name in its title.`,
    );
  }
}

function classOf(model: LinkmlModel, name: string) {
  return model.classes.find((klass) => klass.name === name) ?? refuse(`unknown class '${name}'`);
}

function slotOf(model: LinkmlModel, name: string) {
  return model.slots.find((slot) => slot.name === name) ?? refuse(`unknown slot '${name}'`);
}

function enumOf(model: LinkmlModel, name: string) {
  return model.enums.find((entry) => entry.name === name) ?? refuse(`unknown enum '${name}'`);
}

/** The fields whose value the document takes as it is, once it has the right type. */
const BOOLEAN_FIELDS: SlotField[] = ["required", "multivalued", "deprecated"];
const TEXT_FIELDS: SlotField[] = ["description", "pattern"];
const NUMBER_FIELDS: SlotField[] = ["minimum_value", "maximum_value"];

function setSlotField(
  document: Document,
  model: LinkmlModel,
  name: string,
  field: SlotField,
  value: unknown,
): void {
  const slot = slotOf(model, name);
  const path = ["slots", name];
  if (BOOLEAN_FIELDS.includes(field)) {
    if (typeof value !== "boolean") {
      refuse(`'${field}' of slot '${name}' takes true or false`);
    }
    setOrDelete(document, [...path, field], value);
    return;
  }
  if (TEXT_FIELDS.includes(field)) {
    if (typeof value !== "string") {
      refuse(`'${field}' of slot '${name}' takes text`);
    }
    setOrDelete(document, [...path, field], value);
    return;
  }
  if (NUMBER_FIELDS.includes(field)) {
    if (value !== undefined && value !== null && typeof value !== "number") {
      refuse(`'${field}' of slot '${name}' takes a number`);
    }
    setOrDelete(document, [...path, field], value ?? undefined);
    return;
  }
  if (typeof value !== "string") {
    refuse(`'${field}' of slot '${name}' takes text`);
  }
  const trimmed = value.trim();
  switch (field) {
    case "range": {
      const known = [
        ...RANGES,
        ...model.enums.map((entry) => entry.name),
        ...model.classes.map((klass) => klass.name),
      ];
      if (trimmed && !known.includes(trimmed)) {
        refuse(`range '${trimmed}' of slot '${name}' is neither a type, an enum nor a class of this model`);
      }
      setOrDelete(document, [...path, "range"], trimmed);
      return;
    }
    case "slot_uri": {
      const reserved = trimmed ? reservedNamespace(trimmed, model.prefixes) : undefined;
      if (reserved && !slot.upstream) {
        refuse(`slot '${name}' would mint '${trimmed}' under ${reserved}, which belongs to someone else`);
      }
      setOrDelete(document, [...path, "slot_uri"], trimmed);
      return;
    }
    case "kind": {
      if (!(NGSI_LD_KINDS as readonly string[]).includes(trimmed)) {
        refuse(`kind '${trimmed}' of slot '${name}' is not one of ${NGSI_LD_KINDS.join(", ")}`);
      }
      setOrDelete(
        document,
        [...path, "annotations", "ngsi_ld_kind"],
        trimmed === DEFAULT_KIND ? undefined : trimmed,
      );
      return;
    }
    case "unit": {
      if (!trimmed) {
        document.deleteIn([...path, "unit"]);
        return;
      }
      const unit = UNIT_CODES.find((entry) => entry.code === trimmed);
      if (!unit) {
        refuse(`unit '${trimmed}' of slot '${name}' is not a known UN/CEFACT common code`);
      }
      // The wire value and the anchor together (DM-06, DM-59): NGSI-LD puts the UN/CEFACT code
      // on the wire as `unitCode`, and the QUDT unit is what a federated reader dereferences
      // to align this measurement with somebody else's. The quantity kind says what dimension
      // is being measured, which is what makes two units comparable at all.
      document.setIn([...path, "unit"], {
        ucum_code: unit.ucum,
        exact_mappings: [`ucefact:${unit.code}`, `qudt-unit:${unit.qudt}`],
        has_quantity_kind: `qudt-quantkind:${unit.quantityKind}`,
      });
      declarePrefixes(document);
      return;
    }
    default:
      refuse(`'${field}' is not a slot field the editor sets`);
  }
}

/**
 * The prefixes a unit mapping cites, declared on the document that carries one (DM-59).
 *
 * A CURIE under an undeclared prefix is a dangling string in every artifact that carries it,
 * and Model Tools refuses to render the model at all — so the editor writes the declaration
 * with the mapping rather than leaving the author to find out at generation time.
 */
function declarePrefixes(document: Document): void {
  for (const [prefix, namespace] of [
    ["ucefact", "https://vocabulary.uncefact.org/UnitMeasureCode#"],
    ["qudt-unit", "http://qudt.org/vocab/unit/"],
    ["qudt-quantkind", "http://qudt.org/vocab/quantitykind/"],
  ]) {
    if (document.getIn(["prefixes", prefix]) === undefined) {
      document.setIn(["prefixes", prefix], namespace);
    }
  }
}

function classSlots(document: Document, name: string): string[] {
  const listed = document.getIn(["classes", name, "slots"], true);
  const values = (listed as { toJSON?: () => unknown } | undefined)?.toJSON?.();
  return Array.isArray(values) ? values.filter((s): s is string => typeof s === "string") : [];
}

/** Renames a key of a top-level section in place, so its value, its comments and its position stay. */
function renameKey(document: Document, section: string, name: string, to: string): void {
  const entries = document.get(section, true);
  const pair = isMap(entries)
    ? entries.items.find((item) => (isScalar(item.key) ? item.key.value : item.key) === name)
    : undefined;
  if (pair && isScalar(pair.key)) {
    pair.key.value = to;
  }
}

function mutate(document: Document, model: LinkmlModel, operation: Operation): void {
  switch (operation.op) {
    case "addClass": {
      requireName(operation.name, "class");
      if (model.classes.some((klass) => klass.name === operation.name)) {
        refuse(`class '${operation.name}' already exists`);
      }
      document.setIn(["classes", operation.name], {
        ...(operation.class_uri ? { class_uri: operation.class_uri } : {}),
        ...(operation.description ? { description: operation.description } : {}),
        ...(operation.is_a ? { is_a: operation.is_a } : {}),
        slots: [],
      });
      return;
    }
    case "removeClass":
      classOf(model, operation.name);
      document.deleteIn(["classes", operation.name]);
      return;
    case "renameClass": {
      classOf(model, operation.name);
      requireName(operation.to, "class");
      if (model.classes.some((klass) => klass.name === operation.to)) {
        refuse(`class '${operation.to}' already exists`);
      }
      renameKey(document, "classes", operation.name, operation.to);
      for (const klass of model.classes) {
        const name = klass.name === operation.name ? operation.to : klass.name;
        if (document.getIn(["classes", name, "is_a"]) === operation.name) {
          document.setIn(["classes", name, "is_a"], operation.to);
        }
      }
      for (const slot of model.slots) {
        if (slot.range === operation.name) {
          document.setIn(["slots", slot.name, "range"], operation.to);
        }
      }
      return;
    }
    case "setClass":
      classOf(model, operation.name);
      // A class can only specialise one this model declares, or the hierarchy names nothing.
      if (operation.field === "is_a" && operation.value.trim() !== "") {
        const parent = operation.value.trim();
        if (parent === operation.name) {
          refuse(`class '${operation.name}' cannot specialise itself`);
        }
        classOf(model, parent);
      }
      // An IRI never has whitespace; a description is typed, and trimming would eat every space.
      setOrDelete(
        document,
        ["classes", operation.name, operation.field],
        operation.field === "description" ? operation.value : operation.value.trim(),
      );
      return;
    case "setClassMixins": {
      classOf(model, operation.name);
      const mixins = operation.mixins.map((one) => one.trim()).filter((one) => one !== "");
      for (const mixin of mixins) {
        if (mixin === operation.name) {
          refuse(`class '${operation.name}' cannot mix itself in`);
        }
        classOf(model, mixin);
      }
      // `setOrDelete` reads "" and undefined as empty, not `[]`, so an emptied list is passed
      // as undefined and the key goes rather than staying behind as `mixins: []`.
      setOrDelete(
        document,
        ["classes", operation.name, "mixins"],
        mixins.length > 0 ? mixins : undefined,
      );
      return;
    }
    case "setSlotSubsets": {
      slotOf(model, operation.name);
      const subsets = operation.subsets.map((one) => one.trim()).filter((one) => one !== "");
      for (const subset of subsets) {
        requireName(subset, "subset");
      }
      setOrDelete(
        document,
        ["slots", operation.name, "subsets"],
        subsets.length > 0 ? subsets : undefined,
      );
      return;
    }
    case "addSlot": {
      requireName(operation.name, "slot");
      if (model.slots.some((slot) => slot.name === operation.name)) {
        refuse(`slot '${operation.name}' already exists`);
      }
      const owner = operation.class === undefined ? undefined : classOf(model, operation.class);
      // A node, not a plain object: the kind and the IRI below are set into it in this same
      // operation, and `setIn` can only descend into a YAML collection.
      document.setIn(
        ["slots", operation.name],
        document.createNode({ range: operation.range ?? "string" }),
      );
      if (owner) {
        document.setIn(
          ["classes", owner.name, "slots"],
          [...classSlots(document, owner.name), operation.name],
        );
      }
      // The IRI and the kind go through the same checks a later edit would.
      const declared = parseModel(document.toString());
      if (operation.slot_uri !== undefined) {
        setSlotField(document, declared, operation.name, "slot_uri", operation.slot_uri);
      }
      if (operation.kind !== undefined) {
        setSlotField(document, declared, operation.name, "kind", operation.kind);
      }
      return;
    }
    case "removeSlot":
      slotOf(model, operation.name);
      document.deleteIn(["slots", operation.name]);
      for (const klass of model.classes) {
        if (klass.slots.includes(operation.name)) {
          document.setIn(
            ["classes", klass.name, "slots"],
            klass.slots.filter((slot) => slot !== operation.name),
          );
        }
      }
      return;
    case "renameSlot": {
      slotOf(model, operation.name);
      requireName(operation.to, "slot");
      if (model.slots.some((slot) => slot.name === operation.to)) {
        refuse(`slot '${operation.to}' already exists`);
      }
      renameKey(document, "slots", operation.name, operation.to);
      for (const klass of model.classes) {
        if (klass.slots.includes(operation.name)) {
          document.setIn(
            ["classes", klass.name, "slots"],
            klass.slots.map((slot) => (slot === operation.name ? operation.to : slot)),
          );
        }
      }
      return;
    }
    case "attachSlot": {
      const owner = classOf(model, operation.class);
      slotOf(model, operation.slot);
      if (!owner.slots.includes(operation.slot)) {
        document.setIn(["classes", owner.name, "slots"], [...owner.slots, operation.slot]);
      }
      return;
    }
    case "detachSlot": {
      const owner = classOf(model, operation.class);
      if (!owner.slots.includes(operation.slot)) {
        refuse(`class '${owner.name}' does not use slot '${operation.slot}'`);
      }
      document.setIn(
        ["classes", owner.name, "slots"],
        owner.slots.filter((slot) => slot !== operation.slot),
      );
      return;
    }
    case "setSlot":
      setSlotField(document, model, operation.name, operation.field, operation.value);
      return;
    case "setTitle": {
      const plural = operation.target === "class" ? "classes" : "slots";
      if (operation.target === "class") {
        classOf(model, operation.name);
      } else {
        slotOf(model, operation.name);
      }
      setOrDelete(document, [plural, operation.name, "title", operation.locale], operation.value);
      return;
    }
    case "addEnum":
      requireName(operation.name, "enum");
      if (model.enums.some((entry) => entry.name === operation.name)) {
        refuse(`enum '${operation.name}' already exists`);
      }
      document.setIn(["enums", operation.name, "permissible_values"], {});
      return;
    case "addEnumValue": {
      const entry = enumOf(model, operation.enum);
      if (entry.permissible_values.some((value) => value.name === operation.value)) {
        refuse(`enum '${entry.name}' already has the value '${operation.value}'`);
      }
      document.setIn(
        ["enums", entry.name, "permissible_values", operation.value],
        operation.description ? { description: operation.description } : {},
      );
      return;
    }
    default:
      refuse(`unknown operation '${(operation as { op: string }).op}'`);
  }
}

/**
 * Applies the operations in order and returns the new source.
 *
 * All or nothing: when any operation is refused the source that came in is returned unchanged
 * together with every refusal, so a caller never has to work out which half of its list landed.
 * Edits go through the YAML document, so comments, key order and formatting survive (DM-13).
 */
export function applyOperations(source: string, operations: Operation[]): Applied {
  const refused: Refusal[] = [];
  let current = source;
  operations.forEach((operation, index) => {
    const model = parseModel(current);
    if (model === EMPTY_MODEL) {
      refused.push({ index, reason: "the source does not parse as YAML" });
      return;
    }
    try {
      current = edit(current, (document) => mutate(document, model, operation));
    } catch (error) {
      refused.push({
        index,
        reason: error instanceof Error ? error.message : String(error),
      });
    }
  });
  return refused.length > 0 ? { source, refused } : { source: current, refused };
}
