/**
 * The LinkML document the editor edits, and the rules it is judged by.
 *
 * The YAML text is the single source of truth: the structured view reads a projection of it
 * and writes back through the YAML document, so a comment or a hand-made ordering survives a
 * click in the tree, and the two views cannot drift (DM-13). Nothing here reaches the network;
 * the authoritative artifacts come from Model Tools (DM-18).
 */
import { parseDocument, type Document } from "yaml";

/** The NGSI-LD kinds a slot may declare (DM-05), in the annotation Model Tools reads. */
export const NGSI_LD_KINDS = [
  "Property",
  "GeoProperty",
  "Relationship",
  "LanguageProperty",
  "ListProperty",
  "JsonProperty",
  "VocabProperty",
] as const;

export type NgsiLdKind = (typeof NGSI_LD_KINDS)[number];

/** A slot without the annotation is a plain Property: the common case stays unwritten. */
export const DEFAULT_KIND: NgsiLdKind = "Property";

/**
 * Namespaces an organisation must never mint its own terms under (DM-04, DM-16). Reusing an
 * upstream IRI is how a model says it means the same thing as a standard; minting a new term
 * under someone else's namespace is squatting. The list is Model Tools' own, so the editor
 * refuses exactly what generation would refuse.
 */
export const RESERVED_NAMESPACES = [
  "https://smartdatamodels.org/",
  "https://raw.githubusercontent.com/smart-data-models/",
  "https://github.com/smart-data-models/",
  "https://uri.etsi.org/",
  "http://uri.etsi.org/",
  "https://www.w3.org/",
  "http://www.w3.org/",
  "https://w3id.org/linkml/",
] as const;

/** A slot carrying this annotation cites an upstream term, so a reserved IRI is a citation. */
export const UPSTREAM_ANNOTATION = "upstream_source";

/**
 * UN/CEFACT common codes for the units a municipal model actually measures in (DM-06).
 *
 * The full CEFACT recommendation 20 is thousands of codes; this is the working set the unit
 * picker offers, and any other code can still be typed. `ucum` is what LinkML's `unit.ucum_code`
 * takes, `code` is the CEFACT common code that travels in `exact_mappings`.
 */
export const UNIT_CODES = [
  { code: "GQ", ucum: "ug/m3", label: "microgram per cubic metre" },
  { code: "M1", ucum: "mg/L", label: "milligram per litre" },
  { code: "CEL", ucum: "Cel", label: "degree Celsius" },
  { code: "P1", ucum: "%", label: "percent" },
  { code: "MTR", ucum: "m", label: "metre" },
  { code: "KMT", ucum: "km", label: "kilometre" },
  { code: "MTS", ucum: "m/s", label: "metre per second" },
  { code: "KMH", ucum: "km/h", label: "kilometre per hour" },
  { code: "SEC", ucum: "s", label: "second" },
  { code: "HUR", ucum: "h", label: "hour" },
  { code: "KGM", ucum: "kg", label: "kilogram" },
  { code: "TNE", ucum: "t", label: "tonne" },
  { code: "LTR", ucum: "L", label: "litre" },
  { code: "MTQ", ucum: "m3", label: "cubic metre" },
  { code: "KWH", ucum: "kW.h", label: "kilowatt hour" },
  { code: "WTT", ucum: "W", label: "watt" },
  { code: "A24", ucum: "cd/m2", label: "candela per square metre" },
  { code: "2N", ucum: "dB", label: "decibel" },
  { code: "HPA", ucum: "hPa", label: "hectopascal" },
  { code: "C62", ucum: "1", label: "one (dimensionless)" },
] as const;

/** The LinkML ranges the editor offers, and how a dashboard may use each of them (DM-20). */
export const RANGES = [
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
] as const;

export interface LinkmlUnit {
  ucum_code?: string;
  symbol?: string;
  exact_mappings?: string[];
}

export interface LinkmlSlot {
  name: string;
  range?: string;
  required?: boolean;
  multivalued?: boolean;
  deprecated?: boolean;
  description?: string;
  title?: Record<string, string>;
  slot_uri?: string;
  unit?: LinkmlUnit;
  kind: NgsiLdKind;
  /** Where an upstream term came from, when this slot cites one (DM-08, DM-16). */
  upstream?: string;
  pattern?: string;
  minimum_value?: number;
  maximum_value?: number;
}

