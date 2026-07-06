import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  // Don't wipe the terminal — keep Rust/Tauri compiler output visible.
  clearScreen: false,
  // Tauri expects a fixed dev-server port; fail loudly instead of silently
  // hopping to another port if 5173 is taken.
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      // `src-tauri` lives under this Vite root, so Vite would otherwise try to
      // watch the Rust build tree. During `tauri dev` cargo constantly writes
      // and locks build artifacts there, which makes Vite's watcher crash with
      // `EBUSY`. The Rust side has its own recompile-on-change, so ignore it.
      ignored: ["**/src-tauri/**"],
    },
  },
});
