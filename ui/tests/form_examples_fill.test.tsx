/**
 * A form filled from nothing but its own examples ends up complete (T-2257, T-1606; UI-02, PF-57).
 *
 * The live journey `ui/e2e/live/forms-examples.spec.ts` measured this on dev on 2026-09-19 and the
 * data source form failed it: two array fields answered "Invalid type" and their manifest had no
 * `spec.mqtt.urls` at all, because a list example was written at the array while the control a
 * person types into is the array's *item*. This is that journey's shape in jsdom, on the kind it
 * failed on, so the next list field is covered before it reaches the cluster.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { arrange, index, paths } from "../src/components/forms/uischema";
import { portalThemeWidgets } from "../src/components/forms/theme";
import { portalWidgets } from "../src/components/forms/widgets";
import { shippedForms } from "../src/schemas/forms";
import { dataSourceSchema } from "../src/schemas/kinds";
import type { JsonSchema } from "../src/components/forms/types";

/** The arrangement a dialog would hand the form for one kind and schema. */
function arranged(kind: string, schema: JsonSchema) {
  const manifest = index([...shippedForms]).forms[kind];
  expect(manifest, `${kind} ships an arrangement`).toBeDefined();
  return arrange(manifest, {
    locale: "en",
    properties: paths(schema),
    required: schema.required,
    widgets: [...Object.keys(portalThemeWidgets), ...Object.keys(portalWidgets)],
  });
}

/** Clicks every offered example until none is left, the way a person works down the form. */
async function takeEveryExample(user: ReturnType<typeof userEvent.setup>): Promise<number> {
  let taken = 0;
  for (let round = 0; round < 40; round += 1) {
    const offers = screen
      .queryAllByRole("button", { name: en.form.useExample })
      .filter((button) => !button.hasAttribute("disabled"));
    if (offers.length === 0) {
      return taken;
    }
    await user.click(offers[0]);
    taken += 1;
  }
  return taken;
}

describe("a form filled from its own examples", () => {
  it("fills a list field's first item, not the list itself", async () => {
    const user = userEvent.setup();
    const schema = dataSourceSchema((key: string) => key, "mqtt");
    const { uiSchema, problems } = arranged("DataSource", schema);
    expect(problems).toEqual([]);

    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={schema}
          uiSchema={uiSchema}
          formData={{ name: "hsl-mqtt", mqtt: { urls: [""], topics: [""] } }}
          submitLabel="Check"
          onSubmit={() => {}}
        />
      </I18nextProvider>,
    );

    const taken = await takeEveryExample(user);
    expect(taken, "the form offers examples to take").toBeGreaterThan(2);

    // The item holds the example, and nothing holds the list's own JSON.
    const urls = screen.getAllByRole("textbox").map((field) => (field as HTMLInputElement).value);
    expect(
      urls.some((value) => value === "wss://mqtt.hsl.fi:443/"),
      `an item of Broker URLs holds the example: ${JSON.stringify(urls)}`,
    ).toBe(true);
    expect(
      urls.some((value) => value.startsWith("[") || value.includes("object Object")),
      `no field holds the list itself: ${JSON.stringify(urls)}`,
    ).toBe(false);

    // And every required field the form shows is filled by the end of it.
    const empty = screen
      .getAllByRole("textbox")
      .filter((field) => {
        const input = field as HTMLInputElement;
        return (
          input.value === "" &&
          (input.required || input.getAttribute("aria-required") === "true")
        );
      })
      .map((field) => (field as HTMLInputElement).id);
    expect(empty, "a required field its own examples cannot fill").toEqual([]);
  });
});
