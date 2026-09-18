/**
 * A model from a dropped file (T-0599, DM-13, DM-31, DM-54, DM-55): the file goes up as
 * multipart with the CSRF token, the draft is previewed as text, and "Populate the editor"
 * applies the operations through the editor's own seam.
 */
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { queryKeys } from "../src/api/client";
import en from "../src/locales/en.json";
import { ModelFileDrop, draftOf, inferAnswerOf, slotRows } from "../src/pages/models/ModelFileDrop";
import type { InferAnswer } from "../src/pages/models/ModelFileDrop";
import { parseModel } from "../src/pages/models/linkml";

const ANSWER: InferAnswer = {
  linkml: "name: sensors\n",
  operations: [
    { op: "addClass", name: "Sensors", is_a: "Entity" },
    { op: "addSlot", name: "b_Temp_b", class: "Sensors", range: "float" },
    { op: "setTitle", target: "slot", name: "b_Temp_b", locale: "en", value: "<b>Temp</b> (°C)" },
    { op: "setSlot", name: "b_Temp_b", field: "minimum_value", value: 19 },
    { op: "setSlot", name: "b_Temp_b", field: "unit", value: "CEL" },
    { op: "addSlot", name: "plate", class: "Sensors", range: "string" },
    { op: "setSlot", name: "plate", field: "pattern", value: "^[A-Z]{2}[0-9]{3}[A-Z]{2}$" },
    { op: "addSlot", name: "parent", class: "Sensors", range: "uriorcurie", kind: "Relationship" },
    { op: "addSlot", name: "note", class: "Sensors", range: "string" },
  ],
  detectedTypes: { b_Temp_b: "float", plate: "string", parent: "Relationship", note: "string" },
  matches: {
    b_Temp_b: { model: "dataModel.Environment/AirQualityObserved", slotUri: "https://smartdatamodels.org/dataModel.Environment/temperature" },
  },
  untyped: [{ slot: "note", reason: "mixed values: 1 text, 1 integer" }],
  rows: 2,
};

function csv(): File {
  return new File(["id,<b>Temp</b> (°C),plate,parent,note\n1,21.5,BB123AB,urn:ngsi-ld:S:a:b:1,ok\n"], "sensors.csv", {
    type: "text/csv",
  });
}

/** A `Request`, a `URL` or a string: openapi-fetch sends one, the drop's own upload another. */
function urlOf(input: unknown): string {
  return typeof input === "string" ? input : input instanceof Request ? input.url : String(input);
}

/** The calls the drop itself makes, apart from the Organization list every page of the app reads. */
function inferCalls(fetchMock: ReturnType<typeof vi.fn>) {
  return fetchMock.mock.calls.filter((call) => urlOf(call[0]).includes("/tools/infer-schema"));
}

function renderDrop(response: () => Response, organizations: unknown[] = []) {
  const fetchMock = vi.fn((input: RequestInfo | URL) =>
    urlOf(input).includes("/organizations")
      ? Promise.resolve(
          new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: organizations }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        )
      : Promise.resolve(response()),
  );
  vi.stubGlobal("fetch", fetchMock);
  const onPopulate = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The Organization list as another page of the app leaves it in the cache.
  client.setQueryData(queryKeys.list("helsinki", "organizations"), {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "List",
    items: organizations,
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ModelFileDrop project="helsinki" onPopulate={onPopulate} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, onPopulate };
}

