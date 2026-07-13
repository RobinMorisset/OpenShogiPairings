import { defineConfig } from "vitest/config";

// Unit tests cover the pure TypeScript helpers (grade math, threshold
// normalization, no-show bit-logic, tie-break breakdown) — plain functions with
// no DOM — so a Node environment and no Svelte plugin are all that's needed.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
