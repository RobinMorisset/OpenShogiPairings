import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// Most unit tests cover the pure TypeScript helpers (grade math, threshold
// normalization, no-show bit-logic, tie-break breakdown) — plain functions with
// no DOM, which is why the environment below is Node. The exception is the
// static public-page export, whose whole job is to render the real components
// and read back the DOM they build; those tests ask for a DOM with a
// `// @vitest-environment jsdom` comment of their own, so nothing else pays for
// one. That exception is what the Svelte plugin and the resolve condition are
// here for.
export default defineConfig({
  plugins: [svelte()],
  resolve: {
    // Resolve packages by their *browser* entry points. Svelte otherwise hands
    // back its server-side runtime, whose `mount` renders nothing at all — and
    // an export test would then pass against an empty document instead of
    // failing.
    conditions: ["browser"],
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
