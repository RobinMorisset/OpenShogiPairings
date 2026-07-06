import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  // Tauri expects a fixed dev-server port; fail loudly instead of silently
  // hopping to another port if 5173 is taken.
  server: {
    port: 5173,
    strictPort: true,
  },
});
