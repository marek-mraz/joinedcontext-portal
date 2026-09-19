import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { arrange, index, localized, paths, requiredProgress } from "../src/components/forms/uischema";
import type { UiSchemaManifest } from "../src/components/forms/uischema";
import type { JsonSchema } from "../src/components/forms/types";
import en from "../src/locales/en.json";

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

  it("reads a plain label as written, in any locale (UI-50)", () => {
    expect(localized("Basics", "de")).toBe("Basics");
    expect(localized("Basics")).toBe("Basics");
    const arranged = arrange({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "UiSchema",
      metadata: { name: "endpoint" },
      spec: {
        for: "Endpoint",
        groups: [{ title: "Access", description: "Who may read", fields: ["slug"] }],
        fields: { slug: { help: "The unguessable address" } },
      },
    } as UiSchemaManifest);
    const options = arranged.uiSchema["ui:options"] as { groups: { title?: string; description?: string }[] };
    expect(options.groups[0]).toMatchObject({ title: "Access", description: "Who may read" });
    expect(arranged.uiSchema.slug).toMatchObject({ "ui:help": "The unguessable address" });
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

  it("says so when the manifest arranges a field inside an object the form does render", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: { for: "Endpoint", fields: { "caching.gone": { widget: "text" } } },
    };
    const { uiSchema, problems } = arrange(manifest, {
      properties: ["caching", "caching.maxAgeSeconds"],
    });
    expect(uiSchema.caching).toBeUndefined();
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("caching.gone");
  });

  /**
   * A data source's form renders one branch of its schema at a time — `mqtt`, `http`,
   * `webSocket`, `gtfsRt` — and one manifest arranges them all, so the three branches the person
   * did not pick are absent from this schema and must be passed over without a word. Telling a
   * person filling in an HTTP source that "the schema has no field `mqtt.topics`" is noise about a
   * form they are not looking at (T-1613). Only the branches `spec.variants` names are passed over:
   * anything else missing is a typo, and the form still says so.
   */
  it("passes over a branch of the schema the form is not rendering, silently", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: {
        for: "DataSource",
        variants: ["mqtt", "http"],
        order: ["name", "http", "mqtt"],
        groups: [{ fields: ["name", "http", "mqtt"] }],
        fields: { "http.url": { placeholder: "https://www.hel.fi/en/news/rss" }, "mqtt.qos": {} },
      },
    };
    const { uiSchema, problems } = arrange(manifest, {
      properties: ["name", "http", "http.url"],
    });
    expect(problems).toEqual([]);
    expect(uiSchema.http).toEqual({ url: { "ui:placeholder": "https://www.hel.fi/en/news/rss" } });
    expect(uiSchema.mqtt).toBeUndefined();
    expect(uiSchema["ui:order"]).toEqual(["name", "http", "*"]);
  });

  it("says nothing about a form whose schema has not arrived yet", () => {
    // A dialog opens while the lists its enums come from are still loading; the manifest is not
    // wrong because the schema is not there yet.
    const { uiSchema, problems } = arrange(MANIFEST, { properties: [] });
    expect(problems).toEqual([]);
    expect(uiSchema["ui:order"]).toEqual(["audience", "name", "slug", "*"]);
  });

  it("reads every field of a schema by its path, arrays and objects alike", () => {
    expect(
      paths({
        properties: {
          name: { type: "string" },
          caching: { type: "object", properties: { maxAgeSeconds: { type: "integer" } } },
          representations: { type: "array", items: { type: "string" } },
          pages: {
            type: "array",
            items: { type: "object", properties: { layers: { type: "array", items: { type: "string" } } } },
          },
        },
      }),
    ).toEqual([
      "name",
      "caching",
      "caching.maxAgeSeconds",
      // An array of plain strings is one field, not a field with children.
      "representations",
      "pages",
      "pages[].layers",
    ]);
  });

  it("arranges a field by its path, into the nested uiSchema RJSF reads", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: {
        for: "Dashboard",
        fields: {
          "pages[].widgets[].property": { help: "One attribute of the entity.", columns: 6 },
        },
      },
    };
    const { uiSchema, problems } = arrange(manifest, {
      properties: ["pages", "pages[].widgets", "pages[].widgets[].property"],
    });
    expect(problems).toEqual([]);
    expect(uiSchema).toEqual({
      pages: {
        items: {
          widgets: {
            items: {
              property: { "ui:help": "One attribute of the entity.", "ui:options": { columns: 6 } },
            },
          },
        },
      },
    });
  });

  it("puts a list's example on its items, where the person types (T-2257)", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: {
        for: "DataSource",
        fields: {
          "mqtt.urls": {
            help: "Network addresses of the message brokers.",
            placeholder: ["wss://mqtt.hsl.fi:443/", "wss://second.example/"],
          },
        },
      },
    };
    const { uiSchema, problems } = arrange(manifest, {
      properties: ["mqtt", "mqtt.urls"],
    });
    expect(problems).toEqual([]);
    expect(uiSchema).toEqual({
      mqtt: {
        urls: {
          "ui:help": "Network addresses of the message brokers.",
          // The list keeps no example of its own: the control that takes one is the item, and a
          // list written there reads as "Invalid type".
          items: { "ui:placeholder": "wss://mqtt.hsl.fi:443/" },
        },
      },
    });
  });

  it("keeps a nested path out of the reading order and out of a group, with the reason", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: {
        for: "DataSource",
        order: ["name", "http.url"],
        groups: [{ fields: ["name", "http.url"] }],
      },
    };
    const { uiSchema, problems } = arrange(manifest, {
      properties: ["name", "http", "http.url"],
    });
    expect(uiSchema["ui:order"]).toEqual(["name", "*"]);
    expect(uiSchema["ui:options"]).toMatchObject({ groups: [{ fields: ["name"] }] });
    expect(problems).toHaveLength(2);
    expect(problems.every((problem) => problem.includes("http.url"))).toBe(true);
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

  /**
   * T-1604, UI-02: the help is written for the person who fills the form; the schema's
   * description is the field's rustdoc, carried into the API and MCP for engineers. A form that
   * answers "the reconciler renders it as the last `mapping` processor of the generated
   * bento.yaml" has answered a question nobody asked, so help comes first and the description is
   * the fallback.
   */
  it("shows a field's help in the person's language, and the schema's description only without it", async () => {
    const schema: JsonSchema = {
      type: "object",
      properties: {
        mapping: {
          type: "string",
          title: "Mapping",
          description: "The inline mapping of a `bloblang` step (PL-41): the reconciler renders it",
        },
        plain: { type: "string", title: "Plain", description: "What the engineer wrote" },
      },
    };
    const manifest: UiSchemaManifest = {
      kind: "UiSchema",
      metadata: { name: "pipeline" },
      spec: {
        for: "Pipeline",
        fields: { mapping: { help: { sk: "Ako sa záznam prepíše na entitu", en: "How one record becomes one entity" } } },
      },
    };
    for (const [locale, said] of [
      ["en", "How one record becomes one entity"],
      ["sk", "Ako sa záznam prepíše na entitu"],
    ] as const) {
      await i18n.changeLanguage(locale);
      const { uiSchema } = arrange(manifest, { properties: ["mapping", "plain"], locale });
      const { unmount } = render(
        <I18nextProvider i18n={i18n}>
          <SchemaForm schema={schema} uiSchema={uiSchema} onSubmit={() => {}} />
        </I18nextProvider>,
      );
      expect(screen.getByText(said)).toBeInTheDocument();
      // The rustdoc is not shown beside it: one sentence, the person's.
      expect(screen.queryByText(/bloblang. step .PL-41/)).toBeNull();
      // A field the manifest says nothing about keeps the description it always had.
      expect(screen.getByText("What the engineer wrote")).toBeInTheDocument();
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  /** T-1604: the example is a value the field accepts, and one action puts it there. */
  it("fills the field from its example, and offers the action only while the field is empty", async () => {
    const user = userEvent.setup();
    const schema: JsonSchema = {
      type: "object",
      properties: {
        url: { type: "string", title: "URL" },
        perMinute: { type: "integer", title: "Requests per minute" },
      },
    };
    const manifest: UiSchemaManifest = {
      kind: "UiSchema",
      metadata: { name: "datasource" },
      spec: {
        for: "DataSource",
        fields: {
          url: { placeholder: "https://opendata.example.org/aq.json" },
          perMinute: { placeholder: "600" },
        },
      },
    };
    const { uiSchema } = arrange(manifest, { properties: ["url", "perMinute"], locale: "en" });
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm schema={schema} uiSchema={uiSchema} onSubmit={() => {}} />
      </I18nextProvider>,
    );

    const actions = screen.getAllByRole("button", { name: en.form.useExample });
    expect(actions).toHaveLength(2);
    await user.click(actions[0]);
    expect(screen.getByLabelText(/^URL/)).toHaveValue("https://opendata.example.org/aq.json");
    // A field that holds something is not offered an overwrite nobody asked for.
    expect(screen.getAllByRole("button", { name: en.form.useExample })).toHaveLength(1);

    // A number example reaches the field as a number, not as the string "600".
    await user.click(screen.getByRole("button", { name: en.form.useExample }));
    expect(screen.getByLabelText(/^Requests per minute/)).toHaveValue(600);
  });

  /**
   * T-2251: the whole word arrives. The action that fills in an example used to be mounted only
   * while the field was empty, so the first character moved the input into another parent, React
   * built a new input, the caret went with the old one and the rest of the word was typed into
   * nothing. Measured on the endpoint form on 2026-09-19.
   */
  it("keeps the caret in a field with an example while a whole word is typed", async () => {
    const user = userEvent.setup();
    const schema: JsonSchema = {
      type: "object",
      properties: { url: { type: "string", title: "URL" } },
    };
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: {
        for: "DataSource",
        fields: { url: { placeholder: "https://opendata.example.org/aq.json" } },
      },
    };
    const { uiSchema } = arrange(manifest, { properties: ["url"], locale: "en" });
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm schema={schema} uiSchema={uiSchema} onSubmit={() => {}} />
      </I18nextProvider>,
    );

    const field = screen.getByLabelText(/^URL/);
    await user.type(field, "https://www.hel.fi/en/news/rss");
    expect(field).toHaveValue("https://www.hel.fi/en/news/rss");
    expect(document.activeElement).toBe(field);
    // And the offer is gone from the page's actions, without having moved the input to do it.
    expect(screen.queryByRole("button", { name: en.form.useExample })).not.toBeInTheDocument();
  });

  /**
   * T-1607: a long form shows what matters and folds the rest. A group the manifest folds opens on a
   * click and is in the page all along, so a person searching the form with the keyboard finds it.
   */
  it("folds the group the manifest folds and keeps a group with a required field open", () => {
    const manifest: UiSchemaManifest = {
      ...MANIFEST,
      spec: {
        for: "Endpoint",
        groups: [
          { title: "Basics", fields: ["name"] },
          { title: "Limits", description: "Rate and cache", folded: true, fields: ["notes"] },
          { title: "Access", folded: true, fields: ["audience"] },
        ],
      },
    };
    const { uiSchema } = arrange(manifest, {
      properties: PROPERTIES,
      locale: "en",
      required: ["audience"],
    });
    const groups = (uiSchema["ui:options"] as { groups: { title?: string; folded?: boolean }[] })
      .groups;
    expect(groups.map((group) => [group.title, group.folded === true])).toEqual([
      ["Basics", false],
      ["Limits", true],
      // A group that holds a required field is never folded, whatever the manifest asks.
      ["Access", false],
    ]);

    const { container } = form(uiSchema);
    const folded = container.querySelector("details") as HTMLDetailsElement;
    expect(folded).not.toBeNull();
    expect(folded.open).toBe(false);
    expect(folded.querySelector("summary")?.textContent).toContain("Limits");
    // The field is in the page, folded or not: nothing is removed from the form to tidy it.
    expect(screen.getByLabelText(/^Notes/)).toBeInTheDocument();
  });

  /** T-1607: the footer counts what the form still wants, so a folded group hides no reason. */
  it("counts the required fields the form still wants", async () => {
    const user = userEvent.setup();
    const schema: JsonSchema = {
      type: "object",
      required: ["name", "url", "tls"],
      properties: {
        name: { type: "string", title: "Name" },
        url: { type: "string", title: "URL" },
        // An object that requires something of its own: not asked for until it holds anything.
        tls: {
          type: "object",
          required: ["caCert"],
          properties: { caCert: { type: "string", title: "CA certificate" } },
        },
      },
    };
    expect(requiredProgress(schema, {})).toEqual({ filled: 0, total: 3 });
    expect(requiredProgress(schema, { name: "a", url: "" })).toEqual({ filled: 1, total: 3 });
    expect(requiredProgress(schema, { name: "a", url: "b", tls: {} })).toEqual({
      filled: 2,
      total: 3,
    });
    // An optional object is not counted at all, whatever it requires of its own.
    expect(
      requiredProgress(
        { properties: { tls: schema.properties!.tls as object }, required: [] },
        {},
      ),
    ).toEqual({ filled: 0, total: 0 });
    expect(requiredProgress(schema, { name: "a", url: "b", tls: { caCert: "c" } })).toEqual({
      filled: 3,
      total: 3,
    });

    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm schema={schema} onSubmit={() => {}} />
      </I18nextProvider>,
    );
    expect(screen.getByTestId("required-count").textContent).toBe(
      "0 of 3 required fields filled",
    );
    await user.type(screen.getByLabelText(/^Name/), "helsinki-parking");
    expect(screen.getByTestId("required-count").textContent).toBe(
      "1 of 3 required fields filled",
    );
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
    ] as unknown[]);

    expect(Object.keys(forms)).toEqual(["Endpoint"]);
    expect(forms.Endpoint.metadata?.name).toBe("endpoint");
    expect(problems).toHaveLength(3);
    expect(problems.some((problem) => problem.includes("nameless"))).toBe(true);
  });

  /**
   * UI-02: the arrangement the UI ships meets the one an organization commits to `portal/forms/`.
   * A whole-file override would throw away the shipped help for every field the organization did
   * not write about, which is how a form with help ends up with none (T-1612).
   */
  it("lets a later manifest change a field without losing the arrangement of the others", () => {
    const { forms, problems } = index([
      MANIFEST,
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "UiSchema",
        metadata: { name: "endpoint" },
        spec: {
          for: "Endpoint",
          fields: { slug: { help: "Ask the data office before you change one." } },
        },
      },
    ] as unknown[]);

    const fields = forms.Endpoint.spec.fields ?? {};
    expect(fields.slug?.help).toBe("Ask the data office before you change one.");
    // The shipped arrangement of the same field survives beside the organization's own help.
    expect(fields.slug?.placeholder).toBe("26 znakov");
    expect(fields.slug?.columns).toBe(6);
    expect(fields.notes?.help).toEqual({ sk: "Voľný text", en: "Free text" });
    expect(Object.keys(fields).sort()).toEqual(Object.keys(MANIFEST.spec.fields ?? {}).sort());
    expect(forms.Endpoint.spec.order).toEqual(MANIFEST.spec.order);
    expect(problems).toEqual([]);
  });
});
