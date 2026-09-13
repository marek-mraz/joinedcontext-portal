/**
 * Load in 60 seconds, through the real UI on dev (T-0630, PL-39, PL-43, PL-44, PL-48, PL-49,
 * MF-39, UI-32, CC-34): a steward adds an HTTP data source, checks it (plan and one fetch of the
 * feed), proposes it; an approver approves; the steward builds a pipeline on it in the studio,
 * tests the mapping on the source's own URL, proposes it; the approver approves; the entities
 * appear in Explore. The clock measures the person's part: from the first click to the second
 * approval, excluding the waits for the platform to merge and reconcile.
 */
import { expect, test } from "@playwright/test";
import { APPROVER, STEWARD, approve, proposedChange, signIn } from "./portal";

const PROJECT = "helsinki";
const FEED = "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/free_bike_status.json";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const SOURCE = `hsl-citybikes-free-${SUFFIX}`;
const PIPELINE = `citybikes-free-${SUFFIX}`;
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";

// One GBFS document in, one Vehicle per bike out (PL-48: the array is split by the runner).
const MAPPING = [
  'let domain = env("JC_ORG_DOMAIN")',
  'let seen = this.last_updated.number().ts_format("2006-01-02T15:04:05Z")',
  "root = this.data.bikes.map_each(b -> {",
  '  "id": "urn:ngsi-ld:Vehicle:%v:helsinki:%v".format($domain, b.bike_id),',
  '  "type": "Vehicle",',
  '  "vehicleType": { "type": "Property", "value": "bicycle" },',
  '  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [b.lon, b.lat] } },',
  '  "serviceStatus": { "type": "Property", "value": if b.is_disabled { "outOfService" } else if b.is_reserved { "reserved" } else { "available" }, "observedAt": $seen },',
  '  "dateObserved": { "type": "Property", "value": $seen }',
  "})",
].join("\n");

test("a data source and a pipeline, checked, tested, proposed and approved through the UI; entities in Explore", async ({ browser }, info) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;
  let personMs = 0;
  const tick = (start: number) => {
    personMs += Date.now() - start;
  };

  // 1. The data source: type first (a type change clears the draft), then the dialog.
  let start = Date.now();
  await page.getByLabel("Type").selectOption("http");
  await page.getByRole("button", { name: "New data source" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel(/^Name/).fill(SOURCE);
  await dialog.getByLabel(/^URL/).fill(FEED);
  await dialog.getByLabel(/^Timeout/).fill("15s");
  await dialog.getByRole("button", { name: "Check" }).click();
  await expect(dialog.getByText("Planned change")).toBeVisible({ timeout: 30_000 });
  const probe = dialog.getByTestId("datasource-probe");
  await expect(probe).toBeVisible({ timeout: 30_000 });
  const probeText = (await probe.textContent()) ?? "";
  info.annotations.push({ type: "probe", description: probeText.slice(0, 200) });
  expect(probeText).toMatch(/records|Not fetched/);
  await dialog.getByRole("button", { name: "Propose change" }).click();
  const sourceChange = await proposedChange(page);
  tick(start);

  // 2. Approved by someone else (CC-34).
  start = Date.now();
  await approve(approver.page, PROJECT, sourceChange);
  tick(start);

  // 3. The pipeline, once the mirror lists the approved source (platform time, not the person's).
  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "networkidle" });
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/datasources`);
        const body = (await answer.json()) as { items?: { metadata: { name: string } }[] };
        return (body.items ?? []).some((item) => item.metadata.name === SOURCE);
      },
      { timeout: 240_000, intervals: [3_000] },
    )
    .toBe(true);
  await page.reload({ waitUntil: "networkidle" });

  start = Date.now();
  await page.getByRole("button", { name: "New pipeline" }).click();
  const studio = page.getByRole("dialog");
  await studio.locator("#studio-source-kind").selectOption("datasource");
  await studio.locator("#studio-datasource").selectOption({ value: SOURCE });
  await studio.locator("#root_name").fill(PIPELINE);
  await studio.locator("#root_period").fill("60s");
  const target = studio.locator("#root_targetEndpoint");
  if ((await target.evaluate((el) => el.tagName)) === "SELECT") {
    await target.selectOption({ value: TARGET });
  } else {
    await target.fill(TARGET);
  }
  await studio.locator("#root_compute_kind").selectOption("bloblang");
  await studio.locator("#root_compute_bloblang").fill(MAPPING);
  await studio.locator("#root_output_type").fill("Vehicle");
  // Propose is closed until the test is green for this mapping (PL-49).
  const propose = studio.getByRole("button", { name: "Propose change" });
  await expect(propose).toBeDisabled();
  await studio.getByRole("button", { name: "Test on the source's URL" }).click();
  await studio.getByRole("button", { name: "Test mapping" }).click();
  const ready = studio.getByText(/All \d+ messages map to entities/);
  await expect(ready).toBeVisible({ timeout: 60_000 });
  info.annotations.push({ type: "test", description: (await ready.textContent()) ?? "" });
  await expect(propose).toBeEnabled();
  await propose.click();
  const pipelineChange = await proposedChange(page);
  tick(start);

  // 4. Approved by someone else again.
  start = Date.now();
  await approve(approver.page, PROJECT, pipelineChange);
  tick(start);
  info.annotations.push({ type: "person-seconds", description: (personMs / 1000).toFixed(1) });
  expect(personMs, "the person's part of Load stays under a minute").toBeLessThan(60_000);

  // 5. The sink is live: Vehicle entities readable through the endpoint (PL-47, T-0646).
  await page.goto(`/projects/${PROJECT}/explore?space=${PROJECT}&endpoint=helsinki-all&lang=en`, { waitUntil: "networkidle" });
  const kind = page.locator("#explore-type");
  if ((await kind.evaluate((el) => el.tagName)) === "SELECT") {
    await kind.selectOption("Vehicle");
  } else {
    await kind.fill("Vehicle");
    await kind.press("Enter");
  }
  await expect
    .poll(
      async () => {
        await page.reload({ waitUntil: "networkidle" });
        const count = await page.getByText(/^\d+ entities$/).first().textContent().catch(() => "0 entities");
        return Number.parseInt(count ?? "0", 10);
      },
      { timeout: 240_000, intervals: [10_000] },
    )
    .toBeGreaterThan(0);

  await steward.context.close();
  await approver.context.close();
});
