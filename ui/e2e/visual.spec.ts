/**
 * Visual baselines of the pages a recording shows (T-0756, UI-01, UI-27, UI-28): the built
 * bundle, the API answered in the browser, one screenshot per page at the recording's size
 * (1920×1080 at zoom 1.5, laid out at 1280 CSS px) and at phone width. A baseline changes only
 * on purpose: `pnpm e2e -- visual --update-snapshots`, and the diff is reviewed like code.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

const PROJECT = "helsinki";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: PROJECT }] };

const SOURCE_URL = `https://git.example.sk/city/org/src/branch/main/projects/${PROJECT}/spaces/ovzdusie/space.yaml`;

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "ovzdusie", namespace: PROJECT, title: { sk: "Ovzdušie", en: "Air quality" } },
    spec: { dataModelRef: "AirQualityObserved" },
    status: { phase: "Live", observedRevision: "9c1f0ab", sourceUrl: SOURCE_URL },
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "doprava", namespace: PROJECT, title: { en: "Traffic" } },
    spec: {},
    status: { phase: "Pending" },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: PROJECT, title: { en: "Public air quality" } },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "geojson", "csv"],
      rateLimits: { requestsPerMinute: 600 },
    },
    status: { phase: "Live", sourceUrl: SOURCE_URL },
  },
]);

const PIPELINES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name: "aq-mqtt-ingest", namespace: PROJECT, title: { en: "Air quality MQTT" } },
    spec: { class: "resident", enabled: true, secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }] },
    status: { phase: "Live", sourceUrl: SOURCE_URL },
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name: "hel-news", namespace: PROJECT, title: { en: "City news from the RSS feed" } },
    spec: { class: "batch", schedule: "*/15 * * * *", enabled: false },
    status: { phase: "Live" },
  },
]);

const APPS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: {
      name: "mapa-ovzdusia",
      namespace: PROJECT,
      title: { en: "Air quality map" },
      description: { en: "Stations coloured by PM10" },
    },
    spec: {
      kind: "static",
      visibility: "project",
      lifecycle: "preview",
      embeddable: true,
      dataNeeds: [{ contextSpaceRef: { name: "ovzdusie" }, types: ["AirQualityObserved"], operations: ["queryEntity"] }],
    },
    status: { phase: "Live", observedRevision: "9f1c2ab", sourceUrl: SOURCE_URL },
  },
]);

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: PROJECT },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "public-air", fields: 2 } },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: {
    lane: "yellow",
    phase: "PendingApproval",
    plan: { update: 1 },
    mergeRequest: "https://git.example.sk/city/org/pulls/7",
  },
  planFields: [
    { path: "spec.audience", from: "public", to: "internal" },
    { path: "spec.rateLimits.requestsPerMinute", from: 600, to: 300 },
  ],
};

const RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "public-air",
  appClass: "fullstack",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 12,
  tokensUsed: 48_210,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

const LISTS: Record<string, unknown> = {
  spaces: SPACES,
  endpoints: ENDPOINTS,
  pipelines: PIPELINES,
  apps: APPS,
  datamodels: list([]),
  datasources: list([]),
  changes: list([PROPOSAL]),
  "agent-runs": list([RUN]),
};

/** The API from the browser: the lists above, the one change, the one run; the rest 404. */
async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (route.request().method() !== "GET") {
      return json({ type: "about:blank", title: "Not in the baseline", status: 404 }, 404);
    }
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(PROJECTS);
    if (path.endsWith("/events")) {
      return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    }
    if (path.endsWith(`/agent-runs/${RUN.id}`)) return json(RUN);
    if (path.endsWith(`/changes/${PROPOSAL.metadata.name}`)) return json(PROPOSAL);
    const listed = path.match(new RegExp(`^/api/v1/projects/${PROJECT}/([a-z-]+)$`));
    if (listed && listed[1] in LISTS) return json(LISTS[listed[1]]);
    if (path.startsWith("/api/endpoint/")) return json([]);
    return json({ type: "about:blank", title: "Not in the baseline", status: 404 }, 404);
  });
}

interface Screen {
  name: string;
  path: string;
  /** Anything to do once the page has settled, such as opening the assistant beside a run. */
  then?: (page: Page) => Promise<void>;
}

const SCREENS: Screen[] = [
  { name: "spaces", path: `/projects/${PROJECT}/spaces` },
  { name: "endpoints", path: `/projects/${PROJECT}/endpoints` },
  { name: "pipelines", path: `/projects/${PROJECT}/pipelines` },
  { name: "space-complete", path: `/projects/${PROJECT}/spaces/complete` },
  { name: "approval", path: `/projects/${PROJECT}/approvals/${PROPOSAL.metadata.name}` },
  { name: "apps", path: `/projects/${PROJECT}/apps` },
  {
    name: "app-run-with-dock",
    path: `/projects/${PROJECT}/apps/${RUN.appName}`,
    then: async (page) => {
      const bubble = page.getByRole("button", { name: "Open the assistant" });
      if (await bubble.count()) {
        await bubble.first().click();
      }
    },
  },
  { name: "explore", path: `/projects/${PROJECT}/explore` },
];

const SIZES = [
  // The recording: 1920×1080 at zoom 1.5 lays the page out at 1280 CSS px.
  { name: "1920x1080-zoom1.5", viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1.5 },
  { name: "400px", viewport: { width: 400, height: 860 }, deviceScaleFactor: 1 },
];

for (const size of SIZES) {
  test.describe(size.name, () => {
    test.use({ viewport: size.viewport, deviceScaleFactor: size.deviceScaleFactor });

    for (const screen of SCREENS) {
      test(`${screen.name} at ${size.name}`, async ({ page }) => {
        await stubApi(page);
        // A frozen clock: "n s since the run started" and every other relative time hold still.
        await page.clock.setFixedTime(new Date("2026-09-12T08:10:00Z"));
        await page.goto(`${screen.path}?lang=en`, { waitUntil: "networkidle" });
        // Attached, not visible: at phone width the assistant, open beside a run, takes the screen.
        await expect(page.getByRole("main")).toBeAttached();
        await screen.then?.(page);
        await page.evaluate(() => document.fonts.ready);
        await expect(page).toHaveScreenshot(`${screen.name}-${size.name}.png`, {
          fullPage: true,
          animations: "disabled",
          caret: "hide",
          // Another machine's chromium rasterizes text a shade differently; a layout change is far above this.
          maxDiffPixelRatio: 0.03,
        });
      });
    }
  });
}
