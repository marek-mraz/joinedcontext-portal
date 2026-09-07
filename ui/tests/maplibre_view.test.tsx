import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

// jsdom has no WebGL, so the map itself is a recorder: the assertions are about what the
// view asks MapLibre for — the sources, the layers, the click handlers.
const calls = vi.hoisted(() => ({
  constructed: [] as Record<string, unknown>[],
  sources: [] as { id: string; source: Record<string, unknown> }[],
  layers: [] as Record<string, unknown>[],
  clickHandlers: new Map<string, (event: unknown) => void>(),
  popups: [] as { html: string }[],
  throwOnConstruct: false,
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor(options: Record<string, unknown>) {
      if (calls.throwOnConstruct) {
        throw new Error("WebGL is not supported");
      }
      calls.constructed.push(options);
    }
    addControl() {}
    addSource(id: string, source: Record<string, unknown>) {
      calls.sources.push({ id, source });
    }
    addLayer(layer: Record<string, unknown>) {
      calls.layers.push(layer);
    }
    on(event: string, second: unknown, third?: unknown) {
      if (event === "load" && typeof second === "function") {
        (second as () => void)();
        return;
      }
      if (event === "click" && typeof second === "string" && typeof third === "function") {
        calls.clickHandlers.set(second, third as (event: unknown) => void);
      }
    }
    remove() {}
    getSource() {
      return undefined;
    }
    removeLayer() {}
    removeSource() {}
    fitBounds() {}
    getBounds() {
      return { getWest: () => 0, getSouth: () => 0, getEast: () => 1, getNorth: () => 1 };
    }
  }
  class FakePopup {
    setLngLat() {
      return this;
    }
    setHTML(html: string) {
      calls.popups.push({ html });
      return this;
    }
    addTo() {
      return this;
    }
  }
  return {
    Map: FakeMap,
    NavigationControl: class {},
    Popup: FakePopup,
    default: { Map: FakeMap, NavigationControl: class {}, Popup: FakePopup },
  };
});

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

function dashboard(visibility: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Dashboard",
    metadata: { name: "ovzdusie-prehlad", namespace: "banskabystrica" },
    spec: {
      title: { sk: "Kvalita ovzdušia", en: "Air quality" },
      visibility,
      pages: [{ title: "Mapa staníc", layout: "full-map", layers: ["air-quality-stations"] }],
    },
    status: { phase: "Live" },
  };
}

const LAYER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: "air-quality-stations", namespace: "banskabystrica" },
  spec: {
    sourceEndpointRef: "public-air",
    entityType: "AirQualityObserved",
    style: "circle",
    filter: { q: "pm10>0", scopeQ: "/geo/SK/BB/#" },
    colorBy: { property: "pm10", palette: "YlOrRd", domain: [0, 100] },
    sizeBy: { property: "pm10", range: [4, 18] },
    popupProperties: ["stationName", "pm10"],
  },
  status: { phase: "Live" },
};

function endpoint(audience: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "banskabystrica" },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
      audience,
      enabledRepresentations: ["ngsi-ld", "geojson"],
    },
    status: { phase: "Live" },
  };
}

function renderDashboards(options: { visibility?: string; audience?: string } = {}) {
  const list = (items: unknown[]) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "List",
    items,
  });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : (input as Request).url;
      const path = new URL(url, window.location.origin).pathname;
      let body: unknown = list([]);
      if (path.endsWith("/auth/me")) {
        body = IDENTITY;
      } else if (path.endsWith("/dashboards")) {
        body = list([dashboard(options.visibility ?? "project")]);
      } else if (path.endsWith("/layers")) {
        body = list([LAYER]);
      } else if (path.endsWith("/endpoints")) {
        body = list([endpoint(options.audience ?? "public")]);
      }
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }),
  );

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("map dashboard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/dashboards");
    calls.constructed.length = 0;
    calls.sources.length = 0;
    calls.layers.length = 0;
    calls.popups.length = 0;
    calls.clickHandlers.clear();
    calls.throwOnConstruct = false;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("mounts a map container labelled with the dashboard title", async () => {
    renderDashboards();
    expect(
      await screen.findByRole("application", { name: "Air quality" }),
    ).toBeInTheDocument();
    await waitFor(() => expect(calls.constructed).toHaveLength(1));
  });

  it("loads the layer as GeoJSON from the endpoint, filters included (UI-22)", async () => {
    renderDashboards();

    await waitFor(() => expect(calls.sources).toHaveLength(1));
    expect(calls.sources[0].id).toBe("air-quality-stations");
    expect(calls.sources[0].source).toMatchObject({ type: "geojson" });
    const url = new URL(String(calls.sources[0].source.data), window.location.origin);
    expect(url.pathname).toBe("/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/file.geojson");
    expect(url.searchParams.get("type")).toBe("AirQualityObserved");
    expect(url.searchParams.get("q")).toBe("pm10>0");
    expect(url.searchParams.get("scopeQ")).toBe("/geo/SK/BB/#");
  });

  it("styles the points from colorBy and sizeBy", async () => {
    renderDashboards();

    await waitFor(() => expect(calls.layers).toHaveLength(1));
    const paint = calls.layers[0].paint as Record<string, unknown>;
    expect(calls.layers[0]).toMatchObject({ id: "air-quality-stations", type: "circle" });
    expect(JSON.stringify(paint["circle-color"])).toContain("pm10");
    expect(JSON.stringify(paint["circle-radius"])).toContain("18");
  });

  it("opens a popup with the layer's properties on a feature click", async () => {
    renderDashboards();

    await waitFor(() => expect(calls.clickHandlers.size).toBe(1));
    calls.clickHandlers.get("air-quality-stations")?.({
      lngLat: { lng: 19.1, lat: 48.7 },
      features: [{ properties: { stationName: "Štiavničky", pm10: 42, secret: "hidden" } }],
    });

    expect(calls.popups).toHaveLength(1);
    expect(calls.popups[0].html).toContain("Štiavničky");
    expect(calls.popups[0].html).toContain("42");
    // Only the properties the Layer manifest names are shown.
    expect(calls.popups[0].html).not.toContain("hidden");
  });

  it("refuses a private endpoint behind a public dashboard (UI-19)", async () => {
    renderDashboards({ visibility: "public", audience: "organization" });

    expect(await screen.findByText(/only read public endpoints/)).toBeInTheDocument();
    await waitFor(() => expect(calls.constructed).toHaveLength(1));
    expect(calls.sources).toHaveLength(0);
  });

  it("says so instead of blanking the page when WebGL is missing", async () => {
    calls.throwOnConstruct = true;
    renderDashboards();

    expect(await screen.findByText(en.dashboards.mapUnavailable)).toBeInTheDocument();
  });

  it("lists the layers with what colours them", async () => {
    renderDashboards();

    const legend = await screen.findByText("air-quality-stations");
    expect(within(legend.closest("li") as HTMLElement).getByText(/pm10/)).toBeInTheDocument();
  });
});
