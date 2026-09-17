import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Frontend unit tests (jsdom). `npm test` runs them, `npm run test:coverage`
// enforces the line-coverage floor documented in CONTRIBUTING.md.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    restoreMocks: true,
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/**/*.test.{ts,tsx}", "src/test/**", "src/vite-env.d.ts"],
      reporter: ["text", "html"],
      thresholds: { lines: 75 },
    },
  },
});
