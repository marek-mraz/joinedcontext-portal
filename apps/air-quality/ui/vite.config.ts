import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `base` is a build-time constant here and JC_BASE_PATH is the run-time one; they are the
// same value because the reconciler serves the app under exactly one path (AP-14).
export default defineConfig({
  base: process.env.JC_BASE_PATH ?? "/apps/air-quality/",
  plugins: [react()],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/**/*.test.tsx"],
  },
});
