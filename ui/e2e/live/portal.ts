import { expect } from "@playwright/test";
import type { Browser, BrowserContext, Page } from "@playwright/test";

/** The two demo people of the Load journey: one proposes, the other approves (CC-34). */
export const STEWARD = { user: "demo.steward@hel.fi", password: process.env.PORTAL_PASSWORD ?? "" };
export const APPROVER = { user: "demo.approver@hel.fi", password: process.env.APPROVER_PASSWORD ?? "" };
/** A person who may read the projects and change nothing. */
export const VIEWER = { user: "demo.viewer@hel.fi", password: process.env.VIEWER_PASSWORD ?? "" };

/**
 * Signs one browser context in through the edge: the Portal's /login button, Keycloak's form
 * (`#username`, `#password`, `#kc-login`), back to the page asked for. Mirrors
 * joinedcontext-presentation/record/acts/_portal.py so a spec and a recording take one path.
 */
export async function signIn(browser: Browser, who: { user: string; password: string }, path: string): Promise<{ context: BrowserContext; page: Page }> {
  if (!who.password) {
    throw new Error(`no password in the environment for ${who.user} (PORTAL_PASSWORD / APPROVER_PASSWORD)`);
  }
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(path, { waitUntil: "networkidle" });
  for (let step = 0; step < 4; step += 1) {
    if (await page.locator("#username").count()) {
      await page.fill("#username", who.user);
      await page.fill("#password", who.password);
      await page.click("#kc-login");
      await page.waitForLoadState("networkidle");
      continue;
    }
    const signInButton = page.getByRole("button", { name: "Sign in" });
    if (page.url().includes("/login") && (await signInButton.count())) {
      await signInButton.first().click();
      await page.waitForLoadState("networkidle");
      continue;
    }
    break;
  }
  await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible({ timeout: 60_000 });
  return { context, page };
}

/** The change a proposal answered with, read off the notice the page shows (UI-23). */
export async function proposedChange(page: Page): Promise<string> {
  const review = page.getByRole("link", { name: "Review it in Approvals" });
  await expect(review).toBeVisible({ timeout: 60_000 });
  const href = (await review.getAttribute("href")) ?? "";
  const id = href.split("/approvals/")[1]?.split(/[?#]/)[0];
  if (!id) {
    throw new Error(`the notice links nowhere useful: ${href}`);
  }
  return id;
}

/**
 * Approves one change as the approver. A Yellow lane needs no typed confirmation; a Red one
 * (a public Endpoint, CC-19) asks for the resource name, typed as a person would.
 */
export async function approve(page: Page, project: string, change: string, confirm?: string): Promise<void> {
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "networkidle" });
  const button = page.getByRole("button", { name: "Approve", exact: true });
  if (confirm) {
    const input = page.locator("#confirm-resource-name");
    await expect(input).toBeEnabled({ timeout: 60_000 });
    await input.pressSequentially(confirm, { delay: 60 });
  }
  await expect(button).toBeEnabled({ timeout: 60_000 });
  await button.click();
  await expect(page.getByText(/Deploying|Merged|Applied|Live/).first()).toBeVisible({ timeout: 90_000 });
}

/** Rejects one change as the approver, so a proposal a spec made leaves dev as it was. */
export async function reject(page: Page, project: string, change: string): Promise<void> {
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "networkidle" });
  const button = page.getByRole("button", { name: "Reject", exact: true });
  await expect(button).toBeEnabled({ timeout: 60_000 });
  await button.click();
  await expect(page.getByText(/Rejected/).first()).toBeVisible({ timeout: 60_000 });
}

/**
 * Proposes the deletion of one resource the way a person does (a Red Change, CC-19): its row's
 * Delete on the kind's list, the name typed back, Propose removal; returns the change's id.
 */
export async function proposeDelete(page: Page, project: string, plural: string, name: string): Promise<string> {
  await page.goto(`/projects/${project}/${plural}`);
  const row = page.locator("tr, li").filter({ hasText: name }).first();
  await row.getByRole("button", { name: /^Delete / }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel(`Type ${name} to confirm`).fill(name);
  await dialog.getByRole("button", { name: "Propose removal" }).click();
  const id = dialog.getByText(/^chg-[0-9a-f]{8}$/);
  await expect(id, `delete ${plural}/${name}`).toBeVisible({ timeout: 30_000 });
  return (await id.textContent()) ?? "";
}

/** The names of a project's resources of one kind, as the list route answers them. */
export async function listedNames(page: Page, project: string, plural: string): Promise<string[]> {
  const answer = await page.request.get(`/api/v1/projects/${project}/${plural}`);
  expect(answer.ok(), `list ${plural}`).toBe(true);
  const body = (await answer.json()) as { items?: { metadata: { name: string } }[] };
  return (body.items ?? []).map((item) => item.metadata.name);
}
