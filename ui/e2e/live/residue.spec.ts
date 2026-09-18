/**
 * Sweeps what takes and e2e left behind in helsinki (T-0667, T-0750): every resource whose name
 * carries a take's HHMM suffix, or the fixed name an act creates, is deleted through the
 * Portal's own delete, a Red change the approver approves with the name typed back, so the
 * reconciler drops what it had deployed. Dependents first, then the spaces, then the models the spaces referenced.
 * Only these names, only this project; nothing to sweep is a pass.
 *
 * A copy is swept too, and by its own door (T-1592): a workspace is not a resource under
 * `/projects/{project}/{plural}`, it is discarded, and an expired one is invisible to the list it
 * is no longer in. Until this, a copy a take or a journey left behind was residue nobody looked
 * at — the sweep knew seven plurals and not this one.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, listedNames, signIn } from "./portal";

const PROJECT = "helsinki";
/** A take's suffix is the HH:MM it started at; an act with a fixed name is listed by it. */
const TAKE = /-\d{4}$/;
const SWEEP: { plural: string; residue: (name: string) => boolean }[] = [
  { plural: "endpoints", residue: (name) => /^bikes-regional-[a-z0-9]+$/.test(name) },
  { plural: "pipelines", residue: (name) => TAKE.test(name) },
  { plural: "datasources", residue: (name) => TAKE.test(name) },
  { plural: "dashboards", residue: (name) => TAKE.test(name) || name === "city-bikes" || name === "city-bike-stations" },
  { plural: "apps", residue: (name) => TAKE.test(name) || name === "large-map-city" },
  // A space references its model (dataModelRef), so the space goes first.
  { plural: "spaces", residue: (name) => /^citybikes-\d{4}$/.test(name) || name === "city-bikes" || name === "city-bike-stations" },
  { plural: "datamodels", residue: (name) => /^citybikes-\d{4}$/.test(name) },
];

test.setTimeout(3_600_000);
test.use({ trace: "off" });

/** The Portal's own delete through the session: a Red change, its id returned (CC-19). */
async function proposeDelete(page: Page, context: BrowserContext, plural: string, name: string): Promise<string> {
  const csrf = (await context.cookies()).find((cookie) => cookie.name === "jc_csrf")?.value ?? "";
  const answer = await page.request.delete(`/api/v1/projects/${PROJECT}/${plural}/${name}`, {
    headers: { "x-csrf-token": csrf },
  });
  expect(answer.status(), `delete ${plural}/${name}: ${await answer.text()}`).toBe(202);
  return ((await answer.json()) as { metadata: { name: string } }).metadata.name;
}

/** The copies of this project, as their own route answers them (T-1592). */
async function listedCopies(page: Page): Promise<{ name: string; expiresAt: string }[]> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/workspaces`);
  expect(answer.ok(), `list workspaces: ${await answer.text()}`).toBe(true);
  const body = (await answer.json()) as { items?: { name: string; expiresAt: string }[] };
  return body.items ?? [];
}

/**
 * A copy a take or a journey left behind: its name carries a take's suffix or a journey's prefix,
 * or it is already past its own `expiresAt` and still listed — which is residue whichever name it
 * carries, because a copy nobody discarded holds a branch and may hold a preview slot.
 */
function copyResidue(copy: { name: string; expiresAt: string }): boolean {
  return (
    TAKE.test(copy.name) ||
    /^(e2e|t1[0-9]{3}|journey)-/.test(copy.name) ||
    Date.parse(copy.expiresAt) < Date.now()
  );
}

test("no copy of a take or a journey is left in helsinki", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/workspaces?lang=en`);
  try {
    const found = (await listedCopies(page)).filter(copyResidue).map((copy) => copy.name);
    test.info().annotations.push({ type: "copies", description: found.join(", ") || "none" });
    const token = (await context.cookies()).find((each) => each.name === "jc_csrf")?.value ?? "";
    expect(token, "no jc_csrf cookie: the session did not complete").not.toBe("");
    for (const name of found) {
      // A copy is discarded, not proposed away: no Change, no approval, and its branch goes with
      // it (`discarding_removes_the_branch_and_the_record`).
      const answer = await page.request.delete(`/api/v1/projects/${PROJECT}/workspaces/${name}`, {
        headers: { "x-csrf-token": token },
      });
      expect([204, 404], `discard ${name}: ${await answer.text()}`).toContain(answer.status());
    }
    await expect
      .poll(async () => (await listedCopies(page)).filter(copyResidue).map((copy) => copy.name), {
        message: "copies still listed after their discard",
        timeout: 120_000,
        intervals: [5_000],
      })
      .toEqual([]);
  } finally {
    await context.close();
  }
});

test("no residue of takes or e2e is left in helsinki", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/endpoints?lang=en`);
  const found: string[] = [];
  for (const { plural, residue } of SWEEP) {
    const names = (await listedNames(steward.page, PROJECT, plural)).filter(residue);
    found.push(...names.map((name) => `${plural}/${name}`));
  }
  test.info().annotations.push({ type: "residue", description: found.join(", ") || "none" });
  if (found.length > 0) {
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    for (const item of found) {
      const [plural, name] = item.split("/");
      const change = await proposeDelete(steward.page, steward.context, plural, name);
      await approve(approver.page, PROJECT, change, name);
    }
    await approver.context.close();
  }
  for (const { plural, residue } of SWEEP) {
    await expect
      .poll(async () => (await listedNames(steward.page, PROJECT, plural)).filter(residue), {
        message: `${plural} still list residue`,
        timeout: 300_000,
        intervals: [10_000],
      })
      .toEqual([]);
  }
  await steward.context.close();
});
