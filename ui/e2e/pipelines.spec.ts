import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// `vite preview` has no portal API behind it, so the API is answered in the browser and
// everything above it — routing, polling, the pause button — is the real thing.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const PIPELINES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: {
        name: "aq-mqtt-ingest",
        namespace: "banskabystrica",
        title: { sk: "Ovzdušie MQTT", en: "Air quality MQTT" },
      },
      spec: {
        class: "resident",
        secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }],
      },
      status: { phase: "Live" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

async function stubApi(page: Page): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  let received = 128_400;
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
      return json(CHANGE, 202);
    }
    if (path.endsWith("/metrics")) {
      // A counter that grows between polls, the way a running stream's does.
      received += 25;
      return json({
        pipeline: "aq-mqtt-ingest",
        scrapedAt: new Date().toISOString(),
        received,
        sent: received - 3,
        errors: 3,
      });
    }
    if (path.endsWith("/pipelines")) {
      return json(PIPELINES);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { writes };
}

test.describe("pipelines", () => {
  test("the resident stream's counter moves while the view is open (DEMO step 3)", async ({
    page,
  }) => {
    await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines?lang=en");
    const row = page.getByRole("row").filter({ hasText: "Air quality MQTT" });
    await expect(row.getByText("resident")).toBeVisible();
    await expect(row.getByText("128,425")).toBeVisible();
    // The next poll is five seconds out; the number has to change on its own.
    await expect(row.getByText("128,450")).toBeVisible({ timeout: 15_000 });
    await expect(row.getByText("mqtt-credentials/password → $MQTT_PASSWORD")).toBeVisible();
  });

  test("pausing a pipeline proposes a change", async ({ page }) => {
    const { writes } = await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines?lang=en");
    await page.getByRole("button", { name: "Pause" }).click();

    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    expect(writes).toHaveLength(1);
    expect(writes[0]).toContain("PUT /api/v1/projects/banskabystrica/pipelines/aq-mqtt-ingest");
    expect(writes[0]).toContain('"enabled":false');
  });

  test("the pipelines route has no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines?lang=sk");
    await expect(page.getByRole("table")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
