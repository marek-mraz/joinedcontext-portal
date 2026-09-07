import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { arrange, index, localized } from "../src/components/forms/uischema";
import type { UiSchemaManifest } from "../src/components/forms/uischema";
import type { JsonSchema } from "../src/components/forms/types";

/**
 * T-0202: the `kind: UiSchema` manifest arranges a form — order, widgets, help and grouping —
 * and hides the Git mechanics from the default view (UI-02, CC-29, CC-31).
 */

const SCHEMA: JsonSchema = {
  type: "object",
  properties: {
    name: { type: "string", title: "Name" },
    slug: { type: "string", title: "Slug" },
    audience: { type: "string", title: "Audience", enum: ["public", "internal"] },
    notes: { type: "string", title: "Notes" },
    commitMessage: { type: "string", title: "Commit message" },
  },
};

const PROPERTIES = ["name", "slug", "audience", "notes", "commitMessage"];

const MANIFEST: UiSchemaManifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "UiSchema",
  metadata: { name: "endpoint" },
  spec: {
    for: "Endpoint",
    order: ["audience", "name", "slug"],
    groups: [
      { title: { sk: "Základ", en: "Basics" }, fields: ["name", "slug"] },
      {
        title: { sk: "Prístup", en: "Access" },
        description: { sk: "Kto smie čítať", en: "Who may read" },
        fields: ["audience"],
      },
    ],
    fields: {
      notes: { widget: "textarea", help: { sk: "Voľný text", en: "Free text" } },
      slug: { placeholder: "26 znakov", columns: 6, readOnly: true },
      commitMessage: { advanced: true },
    },
  },
};

function form(uiSchema: ReturnType<typeof arrange>["uiSchema"]) {
  return render(
    <I18nextProvider i18n={i18n}>
      <SchemaForm schema={SCHEMA} uiSchema={uiSchema} onSubmit={() => {}} />
    </I18nextProvider>,
  );
}

describe("the UiSchema manifest arranges the form", () => {
  it("puts the manifest's order first and everything else after it", () => {
    const { uiSchema, problems } = arrange(MANIFEST, { properties: PROPERTIES });
    expect(problems).toEqual([]);
    // The trailing `*` is what keeps a field the manifest forgot on the form.
    expect(uiSchema["ui:order"]).toEqual(["audience", "name", "slug", "*"]);
  });

  it("binds the widget, the help text, the placeholder and the column count", () => {
    const { uiSchema } = arrange(MANIFEST, { properties: PROPERTIES, locale: "en" });
    expect(uiSchema.notes).toMatchObject({ "ui:widget": "textarea", "ui:help": "Free text" });
    expect(uiSchema.slug).toMatchObject({
      "ui:placeholder": "26 znakov",
      "ui:readonly": true,
      "ui:options": { columns: 6 },
    });
  });

  it("resolves a language map in the caller's locale, then Slovak, then anything", () => {
    expect(localized({ sk: "Základ", en: "Basics" }, "en")).toBe("Basics");
    expect(localized({ sk: "Základ", en: "Basics" }, "de")).toBe("Základ");
    expect(localized({ de: "Grundlage" }, "en")).toBe("Grundlage");
    expect(localized(undefined, "en")).toBeUndefined();
  });

  it("refuses a widget the Portal has not registered rather than throwing at render", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: { for: "Endpoint", fields: { notes: { widget: "hologram" } } },
    };
    const { uiSchema, problems } = arrange(manifest, { properties: PROPERTIES });
    expect(uiSchema.notes).toBeUndefined();
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("hologram");
  });

  it("takes a widget the Portal registered beyond the built-in ones", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: { for: "Endpoint", fields: { notes: { widget: "entityPicker" } } },
    };
    const { uiSchema, problems } = arrange(manifest, {
      properties: PROPERTIES,
      widgets: ["entityPicker"],
    });
    expect(uiSchema.notes).toMatchObject({ "ui:widget": "entityPicker" });
    expect(problems).toEqual([]);
  });

  it("says so when the manifest arranges a field the schema does not have", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: {
        for: "Endpoint",
        order: ["name", "gone"],
        fields: { gone: { widget: "text" } },
        groups: [{ fields: ["gone"] }],
      },
    };
    const { uiSchema, problems } = arrange(manifest, { properties: PROPERTIES });
    expect(uiSchema.gone).toBeUndefined();
    expect(uiSchema["ui:order"]).toEqual(["name", "*"]);
    expect(uiSchema["ui:options"]).toBeUndefined();
    expect(problems.every((problem) => problem.includes("gone"))).toBe(true);
  });

  it("refuses a column count that is not a twelfth of a row", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: { for: "Endpoint", fields: { slug: { columns: 13 } } },
    };
    const { uiSchema, problems } = arrange(manifest, { properties: PROPERTIES });
    expect(uiSchema.slug).toBeUndefined();
    expect(problems[0]).toContain("13");
  });
});

