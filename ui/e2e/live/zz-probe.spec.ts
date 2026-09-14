import { test } from "@playwright/test";
import { signIn, STEWARD } from "./portal";

test("probe: target endpoint options", async ({ browser }) => {
  const { page } = await signIn(browser, STEWARD, "/projects/helsinki/pipelines?lang=en");
  await page.getByRole("button", { name: "New pipeline" }).click();
  const studio = page.getByRole("dialog");
  await studio.locator("#studio-source-kind").selectOption("datasource");
  await studio.locator("#studio-datasource").selectOption({ index: 1 });
  await page.waitForTimeout(2000);
  const target = studio.locator("#root_targetEndpoint");
  console.log("PROBE tag", await target.evaluate((el) => el.tagName));
  console.log("PROBE options", await target.evaluate((el) => [...el.querySelectorAll("option")].map((o) => `${o.label}=${o.value}`).join(" | ")));
  console.log("PROBE datasources", await studio.locator("#studio-datasource").evaluate((el) => [...el.querySelectorAll("option")].map((o) => o.label).join(",")));
});
