import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

export default {
  // Enables TypeScript (and other) preprocessing inside .svelte files.
  preprocess: vitePreprocess(),
};
