import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

// T-0307, UI-28: the federation graph in the built bundle, with the API answered in the browser
// the way the other journeys answer it (`vite preview` has no Portal behind it).

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };

const GRAPH = {
  nodes: [
    { id: "ContextSpace/helsinki", kind: "ContextSpace", name: "helsinki", health: "ok", phase: "Live" },
    { id: "Endpoint/helsinki-all", kind: "Endpoint", name: "helsinki-all", health: "ok", phase: "Live" },
    { id: "Endpoint/helsinki-bikes", kind: "Endpoint", name: "helsinki-bikes", health: "unknown", phase: "Deploying" },
    { id: "Pipeline/bikes", kind: "Pipeline", name: "bikes", health: "degraded", phase: "Error" },
    {
      id: "CkanInstance/hel-fi",
      kind: "CkanInstance",
      name: "hel-fi",
      title: { en: "Helsinki Region Context catalogue" },
      health: "ok",
      phase: "Live",
    },
  ],
  edges: [
    { from: "Endpoint/helsinki-all", to: "ContextSpace/helsinki", kind: "serves", manifest: "Endpoint/helsinki-all" },
    { from: "Endpoint/helsinki-bikes", to: "ContextSpace/helsinki", kind: "serves", manifest: "Endpoint/helsinki-bikes" },
    { from: "Endpoint/helsinki-all", to: "CkanInstance/hel-fi", kind: "publishes", manifest: "Endpoint/helsinki-all" },
    { from: "Pipeline/bikes", to: "Endpoint/helsinki-all", kind: "feeds", manifest: "Pipeline/bikes" },
  ],
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path === "/api/v1/projects") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    }
    if (path.endsWith("/federation-graph")) {
      return json(GRAPH);
    }
    return json(LIST);
  });
}

async function viewBox(graph: Locator): Promise<number[]> {
  return ((await graph.getAttribute("viewBox")) ?? "").split(" ").map(Number);
}

test.describe("topology graph", () => {
  test("pans, zooms, filters and opens a node's card", async ({ page }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/federation?lang=en");

    const graph = page.getByRole("group", { name: "Federation graph" });
    await expect(graph).toBeVisible();
    await expect(graph).toHaveAttribute("viewBox", "0 0 100 100");

    // Zoom: the window shrinks about the middle.
    await page.getByRole("button", { name: "Zoom in" }).click();
    const [x, y, width] = await viewBox(graph);
    expect(width).toBeCloseTo(100 / 1.5);
    expect(x + width / 2).toBeCloseTo(50);
    expect(y + width / 2).toBeCloseTo(50);

    // Pan: dragging the background moves the window the other way.
    const box = await graph.boundingBox();
    if (!box) {
      throw new Error("the graph has no box");
    }
    await page.mouse.move(box.x + 10, box.y + 10);
    await page.mouse.down();
    await page.mouse.move(box.x + 110, box.y + 60, { steps: 5 });
    await page.mouse.up();
    const [pannedX, pannedY] = await viewBox(graph);
    expect(pannedX).toBeLessThan(x);
    expect(pannedY).toBeLessThan(y);

    await page.getByRole("button", { name: "Reset view" }).click();
    await expect(graph).toHaveAttribute("viewBox", "0 0 100 100");

    // A node's card: title, status chip, and the list it is managed in.
    await page.getByRole("button", { name: "Open data catalogue: hel-fi" }).click();
    const card = page.getByRole("complementary", { name: "Selected object" });
    await expect(card.getByText("Helsinki Region Context catalogue")).toBeVisible();
    await expect(card.getByText("Live", { exact: true })).toBeVisible();
    await expect(
      card.getByRole("navigation", { name: "Where this object is managed" }).getByRole("link", { name: "Open data" }),
    ).toHaveAttribute("href", "/projects/helsinki/ckan");

    // Filter: switching a kind off takes its nodes, its edges and its open card with it.
    await expect(graph.locator(`path[aria-label="publishes to"]`)).toHaveCount(1);
    await page.getByRole("checkbox", { name: "Open data catalogue" }).uncheck();
    await expect(page.getByRole("button", { name: "Open data catalogue: hel-fi" })).toHaveCount(0);
    await expect(graph.locator(`path[aria-label="publishes to"]`)).toHaveCount(0);
    await expect(card).toHaveCount(0);

    await page.getByRole("checkbox", { name: "Pipeline" }).uncheck();
    await expect(graph.locator("path[aria-label]")).toHaveCount(2);
    await page.getByRole("checkbox", { name: "Pipeline" }).check();
    await expect(graph.locator("path[aria-label]")).toHaveCount(3);
  });

  test("is not on the project's landing page: the Federation page holds it (UI-28)", async ({ page }) => {
    await stubApi(page);
    await page.goto("/?lang=en");

    await expect(page).toHaveURL(/\/projects\/helsinki\/spaces$/);
    await expect(page.getByRole("heading", { level: 1, name: "Context Spaces" })).toBeVisible();
    await expect(page.getByRole("group", { name: "Federation graph" })).toHaveCount(0);
  });
});
