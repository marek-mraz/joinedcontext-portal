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

/** Asks in the docked assistant: its first composer, or the conversation's once one is running. */
export async function ask(page: Page, text: string): Promise<void> {
  const bubble = page.getByRole("button", { name: "Open the assistant" });
  if (await bubble.count()) {
    await bubble.first().click();
  }
  const composer = page
    .getByLabel(/^(Ask the assistant|Tell the assistant what to build or change…)$/)
    .first();
  await composer.fill(text);
  await composer.press("Enter");
}

/** The change a proposal answered with, read off the notice the page shows (UI-23). */
export async function proposedChange(page: Page): Promise<string> {
  const review = page.getByRole("link", { name: "Review it in Approvals" });
  try {
    await expect(review).toBeVisible({ timeout: 60_000 });
  } catch (err) {
    // What the page says instead of the notice: a verdict, a refusal, a dialog still open.
    const said = await page.locator("[role=dialog], [role=alert], [role=status]").allInnerTexts();
    throw new Error(`no proposal notice; the page says: ${JSON.stringify(said)}\n${String(err)}`);
  }
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
  // Only a Red lane asks for the name typed back (CC-19). A caller that knows the name passes it
  // and this types it when the page asks; a Yellow change has no such field, and waiting for one
  // would fail on a change that needed no confirmation.
  const input = page.locator("#confirm-resource-name");
  if (confirm && (await input.count())) {
    await expect(input).toBeEnabled({ timeout: 60_000 });
    await input.pressSequentially(confirm, { delay: 60 });
  }
  await expect(button).toBeEnabled({ timeout: 60_000 });
  await button.click();
  await expect(page.getByText(/Deploying|Merged|Applied|Live/).first()).toBeVisible({ timeout: 90_000 });
}

/**
 * Rejects one change as the approver, so a proposal a spec made leaves dev as it was.
 *
 * Reject opens a dialog and its confirmation stays disabled until a reason is typed: the proposer
 * reads that reason on the change. A helper that only clicked Reject left the dialog open and the
 * change PendingApproval — which is how this journey left changes behind on dev (T-1597).
 */
export async function reject(
  page: Page,
  project: string,
  change: string,
  reason = "Rejected by a live journey: this change was proposed only to prove the form.",
): Promise<void> {
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "networkidle" });
  const button = page.getByRole("button", { name: "Reject", exact: true });
  await expect(button).toBeEnabled({ timeout: 60_000 });
  await button.click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Why are you rejecting this?").fill(reason);
  await dialog.getByRole("button", { name: "Reject the change" }).click();
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

/**
 * The CSRF token of a signed-in context, so a spec's own API call goes through the same door the
 * page's calls go through. A request without it is refused by the middleware, and a spec that
 * reads that refusal as the rule it meant to test proves nothing (T-1585).
 */
export async function csrf(context: BrowserContext): Promise<string> {
  const cookie = (await context.cookies()).find((each) => each.name === "jc_csrf");
  if (!cookie) {
    throw new Error("no jc_csrf cookie in this context: the session did not complete");
  }
  return cookie.value;
}

/**
 * Removes a resource a journey created, all the way: the removal is itself a Change, so it is
 * proposed and then approved. A journey that only sent the DELETE left the resource standing and the
 * change open — which is how three spaces were found on dev on 2026-09-18 (T-2236).
 *
 * The **owner** approves it, not the approver: approving a removal needs `delete` on the kind
 * (`an_approver_approves_a_grant_only_within_their_own_rights_and_a_removal_only_with_delete`), which
 * the `approver` role does not carry and `demo.steward` does through `org-admin`. A steward
 * approving their own removal is the administrator exception of CC-34, the one
 * `roles-refusals.spec.ts` plays.
 */
export async function removeCompletely(
  owner: { context: BrowserContext; page: Page },
  project: string,
  plural: string,
  name: string,
): Promise<void> {
  const token = await csrf(owner.context);
  const answer = await owner.page.request.delete(`/api/v1/projects/${project}/${plural}/${name}`, {
    headers: { "x-csrf-token": token },
    data: { confirm: name },
  });
  if (answer.status() === 404) {
    return;
  }
  if (answer.status() !== 202) {
    throw new Error(`removal of ${plural}/${name} was refused: ${await answer.text()}`);
  }
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  if (!change) {
    throw new Error(`the removal of ${plural}/${name} named no change`);
  }
  await approve(owner.page, project, change, name);
}
