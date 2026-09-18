import { writeFileSync } from "node:fs";
import { test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { axeViolations } from "../axe";
import { STEWARD, VIEWER, signIn } from "./portal";

const PROJECT = "helsinki";
const REPORT = process.env.FORMS_REPORT ?? "test-results/forms-checklist.json";

interface Finding {
  page: string;
  check: string;
  detail: string;
}

/**
 * The form checklist of goal.md, "Forms people trust", measured on dev (UI-44, UI-45, UI-61).
 * A survey, not a gate: it never fails on a finding, it writes them down, and each becomes a
 * task somebody reads before building. Nothing is submitted with valid data, so nothing is
 * proposed: the only button it clicks inside a form is Check, which is a dry run.
 */
test("every create form, against the checklist", async ({ browser }) => {
  const findings: Finding[] = [];
  const { page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const pages = await projectPages(page);

  for (const path of pages) {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`${path}?lang=en`, { waitUntil: "networkidle" });
    const note = (check: string, detail: string) => findings.push({ page: path, check, detail });

    for (const violation of await axeViolations(page)) note("axe on the page", violation);
    const opener = page.getByRole("main").getByRole("button", { name: /^New / }).first();
    if (!(await opener.count())) continue;
    if (await opener.isDisabled()) {
      note("create is disabled for a steward", (await opener.getAttribute("title")) ?? "no reason given");
      continue;
    }
    await opener.click();
    const dialog = page.getByRole("dialog");
    if (!(await dialog.isVisible().catch(() => false))) {
      note("New opens no dialog", (await opener.innerText()).trim());
      continue;
    }

    for (const violation of await axeViolations(page)) note("axe with the form open", violation);
    for (const name of await unnamedFields(dialog)) note("field without an accessible name", name);
    if (!(await dialog.evaluate((el) => el.contains(document.activeElement)))) {
      note("focus is not moved into the dialog", "");
    }

    // An empty submit: the form says what is missing, beside the field, in words.
    // Only a check is ever clicked. A form that proposes on its first button would open a real
    // Change from its placeholder values; the New role form did exactly that (T-1492).
    const submit = dialog.getByRole("button", { name: /^Check/ }).first();
    if (await submit.count()) {
      if (await submit.isDisabled()) {
        const why = (await submit.getAttribute("title")) ?? (await submit.getAttribute("aria-describedby"));
        if (!why) note("submit is disabled on an empty form without saying why", (await submit.innerText()).trim());
      } else {
        await submit.click();
        await page.waitForTimeout(1500);
        if (!(await dialog.isVisible().catch(() => false))) {
          note("an empty form was accepted", "the dialog closed");
        } else {
          const invalid = await dialog.locator("[aria-invalid=true]").count();
          const said = (await dialog.locator("[role=alert], [role=status], [aria-live]").allInnerTexts()).join(" | ");
          if (invalid === 0 && said.trim() === "") note("an empty submit says nothing", "");
          if (invalid === 0 && said.trim() !== "") note("the error is not tied to a field (no aria-invalid)", said.slice(0, 200));
          if (/\b[45]\d\d\b|^\s*[{[]|bad request|unprocessable/i.test(said)) note("the error is a status code or raw JSON", said.slice(0, 200));
        }
      }
    } else {
      note("the form has no Check step before it proposes", (await dialog.getByRole("button").allInnerTexts()).join(", "));
    }

    await page.setViewportSize({ width: 400, height: 900 });
    await page.waitForTimeout(300);
    const overflow = await dialog.evaluate((el) => el.scrollWidth - el.clientWidth);
    if (overflow > 1) note("the form overflows sideways at 400 px", `${overflow}px`);
    await page.setViewportSize({ width: 1600, height: 1000 });

    await page.keyboard.press("Escape");
    await page.waitForTimeout(400);
    if (await dialog.isVisible().catch(() => false)) note("Escape does not close the form", "");
    else if (!(await opener.evaluate((el) => el === document.activeElement))) note("focus does not return to the button that opened the form", "");
  }

  // A viewer sees why, never a dead button.
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/spaces?lang=en`);
  for (const path of pages) {
    await viewer.page.goto(`${path}?lang=en`, { waitUntil: "networkidle" });
    const opener = viewer.page.getByRole("main").getByRole("button", { name: /^New / }).first();
    if (!(await opener.count())) continue;
    const disabled = (await opener.isDisabled()) || (await opener.getAttribute("aria-disabled")) === "true";
    const reason = (await opener.getAttribute("title")) ?? (await opener.getAttribute("aria-describedby"));
    if (!disabled) findings.push({ page: path, check: "a viewer is offered New", detail: "enabled" });
    else if (!reason) findings.push({ page: path, check: "a viewer's disabled New gives no reason", detail: "" });
  }

  writeFileSync(REPORT, JSON.stringify({ pages, findings }, null, 2));
  console.log(`forms checklist: ${pages.length} pages, ${findings.length} findings -> ${REPORT}`);
});

/** Every page of the project the navigation links to. */
async function projectPages(page: Page): Promise<string[]> {
  const hrefs = await page
    .getByRole("navigation", { name: "Main navigation" })
    .locator(`a[href^="/projects/${PROJECT}/"]`)
    .evaluateAll((links) => links.map((a) => new URL((a as HTMLAnchorElement).href).pathname));
  return [...new Set(hrefs)];
}

/** Inputs, selects and text areas of the form with no accessible name. */
async function unnamedFields(dialog: Locator): Promise<string[]> {
  return dialog.locator("input:not([type=hidden]), select, textarea").evaluateAll((fields) =>
    fields
      .filter((field) => {
        const el = field as HTMLInputElement;
        const labelled = (el.labels?.length ?? 0) > 0 || el.getAttribute("aria-label") || el.getAttribute("aria-labelledby");
        return !labelled && el.offsetParent !== null;
      })
      .map((field) => `${field.tagName.toLowerCase()}[name=${field.getAttribute("name") ?? ""}][id=${field.id}]`),
  );
}