export interface LinkmlClass {
  name: string;
  class_uri?: string;
  description?: string;
  title?: Record<string, string>;
  slots: string[];
}

export interface LinkmlEnumValue {
  name: string;
  meaning?: string;
  description?: string;
}

export interface LinkmlEnum {
  name: string;
  permissible_values: LinkmlEnumValue[];
}

/** The projection the structured view renders. Never the thing that is saved. */
export interface LinkmlModel {
  id?: string;
  name?: string;
  title?: Record<string, string>;
  prefixes: Record<string, string>;
  default_prefix?: string;
  classes: LinkmlClass[];
  slots: LinkmlSlot[];
  enums: LinkmlEnum[];
}

export interface Diagnostic {
  /** 1-based, so it can be shown next to the line the editor numbers (DM-14). */
  line: number;
  column: number;
  severity: "error" | "warning";
  message: string;
  /** What the message is about, so the structured view can point at the same thing. */
  path?: string;
}

export const EMPTY_MODEL: LinkmlModel = {
  prefixes: {},
  classes: [],
  slots: [],
  enums: [],
};

/** A new model, as the editor starts one (DM-13). */
export function blankSource(organizationDomain: string, name: string): string {
  const prefix = organizationDomain.split(".")[0] || "org";
  return [
    `id: https://${organizationDomain}/models/${name}`,
    `name: ${name}`,
    "prefixes:",
    `  ${prefix}: https://${organizationDomain}/terms/`,
    "  linkml: https://w3id.org/linkml/",
    `default_prefix: ${prefix}`,
    "imports:",
    "  - linkml:types",
    "  - ngsi-ld-core",
    "classes: {}",
    "slots: {}",
    "enums: {}",
    "",
  ].join("\n");
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function languageMap(value: unknown): Record<string, string> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const map: Record<string, string> = {};
  for (const [locale, phrase] of Object.entries(value as Record<string, unknown>)) {
    if (typeof phrase === "string") {
      map[locale] = phrase;
    }
  }
  return Object.keys(map).length > 0 ? map : undefined;
}

function annotations(raw: Record<string, unknown>): Record<string, unknown> {
  return record(raw.annotations);
}

function slotOf(name: string, raw: Record<string, unknown>): LinkmlSlot {
  const annotated = annotations(raw);
  const kind = text(annotated.ngsi_ld_kind);
  const unit = record(raw.unit);
  const mappings = Array.isArray(unit.exact_mappings)
    ? (unit.exact_mappings as unknown[]).filter((m): m is string => typeof m === "string")
    : undefined;
  return {
    name,
    range: text(raw.range),
    required: raw.required === true,
    multivalued: raw.multivalued === true,
    deprecated: raw.deprecated === true,
    description: text(raw.description),
    title: languageMap(raw.title),
    slot_uri: text(raw.slot_uri),
    unit:
      Object.keys(unit).length > 0
        ? {
            ucum_code: text(unit.ucum_code),
            symbol: text(unit.symbol),
            ...(mappings && mappings.length > 0 ? { exact_mappings: mappings } : {}),
          }
        : undefined,
    kind: (NGSI_LD_KINDS as readonly string[]).includes(kind ?? "")
      ? (kind as NgsiLdKind)
      : DEFAULT_KIND,
    upstream: text(annotated[UPSTREAM_ANNOTATION]),
    pattern: text(raw.pattern),
    minimum_value: typeof raw.minimum_value === "number" ? raw.minimum_value : undefined,
    maximum_value: typeof raw.maximum_value === "number" ? raw.maximum_value : undefined,
  };
}

