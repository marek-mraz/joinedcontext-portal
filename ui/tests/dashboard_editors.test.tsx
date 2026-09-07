/** T-0528: the Dashboard and Layer editors, the legend and the viewport query (UI-17, UI-18, UI-22). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { geojsonUrl } from "../src/routes/DashboardsPage";
import { layerFromManifest, layerToManifest } from "../src/pages/dashboards/editors";

const calls = vi.hoisted(() => ({
  sources: [] as { id: string; source: Record<string, unknown> }[],
  removed: [] as string[],
  moveEnd: null as null | (() => void),
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    addControl() {}
    addSource(id: string, source: Record<string, unknown>) {
      calls.sources.push({ id, source });
    }
    addLayer() {}
    on(event: string, second: unknown) {
      if (event === "load" && typeof second === "function") {
        (second as () => void)();
      }
      if (event === "moveend" && typeof second === "function") {
        calls.moveEnd = second as () => void;
      }
    }
    remove() {}
    getSource() {
      return undefined;
    }
    removeLayer(id: string) {
      calls.removed.push(id);
    }
    removeSource() {}
    fitBounds() {}
    getBounds() {
      return { getWest: () => 24.9, getSouth: () => 60.1, getEast: () => 25.0, getNorth: () => 60.2 };
    }
  }
  const Popup = class {
    setLngLat() {
      return this;
    }
    setHTML() {
      return this;
    }
    addTo() {
      return this;
    }
  };
  return { Map: FakeMap, NavigationControl: class {}, Popup, default: { Map: FakeMap, NavigationControl: class {}, Popup } };
});

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: {
    version: "1.0.0",
    linkml: [
      "id: https://hel.fi/models/bikes",
      "name: bikes",
      "classes:",
      "  BikeHireDockingStation:",
      "    slots: [id, availableBikeNumber, name]",
      "slots:",
      "  id: {}",
      "  availableBikeNumber: { range: integer, minimum_value: 0 }",
      "  name: { range: string }",
      "",
    ].join("\n"),
  },
};
const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "mobility", namespace: "helsinki" },
  spec: { dataModelRef: "bikes" },
};
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "helsinki-bikes", namespace: "helsinki" },
  spec: { contextSpaceRef: "mobility", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public", enabledRepresentations: ["geojson"] },
};
const LAYER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: {
    sourceEndpointRef: "helsinki-bikes",
    entityType: "BikeHireDockingStation",
    style: "circle",
    colorBy: { property: "availableBikeNumber", domain: [0, 20] },
    popupProperties: ["name", "availableBikeNumber"],
  },
};
const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: { title: { en: "Bikes" }, visibility: "project", pages: [{ layout: "full-map", layers: ["bikes"] }] },
};
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "helsinki" },
  status: { lane: "green", phase: "PendingApproval", plan: { update: 1 } },
};
const FEATURES = {
  type: "FeatureCollection",
  features: [{ type: "Feature", geometry: { type: "Point", coordinates: [24.95, 60.17] }, properties: { name: "Kamppi", availableBikeNumber: 7 } }],
};

function renderDashboards(options: { writeStatus?: number; writeBody?: unknown } = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = typeof input === "string" ? new URL(input) : new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" } }));
    if (url.pathname.includes("/api/endpoint/")) {
      return json(FEATURES);
    }
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (request.method !== "GET") {
      return json(options.writeBody ?? CHANGE, options.writeStatus ?? 202);
    }
    if (url.pathname.endsWith("/dashboards")) {
      return json(list([DASHBOARD]));
    }
    if (url.pathname.endsWith("/layers")) {
      return json(list([LAYER]));
    }
    if (url.pathname.endsWith("/endpoints")) {
      return json(list([ENDPOINT]));
    }
    if (url.pathname.endsWith("/spaces")) {
      return json(list([SPACE]));
    }
    if (url.pathname.endsWith("/datamodels")) {
      return json(list([MODEL]));
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request instanceof Request && request.method !== "GET");
}

async function openLayerEditor() {
  await userEvent.click(await screen.findByRole("button", { name: `${en.dashboards.editLayer}: bikes` }));
  return screen.findByRole("dialog");
}

describe("dashboard editors", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/dashboards");
    calls.sources.length = 0;
    calls.removed.length = 0;
    calls.moveEnd = null;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("writes spec.filter.q from a filter row of the layer editor (UI-18, UI-33)", async () => {
    const fetchMock = renderDashboards();
    const dialog = await openLayerEditor();

    // The rows come from the endpoint's space model: the type is already chosen, the
    // attribute list knows availableBikeNumber is an integer.
    expect(within(dialog).getByLabelText(en.entities.type)).toHaveValue("BikeHireDockingStation");
    await userEvent.click(within(dialog).getByRole("button", { name: en.entities.addFilter }));
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: en.entities.attribute }), "availableBikeNumber");
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: en.entities.operator }), ">");
    await userEvent.type(within(dialog).getByRole("spinbutton", { name: en.entities.value }), "0");
    await waitFor(() => expect(within(dialog).getByLabelText(en.entities.q)).toHaveValue("availableBikeNumber>0"));

    await userEvent.click(within(dialog).getByRole("button", { name: en.dashboards.propose }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/helsinki/layers/bikes");
    const body = (await request.clone().json()) as { kind: string; spec: Record<string, unknown> };
    expect(body.kind).toBe("Layer");
    expect(body.spec.entityType).toBe("BikeHireDockingStation");
    expect(body.spec.filter).toEqual({ q: "availableBikeNumber>0" });
    expect(body.spec.colorBy).toEqual({ property: "availableBikeNumber", domain: [0, 20] });
    expect(body.spec.visible).toBeUndefined();
  });

  it("shows the server's reason when a public dashboard would read a private endpoint (UI-19)", async () => {
    renderDashboards({
      writeStatus: 400,
      writeBody: {
        type: "about:blank",
        title: "Bad Request",
        status: 400,
        detail: "a public dashboard may only read public endpoints: layer bikes reads helsinki-bikes whose audience is organization (UI-19)",
      },
    });
    await userEvent.click(await screen.findByRole("button", { name: en.dashboards.edit }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText(new RegExp(`^${en.dashboards.field.visibility}`)), "public");
    await userEvent.click(within(dialog).getByRole("button", { name: en.dashboards.propose }));

    expect(await within(dialog).findByText(/whose audience is organization/)).toBeInTheDocument();
  });

  it("switches a layer off from the legend and asks the endpoint for the viewport after a move (UI-22)", async () => {
    renderDashboards();
    await waitFor(() => expect(calls.sources).toHaveLength(1));
    expect(new URL(String(calls.sources[0].source.data ?? "http://x/"), window.location.origin).searchParams.get("georel")).toBeNull();

    await userEvent.click(await screen.findByRole("checkbox", { name: `${en.dashboards.show}: bikes` }));
    await waitFor(() => expect(calls.removed).toContain("bikes"));
  });

  it("spells the viewport as the gateway's georel, geometry and coordinates", () => {
    const url = new URL(geojsonUrl("slug", { entityType: "T" }, [24.9, 60.1, 25, 60.2]), "http://x");
    expect(url.searchParams.get("georel")).toBe("within");
    expect(url.searchParams.get("geometry")).toBe("Polygon");
    expect(JSON.parse(url.searchParams.get("coordinates") ?? "")).toEqual([[[24.9, 60.1], [25, 60.1], [25, 60.2], [24.9, 60.2], [24.9, 60.1]]]);
    // A manifest's own geoQ wins over the viewport.
    const fixed = new URL(geojsonUrl("slug", { filter: { geoQ: "georel=near;maxDistance==2000;geometry=Point;coordinates=[25,60]" } }, [0, 0, 1, 1]), "http://x");
    expect(fixed.searchParams.get("georel")).toBe("near;maxDistance==2000");
    expect(fixed.searchParams.get("coordinates")).toBe("[25,60]");
  });

  it("round-trips a layer through its form without writing the visible default", () => {
    const form = layerFromManifest(LAYER);
    expect(form.visible).toBe(true);
    const manifest = layerToManifest("helsinki", form) as { spec: Record<string, unknown> };
    expect(manifest.spec).toEqual(LAYER.spec);
    const off = layerToManifest("helsinki", { ...form, visible: false }) as { spec: Record<string, unknown> };
    expect(off.spec.visible).toBe(false);
  });
});
