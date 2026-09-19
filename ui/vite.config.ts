/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // `@joinedcontext/sdk` is linked from this repository (T-1439) and carries its own `react` in
  // `sdk/node_modules`. Without deduping, a component from it renders against a second copy of
  // React whose hook dispatcher is null — every hook inside the grid throws "invalid hook call"
  // (measured: `Cannot read properties of null (reading 'useMemo')`).
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  build: {
    outDir: "dist",
  },
  // The policy the Rust server sends (src/server.rs), so the e2e journeys run under the same
  // rules as dev: a library that needs `eval` fails here before it fails there.
  preview: {
    headers: {
      "Content-Security-Policy":
        "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; " +
        "form-action 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; " +
        "font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:",
    },
  },
  server: {
    port: 5173,
    proxy: {
      "/api": "http://localhost:8080",
    },
  },
  test: {
    environment: "jsdom",
    // i18next-icu's ESM build default-imports intl-messageformat, whose CJS entry has no
    // __esModule marker — Node's interop then hands back the namespace object and
    // `new IntlMessageFormat()` throws. Inlining makes vitest resolve both through Vite,
    // which picks the ESM build the browser bundle already uses.
    server: { deps: { inline: ["i18next-icu", "intl-messageformat"] } },
    setupFiles: ["./tests/setup.ts"],
    globals: true,
    css: true,
    include: ["tests/**/*.test.{ts,tsx}"],
    // Rendering the whole app (router, i18n, rjsf) takes seconds on a shared CPU, and the
    // 5 s default would kill a test in the middle of an assertion that is merely slow.
    testTimeout: 20_000,
  },
});
