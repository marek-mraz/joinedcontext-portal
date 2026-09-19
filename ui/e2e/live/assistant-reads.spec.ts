/**
 * What the docked assistant can answer about the project it sits in, on dev (T-1475; AG-64, AG-70).
 *
 * The dock runs the `app-builder` profile, and a run may only call what that profile names
 * (`joinedcontext-deployment/components/agent-runner/seed/app-builder.yaml`). Before T-1475 the
 * profile named eleven operations, so "what waits for approval" had nothing behind it and the
 * assistant said it could not answer. This is that task's own measurement, kept: the question is
 * asked as a person asks it, and the proof is the operation line in the conversation — the platform
 * answered from `jc_change_list`, not from the model's memory of the page.
 *
 * Nothing is proposed, approved or changed here, so the journey may run against dev at any time.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, ask, signIn } from "./portal";

const PROJECT = "helsinki";

test.setTimeout(900_000);

/** The conversation line of one operation, with the icon that says how it ended. */
function toolLine(page: Page, operation: string) {
  return page.getByRole("listitem").filter({ hasText: operation }).first();
}

test("the assistant answers what waits for approval from the platform's own change list", async ({
  browser,
}) => {
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/activity?lang=en`);
  try {
    await ask(page, `What waits for approval in ${PROJECT}?`);

    const line = toolLine(page, "jc_change_list");
    await expect(line, "the answer comes from the platform's change list").toBeVisible({
      timeout: 300_000,
    });
    await expect(
      line.getByRole("img", { name: "Succeeded" }),
      "the operation the profile names is granted at call time too (AG-70)",
    ).toBeVisible();

    // And the person gets an answer in words, not a refusal.
    const answer = page.getByRole("listitem").filter({ hasText: "Assistant" }).last();
    await expect(answer).toBeVisible({ timeout: 300_000 });
    await expect(answer).not.toContainText(/cannot|not allowed|forbidden|does not grant/i);
  } finally {
    await context.close();
  }
});
