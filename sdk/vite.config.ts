import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const sdk = (file: string): string => fileURLToPath(new URL(`./src/sdk/${file}`, import.meta.url));

/** `@joinedcontext/sdk` and its entry points resolved to this checkout, for the template's tests and build. */
export const sdkAlias = [
  { find: /^@joinedcontext\/sdk$/, replacement: sdk("index.ts") },
  { find: /^@joinedcontext\/sdk\/(server|testing)$/, replacement: sdk("$1.ts") },
  { find: /^@joinedcontext\/sdk\/style\.css$/, replacement: sdk("style.css") },
];

// One script and one stylesheet with fixed names: the Portal inlines both into the preview
// document, which is the only way a bundle reaches a sandboxed frame that has no session to
// fetch assets with (Architecture/19 §1.2). No hashes, no code splitting, no base path.
export default defineConfig({
  base: "./",
  plugins: [react()],
  resolve: { alias: sdkAlias },
  build: {
    cssCodeSplit: false,
    rollupOptions: {
      output: {
        entryFileNames: "kit.js",
        chunkFileNames: "kit-[name].js",
        assetFileNames: "kit.[ext]",
        manualChunks: () => "kit",
      },
    },
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/**/*.test.ts", "tests/**/*.test.tsx", "template/src/**/*.test.{ts,tsx}", "template/functions/**/*.test.ts"],
  },
});
