import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// `?lang=` is the first source i18next's detector consults, so the journey does not depend on
// the browser's own locale.
test.describe("portal shell", () => {
  test("renders in Slovak, switches to English and keeps the document language in step", async ({
    page,
  }) => {
    await page.goto("/?lang=sk");

    await expect(page.getByRole("heading", { name: "joinedcontext Portal" })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", "sk");
    await expect(page.getByRole("button", { name: "Odoslať" })).toBeVisible();

    await page.getByRole("button", { name: "Jazyk" }).click();
    await page.getByRole("menuitem", { name: "English" }).click();

    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.getByRole("button", { name: "Submit" })).toBeVisible();
  });

  test("refuses an invalid manifest and echoes a valid one", async ({ page }) => {
    await page.goto("/?lang=en");

    const name = page.getByLabel("Name");
    await name.fill("ab");
    await expect(name).toHaveAttribute("aria-invalid", "true");

    await name.fill("sensor-hub");
    await page.getByLabel("Replicas").fill("2");
    await expect(name).not.toHaveAttribute("aria-invalid", "true");

    await page.getByRole("button", { name: "Submit" }).click();

    const echo = page.getByLabel("Submitted Data");
    await expect(echo).toContainText("sensor-hub");
    await expect(echo).toContainText("2");
  });

  test("has no axe violations", async ({ page }) => {
    await page.goto("/?lang=sk");
    await expect(page.getByRole("heading", { name: "joinedcontext Portal" })).toBeVisible();

    expect(await axeViolations(page)).toEqual([]);
  });
});
