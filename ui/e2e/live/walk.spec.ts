/**
 * A walk of every page as each demo person (T-0759): the Spaces page (React #31 on an object
 * reference) and the Models page (`filter is not a function`) crashed on dev and a person found
 * them first. Every route of the main navigation, in both projects, plus the detail pages and
 * the assistant, is opened; a console error, an uncaught error, the error screen, a 5xx or a 4xx
 * the person should not meet is a finding. The findings are attached as a report.
 */
import { expect, test } from "@playwright/test";
import type { Page, Response } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, signIn } from "./portal";

const PROJECTS = ["helsinki", "banskabystrica"];

interface Finding {
  who: string;
  route: string;
  kind: "console" | "pageerror" | "error-screen" | "http";
  detail: string;
}

/** A 4xx a person may meet without anything being wrong: a viewer refused a write or a grant. */
function expected(who: string, response: Response): boolean {
  const status = response.status();
  if (status === 403 && who === "viewer") {
    return true;
  }
  // A feed or a run that has expired answers 404 or 410 on purpose; the pages say so in words.
  return status === 410 || (status === 404 && /\/agent-runs\/[^/]+\/(preview|events)/.test(response.url()));
}

async function routesOf(page: Page, project: string): Promise<string[]> {
  await page.goto(`/projects/${project}/spaces?lang=en`, { waitUntil: "networkidle" });
  const nav = page.getByRole("navigation", { name: "Main navigation" });
  const hrefs = await nav.locator("a[href^='/']").evaluateAll((links) =>
    links.map((link) => (link as HTMLAnchorElement).getAttribute("href") ?? ""),
  );
  const details = await page
    .locator("main a[href*='/spaces/']:not([href*='/spaces/complete'])")
    .evaluateAll((links) => links.slice(0, 1).map((link) => (link as HTMLAnchorElement).getAttribute("href") ?? ""));
  return [
    ...new Set([
      ...hrefs.map((href) => href.replace(/\/projects\/[^/]+\//, `/projects/${project}/`)),
      ...details,
      `/projects/${project}/spaces/complete`,
      `/projects/${project}/assistant`,
      `/projects/${project}/explore`,
      `/projects/${project}/models`,
    ]),
  ].filter((href) => href.startsWith("/"));
}

for (const [who, person] of [
  ["steward", STEWARD],
  ["approver", APPROVER],
  ["viewer", VIEWER],
] as const) {
  test(`every page opens cleanly for the ${who}`, async ({ browser }) => {
    test.setTimeout(900_000);
    const { context, page } = await signIn(browser, person, "/projects/helsinki/spaces?lang=en");
    const findings: Finding[] = [];
    let route = "";
    page.on("console", (message) => {
      if (message.type() === "error") {
        findings.push({ who, route, kind: "console", detail: message.text().slice(0, 300) });
      }
    });
    page.on("pageerror", (error) => findings.push({ who, route, kind: "pageerror", detail: error.message.slice(0, 300) }));
    page.on("response", (response) => {
      const status = response.status();
      if (!response.url().includes("/api/") || status < 400 || expected(who, response)) {
        return;
      }
      findings.push({ who, route, kind: "http", detail: `${status} ${response.request().method()} ${new URL(response.url()).pathname}` });
    });

    for (const project of PROJECTS) {
      for (const next of await routesOf(page, project)) {
        route = next;
        await page.goto(`${next}${next.includes("?") ? "&" : "?"}lang=en`, { waitUntil: "networkidle" });
        if (await page.getByText("Something went wrong!").count()) {
          findings.push({ who, route, kind: "error-screen", detail: (await page.locator("body").innerText()).slice(0, 300) });
        }
      }
      // The assistant opens and closes on a project page without an error.
      route = `/projects/${project}/spaces (assistant)`;
      await page.goto(`/projects/${project}/spaces?lang=en`, { waitUntil: "networkidle" });
      const bubble = page.getByRole("button", { name: "Open the assistant" });
      if (await bubble.count()) {
        await bubble.first().click();
        await page.getByRole("button", { name: "Close the assistant" }).first().click();
      }
    }

    await test.info().attach(`walk-${who}.json`, { body: JSON.stringify(findings, null, 2), contentType: "application/json" });
    await context.close();
    expect(findings).toEqual([]);
  });
}