/** The structured projection of a source that parses; `EMPTY_MODEL` for one that does not. */
export function parseModel(source: string): LinkmlModel {
  const document = parseDocument(source);
  if (document.errors.length > 0) {
    return EMPTY_MODEL;
  }
  const root = record(document.toJS({ maxAliasCount: 100 }));

  const prefixes: Record<string, string> = {};
  for (const [prefix, target] of Object.entries(record(root.prefixes))) {
    const expansion =
      typeof target === "string" ? target : text(record(target).prefix_reference);
    if (expansion) {
      prefixes[prefix] = expansion;
    }
  }

  const classes: LinkmlClass[] = Object.entries(record(root.classes)).map(([name, value]) => {
    const raw = record(value);
    const slots = Array.isArray(raw.slots)
      ? (raw.slots as unknown[]).filter((s): s is string => typeof s === "string")
      : [];
    return {
      name,
      class_uri: text(raw.class_uri),
      description: text(raw.description),
      title: languageMap(raw.title),
      slots,
    };
  });

  const slots = Object.entries(record(root.slots)).map(([name, value]) =>
    slotOf(name, record(value)),
  );

  const enums: LinkmlEnum[] = Object.entries(record(root.enums)).map(([name, value]) => {
    const raw = record(value);
    const permissible = Object.entries(record(raw.permissible_values)).map(
      ([valueName, entry]) => {
        const details = record(entry);
        return {
          name: valueName,
          meaning: text(details.meaning),
          description: text(details.description),
        };
      },
    );
    return { name, permissible_values: permissible };
  });

  return {
    id: text(root.id),
    name: text(root.name),
    title: languageMap(root.title),
    prefixes,
    default_prefix: text(root.default_prefix),
    classes,
    slots,
    enums,
  };
}

/** A CURIE resolved against the model's own prefixes; anything else is returned unchanged. */
export function expandIri(iri: string, prefixes: Record<string, string>): string {
  const colon = iri.indexOf(":");
  if (colon <= 0 || iri.startsWith("http://") || iri.startsWith("https://")) {
    return iri;
  }
  const expansion = prefixes[iri.slice(0, colon)];
  return expansion ? `${expansion}${iri.slice(colon + 1)}` : iri;
}

/** The reserved namespace an IRI falls under, or `undefined` where it is the organisation's. */
export function reservedNamespace(
  iri: string,
  prefixes: Record<string, string>,
): string | undefined {
  const expanded = expandIri(iri, prefixes);
  return RESERVED_NAMESPACES.find((namespace) => expanded.startsWith(namespace));
}

/**
 * Every problem the editor can see without compiling: the YAML itself, and the metamodel rules
 * a model has to satisfy before it may be published (DM-04, DM-05, DM-06, DM-14, DM-15, DM-16).
 *
 * `locales` are the organisation's configured languages; a title missing one of them is a
 * warning here and a block at publish time (DM-15).
 */
export function diagnose(source: string, locales: string[] = []): Diagnostic[] {
  const document = parseDocument(source);
  if (document.errors.length > 0) {
    return document.errors.map((error) => ({
      line: error.linePos?.[0]?.line ?? 1,
      column: error.linePos?.[0]?.col ?? 1,
      severity: "error" as const,
      message: error.message,
    }));
  }

  const model = parseModel(source);
  const root = record(document.toJS({ maxAliasCount: 100 }));
  const found: Diagnostic[] = [];
  const at = (path: (string | number)[]): Pick<Diagnostic, "line" | "column"> =>
    positionOf(document, path);

  if (!model.id) {
    found.push({ ...at([]), severity: "error", message: "the model needs an `id`", path: "id" });
  }
  if (!model.name) {
    found.push({
      ...at([]),
      severity: "error",
      message: "the model needs a `name`",
      path: "name",
    });
  }
  if (model.classes.length === 0) {
    found.push({
      ...at([]),
      severity: "warning",
      message: "the model declares no class, so it defines no entity type",
      path: "classes",
    });
  }

  const declared = new Set(model.slots.map((slot) => slot.name));
  for (const klass of model.classes) {
    if (!klass.class_uri) {
      found.push({
        ...at(["classes", klass.name]),
        severity: "warning",
        message: `class '${klass.name}' has no class_uri, and a model without complete IRI bindings cannot be published`,
        path: `classes.${klass.name}`,
      });
    }
    for (const slot of klass.slots) {
      if (!declared.has(slot)) {
        found.push({
          ...at(["classes", klass.name, "slots"]),
          severity: "error",
          message: `class '${klass.name}' uses slot '${slot}', which the model does not declare`,
          path: `classes.${klass.name}`,
        });
      }
    }
    found.push(
      ...missingLocales(
        klass.title,
        locales,
        `class '${klass.name}'`,
        at(["classes", klass.name]),
        `classes.${klass.name}`,
      ),
    );
  }

  for (const slot of model.slots) {
    const where = at(["slots", slot.name]);
    if (!slot.slot_uri) {
      found.push({
        ...where,
        severity: "warning",
        message: `slot '${slot.name}' has no slot_uri, and a model without complete IRI bindings cannot be published`,
        path: `slots.${slot.name}`,
      });
    } else {
      const reserved = reservedNamespace(slot.slot_uri, model.prefixes);
      if (reserved && !slot.upstream) {
        found.push({
          ...where,
          severity: "error",
          message: `slot '${slot.name}' mints '${slot.slot_uri}' under ${reserved}, which belongs to someone else; use the organisation's own prefix, or cite the upstream term with an ${UPSTREAM_ANNOTATION} annotation`,
          path: `slots.${slot.name}`,
        });
      }
    }
    if (!slot.range) {
      found.push({
        ...where,
        severity: "warning",
        message: `slot '${slot.name}' has no range, so it falls back to the model's default_range`,
        path: `slots.${slot.name}`,
      });
    }
    if (slot.unit && !slot.unit.ucum_code && !unitCode(slot.unit)) {
      found.push({
        ...where,
        severity: "warning",
        message: `slot '${slot.name}' declares a unit without a UN/CEFACT common code, so exports and dashboards cannot label it`,
        path: `slots.${slot.name}`,
      });
    }
    found.push(
      ...missingLocales(slot.title, locales, `slot '${slot.name}'`, where, `slots.${slot.name}`),
    );
  }

  // The annotation is free text in YAML, so a typo would only surface at generation.
  for (const [name, raw] of Object.entries(record(root.slots))) {
    const kind = text(annotations(record(raw)).ngsi_ld_kind);
    if (kind && !(NGSI_LD_KINDS as readonly string[]).includes(kind)) {
      found.push({
        ...at(["slots", name]),
        severity: "error",
        message: `slot '${name}' declares ngsi_ld_kind '${kind}', which is not one of ${NGSI_LD_KINDS.join(", ")}`,
        path: `slots.${name}`,
      });
    }
  }

  return found;
}

