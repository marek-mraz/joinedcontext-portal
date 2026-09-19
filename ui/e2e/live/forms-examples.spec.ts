/**
 * Every create form is filled from its own examples and the check comes back green (T-1606; UI-02,
 * PF-57).
 *
 * The form now offers an example for every field it asks for and one action that writes it in
 * (T-1604, T-1612). Nothing proved that following that guidance ends in a manifest the platform
 * accepts — and guidance the platform refuses is worse than none, because the person trusts it.
 *
 * What one kind's case does: open New, take the example into every required field the form offers one
 * for, type a name nobody has used (the example's own name may well be taken on dev, and the check
 * refuses a name that exists), press Check, and read the verdict. Nothing is ever proposed: the only
 * button pressed is Check, a dry run, so dev keeps what it had. A red verdict is reported with the
 * finding the platform gave, because that finding names the example that is wrong.
 *
 * The pages are discovered from the navigation rather than listed here: a kind whose form is added
 * later is covered without editing this file, and the report says which pages were walked.
 */
import { writeFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { STEWARD, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const REPORT = process.env.FORMS_REPORT ?? "test-results/forms-examples.json";
const SUFFIX = new Date().toISOString().slice(11, 19).replace(/:/g, "");
/** The name every form of this run gets, so a check never trips over a name dev already holds. */
const NAME = `t1606-${SUFFIX}`;

test.setTimeout(1_800_000);

interface Walked {
  page: string;
  examplesTaken: number;
  verdict: string;
  finding?: string;
}

/** Every page of the project the navigation links to, in the order a person meets them. */
async function projectPages(page: Page): Promise<string[]> {
  const hrefs = await page
    .getByRole("navigation", { name: "Main navigation" })
    .locator(`a[href^="/projects/${PROJECT}/"]`)
    .evaluateAll((links) =>
      links.map((a) => new URL((a as HTMLAnchorElement).href).pathname),
    );
  return [...new Set(hrefs)];
}

/**
 * Takes the example into every required field that offers one, and says how many it took.
 *
 * Required only: a form's examples are each a value the field accepts, but two optional fields of
 * one kind can be alternatives to each other — a pipeline's schedule and its period, a compute step
 * written as Bloblang or named as a mapping — and filling both is a manifest a person would never
 * write. The buttons are re-read after every click: writing a field's example takes that field's
 * offer out of the accessibility tree.
 */
async function takeTheExamples(dialog: Locator): Promise<number> {
  let taken = 0;
  for (let round = 0; round < 40; round += 1) {
    const offers = dialog.getByRole("button", { name: "Use the example" });
    const fields = await offers.all();
    let clicked = false;
    for (const offer of fields) {
      const required = await offer.evaluate((button) => {
        const row = button.closest("div.flex");
        const field = row?.querySelector(
          "input, select, textarea",
        ) as HTMLInputElement | null;
        if (!field) {
          return false;
        }
        const empty = field.value === "";
        return (
          empty &&
          (field.required || field.getAttribute("aria-required") === "true")
        );
      });
      if (!required) {
        continue;
      }
      await offer.click();
      taken += 1;
      clicked = true;
      break;
    }
    if (!clicked) {
      return taken;
    }
  }
  return taken;
}

/** What the form is saying right now: its alerts, its field errors, in one line. */
async function said(dialog: Locator): Promise<string> {
  const lines = await dialog
    .locator("[role=alert], [role=status], .text-danger")
    .allInnerTexts();
  return lines.join(" | ").replace(/\s+/g, " ").slice(0, 400) || "nothing";
}

/** The required fields still empty, by the label a person reads: why a check cannot run. */
async function emptyRequired(dialog: Locator): Promise<string> {
  const names = await dialog
    .locator("input, select, textarea")
    .evaluateAll((fields) =>
      fields
        .filter((field) => {
          const el = field as HTMLInputElement;
          const required =
            el.required || el.getAttribute("aria-required") === "true";
          return required && el.value === "" && el.offsetParent !== null;
        })
        .map((field) => {
          const el = field as HTMLInputElement;
          return el.labels?.[0]?.textContent?.trim() || el.id;
        }),
    );
  return names.length
    ? `required and still empty: ${names.join(", ")}`
    : "every required field is filled";
}

test("every create form is green from its own examples", async ({
  browser,
}) => {
  const steward = await signIn(
    browser,
    STEWARD,
    `/projects/${PROJECT}/spaces?lang=en`,
  );
  const walked: Walked[] = [];
  const wrong: string[] = [];

  try {
    for (const path of await projectPages(steward.page)) {
      await steward.page.goto(`${path}?lang=en`, { waitUntil: "networkidle" });
      const opener = steward.page
        .getByRole("main")
        .getByRole("button", { name: /^New / })
        .first();
      if (!(await opener.count()) || (await opener.isDisabled())) {
        continue;
      }
      await opener.click();
      const dialog = steward.page.getByRole("dialog");
      if (!(await dialog.isVisible().catch(() => false))) {
        continue;
      }
      const check = dialog
        .getByRole("button", { name: "Check", exact: true })
        .first();
      if (!(await check.count())) {
        // A form with no check of its own is T-1459's subject, not this journey's.
        await steward.page.keyboard.press("Escape");
        continue;
      }

      const examplesTaken = await takeTheExamples(dialog);
      const name = dialog.locator("#root_name");
      if (await name.count()) {
        await name.fill(NAME);
      }
      await check.click();

      // Three ways this ends, and each is a finding of its own: the check passes, the check
      // refuses what the examples built, or the check never runs because the form itself is not
      // complete — the last one is what a form whose examples do not cover its required fields
      // does, and waiting three minutes for a verdict that cannot come is not a measurement.
      const verdictChip = dialog.getByTestId("draft-verdict");
      let verdict = "the check never answered";
      let finding: string | undefined;
      try {
        await expect(verdictChip).not.toHaveText(/Not checked yet/, {
          timeout: 90_000,
        });
        verdict = (await verdictChip.innerText()).trim();
        if (/Check failed/i.test(verdict)) {
          finding = await said(dialog);
        }
      } catch {
        finding = `${await emptyRequired(dialog)}; the form said: ${await said(dialog)}`;
      }
      walked.push({ page: path, examplesTaken, verdict, finding });
      if (finding !== undefined) {
        wrong.push(`${path}: ${verdict} — ${finding}`);
      }

      await steward.page.keyboard.press("Escape");
      await steward.page.waitForTimeout(300);
    }

    writeFileSync(REPORT, JSON.stringify({ name: NAME, walked }, null, 2));
    console.log(`forms examples: ${walked.length} forms -> ${REPORT}`);
    expect(
      walked.length,
      "at least the six kinds with a create form were walked",
    ).toBeGreaterThan(3);
    expect(wrong, "a form whose own examples the platform refuses").toEqual([]);
  } finally {
    // The check saves a draft of what the form held (PF-57), so this run's drafts go with it.
    await sweepDrafts(steward.context, steward.page, PROJECT, /^t1606-/i);
    await steward.context.close();
  }
});
