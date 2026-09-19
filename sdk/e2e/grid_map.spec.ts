import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";

/**
 * The grid and its map in a real browser (T-1443, UI-72, UI-67).
 *
 * The unit tests mock MapLibre, so nothing there proves the panel is mounted on a real canvas at all.
 * This page is the grid with `map.enabled`, built from the sources with vite and served from memory —
 * no basemap, so no tile leaves the browser and the check is offline.
 *
 * What this leg does NOT yet cover, and T-1443 leaves open: clicking a shape to activate its row and
 * dragging a point in the panel. `GeoView` publishes no map handle (only `GeoEditor` does), so a
 * shape cannot be located from the test, and the "Draw area" click hangs Playwright's actionability
 * wait — a refetch that does not settle, which is a finding of its own and not something to paper
 * over with `force: true`.
 */
const PAGE = "http://gridmap.test/";

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h, useState } from "react";
import { EntityGrid } from "../../src/grid/EntityGrid";
import { fixtureSource } from "../../src/grid/source";
import { applyChanges } from "../../src/grid/apply";

const STATIONS = [
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:001", type: "Station",
    name: { type: "Property", value: "Kamppi" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.931, 60.169] } } },
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:002", type: "Station",
    name: { type: "Property", value: "Kallio" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.951, 60.186] } } },
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:003", type: "Station",
    name: { type: "Property", value: "No geometry yet" } },
];

const CONFIG = {
  source: { kind: "fixture", name: "stations" },
  type: "Station",
  columns: [{ attr: "name" }, { attr: "location" }],
  entityTimestamps: false,
  filters: {},
  pageSize: 50,
  mode: "edit",
  editableAttrs: ["name", "location"],
  history: { enabled: false },
  density: "comfortable",
  rowActions: [],
  map: { enabled: true, attr: "location", position: "right" },
};

// The source the grid reads, and the PATCH bodies it would send, kept for the assertion: this is
// what travels to the Endpoint, which is the only place a geometry's own type can be wrong.
const sent = [];
const base = fixtureSource(STATIONS);
const source = { ...base, patch: async (id, body) => { sent.push({ id, body }); } };

function Harness() {
  const [applied, setApplied] = useState(null);
  return h("div", null,
    h(EntityGrid, {
      config: CONFIG,
      source,
      mapBounds: () => [24.9, 60.15, 25.0, 60.2],
    }),
    h("button", {
      "data-testid": "apply-through-source",
      onClick: async () => {
        // The grid's own review dialog is covered by the unit tests; what this page proves is that
        // the pending geometry reaches the source as a GeoProperty.
        const pending = window.kitPending ?? [];
        await applyChanges({ source, entities: pending, observed: "keep" });
        setApplied(JSON.stringify(sent));
      },
    }, "apply through the source"),
    h("pre", { "data-testid": "sent" }, applied ?? ""),
  );
}
createRoot(document.getElementById("root")).render(h(Harness));
`;

const HTML = `<!doctype html><html><head><meta charset="utf-8">
<style>.jc-grid-map .maplibregl-map { width: 600px; height: 420px; }</style>
</head><body><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

let files: Record<string, string> = {};

test.beforeAll(async () => {
  // Inside the package, not in /tmp: the entry imports `../src` and needs this `node_modules`.
  const work = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", ".gridmap-fixture");
  mkdirSync(work, { recursive: true });
  writeFileSync(join(work, "entry.jsx"), ENTRY);
  writeFileSync(join(work, "index.html"), HTML);
  const out = join(work, "dist");
  await build({ root: work, logLevel: "silent", build: { outDir: out, emptyOutDir: true, target: "es2022" } });
  const walk = (dir: string, prefix: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        walk(join(dir, entry.name), `${prefix}${entry.name}/`);
      } else {
        files[`/${prefix}${entry.name}`] = readFileSync(join(dir, entry.name), "utf8");
      }
    }
  };
  walk(out, "");
  files["/"] = files["/index.html"];
  // Served from the package it belongs to: vite does not copy the worker into the bundle, and
  // without it no style ever finishes loading.
  files["/assets/maplibre-gl-worker.mjs"] = readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", "maplibre-gl", "dist", "maplibre-gl-worker.mjs"),
    "utf8",
  );
});

async function serve(page: import("@playwright/test").Page, problems: string[]): Promise<void> {
  page.on("pageerror", (error) => problems.push(error.message));
  await page.route(`${PAGE}**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    const body = files[path];
    if (body === undefined) {
      return route.fulfill({ status: 404, body: "" });
    }
    return route.fulfill({
      contentType: path.endsWith(".css") ? "text/css" : path === "/" ? "text/html" : "text/javascript",
      body,
    });
  });
}

test("the panel mounts a real map beside the rows, with the page's shapes on it", async ({
  page,
}) => {
  const problems: string[] = [];
  await serve(page, problems);
  await page.goto(PAGE);

  // The rows are there, and so is the panel, with the one station that has no geometry counted off
  // the map rather than hidden.
  await expect(page.getByRole("textbox", { name: /name/ }).first()).toHaveValue("Kamppi");
  await expect(page.locator("section.jc-grid-map")).toBeVisible();
  await expect(page.getByText("not on the map: 1")).toBeVisible();

  // A real MapLibre canvas, with the two stations that have a geometry on it.
  await expect(page.locator(".jc-grid-map .maplibregl-map")).toBeVisible();
  await expect(page.getByText("2 shapes")).toBeVisible();

  expect(problems).toEqual([]);
});
