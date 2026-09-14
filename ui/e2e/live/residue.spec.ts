/**
 * Sweeps the endpoints share takes and the share e2e left behind (T-0667): every
 * `bikes-regional-*` Endpoint of helsinki is deleted through the Portal's own delete, a Red change
 * the approver approves with the name typed back, so the reconciler drops its gateway route.
 * Only that prefix, only this project; nothing to sweep is a pass.
 */
import { expect, test } from "@playwright/test";
import { APPROVER, STEWARD, approve, listedNames, proposeDelete, signIn } from "./portal";

const PROJECT = "helsinki";
const RESIDUE = /^bikes-regional-[a-z0-9]+$/;

test.setTimeout(900_000);

test("no bikes-regional-* endpoint is left in helsinki", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/endpoints?lang=en`);
  const residue = (await listedNames(steward.page, PROJECT, "endpoints")).filter((name) => RESIDUE.test(name));
  test.info().annotations.push({ type: "residue", description: residue.join(", ") || "none" });
  if (residue.length > 0) {
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    for (const name of residue) {
      await approve(approver.page, PROJECT, await proposeDelete(steward.page, PROJECT, "endpoints", name), name);
    }
    await approver.context.close();
  }
  await expect
    .poll(async () => (await listedNames(steward.page, PROJECT, "endpoints")).filter((name) => RESIDUE.test(name)), {
      timeout: 300_000,
      intervals: [10_000],
    })
    .toEqual([]);
  await steward.context.close();
});
