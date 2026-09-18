import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The assistant asks which endpoints to read, and the person answers by clicking two of them
// (UI-73, UI-74, T-1452): the built bundle, the API answered from the browser, and the run's
// stream fulfilled with the one question.

const PROJECT = "helsinki";
const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};
const RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "bikes-and-air",
  endpointName: "bikes",
  appClass: "fullstack",
  visibility: "project",
  prompt: "Build me a dashboard of bikes and air quality",
  status: "interviewing",
  steps: 2,
  tokensUsed: 1_200,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-18T08:00:00Z",
};
const QUESTION = {
  seq: 1,
  questionId: "q-endpoints",
  pick: "endpoints",
  multiple: true,
  min: 1,
  schema: {
    type: "object",
    title: "Which endpoints should the app read?",
    properties: {
      answer: {
        type: "array",
        title: "Which endpoints should the app read?",
        items: {
          type: "string",
          oneOf: [
            { const: "bikes", title: "City bikes", description: "space helsinki · ngsi-ld, geojson · organization" },
            { const: "air", title: "Air quality", description: "space helsinki · csv · public" },
            { const: "noise", title: "Noise", description: "space helsinki · ngsi-ld · organization" },
          ],
        },
        uniqueItems: true,
        minItems: 1,
      },
    },
    required: ["answer"],
  },
};

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

async function stubApi(page: Page, answers: unknown[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (request.method() === "POST" && path.endsWith(`/agent-runs/${RUN.id}/answers`)) {
      answers.push(request.postDataJSON());
      return route.fulfill({ status: 204 });
    }
    if (request.method() !== "GET") {
      return json({ type: "about:blank", title: "Not stubbed", status: 404 }, 404);
    }
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ ...list([{ name: PROJECT }]), kind: "ProjectList" });
    if (path.endsWith(`/agent-runs/${RUN.id}/events`)) {
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        headers: { "cache-control": "no-cache" },
        body: `id: 1\nevent: question\ndata: ${JSON.stringify(QUESTION)}\n\n`,
      });
    }
    if (path.endsWith(`/agent-runs/${RUN.id}`)) return json(RUN);
    if (path.endsWith("/agent-runs")) return json(list([RUN]));
    if (/^\/api\/v1\/projects\/[a-z-]+\/[a-z-]+$/.test(path)) return json(list([]));
    return json({ type: "about:blank", title: "Not stubbed", status: 404 }, 404);
  });
}

test("the person answers the assistant's question by clicking two endpoints", async ({ page }) => {
  const answers: unknown[] = [];
  await stubApi(page, answers);
  await page.goto(`/projects/${PROJECT}/apps/${RUN.appName}?lang=en`, { waitUntil: "networkidle" });
  const bubble = page.getByRole("button", { name: "Open the assistant" });
  if (await bubble.count()) {
    await bubble.first().click();
  }

  const group = page.getByRole("group", { name: "Which endpoints should the app read?" });
  await expect(group).toBeVisible();
  await expect(group.getByText("space helsinki · csv · public")).toBeVisible();
  await expect(page.getByRole("button", { name: "Something else…" })).toHaveCount(0);

  await group.getByRole("button", { name: /City bikes/ }).click();
  await group.getByRole("button", { name: /Air quality/ }).click();
  await page.getByRole("button", { name: "Use these (2)" }).click();

  await expect.poll(() => answers).toEqual([{ questionId: "q-endpoints", answers: { answer: ["bikes", "air"] } }]);
  expect(await axeViolations(page)).toEqual([]);
});
