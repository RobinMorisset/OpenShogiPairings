import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  // Don't wipe the terminal — keep Rust/Tauri compiler output visible.
  clearScreen: false,
  // Tauri expects a fixed dev-server port; fail loudly instead of silently
  // hopping to another port if it is taken. `strictPort` is what enforces that,
  // and it is what the fixity is really for — Tauri needs the port to be
  // *known*, not to be 5173. So the number is overridable, and `tauri dev`
  // (which leaves `OSP_DEV_PORT` unset, and whose `devUrl` in tauri.conf.json
  // names 5173) is unaffected.
  //   The point is running two checkouts at once: a second worktree sets
  // OSP_DEV_PORT here, OSP_BIND and OSP_DATA_DIR on the server, VITE_API_BASE to
  // point this frontend at that server, and OSP_EXTRA_ORIGINS to let that server
  // answer this frontend (see `CROSS_ORIGIN_CLIENTS` in crates/server/src/lib.rs).
  server: {
    port: Number(process.env.OSP_DEV_PORT ?? 5173),
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
