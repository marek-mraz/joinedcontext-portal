import { expect, test } from "@playwright/test";

const BASE = "/apps/air-quality/";
const ID = "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:station-01";
const SAVE = "Save note";

/** The headers the edge puts in front of the app for a signed-in person (AP-28, ADR-N-019). */
const edge = (who: string) => ({
  "x-access-token": `token-for-${who}`,
  "x-userinfo": Buffer.from(
    JSON.stringify({ sub: `f:1:demo.${who}`, preferred_username: `demo.${who}`, email: `demo.${who}@hel.fi` }),
  ).toString("base64"),
});

test.describe("who may write a station note", () => {
  test("an anonymous reader sees the measurements and no note box", async ({ page }) => {
    await page.goto(BASE);

    await expect(page.getByRole("heading", { name: "Kallio" })).toBeVisible();
    await expect(page.getByText("34.2 µg/m³")).toBeVisible();
    await expect(page.getByText("You are viewing anonymously.")).toBeVisible();
    await expect(page.getByRole("button", { name: SAVE })).toHaveCount(0);
  });

  test("a signed-in viewer is named on the page and still has no note box", async ({ page }) => {
    await page.setExtraHTTPHeaders(edge("viewer"));
    await page.goto(BASE);

    await expect(page.getByText("demo.viewer@hel.fi")).toBeVisible();
    await expect(page.getByRole("button", { name: SAVE })).toHaveCount(0);
  });

  test("a steward types a note and the station shows it", async ({ page }) => {
    await page.setExtraHTTPHeaders(edge("steward"));
    await page.goto(BASE);

    await expect(page.getByText("demo.steward@hel.fi")).toBeVisible();
    const box = page.getByLabel(/Steward note/);
    await box.fill("Sensor cleaned.");
    await page.getByRole("button", { name: SAVE }).click();

    await expect(page.getByText("Note: Sensor cleaned.")).toBeVisible();
    await expect(box).toHaveValue("");
  });

  // The button is a convenience; the refusal is the control. These two call the app the way a
  // script would, with no page in the way, so what is asserted is the server's own answer.
  test("the app refuses a note with no access token and does not retry anonymously", async ({
    request,
  }) => {
    const response = await request.post(`${BASE}api/stations/${encodeURIComponent(ID)}/note`, {
      data: { note: "Anyone at all." },
    });

    expect(response.status()).toBe(401);
    expect(await response.text()).toContain("signed-in");
  });

  test("a viewer who calls the write directly is refused in the gateway's own words", async ({
    request,
  }) => {
    const response = await request.post(`${BASE}api/stations/${encodeURIComponent(ID)}/note`, {
      headers: edge("viewer"),
      data: { note: "Trying anyway." },
    });

    expect(response.status()).toBe(403);
    expect(await response.text()).toContain("project-steward role");
  });
});
