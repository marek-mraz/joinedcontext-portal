import { defineConfig, devices } from "@playwright/test";

const PORT = 4173;
const BASE_URL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: BASE_URL,
    trace: "on-first-retry",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  // The journeys exercise the built bundle, which is what rust-embed ships inside the binary.
  webServer: {
    // npx, not a package-manager script: webServer runs through /bin/sh and pnpm is not
    // guaranteed to be on PATH there. `--host 127.0.0.1` is not optional: without it vite
    // binds only to ::1 and Playwright's readiness poll on 127.0.0.1 never connects.
    command: `npx vite build && npx vite preview --port ${PORT} --strictPort --host 127.0.0.1`,
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 180_000,
  },
});
