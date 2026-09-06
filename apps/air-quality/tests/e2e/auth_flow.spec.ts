import { expect, test } from "@playwright/test";

const BASE = "/apps/air-quality/";
const ID = "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:station-01";
const SAVE = "Uložiť poznámku";

/** The headers oauth2-proxy puts in front of the app for a signed-in person (AP-28). */
const sidecar = (who: string) => ({
  "x-forwarded-access-token": `token-for-${who}`,
  "x-forwarded-email": `demo.${who}@banskabystrica.sk`,
  "x-forwarded-user": `demo.${who}@banskabystrica.sk`,
});

test.describe("who may write a station note", () => {
  test("an anonymous reader sees the measurements and no note box", async ({ page }) => {
    await page.goto(BASE);

    await expect(page.getByRole("heading", { name: "Štiavničky" })).toBeVisible();
    await expect(page.getByText("34.2 µg/m³")).toBeVisible();
    await expect(page.getByText("Prezeráte anonymne.")).toBeVisible();
    await expect(page.getByRole("button", { name: SAVE })).toHaveCount(0);
  });

  test("a signed-in viewer is named on the page and still has no note box", async ({ page }) => {
    await page.setExtraHTTPHeaders(sidecar("viewer"));
    await page.goto(BASE);

    await expect(page.getByText("demo.viewer@banskabystrica.sk")).toBeVisible();
    await expect(page.getByRole("button", { name: SAVE })).toHaveCount(0);
  });

  test("a steward types a note and the station shows it", async ({ page }) => {
    await page.setExtraHTTPHeaders(sidecar("steward"));
    await page.goto(BASE);

    await expect(page.getByText("demo.steward@banskabystrica.sk")).toBeVisible();
    const box = page.getByLabel(/Poznámka správcu/);
    await box.fill("Senzor vyčistený.");
    await page.getByRole("button", { name: SAVE }).click();

    await expect(page.getByText("Poznámka: Senzor vyčistený.")).toBeVisible();
    await expect(box).toHaveValue("");
  });

  // The button is a convenience; the refusal is the control. These two call the app the way a
  // script would, with no page in the way, so what is asserted is the server's own answer.
  test("the app refuses a note with no forwarded token and does not retry anonymously", async ({
    request,
  }) => {
    const response = await request.post(`${BASE}api/stations/${encodeURIComponent(ID)}/note`, {
      data: { note: "Ktokoľvek." },
    });

    expect(response.status()).toBe(401);
    expect(await response.text()).toContain("signed-in");
  });

  test("a viewer who calls the write directly is refused in the gateway's own words", async ({
    request,
  }) => {
    const response = await request.post(`${BASE}api/stations/${encodeURIComponent(ID)}/note`, {
      headers: sidecar("viewer"),
      data: { note: "Skúšam to aj tak." },
    });

    expect(response.status()).toBe(403);
    expect(await response.text()).toContain("project-steward role");
  });
});
