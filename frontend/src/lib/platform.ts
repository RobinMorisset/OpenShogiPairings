// Small platform helpers shared across the app.

/** True when running inside the Tauri desktop shell (vs a plain browser). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Open the print dialog.
 *
 * In a browser this is just `window.print()`, and page orientation comes from
 * the CSS `@page` rules. But `window.print()` is a no-op inside WKWebView on
 * macOS, so in Tauri we route through a native command that triggers the
 * platform webview's own print operation — and that path ignores the CSS
 * `@page { size: landscape }`, so `landscape` is forwarded to be applied
 * natively there.
 */
export async function printPage(landscape = false): Promise<void> {
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("print_window", { landscape });
    return;
  }
  window.print();
}
