/**
 * The assistant opens each create form, filled from one sentence, on dev (T-1597; AG-45, UI-45, AG-77).
 *
 * Two of the six creates were already driven by the assistant and proved:
 * roles in `change.spec.ts` and endpoints in
 * `joinedcontext-conformance/e2e/journeys/17-assistant-navigation.spec.ts`. The four here — a context
 * space, a data source, a pipeline and a dashboard — were driven by nobody: `load.spec.ts` and
 * `analyse.spec.ts` create a source and two pipelines through the *forms*, and `change.spec.ts` asks
 * the assistant for edits and a removal, never a create.
 *
 * What each case holds: one sentence in the dock lands on the kind's own page with its form open,
 * the form carries what the sentence said (the name at least, and whatever else the sentence named),
 * and the person's Propose opens a change whose plan still holds those values — nothing the assistant
 * wrote is lost between the dock, the form and the change.
 *
 * Every proposal is rejected again, so dev keeps what it had: this journey creates nothing. A
 * rejection is also the honest end, because a create the assistant drafted is exactly the change a
 * person is supposed to decide (PF-58).
 *
 * What the first run measured on dev (2026-09-19, portal 4f03b8c): the dashboard case is green; the
 * other three have no create path in the dock's prompt at all (T-2246), so they carry `test.fail()`
 * with that id — they run whole, they are expected to fail, and the run turns red the day they pass.
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { APPROVER, STEWARD, ask, proposedChange, reject, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = new Date().toISOString().slice(11, 19).replace(/:/g, "");

test.setTimeout(1_800_000);

/**
 * The plan of a change, as the approvals route reads it: every file it would write with its fields.
 * The whole body, because a create of one kind writes more than one file — a dashboard carries its
 * layers — and `planFields` holds the fields of the first file alone.
 */
async function planOf(page: Page, change: string): Promise<string> {
  const detail = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
  expect(detail.ok(), `read change ${change}`).toBe(true);
  return JSON.stringify(await detail.json());
}

/** Proposes from the form the assistant opened, running the check the strict gate wants (PF-57). */
async function proposeFrom(form: Locator, label = /^Propose/): Promise<void> {
  const propose = form.getByRole("button", { name: label });
  if (await propose.isDisabled()) {
    await form.getByRole("button", { name: "Check", exact: true }).click();
  }
  await expect(propose).toBeEnabled({ timeout: 180_000 });
  await propose.click();
}

interface Create {
  /** The kind, for the test name. */
  what: string;
  /**
   * The task for the create path the dock does not have yet (T-2246, measured on dev 2026-09-19).
   * The case runs whole and is expected to fail; when the path lands, the run reports "expected to
   * fail but passed" and this line goes away with the marker. Nothing in the case is softened.
   */
  missing?: string;
  /** What a person types into the dock. */
  sentence: (name: string) => string;
  /** The page the request must land on. */
  route: RegExp;
  /** What the form must already hold, by the label a person reads. */
  filled: (name: string) => [RegExp | string, string][];
  /** A value the change's plan must still carry, beyond the name. */
  carries?: string;
}

const CREATES: Create[] = [
  {
    what: "a context space",
    missing: "T-2246",
    sentence: (name) => `Create a context space called ${name} in the helsinki project`,
    route: /\/projects\/helsinki\/spaces/,
    filled: (name) => [[/^Name/, name]],
  },
  {
    what: "a data source",
    missing: "T-2246",
    sentence: (name) =>
      `Add an HTTP data source called ${name} that polls https://opendata.example.org/aq.json every 5 minutes`,
    route: /\/projects\/helsinki\/datasources/,
    filled: (name) => [["Name", name]],
    carries: "opendata.example.org",
  },
  {
    what: "a pipeline",
    missing: "T-2246",
    // Names a data source the project really has: asked for "the helsinki-bikes data source",
    // which is an endpoint and not a source, the assistant rightly asked which source to read
    // and the journey waited for an answer nobody was there to give (measured 2026-09-19).
    sentence: (name) =>
      `Create a pipeline called ${name} that reads the hel-news-rss data source every 15 minutes and writes into the helsinki space through the helsinki-all endpoint`,
    route: /\/projects\/helsinki\/pipelines/,
    filled: (name) => [[/^(Name|Pipeline)/, name]],
  },
  {
    what: "a dashboard",
    sentence: (name) => `Make a dashboard called ${name} showing the bikes of the helsinki space`,
    route: /\/projects\/helsinki\/dashboards/,
    filled: (name) => [[/^(Name|Title)/, name]],
  },
];

for (const create of CREATES) {
  test(`the assistant opens the form for ${create.what}, filled from one sentence`, async ({
    browser,
  }) => {
    if (create.missing) {
      test.fail(true, `the dock has no create path for ${create.what} yet (${create.missing})`);
    }
    // Starts away from every route under test: a start page that is already the target would let a
    // case pass on a request the assistant ignored (the space case did, on the first run).
    const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/activity?lang=en`);
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    const name = `t1597-${create.what.replace(/[^a-z]/g, "")}-${SUFFIX}`.slice(0, 40);
    let change = "";
    try {
      await ask(steward.page, create.sentence(name));
      await expect(steward.page, "the request lands on the kind's own page").toHaveURL(create.route, {
        timeout: 300_000,
      });
      const form = steward.page.getByRole("dialog");
      await expect(form, "with the form open, not on the list alone").toBeVisible({
        timeout: 60_000,
      });

      // What the sentence said is already in the form: an assistant that navigated and filled
      // nothing would leave the person to type it all again (AG-45).
      for (const [label, value] of create.filled(name)) {
        await expect(form.getByLabel(label).first(), `${label} is filled`).toHaveValue(
          new RegExp(value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "i"),
          { timeout: 60_000 },
        );
      }

      await proposeFrom(form);
      change = await proposedChange(steward.page);
      expect(change).toMatch(/^chg-/);

      // And the change still holds it: nothing is lost between the form and the plan.
      const plan = await planOf(steward.page, change);
      expect(plan, `the plan carries the name`).toContain(name);
      if (create.carries) {
        expect(plan, `the plan carries what the sentence named`).toContain(create.carries);
      }
    } finally {
      if (change) {
        await reject(approver.page, PROJECT, change);
      }
      await steward.context.close();
      await approver.context.close();
    }
  });
}
