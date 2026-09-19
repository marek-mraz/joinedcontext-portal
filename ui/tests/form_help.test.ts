/**
 * UI-02, UI-11, UI-12: every field of every form the Portal renders carries one sentence of help in
 * the four shipped locales and an example a person can apply.
 *
 * Measured on main on 2026-09-19, before the arrangements this test guards: the seven forms render
 * 87 fields, 80 of them showed no help at all and 63 no example, default or list of choices — and
 * help for a nested field such as `http.url` could not even be written, because the arrangement
 * reached top-level names only (T-1612 to T-1619, T-2250).
 *
 * The test reads the arrangements the UI ships and the form schemas the pages pass, so it fails for
 * a field added to a form without help, for a locale left behind, and for an example the field's own
 * pattern would reject.
 */
import { describe, expect, it } from "vitest";
import { arrange, localized, paths } from "../src/components/forms/uischema";
import type { UiSchemaManifest } from "../src/components/forms/uischema";
import { shippedForms } from "../src/schemas/forms";
import * as kinds from "../src/schemas/kinds";
import type { JsonSchema } from "../src/components/forms/types";
import en from "../src/locales/en.json";

const LOCALES = ["en", "sk", "cs", "de"] as const;

/** The labels come from the bundles, like they do in the browser. */
const t = (key: string): string => {
  const value = key
    .split(".")
    .reduce<unknown>(
      (node, step) => (node as Record<string, unknown> | undefined)?.[step],
      en,
    );
  return typeof value === "string" ? value : key;
};

/**
 * The schemas the pages pass to `ResourceFormDialog`, with the arguments they pass on dev.
 *
 * A data source and a sync source render one branch of their schema at a time — the type, the
 * origin — and one arrangement covers every branch, so every branch is a schema of its own here.
 * Help written for `mqtt.qos` alone would leave the person who picked GTFS-RT with nothing.
 */
const FORMS: Record<string, JsonSchema[]> = {
  ContextSpace: [kinds.contextSpaceSchema(t)],
  Endpoint: [kinds.endpointSchema(t, ["helsinki"], ["helsinki"])],
  DataSource: kinds.DATA_SOURCE_TYPES.map((type) =>
    kinds.dataSourceSchema(t, type, ["hsl-api-token"]),
  ),
  SyncSource: kinds.SYNC_ORIGINS.map((origin) =>
    kinds.syncSourceSchema(t, origin, ["forge-token"]),
  ),
  Pipeline: [
    kinds.pipelineSchema(
      t,
      ["hel-news-rss"],
      [
        {
          urn: "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all",
          name: "helsinki-all",
        },
      ],
    ),
  ],
  Dashboard: [kinds.dashboardSchema(t, ["bikes"])],
  Layer: [kinds.layerSchema(t, ["helsinki"], ["Bike"])],
};

/** One field of a form: the leaf a person types into, and what its schema allows. */
interface Leaf {
  path: string;
  definition: Record<string, unknown>;
}

function leaves(schema: unknown, prefix = "", found: Leaf[] = []): Leaf[] {
  const properties = (
    schema as { properties?: Record<string, unknown> } | undefined
  )?.properties;
  for (const [name, raw] of Object.entries(properties ?? {})) {
    if (!raw || typeof raw !== "object") {
      continue;
    }
    const definition = raw as Record<string, unknown>;
    const path = prefix ? `${prefix}.${name}` : name;
    if (definition.properties) {
      leaves(definition, path, found);
    } else if (
      (definition.items as { properties?: unknown } | undefined)?.properties
    ) {
      leaves(definition.items, `${path}[]`, found);
    } else {
      found.push({ path, definition });
    }
  }
  return found;
}

/** The arrangement of one path, as `arrange` wrote it into the nested RJSF `uiSchema`. */
function entryAt(
  uiSchema: Record<string, unknown>,
  path: string,
): Record<string, unknown> {
  const steps = path
    .split(".")
    .flatMap((step) =>
      step.endsWith("[]") ? [step.slice(0, -2), "items"] : [step],
    );
  let node: unknown = uiSchema;
  for (const step of steps) {
    node = (node as Record<string, unknown> | undefined)?.[step];
    if (!node) {
      return {};
    }
  }
  return node as Record<string, unknown>;
}

function manifestFor(kind: string): UiSchemaManifest {
  const manifest = shippedForms.find((form) => form.spec?.for === kind);
  expect(manifest, `the UI ships an arrangement for ${kind}`).toBeDefined();
  return manifest as UiSchemaManifest;
}

/** A field whose choices or default already tell the person what a value looks like. */
function showsItsOwnValue(definition: Record<string, unknown>): boolean {
  const items = definition.items as
    { enum?: unknown; oneOf?: unknown } | undefined;
  return (
    definition.type === "boolean" ||
    definition.enum !== undefined ||
    definition.oneOf !== undefined ||
    items?.enum !== undefined ||
    items?.oneOf !== undefined ||
    definition.default !== undefined
  );
}

