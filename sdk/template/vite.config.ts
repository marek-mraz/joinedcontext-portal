import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}", "functions/**/*.test.ts"],
  },
});
