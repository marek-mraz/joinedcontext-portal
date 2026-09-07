/**
 * The `kind: UiSchema` manifest as an RJSF `uiSchema` (T-0202, UI-02, CC-29, CC-31).
 *
 * A UiSchema manifest arranges a form; it never declares a field. Everything renderable comes
 * from the JSON Schema, so this consumer only ever reorders, labels, hides and picks widgets
 * for fields the schema already has (Architecture/09 section 2).
 *
 * Two things it refuses to pass through. A widget name the Portal has not registered is
 * dropped with its reason, because RJSF throws on an unknown widget and a form that does not
 * render is a worse answer than a form with a default input. And a field marked `advanced` is
 * hidden unless the caller asks for the advanced mode: Git mechanics are fields like any
 * other, and CC-29 is about which of them a steward has to look at, not about removing them.
 */

import type { UiSchema } from "./types";

/** RJSF's own widgets, by the name a manifest writes. */
const BUILT_IN_WIDGETS = [
  "text",
  "textarea",
  "select",
  "checkboxes",
  "checkbox",
  "radio",
  "range",
  "color",
  "date",
  "date-time",
  "password",
  "updown",
  "email",
  "uri",
  "hidden",
] as const;

/** A human string in every locale the manifest carries (Architecture/09 section 3). */
export type LanguageMap = Record<string, string>;

/** One field's arrangement. */
export interface FieldArrangement {
  widget?: string;
  help?: LanguageMap;
  placeholder?: string;
  /** A twelfth of the row, so `6` is half. */
  columns?: number;
  readOnly?: boolean;
  /** Hidden outside the advanced mode (CC-29). */
  advanced?: boolean;
}

/** One visual grouping of a flat schema. */
export interface Group {
  title?: LanguageMap;
  description?: LanguageMap;
  fields: string[];
}

/** The `spec` of a `kind: UiSchema` manifest. */
export interface UiSchemaSpec {
  /** The kind whose form this arranges. */
  for: string;
  order?: string[];
  groups?: Group[];
  fields?: Record<string, FieldArrangement>;
}

/** A `kind: UiSchema` manifest as it is committed. */
export interface UiSchemaManifest {
  apiVersion?: string;
  kind?: string;
  metadata?: { name?: string };
  spec: UiSchemaSpec;
}

export interface ArrangeOptions {
  /** The locale to resolve language maps in; falls back to Slovak, then to any. */
  locale?: string;
  /** The names the JSON Schema actually declares. Arrangement of anything else is dropped. */
  properties?: string[];
  /** Whether the advanced fields are shown (CC-29). */
  advanced?: boolean;
  /** Widget names the Portal has registered beyond RJSF's own. */
  widgets?: string[];
}

/** What one arrangement produced, and what it could not use. */
export interface Arranged {
  uiSchema: UiSchema;
  /** Everything the manifest asked for that this form cannot do, each in one clause. */
  problems: string[];
}

/** The default locale of the platform, and the fallback of every language map. */
const DEFAULT_LOCALE = "sk";

/** One language map in the caller's locale, then Slovak, then whatever it carries. */
export function localized(map: LanguageMap | undefined, locale?: string): string | undefined {
  if (!map) {
    return undefined;
  }
  const wanted = locale ? map[locale] : undefined;
  return wanted ?? map[DEFAULT_LOCALE] ?? Object.values(map)[0];
}

/**
 * The RJSF `uiSchema` one manifest describes.
 *
 * Order: the manifest's names first, in its order, then `"*"` so RJSF renders everything the
 * manifest did not mention in schema order after them. A form that dropped an unmentioned
 * field would hide a value the user has to fill in.
 */
export function arrange(manifest: UiSchemaManifest, options: ArrangeOptions = {}): Arranged {
  const problems: string[] = [];
  const spec = manifest.spec;
  const known = options.properties;
  const widgets = new Set<string>([...BUILT_IN_WIDGETS, ...(options.widgets ?? [])]);

  const declared = (field: string): boolean => {
    if (!known || known.includes(field)) {
      return true;
    }
    problems.push(`the schema has no field \`${field}\`, so nothing was arranged for it`);
    return false;
  };

  const uiSchema: UiSchema = {};

  for (const [field, arrangement] of Object.entries(spec.fields ?? {})) {
    if (!declared(field)) {
      continue;
    }
    const entry: Record<string, unknown> = {};

    if (arrangement.widget !== undefined) {
      if (widgets.has(arrangement.widget)) {
        entry["ui:widget"] = arrangement.widget;
      } else {
        problems.push(
          `\`${arrangement.widget}\` is not a widget this Portal registers, so \`${field}\` keeps its default input`,
        );
      }
    }

    const help = localized(arrangement.help, options.locale);
    if (help !== undefined) {
      entry["ui:help"] = help;
    }
    if (arrangement.placeholder !== undefined) {
      entry["ui:placeholder"] = arrangement.placeholder;
    }
    if (arrangement.readOnly !== undefined) {
      entry["ui:readonly"] = arrangement.readOnly;
    }
    if (arrangement.columns !== undefined) {
      if (Number.isInteger(arrangement.columns) && arrangement.columns >= 1 && arrangement.columns <= 12) {
        entry["ui:options"] = { columns: arrangement.columns };
      } else {
        problems.push(`\`${field}\` asks for ${arrangement.columns} of twelve columns, which is not a column count`);
      }
    }
    // CC-29: hidden by arrangement, never removed. RJSF keeps the value and submits it, so
    // the commit message a steward never sees is still the one the Change carries.
    if (arrangement.advanced && options.advanced !== true) {
      entry["ui:widget"] = "hidden";
    }

    if (Object.keys(entry).length > 0) {
      uiSchema[field] = entry;
    }
  }

  const order = (spec.order ?? []).filter(declared);
  if (order.length > 0) {
    uiSchema["ui:order"] = [...order, "*"];
  }

  const groups = (spec.groups ?? [])
    .map((group) => ({
      title: localized(group.title, options.locale),
      description: localized(group.description, options.locale),
      fields: (group.fields ?? []).filter(declared),
    }))
    .filter((group) => group.fields.length > 0);
  if (groups.length > 0) {
    uiSchema["ui:options"] = { ...(uiSchema["ui:options"] as object | undefined), groups };
  }

  return { uiSchema, problems };
}

/**
 * The manifests of a directory, indexed by the kind each one arranges.
 *
 * A document that is not a `UiSchema` manifest, or that names no kind, is left out rather
 * than indexed under a guess: `portal/forms/` is read as a whole and one malformed file must
 * not take the other forms down with it.
 */
export function index(
  documents: unknown[],
): { forms: Record<string, UiSchemaManifest>; problems: string[] } {
  const forms: Record<string, UiSchemaManifest> = {};
  const problems: string[] = [];

  for (const document of documents) {
    const manifest = document as UiSchemaManifest | null;
    if (!manifest || typeof manifest !== "object" || manifest.kind !== "UiSchema") {
      problems.push("a document in portal/forms is not a UiSchema manifest and was left out");
      continue;
    }
    const target = manifest.spec?.for;
    if (typeof target !== "string" || target.length === 0) {
      problems.push(
        `\`${manifest.metadata?.name ?? "unnamed"}\` names no kind in spec.for, so no form knows to use it`,
      );
      continue;
    }
    if (forms[target]) {
      problems.push(`two UiSchema manifests arrange \`${target}\`; the first one is used`);
      continue;
    }
    forms[target] = manifest;
  }

  return { forms, problems };
}