describe("the help and the example beside every form field", () => {
  for (const [kind, branches] of Object.entries(FORMS)) {
    /** Every field of every branch, each once: what the arrangement has to cover. */
    const allLeaves = [
      ...new Map(
        branches
          .flatMap((schema) => leaves(schema))
          .map((leaf) => [leaf.path, leaf]),
      ),
    ].map(([, leaf]) => leaf);
    const allPaths = [...new Set(branches.flatMap((schema) => paths(schema)))];
    const allTop = [
      ...new Set(
        branches.flatMap((schema) => Object.keys(schema.properties ?? {})),
      ),
    ];

    describe(kind, () => {
      it("names no field outside the form's own schema", () => {
        const manifest = manifestFor(kind);
        const named = [
          ...Object.keys(manifest.spec.fields ?? {}),
          ...(manifest.spec.order ?? []),
          ...(manifest.spec.groups ?? []).flatMap(
            (group) => group.fields ?? [],
          ),
        ];
        expect([
          ...new Set(named.filter((field) => !allPaths.includes(field))),
        ]).toEqual([]);
        for (const schema of branches) {
          expect(
            arrange(manifest, { properties: paths(schema) }).problems,
          ).toEqual([]);
        }
      });

      it("has one sentence of help for every field, in all four locales", () => {
        const manifest = manifestFor(kind);
        const missing: string[] = [];
        const untranslated: string[] = [];
        for (const locale of LOCALES) {
          const { uiSchema } = arrange(manifest, {
            locale,
            properties: allPaths,
          });
          for (const { path } of allLeaves) {
            const help = entryAt(uiSchema as Record<string, unknown>, path)[
              "ui:help"
            ];
            if (typeof help !== "string" || help.trim().length < 15) {
              missing.push(`${locale}: ${path}`);
            }
          }
        }
        for (const { path } of allLeaves) {
          const help = manifest.spec.fields?.[path]?.help;
          const written = new Set(
            LOCALES.map((locale) => localized(help, locale)),
          );
          // Four locales that are one string are one locale: the text was copied, not translated.
          if (written.size < 3) {
            untranslated.push(path);
          }
        }
        expect(missing, "fields with no help a person can read").toEqual([]);
        expect(
          untranslated,
          "fields whose four locales are the same text",
        ).toEqual([]);
      });

      it("offers an example the field accepts, wherever the field does not show its own values", () => {
        const { uiSchema } = arrange(manifestFor(kind), {
          properties: allPaths,
        });
        const missing: string[] = [];
        const refused: string[] = [];
        for (const { path, definition } of allLeaves) {
          const example = entryAt(uiSchema as Record<string, unknown>, path)[
            "ui:placeholder"
          ];
          if (showsItsOwnValue(definition)) {
            continue;
          }
          if (example === undefined || String(example).trim() === "") {
            missing.push(path);
            continue;
          }
          const pattern = definition.pattern as string | undefined;
          if (pattern && !new RegExp(pattern).test(String(example))) {
            refused.push(
              `${path}: ${String(example)} does not match ${pattern}`,
            );
          }
          if (
            definition.type === "integer" &&
            !Number.isInteger(Number(example))
          ) {
            refused.push(`${path}: ${String(example)} is not a whole number`);
          }
        }
        expect(missing, "fields a person faces with a blank input").toEqual([]);
        expect(refused, "examples the field itself would refuse").toEqual([]);
      });

      it("shows what a first-time person needs and folds what they do not (T-1607)", () => {
        const manifest = manifestFor(kind);
        const groups = manifest.spec.groups ?? [];
        // The first group is never folded: a form that opens with nothing on it explains nothing.
        expect(groups[0]?.folded ?? false, `${kind}'s first group is open`).toBe(false);
        for (const group of groups.filter((one) => one.folded === true)) {
          // A folded group holds no required field, so nothing the form insists on is out of sight.
          const required = new Set(branches.flatMap((schema) => schema.required ?? []));
          expect(
            group.fields.filter((field) => required.has(field)),
            `${kind}: the folded group "${localized(group.title, "en")}" hides a required field`,
          ).toEqual([]);
          expect(
            localized(group.title, "en"),
            `${kind}: a folded group needs a title to open it by`,
          ).toBeTruthy();
        }
      });

      it("reads in groups that hold every field of the form once", () => {
        const manifest = manifestFor(kind);
        const grouped = (manifest.spec.groups ?? []).flatMap(
          (group) => group.fields ?? [],
        );
        expect(grouped.length, `${kind} is arranged in groups`).toBeGreaterThan(
          0,
        );
        expect([...new Set(grouped)].sort(), "no field in two groups").toEqual(
          grouped.sort(),
        );
        expect(
          grouped.slice().sort(),
          "every top-level field is in a group",
        ).toEqual(allTop.slice().sort());
      });
    });
  }

  /**
   * UI-02, T-1608: the form says what its kind is for before it asks anything. A person who opened
   * the Layer form without knowing what a layer is has nowhere else to read it.
   */
  it("says what every kind is for, in all four locales, in two sentences", () => {
    for (const kind of Object.keys(FORMS)) {
      const manifest = manifestFor(kind);
      for (const locale of LOCALES) {
        const about = localized(manifest.spec.about, locale);
        expect(about, `${kind} says what it is for in ${locale}`).toBeTruthy();
        expect(
          (about ?? "").length,
          `${kind} in ${locale} is a paragraph, not a label`,
        ).toBeGreaterThan(80);
        // Two or three sentences: the person reads this standing in a dialog, not in a manual.
        const sentences = (about ?? "")
          .split(/[.!?](\s|$)/)
          .filter((part) => part.trim().length > 2);
        expect(
          sentences.length,
          `${kind} in ${locale} is ${sentences.length} sentences`,
        ).toBeLessThanOrEqual(3);
      }
      const written = new Set(
        LOCALES.map((locale) => localized(manifest.spec.about, locale)),
      );
      expect(
        written.size,
        `${kind} is translated, not copied`,
      ).toBeGreaterThanOrEqual(3);
    }
  });

  it("names every shipped arrangement after the kind it arranges, lowercased (MF-02)", () => {
    for (const manifest of shippedForms) {
      expect(manifest.kind).toBe("UiSchema");
      expect(manifest.metadata?.name).toBe(
        String(manifest.spec.for).toLowerCase(),
      );
    }
  });
});
