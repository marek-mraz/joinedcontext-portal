import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import * as echarts from "echarts";
import App from "../template/src/App";
import type { Row } from "../src/ngsi";
import { JcProvider } from "../src/sdk/hooks";
import { stubClient } from "../src/sdk/testing";
import type { Schema } from "../src/write";
// The artifact Model Tools committed for the helsinki air-quality model (platform examples/datamodels).
import helsinki from "./fixtures/helsinki-air-quality.schema.json";

vi.mock("maplibre-gl", () => ({ Map: class { on = vi.fn(); remove = vi.fn(); }, setWorkerUrl: vi.fn() }));
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

// Rows as the SDK flattens them: the language map is one text, the relationship its URN.
const READINGS: Row[] = [
  ["Kallio", 7.5, "2026-09-01T08:00:00Z", [24.95, 60.18]],
  ["Mäkelänkatu", 12.1, "2026-09-02T08:00:00Z", [24.94, 60.2]],
  ["Leppävaara", 4.2, "2026-09-03T08:00:00Z", [24.81, 60.22]],
].map(([name, pm25, date, coordinates], i) => ({
  id: `urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:${i + 1}`,
  type: "AirQualityObserved",
  stationName: name as string,
  pm25: pm25 as number,
  temperature: 14 + i,
  dateObserved: date as string,
  refStation: `urn:ngsi-ld:Station:hel.fi:air-quality:${i + 1}`,
  location: { type: "Point", coordinates },
}));

describe("the template on the helsinki air-quality endpoint", () => {
  it("renders the type page with map, time series, table and label from the committed schema alone", async () => {
    const client = stubClient({ entities: READINGS, schema: helsinki.definitions as unknown as Schema, functions: { summary: () => ({ types: [] }) } });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );

    fireEvent.click(await screen.findByRole("button", { name: "AirQualityObserved" }));
    const page = screen.getByRole("region", { name: "AirQualityObserved" });

    expect(await within(page).findByText("Mäkelänkatu")).toBeInTheDocument();
    expect(within(page).getByTestId("jc-map")).toBeInTheDocument();
    expect(within(page).getByText("pm25 over time")).toBeInTheDocument();
    expect(within(page).getByRole("spinbutton", { name: /pm25 from/i })).toBeInTheDocument();
    expect(echarts.init).toHaveBeenCalled();

    fireEvent.click(within(page).getByText("Leppävaara"));
    expect(within(page).getByRole("heading", { name: "Leppävaara" })).toBeInTheDocument();

    // Optional numbers are ["number", "null"] and the language map is an object in the schema.
    fireEvent.click(within(page).getByRole("button", { name: "Edit" }));
    expect(within(page).getByRole("spinbutton", { name: "temperature" })).toHaveValue(16);
    expect(within(page).getByRole("textbox", { name: "stationName" })).toHaveValue("Leppävaara");
    expect(within(page).getByRole("textbox", { name: "location" })).toHaveAttribute("placeholder", "lat, lon");
  });
});
