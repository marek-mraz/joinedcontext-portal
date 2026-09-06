import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// The basemap style is stubbed as well as the API: a journey must not depend on a tile
// service being up, and an empty style renders a real MapLibre canvas all the same.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "ovzdusie-prehlad", namespace: "banskabystrica" },
  spec: {
    title: { sk: "Kvalita ovzdušia", en: "Air quality" },
    visibility: "public",
    pages: [{ title: "Mapa staníc", layout: "full-map", layers: ["air-quality-stations"] }],
  },
  status: { phase: "Live" },
};

const LAYER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: "air-quality-stations", namespace: "banskabystrica" },
  spec: {
    sourceEndpointRef: "public-air",
    entityType: "AirQualityObserved",
    style: "circle",
    colorBy: { property: "pm10", palette: "YlOrRd", domain: [0, 100] },
    sizeBy: { property: "pm10", range: [4, 18] },
    popupProperties: ["stationName", "pm10"],
  },
  status: { phase: "Live" },
};

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "public-air", namespace: "banskabystrica" },
  spec: {
    contextSpaceRef: "ovzdusie",
    slug: SLUG,
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "geojson"],
  },
  status: { phase: "Live" },
};

const FEATURES = {
  type: "FeatureCollection",
  features: [
    {
      type: "Feature",
      geometry: { type: "Point", coordinates: [19.146, 48.736] },
      properties: { stationName: "Štiavničky", pm10: 42 },
    },
  ],
};

test.describe("map dashboard", () => {
  test("renders the air quality dashboard from its manifests", async ({ page }) => {
    const geojsonRequests: string[] = [];

    await page.route("https://demotiles.maplibre.org/**", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ version: 8, sources: {}, layers: [] }),
      }),
    );
    await page.route("**/api/endpoint/**", (route) => {
      geojsonRequests.push(new URL(route.request().url()).pathname);
      return route.fulfill({ contentType: "application/json", body: JSON.stringify(FEATURES) });
    });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      const list = (items: unknown[]) => ({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items,
      });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/dashboards")) return json(list([DASHBOARD]));
      if (path.endsWith("/layers")) return json(list([LAYER]));
      if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
      return json(list([]));
    });

    await page.goto("/projects/banskabystrica/dashboards?lang=en");

    await expect(page.getByRole("heading", { name: "Air quality" })).toBeVisible();
    await expect(page.getByText("air-quality-stations")).toBeVisible();
    await expect(page.getByText(/coloured by pm10/)).toBeVisible();

    // Either the canvas is up or the page said WebGL is missing; a blank map is not allowed.
    const map = page.getByRole("application", { name: "Air quality" });
    const missing = page.getByText("The map needs WebGL");
    await expect(map.or(missing).first()).toBeVisible();

    if (await map.isVisible()) {
      await expect(map.locator("canvas")).toBeVisible();
    }
    // Whether MapLibre fetches the features from its worker is MapLibre's business; that the
    // view hands it the right Endpoint URL is asserted in tests/maplibre_view.test.tsx.
    expect(geojsonRequests.every((path) => path.startsWith(`/api/endpoint/${SLUG}/`))).toBe(true);
  });

  test("the dashboards route has no axe violations", async ({ page }) => {
    await page.route("https://demotiles.maplibre.org/**", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ version: 8, sources: {}, layers: [] }),
      }),
    );
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/dashboards"))
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [DASHBOARD] });
      if (path.endsWith("/layers"))
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [LAYER] });
      if (path.endsWith("/endpoints"))
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [ENDPOINT] });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });

    await page.goto("/projects/banskabystrica/dashboards?lang=sk");
    await expect(page.getByRole("heading", { name: "Kvalita ovzdušia" })).toBeVisible();

    expect(await axeViolations(page)).toEqual([]);
  });
});
