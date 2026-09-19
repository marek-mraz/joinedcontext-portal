import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// The journey over entity data (T-1437; UI-64, UI-67, UI-59). `vite preview` has no portal API and
// no gateway behind it, so both are answered in the browser: the grid, the explorer around it and
// the endpoint page are the real thing, and what travels to the endpoint is asserted from the
// requests themselves.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const TYPE = "BikeHireDockingStation";
const EMPTY_DOCK = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:001`;
const FULL_DOCK = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:002`;
/** In the space and not in this endpoint: the comparison marks it (T-1435). */
const HIDDEN_DOCK = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:003`;

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility" } },
    spec: { dataModelRef: "helsinki-mobility", isSandbox: false },
    status: { phase: "Live" },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: {
      name: "helsinki-bikes",
      namespace: "helsinki",
      title: { en: "Helsinki city bike stations" },
    },
    spec: {
      contextSpaceRef: "mobility",
      slug: SLUG,
      audience: "public",
      enabledRepresentations: ["ngsi-ld"],
    },
    status: { phase: "Live" },
  },
]);

/** The model inline, so the slots the edit needs stand between nothing and the page. */
const LINKML = [
  "id: https://hel.fi/models/mobility",
  "name: helsinki-mobility",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, availableBikeNumber, name]",
  "slots:",
  "  id: {}",
  "  availableBikeNumber: { range: integer, minimum_value: 0 }",
  "  name: { range: string }",
  "",
].join("\n");

const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki-mobility", namespace: "helsinki" },
    spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML },
  },
]);

/** What `GET /api/endpoint/{slug}/schema/v{n}/json-schema` publishes, for the endpoint's own page. */
const JSON_SCHEMA = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  $defs: {
    [TYPE]: {
      type: "object",
      properties: { availableBikeNumber: { type: "integer" }, name: { type: "string" } },
    },
  },
};

function dock(id: string, bikes: number, name: string) {
  return {
    id,
    type: TYPE,
    availableBikeNumber: {
      type: "Property",
      value: bikes,
      unitCode: "C62",
      observedAt: "2026-09-19T08:00:00Z",
    },
    name: { type: "Property", value: name },
  };
}

const HISTORY = {
  id: EMPTY_DOCK,
  type: TYPE,
  availableBikeNumber: [
    { type: "Property", value: 3, observedAt: "2026-09-19T06:00:00Z" },
    { type: "Property", value: 0, observedAt: "2026-09-19T08:00:00Z" },
  ],
};

/** A grant that lets this person correct the one attribute, and one that only reads. */
function grant(write: boolean) {
  return {
    permissions: [
      {
        resource: { type: TYPE },
        actions: write ? ["queryEntity", "updateAttrs"] : ["queryEntity"],
        attributes: ["availableBikeNumber", "name"],
      },
    ],
    prohibitions: [],
  };
}

interface Stub {
  /** Every non-GET call to the endpoint: `PATCH <path> <body>`. */
  writes: string[];
  /** The query string of every entity read, so what the filter asked for is asserted, not guessed. */
  reads: string[];
}

async function stubApi(page: Page, options: { write?: boolean } = {}): Promise<Stub> {
  const writes: string[] = [];
  const reads: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({
        status,
        contentType: "application/json",
        headers,
        body: JSON.stringify(body),
      });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    // The access panel asks the endpoint's own PDP one question per action, before any read.
    if (path.endsWith("/access/check")) {
      return json({ decision: options.write === true });
    }
    if (path.endsWith("/access")) {
      return json(grant(options.write === true));
    }
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
      return route.fulfill({ status: 204, body: "" });
    }
    if (path.includes("/temporal/entities/")) {
      return json(HISTORY);
    }
    if (path.endsWith("/schema/index.json")) {
      return json({ models: [{ name: "helsinki-mobility", version: 1 }] });
    }
    if (path.endsWith("/json-schema")) {
      return json(JSON_SCHEMA);
    }
    if (path.endsWith("/entities")) {
      reads.push(url.search);
      // The space holds one dock more than this endpoint answers, and the filter of the question
      // narrows the endpoint's own page to the empty one.
      const q = url.searchParams.get("q") ?? "";
      const docks = path.startsWith("/cs/")
        ? [dock(EMPTY_DOCK, 0, "Rautatientori"), dock(FULL_DOCK, 7, "Kamppi"), dock(HIDDEN_DOCK, 2, "Töölö")]
        : q.includes("availableBikeNumber==0")
          ? [dock(EMPTY_DOCK, 0, "Rautatientori")]
          : [dock(EMPTY_DOCK, 0, "Rautatientori"), dock(FULL_DOCK, 7, "Kamppi")];
      return json(docks, 200, { "NGSILD-Results-Count": String(docks.length) });
    }
    if (path === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path.endsWith("/datamodels")) {
      return json(MODELS);
    }
    return json(list([]));
  });
  return { writes, reads };
}

/**
 * The control `label` names, reached with the Tab key alone: a person who never touches a pointer
 * has to arrive at the cell they are correcting (UI-67). The accessible name is read from the
 * focused element itself, so nothing here depends on how many stops lie between.
 */
async function tabTo(page: Page, label: string): Promise<void> {
  for (let stop = 0; stop < 80; stop += 1) {
    const focused = await page.evaluate(() => {
      const active = document.activeElement as HTMLInputElement | null;
      if (!active) {
        return "";
      }
      // A radio or a checkbox carries its name in the label around it, and a button in its text.
      return (
        active.getAttribute("aria-label") ??
        active.labels?.[0]?.textContent?.trim() ??
        active.textContent?.trim() ??
        ""
      );
    });
    if (focused === label) {
      return;
    }
    await page.keyboard.press("Tab");
  }
  throw new Error(`no control named "${label}" is reachable with the Tab key`);
}

const EXPLORE = "/projects/helsinki/explore";

test.describe("the entity grid", () => {
  test("opens on the endpoint and the filter the assistant handed over (UI-59)", async ({
    page,
  }) => {
    const { reads } = await stubApi(page);

    // What `jc_ui_navigate` builds for the page kind `entities`: an endpoint, a type and the
    // question's own filter, and no space — the page finds the endpoint's own.
    await page.goto(
      `${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&q=${encodeURIComponent("availableBikeNumber==0")}&lang=en`,
    );

    await expect(page.getByLabel("Context space")).toHaveValue("mobility");
    await expect(page.getByLabel("Read through endpoint")).toHaveValue("helsinki-bikes");
    await expect(page.getByRole("button", { name: EMPTY_DOCK })).toBeVisible();
    await expect(page.getByRole("button", { name: FULL_DOCK })).toHaveCount(0);
    expect(reads.some((search) => search.includes("q=availableBikeNumber%3D%3D0"))).toBe(true);
  });

  test("narrows a column and shows when a value was observed", async ({ page }) => {
    const { reads } = await stubApi(page);

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await expect(page.getByRole("button", { name: FULL_DOCK })).toBeVisible();

    // The grid's own filter row, on the column: the operator and then the value.
    await page.getByLabel("Filter: availableBikeNumber").selectOption("equals");
    await page.getByLabel("Value: availableBikeNumber").fill("0");

    await expect(page.getByRole("button", { name: FULL_DOCK })).toHaveCount(0);
    await expect
      .poll(() => reads.filter((search) => search.includes("availableBikeNumber%3D%3D0")).length)
      .toBeGreaterThan(0);

    // When the value was observed is metadata of the same column, switched on from its menu.
    await page.getByLabel("Show metadata for availableBikeNumber").click();
    await page.getByRole("checkbox", { name: "Observed" }).check();
    await expect(
      page.getByRole("columnheader", { name: "availableBikeNumber · Observed" }),
    ).toBeVisible();
  });

  test("corrects a value with the keyboard alone and applies it through the endpoint", async ({
    page,
  }) => {
    const { writes } = await stubApi(page, { write: true });

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    const cell = page.getByLabel("Edit availableBikeNumber").first();
    await expect(cell).toBeVisible();

    await page.keyboard.press("Tab");
    await tabTo(page, "Edit availableBikeNumber");
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type("4");
    await expect(cell).toHaveValue("4");

    await tabTo(page, "Review the changes");
    await page.keyboard.press("Enter");
    const review = page.getByRole("region", { name: "Review the changes" });
    await expect(review).toContainText(EMPTY_DOCK);
    // The corrected value was observed now, not when the old reading was (UI-67). A radio group is
    // one Tab stop on the chosen one, and the arrow keys move inside it.
    await tabTo(page, "keep when each value was observed");
    await page.keyboard.press("ArrowDown");
    await expect(
      review.getByRole("radio", { name: "these values were observed now" }),
    ).toBeChecked();
    await tabTo(page, "Apply");
    await page.keyboard.press("Enter");

    await expect.poll(() => writes.length).toBe(1);
    expect(writes[0]).toContain(`PATCH /api/endpoint/${SLUG}/ngsi-ld/v1/entities/`);
    expect(writes[0]).toContain('"availableBikeNumber":{"type":"Property","value":4');
    expect(writes[0]).toContain('"observedAt"');
  });

  test("opens the history of one attribute of one entity", async ({ page }) => {
    await stubApi(page);

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await expect(page.getByRole("button", { name: EMPTY_DOCK })).toBeVisible();

    await page.getByLabel("Show metadata for availableBikeNumber").click();
    await page.getByRole("button", { name: "History" }).click();

    const history = page.getByRole("dialog").or(page.getByRole("region", { name: "History" }));
    await expect(history.getByText("2026-09-19T06:00:00Z")).toBeVisible();
    await expect(history.getByText("3")).toBeVisible();
  });

  test("compares the endpoint's page with the space behind it", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/helsinki/endpoints?lang=en");
    await page.getByRole("button", { name: "View data" }).click();
    await page.getByRole("button", { name: "Compare with the space" }).click();

    const compare = page.getByRole("region", { name: "The space / This endpoint" });
    await expect(compare.getByRole("heading", { name: "The space" })).toBeVisible();
    await expect(compare.getByRole("heading", { name: "This endpoint" })).toBeVisible();
    // The dock the endpoint's filter or policy leaves out is marked on the space's side, where a
    // person hovers the row to read why it is there and not here.
    await expect(
      compare.locator('tr[title="not in this endpoint (filter or policy)"]'),
    ).toHaveCount(1);
  });

  test("offers a viewer no edit at all", async ({ page }) => {
    const { writes } = await stubApi(page, { write: false });

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await expect(page.getByRole("button", { name: EMPTY_DOCK })).toBeVisible();

    await expect(page.getByLabel("Edit availableBikeNumber")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Review the changes" })).toHaveCount(0);
    expect(writes).toEqual([]);
  });
});

