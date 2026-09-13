import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// One script and one stylesheet with fixed names: the Portal inlines both into the preview
// document, which is the only way a bundle reaches a sandboxed frame that has no session to
// fetch assets with (Architecture/19 §1.2). No hashes, no code splitting, no base path.
export default defineConfig({
  base: "./",
  plugins: [react()],
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
    include: ["tests/**/*.test.ts", "tests/**/*.test.tsx"],
  },
});
