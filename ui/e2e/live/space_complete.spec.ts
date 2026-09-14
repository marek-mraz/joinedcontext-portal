import { expect, test } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import { fileURLToPath } from "url";
import { STEWARD, proposedChange, signIn } from "./portal";

const PROJECT = "helsinki";

test("complete a space from sample file and propose change", async ({ browser }) => {
  const samplePath = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../../../../joinedcontext-presentation/record/samples/bike-stations.csv",
  );

  if (!fs.existsSync(samplePath)) {
    test.skip(true, `sample file not found at ${samplePath}`);
    return;
  }

  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces/complete?lang=en`);
  const page = steward.page;

  await page.locator("#complete-space").fill(`bikes-${Date.now().toString().slice(-4)}`);
  await page.locator("#complete-files").setInputFiles(samplePath);

  const completeBtn = page.locator("#complete-btn");
  await expect(completeBtn).toBeEnabled();
  await completeBtn.click();

  await expect(page.locator("[data-testid='complete-draft-DataModel']")).toBeVisible({ timeout: 45_000 });
  await expect(page.locator("[data-testid='complete-draft-ContextSpace']")).toBeVisible({ timeout: 45_000 });

  const proposeBtn = page.locator("#complete-propose");
  await expect(proposeBtn).toBeVisible({ timeout: 30_000 });
  await expect(proposeBtn).toBeEnabled();
  await proposeBtn.click();

  const changeId = await proposedChange(page);
  expect(changeId).toBeTruthy();

  await steward.context.close();
});
