/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    outDir: "dist",
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
  },
});
