import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-0497: a pipeline created from the form, clicking real controls (PL-04, PL-39, UI-01, AP-13).
// Same shape as the managers journey: `vite preview` has no portal API behind it, so the API
// is answered in the browser and everything above it — the dialog, the selects, Monaco — is
// the real thing. The root route goes to the first project `GET /api/v1/projects` lists.

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
  metadata: { name: "chg-77aa11bb", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

async function stubApi(page: Page): Promise<{ writes: string[] }> {
  const writes: string[] = [];
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
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
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
  return { writes };
}

test.describe("pipeline editor", () => {
  test("creates a pipeline from the form and proposes it as a change", async ({ page }) => {
    const { writes } = await stubApi(page);

    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).click();
    const dialog = page.getByRole("dialog");

    await dialog.getByLabel(/^Name/).fill("aq-ingest");
    await dialog.getByLabel(/^Execution/).selectOption("resident");
    await dialog.getByLabel(/^Data source/).selectOption("mqtt-city");
    await dialog
      .getByLabel(/^Target endpoint/)
      .selectOption("urn:ngsi-ld:Endpoint:hel.fi:air:public-air");
    await dialog.getByLabel(/^Kind/).selectOption("bloblang");
    await expect(dialog.getByText(/bento\.yaml beside this manifest/)).toBeVisible();

    // The YAML view shows the manifest the form wrote, in the real editor.
    await dialog.getByRole("tab", { name: "YAML" }).click();
    await expect(dialog.locator(".monaco-editor")).toBeVisible();
    await expect(dialog.locator(".view-lines")).toContainText("aq-ingest");
    await expect(dialog.locator(".view-lines")).toContainText("targetEndpoint");
    await dialog.getByRole("tab", { name: "Form" }).click();
    await expect(dialog.getByLabel(/^Name/)).toHaveValue("aq-ingest");

    await dialog.getByRole("button", { name: "Propose change" }).click();

    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    await expect(page.getByRole("link", { name: "Review it in Approvals" })).toBeVisible();

    expect(writes).toHaveLength(1);
    const [method, path, body] = writes[0].split(" ", 3);
    expect(`${method} ${path}`).toBe("POST /api/v1/projects/helsinki/pipelines");
    expect(JSON.parse(writes[0].slice(method.length + path.length + 2))).toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "aq-ingest", namespace: "helsinki" },
      spec: {
        class: "resident",
        source: { dataSourceRef: { kind: "DataSource", name: "mqtt-city" } },
        compute: { kind: "bloblang" },
        targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:public-air",
      },
    });
    void body;
  });
});
