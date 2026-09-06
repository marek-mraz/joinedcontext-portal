/**
 * The map is the app (T-0309, AP-38, AP-41). What matters is that a bus moving on the
 * Endpoint reaches the one GeoJSON source the map draws from, without the layer being rebuilt
 * and without the browser ever learning the Endpoint URL.
 *
 * MapLibre needs WebGL, which jsdom does not have, so the library is replaced by a double
 * that records what the app asked it to do. That is the whole contract worth asserting here:
 * one source, one `setData` per fleet.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const setData = vi.fn();
const addSource = vi.fn();
const addLayer = vi.fn();
const remove = vi.fn();
let load: (() => void) | undefined;

vi.mock("maplibre-gl", () => {
  class Map {
    constructor(public options: unknown) {}
    on(event: string, handler: () => void) {
      if (event === "load") {
        load = handler;
      }
    }
    addSource = addSource;
    addLayer = addLayer;
    getSource = () => ({ setData });
    remove = remove;
  }
  return { default: { Map } };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { App, VehicleMap } from "../src/App";
import { featureCollection, lineColor, type Vehicle } from "../src/api";

const BUS = "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-01";
const OTHER = "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-02";

function bus(id: string, longitude: number, refLine = "550"): Vehicle {
  return { id, coordinates: [longitude, 60.17], bearing: 143, speed: 8.5, refLine };
}

/** The stream, as the app's own backend serves it. */
class StubEventSource {
  static last: StubEventSource | undefined;
  closed = false;
  private listeners: Array<(event: MessageEvent<string>) => void> = [];
  constructor(readonly url: string) {
    StubEventSource.last = this;
  }
  addEventListener(_type: string, handler: (event: MessageEvent<string>) => void) {
    this.listeners.push(handler);
  }
  close() {
    this.closed = true;
  }
  /** What the server pushes when a bus moves. */
  push(vehicles: Vehicle[]) {
    const event = new MessageEvent("vehicles", { data: JSON.stringify(vehicles) });
    for (const handler of this.listeners) {
      handler(event);
    }
  }
}

beforeEach(() => {
  setData.mockClear();
  addSource.mockClear();
  addLayer.mockClear();
  remove.mockClear();
  load = undefined;
  StubEventSource.last = undefined;
  vi.stubGlobal("EventSource", StubEventSource);
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify([bus(BUS, 24.94)]), { status: 200 })),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the map source", () => {
  it("is created once, with the fleet the app already had", () => {
    render(<VehicleMap collection={featureCollection([bus(BUS, 24.94)])} />);
    load?.();

    expect(addSource).toHaveBeenCalledTimes(1);
    const [name, source] = addSource.mock.calls[0] as [string, { type: string; data: unknown }];
    expect(name).toBe("vehicles");
    expect(source.type).toBe("geojson");
    expect(addLayer).toHaveBeenCalledTimes(1);
  });

  it("takes a new fleet through setData rather than a second source", () => {
    const { rerender } = render(<VehicleMap collection={featureCollection([bus(BUS, 24.94)])} />);
    load?.();
    setData.mockClear();

    rerender(<VehicleMap collection={featureCollection([bus(BUS, 24.99)])} />);

    expect(setData).toHaveBeenCalledTimes(1);
    const collection = setData.mock.calls[0][0] as ReturnType<typeof featureCollection>;
    expect(collection.features[0].geometry.coordinates).toEqual([24.99, 60.17]);
    expect(addSource).toHaveBeenCalledTimes(1);
    expect(addLayer).toHaveBeenCalledTimes(1);
  });

  it("is not written to before the map has loaded", () => {
    const { rerender } = render(<VehicleMap collection={featureCollection([bus(BUS, 24.94)])} />);
    rerender(<VehicleMap collection={featureCollection([bus(BUS, 24.99)])} />);

    expect(setData).not.toHaveBeenCalled();
  });
});

describe("the app", () => {
  it("moves a bus on the map when the stream says it moved", async () => {
    render(<App />);
    load?.();
    await waitFor(() => expect(StubEventSource.last).toBeDefined());
    await waitFor(() => expect(setData).toHaveBeenCalled());
    setData.mockClear();

    StubEventSource.last?.push([bus(BUS, 25.01)]);

    await waitFor(() => expect(setData).toHaveBeenCalled());
    const collection = setData.mock.lastCall?.[0] as ReturnType<typeof featureCollection>;
    expect(collection.features).toHaveLength(1);
    expect(collection.features[0].geometry.coordinates).toEqual([25.01, 60.17]);
  });

  it("merges a partial update instead of dropping the buses it does not mention", async () => {
    render(<App />);
    load?.();
    await waitFor(() => expect(StubEventSource.last).toBeDefined());

    StubEventSource.last?.push([bus(BUS, 24.94), bus(OTHER, 24.8, "551")]);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("2 buses"));

    StubEventSource.last?.push([bus(OTHER, 24.85, "551")]);

    await waitFor(() => {
      const collection = setData.mock.lastCall?.[0] as ReturnType<typeof featureCollection>;
      expect(collection.features).toHaveLength(2);
    });
    expect(screen.getByRole("status")).toHaveTextContent("2 buses, updating live");
  });

  it("subscribes to its own backend and never to the endpoint", async () => {
    render(<App />);
    await waitFor(() => expect(StubEventSource.last).toBeDefined());

    expect(StubEventSource.last?.url).toBe("/api/stream");
    const calls = (globalThis.fetch as unknown as { mock: { calls: string[][] } }).mock.calls;
    for (const [url] of calls) {
      expect(url).toMatch(/^\/api\//);
    }
  });

  it("closes the stream when the map goes away", async () => {
    const { unmount } = render(<App />);
    await waitFor(() => expect(StubEventSource.last).toBeDefined());

    unmount();

    expect(StubEventSource.last?.closed).toBe(true);
  });

  it("says it is waiting rather than showing an empty map as a finished one", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("{}", { status: 503 })),
    );
    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("Waiting for the first positions"),
    );
  });
});

describe("the line colour", () => {
  it("is the same for two buses on one line and different across lines", () => {
    expect(lineColor("550")).toBe(lineColor("550"));
    expect(lineColor("550")).not.toBe(lineColor("551"));
  });

  it("falls back to one neutral colour when the policy hides the line", () => {
    expect(lineColor(undefined)).toBe("#64748b");
  });
});
