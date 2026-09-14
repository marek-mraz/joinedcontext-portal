import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row, Schema } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "../App";
import { filtersOf, shapeOf } from "./shape";

vi.mock("maplibre-gl", () => ({
  Map: class {
    on = vi.fn();
    remove = vi.fn();
  },
  setWorkerUrl: vi.fn(),
}));
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const STATIONS: Row[] = [
  { id: "urn:ngsi-ld:Station:example.org:demo:1", type: "Station", name: "Kamppi", bikes: 4, status: "open", location: { type: "Point", coordinates: [24.93, 60.17] } },
  { id: "urn:ngsi-ld:Station:example.org:demo:2", type: "Station", name: "Kallio", bikes: 7, status: "closed", location: { type: "Point", coordinates: [24.95, 60.18] } },
];
const NOTES: Row[] = [{ id: "urn:ngsi-ld:Note:example.org:demo:1", type: "Note", text: "check the lock" }];

// Shaped like Model Tools' draft-07 schema: the kind travels as `x-ngsi-ld-kind`.
const SCHEMA = {
  Station: {
    properties: {
      name: { type: "string" },
      bikes: { type: "integer", minimum: 0 },
      status: { type: "string", enum: ["open", "closed"] },
      location: { type: "object", "x-ngsi-ld-kind": "GeoProperty" },
    },
    required: ["name"],
  },
  Note: { properties: { text: { type: "string" } } },
} as Schema;

const ACCESS = {
  permissions: [
    { resource: { type: "*" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const },
    { resource: { type: "Station" }, actions: ["updateAttrs"], attributes: ["bikes", "status"] },
  ],
  prohibitions: [],
};

function app() {
  const client = stubClient({
    entities: [...STATIONS, ...NOTES],
    schema: SCHEMA,
    access: ACCESS,
    functions: { summary: () => ({ types: [{ type: "Note", count: 1, averages: {} }] }) },
  }, { appName: "bikes" });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

describe("template", () => {
  it("opens on an overview with a card per type and the server summary", async () => {
    app();

    expect(await screen.findByRole("heading", { name: "bikes" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Overview" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Station" })).toBeInTheDocument();
    expect(await screen.findByText("Note: 1")).toBeInTheDocument();
  });

  it("a type with a location gets a map, filters from the schema and an edit form where writes are granted", async () => {
    const client = app();
    fireEvent.click(await screen.findByRole("button", { name: "Station" }));

    const page = screen.getByRole("region", { name: "Station" });
    expect(await within(page).findByText("Kallio")).toBeInTheDocument();
    expect(within(page).getByTestId("jc-map")).toBeInTheDocument();
    expect(within(page).getByRole("combobox")).toBeInTheDocument(); // the status select

    fireEvent.click(within(page).getByText("Kallio"));
    fireEvent.click(within(page).getByRole("button", { name: "Edit" }));
    // The form reads the grant itself; Save stays disabled until it has.
    const save = within(page).getByRole("button", { name: "Save" });
    await waitFor(() => expect(save).toBeEnabled());
    fireEvent.change(within(page).getByRole("spinbutton", { name: "bikes" }), { target: { value: "9" } });
    fireEvent.click(save);

    await waitFor(() => expect(client.transport.rows().find((r) => r.id === STATIONS[1].id)?.bikes).toBe(9));
  });

  it("a type without a location or a write grant gets no map and no edit button", async () => {
    app();
    fireEvent.click(await screen.findByRole("button", { name: "Note" }));

    const page = screen.getByRole("region", { name: "Note" });
    fireEvent.click(await within(page).findByText("check the lock"));
    expect(within(page).queryByTestId("jc-map")).not.toBeInTheDocument();
    expect(within(page).getByRole("heading", { name: "check the lock" })).toBeInTheDocument();
    expect(within(page).queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
  });
});

describe("shape", () => {
  it("reads kinds from the schema and falls back to the rows without one", () => {
    expect(shapeOf(SCHEMA.Station, STATIONS)).toMatchObject({ geo: "location", numbers: ["bikes"], categories: ["status"], texts: ["name"], label: "name" });
    expect(shapeOf(undefined, STATIONS)).toMatchObject({ geo: "location", numbers: ["bikes"], label: "name" });
    expect(shapeOf(undefined, [])).toMatchObject({ attrs: [], numbers: [] });
  });

  it("builds a search, a select, a range and a date range", () => {
    const shape = { attrs: [], numbers: ["bikes"], categories: ["status", "zone", "extra"], texts: ["name"], time: "observedAt" };
    expect(filtersOf(shape).map((f) => f.kind)).toEqual(["search", "select", "select", "range", "dateRange"]);
  });
});