describe("a model from a dropped file", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=csrf-token-1";
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("uploads the file as multipart with the CSRF token and previews the draft as text", async () => {
    const { fetchMock } = renderDrop(
      () => new Response(JSON.stringify(ANSWER), { status: 200, headers: { "Content-Type": "application/json" } }),
    );

    await userEvent.upload(screen.getByLabelText(en.models.infer.chooseFile), csv());

    const dialog = await screen.findByRole("dialog");
    expect(inferCalls(fetchMock)).toHaveLength(1);
    const [url, init] = inferCalls(fetchMock)[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/v1/tools/infer-schema");
    expect(init.method).toBe("POST");
    expect(init.credentials).toBe("same-origin");
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBe("csrf-token-1");
    const form = init.body as FormData;
    expect(form.get("file")).toBeInstanceOf(File);
    expect((form.get("file") as File).name).toBe("sensors.csv");

    expect(within(dialog).getByText("Draft from sensors.csv")).toBeInTheDocument();
    expect(within(dialog).getByText("<b>Temp</b> (°C)")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
    expect(within(dialog).getByText("CEL")).toBeInTheDocument();
    expect(within(dialog).getByText("dataModel.Environment/AirQualityObserved")).toBeInTheDocument();
    expect(within(dialog).getByText("Relationship")).toBeInTheDocument();
    expect(within(dialog).getByText(/mixed values: 1 text, 1 integer/)).toBeInTheDocument();
  });

  it("populates the editor through the operations seam", async () => {
    const { onPopulate } = renderDrop(
      () => new Response(JSON.stringify(ANSWER), { status: 200, headers: { "Content-Type": "application/json" } }),
    );
    await userEvent.upload(screen.getByLabelText(en.models.infer.chooseFile), csv());
    const dialog = await screen.findByRole("dialog");

    await userEvent.click(within(dialog).getByRole("button", { name: en.models.infer.populate }));

    expect(onPopulate).toHaveBeenCalledTimes(1);
    const [source] = onPopulate.mock.calls[0] as [string, InferAnswer];
    const model = parseModel(source);
    expect(model.name).toBe("sensors");
    expect(model.classes.map((klass) => klass.name)).toEqual(["Sensors"]);
    expect(model.classes[0].slots).toEqual(["b_Temp_b", "plate", "parent", "note"]);
    const temperature = model.slots.find((slot) => slot.name === "b_Temp_b");
    expect(temperature?.range).toBe("float");
    expect(temperature?.minimum_value).toBe(19);
    expect(temperature?.unit?.exact_mappings).toEqual(["ucefact:CEL", "qudt-unit:DEG_C"]);
    expect(temperature?.title).toEqual({ en: "<b>Temp</b> (°C)" });
    expect(model.slots.find((slot) => slot.name === "plate")?.pattern).toBe("^[A-Z]{2}[0-9]{3}[A-Z]{2}$");
    expect(model.slots.find((slot) => slot.name === "parent")?.kind).toBe("Relationship");
    expect(source).toContain("is_a: Entity");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("refuses a file past 10 MiB before anything is sent", async () => {
    const { fetchMock } = renderDrop(() => new Response("{}", { status: 200 }));
    const huge = new File([new Uint8Array(10 * 1024 * 1024 + 1)], "huge.csv", { type: "text/csv" });

    await userEvent.upload(screen.getByLabelText(en.models.infer.chooseFile), huge);

    expect(await screen.findByRole("alert")).toHaveTextContent(en.models.infer.tooLarge);
    expect(inferCalls(fetchMock)).toHaveLength(0);
  });

  it("mints the draft under the organization's own domain, not a guess from the project (T-0794)", async () => {
    const { onPopulate } = renderDrop(
      () => new Response(JSON.stringify(ANSWER), { status: 200, headers: { "Content-Type": "application/json" } }),
      [
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Organization",
          metadata: { name: "city-of-helsinki", namespace: "helsinki" },
          spec: { domain: "hel.fi" },
        },
      ],
    );
    await userEvent.upload(screen.getByLabelText(en.models.infer.chooseFile), csv());
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: en.models.infer.populate }));

    const [source] = onPopulate.mock.calls[0] as [string, InferAnswer];
    expect(source).toContain("id: https://hel.fi/models/sensors");
    expect(source).not.toContain("helsinki.sk");
  });

  it("shows the Portal's reason when the sample cannot be read", async () => {
    renderDrop(
      () =>
        new Response(JSON.stringify({ detail: "the file is not a JSON document" }), {
          status: 400,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );

    await userEvent.upload(screen.getByLabelText(en.models.infer.chooseFile), csv());

    expect(await screen.findByRole("alert")).toHaveTextContent("the file is not a JSON document");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("takes only an answer Model Tools wrote, and lists what the editor refuses", () => {
    expect(inferAnswerOf(ANSWER)?.operations).toHaveLength(9);
    expect(inferAnswerOf({ linkml: "x" })).toBeNull();
    expect(inferAnswerOf(null)).toBeNull();
    expect(inferAnswerOf({ linkml: "x", operations: [1, { op: "addClass", name: "A" }] })?.operations).toHaveLength(1);

    const rows = slotRows(ANSWER.operations);
    expect(rows.map((row) => row.name)).toEqual(["b_Temp_b", "plate", "parent", "note"]);
    expect(rows[0]).toMatchObject({ klass: "Sensors", title: "<b>Temp</b> (°C)", unit: "CEL", range: "float" });

    const draft = draftOf(
      { ...ANSWER, operations: [...ANSWER.operations, { op: "addSlot", name: "plate", class: "Sensors" }] },
      "hel.fi",
      "My Sensors.csv",
    );
    expect(draft.refused).toEqual([{ index: 9, reason: "slot 'plate' already exists" }]);
    // The editor applies a list all or nothing (DM-13), so the draft is the blank model.
    expect(parseModel(draft.source).classes).toEqual([]);
    expect(parseModel(draft.source).name).toBe("my-sensors");
  });
});
