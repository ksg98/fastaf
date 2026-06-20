import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    // jsdom simulates a browser environment (DOM API + localStorage)
    environment: "jsdom",
    // Inject expect/describe/it globally so each file need not import them
    globals: true,
    // Setup run before each test file (adds @testing-library/jest-dom matchers)
    setupFiles: ["./src/test/setup.ts"],
    // Coverage report
    coverage: {
      provider: "v8",
      reporter: ["text", "lcov"],
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/test/**", "src/vite-env.d.ts", "src/styles.ts"],
    },
  },
});
