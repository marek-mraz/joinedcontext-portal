import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// `vite preview` serves the built bundle with no portal API behind it, so this journey answers
// the API from the browser. The clicks, the routing and the rendering are the real ones; only
// the Rust process is absent (its own journey arrives with the ci-full task that runs the binary).

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
};

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
  summary: {
    key: "change.summary.update",
    params: { kind: "Endpoint", name: "air-quality", fields: 2 },
  },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: {
    lane: "yellow",
    phase: "PendingApproval",
    plan: { update: 1 },
    mergeRequest: "https://gitea.example/city/city-config/pulls/7",
  },
  planFields: [
    { path: "spec.audience", from: "public", to: "internal" },
    { path: "spec.credentials.token", from: "[REDACTED]", to: "[REDACTED]" },
  ],
};

const APPROVED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: PROPOSAL.metadata,
  status: { lane: "yellow", phase: "Deploying", plan: { update: 1 } },
};

async function stubApi(page: Page): Promise<{ approvals: string[] }> {
  const approvals: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/approve")) {
      approvals.push(url.pathname);
      return json(APPROVED, 202);
    }
    if (url.pathname.endsWith("/changes")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ChangeList",
        items: [PROPOSAL],
      });
    }
    if (url.pathname.endsWith(`/changes/${PROPOSAL.metadata.name}`)) {
      return json(PROPOSAL);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { approvals };
}

test.describe("approvals", () => {
  test("an approver reviews a change and approves it", async ({ page }) => {
    const { approvals } = await stubApi(page);

    await page.goto("/?lang=en");
    await page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", {
      name: "Approvals",
    }).click();

    await expect(page).toHaveURL(/\/projects\/banskabystrica\/approvals$/);
    const summary = 'Update Endpoint "air-quality" (2 fields changed)';
    await expect(page.getByRole("link", { name: summary })).toBeVisible();
    await expect(page.getByText("Yellow")).toBeVisible();
    await expect(page.getByText("Marek Mráz")).toBeVisible();

    await page.getByRole("link", { name: summary }).click();

    await expect(page).toHaveURL(/\/approvals\/chg-1a2b3c4d$/);
    await expect(page.getByRole("heading", { name: summary })).toBeVisible();
    await expect(page.getByRole("cell", { name: "spec.audience" })).toBeVisible();
    await expect(page.getByRole("cell", { name: '"internal"' })).toBeVisible();
    // The token appears only as the redaction marker, never as its value.
    await expect(page.getByText("Redacted").first()).toBeVisible();

    await page.getByRole("button", { name: "Approve" }).click();

    await expect(page.getByText("Deploying")).toBeVisible();
    expect(approvals).toEqual(["/api/v1/projects/banskabystrica/changes/chg-1a2b3c4d/approve"]);
  });

  test("the approvals routes have no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/banskabystrica/approvals?lang=sk");
    await expect(page.getByRole("table")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    await page.goto("/projects/banskabystrica/approvals/chg-1a2b3c4d?lang=sk");
    await expect(page.getByRole("button", { name: "Schváliť" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