/** The CEFACT common code a unit carries in `exact_mappings`, per DM-06. */
export function unitCode(unit: LinkmlUnit | undefined): string | undefined {
  const mapping = unit?.exact_mappings?.find((entry) => entry.includes(":"));
  return mapping?.split(":").pop() || undefined;
}

function missingLocales(
  title: Record<string, string> | undefined,
  locales: string[],
  what: string,
  where: Pick<Diagnostic, "line" | "column">,
  path: string,
): Diagnostic[] {
  if (locales.length === 0 || !title) {
    return [];
  }
  const missing = locales.filter((locale) => !title[locale]);
  return missing.length === 0
    ? []
    : [
        {
          ...where,
          severity: "warning",
          message: `${what} has no title in ${missing.join(", ")}`,
          path,
        },
      ];
}

/** Where a path sits in the source, so a message can point at the line it is about. */
export function positionOf(
  document: Document.Parsed,
  path: (string | number)[],
): { line: number; column: number } {
  const node = path.length > 0 ? document.getIn(path, true) : undefined;
  const offset =
    node && typeof node === "object" && "range" in node
      ? (node as { range?: [number, number, number] }).range?.[0]
      : undefined;
  if (offset === undefined) {
    return { line: 1, column: 1 };
  }
  const before = document.toString().slice(0, offset);
  const lines = before.split("\n");
  return { line: lines.length, column: lines[lines.length - 1].length + 1 };
}

/**
 * Applies one structured edit to the source and returns the new source.
 *
 * Every visual edit goes through the YAML document rather than through a re-serialised object,
 * so the user's comments, key order and formatting survive an edit made in the tree (DM-13).
 */
export function edit(source: string, mutate: (document: Document) => void): string {
  const document = parseDocument(source);
  if (document.errors.length > 0) {
    return source;
  }
  mutate(document);
  return document.toString();
}

/** Sets a value, or removes the key when the value is empty. */
export function setOrDelete(
  document: Document,
  path: (string | number)[],
  value: unknown,
): void {
  if (value === undefined || value === "" || value === null || value === false) {
    document.deleteIn(path);
    return;
  }
  document.setIn(path, value);
}

/** How a dashboard may use a slot, derived from its range and kind (DM-20). */
export function slotAffordance(slot: LinkmlSlot): "range" | "select" | "temporal" | "geometry" | "link" | "text" {
  if (slot.kind === "GeoProperty") {
    return "geometry";
  }
  if (slot.kind === "Relationship") {
    return "link";
  }
  if (slot.range === "date" || slot.range === "datetime") {
    return "temporal";
  }
  if (["integer", "float", "double", "decimal"].includes(slot.range ?? "")) {
    return "range";
  }
  return "text";
}
