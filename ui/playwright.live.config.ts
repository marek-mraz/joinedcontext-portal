import { defineConfig, devices } from "@playwright/test";

// The journeys of ui/e2e/live run against a live Portal (dev), signed in through Keycloak as
// the demo people, with no stubbed API: what they prove is the whole path (T-0630, TS-12).
//   PORTAL_URL=https://portal.… PORTAL_PASSWORD=… APPROVER_PASSWORD=… \
//     npx playwright test --config playwright.live.config.ts
// Passwords come from the environment only, read from the cluster Secret at run time.
export default defineConfig({
  testDir: "./e2e/live",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: "list",
  timeout: 420_000,
  use: {
    baseURL: process.env.PORTAL_URL ?? "https://portal.2.28.67.127.sslip.io",
    trace: "retain-on-failure",
    viewport: { width: 1600, height: 1000 },
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
