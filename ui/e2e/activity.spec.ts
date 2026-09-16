import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The same shape as the other journeys: the built bundle, the API answered from the browser.
// The live tail is an EventSource, so the stream route is fulfilled with a body that holds one
// event and then stays open, which is what the Portal sends.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
};

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const LOADED = {
  time: "2026-09-16T09:00:00Z",
  project: "helsinki",
  space: "mobility",
  kind: "config.applied",
  source: "reconciler",
  summary: "The repository at 1a2b3c4 is live: 42 manifests.",
  severity: "info",
  details: { object: "spaces/mobility" },
};

const FORWARDED = {
  time: "2026-09-16T09:05:00Z",
  project: "helsinki",
  space: "mobility",
  kind: "federation.forward",
  source: "broker",
  summary: "A query was forwarded to the hub and answered in 84 ms.",
  severity: "info",
  details: { object: "endpoints/hub-read", count: 1 },
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (url.pathname.endsWith("/activity/stream")) {
      // One event, then the connection stays open the way the Portal's SSE does.
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        headers: { "cache-control": "no-cache" },
        body: `event: federation.forward\ndata: ${JSON.stringify(FORWARDED)}\n\n`,
      });
    }
    if (url.pathname.endsWith("/activity")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ActivityList",
        items: [LOADED],
      });
    }
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname === "/api/v1/projects") {
      return json(PROJECTS);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

test.describe("activity", () => {
  test("an arriving forward reaches the tail and links to its object", async ({ page }) => {
    await stubApi(page);

    await page.goto("/?lang=en");
    await page
      .getByRole("navigation", { name: "Main navigation" })
      .getByRole("link", { name: "Activity" })
      .click();

    await expect(page).toHaveURL(/\/projects\/helsinki\/activity$/);
    await expect(page.getByText("What happened in the last hour")).toBeVisible();
    await expect(page.getByRole("cell", { name: LOADED.summary })).toBeVisible();

    // The tail: the event the stream pushed is on top within two seconds of the page opening.
    await expect(page.getByRole("cell", { name: FORWARDED.summary })).toBeVisible({
      timeout: 2000,
    });
    const rows = page.getByRole("row");
    await expect(rows.nth(1)).toContainText(FORWARDED.summary);
    await expect(page.getByRole("link", { name: "endpoints/hub-read" })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints/hub-read",
    );

    // An info event interrupts nobody: the live region stays empty until an error arrives.
    await expect(page.getByTestId("activity-announce")).toHaveText("");
  });

  test("a filter narrows what the page asks for", async ({ page }) => {
    const asked: string[] = [];
    await stubApi(page);
    page.on("request", (request) => {
      if (request.url().includes("/activity?")) {
        asked.push(request.url());
      }
    });

    await page.goto("/projects/helsinki/activity?lang=en");
    await expect(page.getByRole("table")).toBeVisible();
    await page.getByLabel("Severity").selectOption("error");

    await expect
      .poll(() => asked.some((url) => url.includes("severity=error")))
      .toBe(true);
  });

  test("the activity page has no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/helsinki/activity?lang=en");
    await expect(page.getByRole("table")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