describe("CC-29: the Git mechanics are not in the default view", () => {
  it("hides an advanced field until the advanced mode asks for it", () => {
    const plain = arrange(MANIFEST, { properties: PROPERTIES });
    expect(plain.uiSchema.commitMessage).toMatchObject({ "ui:widget": "hidden" });

    const advanced = arrange(MANIFEST, { properties: PROPERTIES, advanced: true });
    expect(advanced.uiSchema.commitMessage).toBeUndefined();
  });

  it("hides the field on the rendered form and leaves the others alone", () => {
    const { uiSchema } = arrange(MANIFEST, { properties: PROPERTIES, locale: "en" });
    form(uiSchema);
    expect(screen.queryByLabelText(/commit message/i)).not.toBeInTheDocument();
    expect(screen.getByLabelText(/^Name/)).toBeInTheDocument();
  });
});

describe("the form renders what the manifest arranged", () => {
  it("puts each grouped field in its own section and the rest after them", () => {
    const { uiSchema } = arrange(MANIFEST, { properties: PROPERTIES, locale: "en" });
    const { container } = form(uiSchema);

    const sections = Array.from(container.querySelectorAll("fieldset"));
    expect(sections).toHaveLength(2);
    expect(within(sections[0] as HTMLElement).getByText("Basics")).toBeInTheDocument();
    expect(within(sections[0] as HTMLElement).getByLabelText(/^Name/)).toBeInTheDocument();
    expect(within(sections[0] as HTMLElement).getByLabelText(/^Slug/)).toBeInTheDocument();
    expect(within(sections[1] as HTMLElement).getByText("Who may read")).toBeInTheDocument();
    expect(within(sections[1] as HTMLElement).getByLabelText(/^Audience/)).toBeInTheDocument();

    // `notes` is in no group, so it is on the form once, outside every section.
    const notes = screen.getByLabelText(/^Notes/);
    expect(notes).toBeInTheDocument();
    expect(sections.some((section) => section.contains(notes))).toBe(false);
  });

  it("renders the bound widget and the help text", () => {
    const { uiSchema } = arrange(MANIFEST, { properties: PROPERTIES, locale: "en" });
    form(uiSchema);
    expect(screen.getByLabelText(/^Notes/).tagName).toBe("TEXTAREA");
    expect(screen.getByText("Free text")).toBeInTheDocument();
    expect(screen.getByLabelText(/^Slug/)).toHaveAttribute("placeholder", "26 znakov");
  });

  it("renders a form with no manifest at all exactly as before", () => {
    const { container } = form({});
    expect(container.querySelectorAll("fieldset")).toHaveLength(0);
    expect(screen.getByLabelText(/^Name/)).toBeInTheDocument();
    expect(screen.getByLabelText(/commit message/i)).toBeInTheDocument();
  });
});

describe("portal/forms is read as a whole", () => {
  it("indexes the manifests by the kind each one arranges", () => {
    const { forms, problems } = index([MANIFEST]);
    expect(Object.keys(forms)).toEqual(["Endpoint"]);
    expect(problems).toEqual([]);
  });

  it("leaves out what is not a UiSchema manifest instead of taking the directory down", () => {
    const { forms, problems } = index([
      MANIFEST,
      { apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", spec: { for: "x" } },
      { kind: "UiSchema", metadata: { name: "nameless" }, spec: {} },
      null,
      { kind: "UiSchema", metadata: { name: "second" }, spec: { for: "Endpoint" } },
    ] as unknown[]);

    expect(Object.keys(forms)).toEqual(["Endpoint"]);
    expect(forms.Endpoint.metadata?.name).toBe("endpoint");
    expect(problems).toHaveLength(4);
    expect(problems.some((problem) => problem.includes("nameless"))).toBe(true);
    expect(problems.some((problem) => problem.includes("two UiSchema manifests"))).toBe(true);
  });
});
