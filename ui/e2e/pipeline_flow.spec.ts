import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-0643: Pipeline flow canvas e2e journey (UI-49, PL-43, PL-44).
// Validates dragging palette stage onto SVG canvas, inline bloblang editing,
// stage test trace mapping (error state on compute node), fixing mapping and reaching green state.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const BRANDING = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "Helsinki",
  organisation: "City of Helsinki",
  orgDomain: "hel.fi",
  domain: "portal.hel.fi",
  contactEmail: "data@hel.fi",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const DATASOURCES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "mqtt-city", namespace: "helsinki" },
    spec: { type: "mqtt", mqtt: { urls: ["tls://mqtt.hel.fi:8883"], topics: ["aq/#"] } },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "helsinki" },
    spec: { contextSpaceRef: "air", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
  },
]);

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-flow-1234", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

async function stubApi(page: Page, getTestTrace: () => unknown) {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/branding")) {
      return json(BRANDING);
    }
    if (path.endsWith("/pipelines/test")) {
      return json(getTestTrace());
    }
    if (request.method() !== "GET") {
      return json(CHANGE, 202);
    }
    if (path === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (path.endsWith("/datasources")) {
      return json(DATASOURCES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    return json(list([]));
  });
}

test.describe("pipeline flow canvas", () => {
  test("drags stage to flow canvas, reflects test trace error and green states", async ({ page }) => {
    let currentTrace: unknown = {
      input: { events: 1, bytes: 64, sample: { id: "1" } },
      mapping: [],
      validation: [],
      errors: [{ stage: "mapping", line: 1, message: "unexpected token" }],
    };

    await stubApi(page, () => currentTrace);

    await page.goto("/projects/helsinki/pipelines?lang=en");
    // An empty list offers the action twice: in the page header and in the empty state
    // itself (UI-01). The header's is the one a person with pipelines also has.
    await page.getByRole("button", { name: "New pipeline" }).first().click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();

    const paletteBloblang = page.getByTestId("palette-bloblang");
    const canvas = page.getByTestId("flow-canvas");

    await expect(paletteBloblang).toBeVisible();
    await expect(canvas).toBeVisible();

    // Drag compute stage from palette onto SVG canvas
    await paletteBloblang.dragTo(canvas);

    const computeNode = page.getByTestId("flow-node-compute");
    await expect(computeNode).toBeVisible();

    // Provide a sample file for testing
    await page.locator('input[type="file"]').setInputFiles({
      name: "sample.json",
      mimeType: "application/json",
      buffer: Buffer.from(JSON.stringify([{ id: "sensor-01", pm10: 42 }])),
    });

    // Enter bad mapping in flow bloblang editor
    const bloblangArea = page.getByTestId("flow-bloblang");
    await expect(bloblangArea).toBeVisible();
    await bloblangArea.fill("root = invalid_syntax {");

    // Run studio test
    const testButton = page.getByRole("button", { name: "Test mapping" });
    await expect(testButton).toBeEnabled();
    await testButton.click();

    // Compute node turns red (error state)
    await expect(computeNode).toHaveAttribute("data-state", "error");

    // Correct mapping and mock green trace
    currentTrace = {
      input: { events: 1, bytes: 64, sample: { id: "sensor-01", pm10: 42 } },
      mapping: [{ id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:sensor-01", type: "AirQualityObserved" }],
      validation: [{ index: 0, ok: true, problems: [] }],
      errors: [],
    };

    await bloblangArea.fill('root = this\nroot.type = "AirQualityObserved"');
    await testButton.click();

    // Compute node reflects green ok state
    await expect(computeNode).toHaveAttribute("data-state", "ok");
  });
});
