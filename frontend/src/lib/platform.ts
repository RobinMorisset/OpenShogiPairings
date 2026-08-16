// Small platform helpers shared across the app.

/** True when running inside the Tauri desktop shell (vs a plain browser). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Open the print dialog, and resolve when the print is over.
 *
 * In a browser this is just `window.print()`, which blocks until the dialog is
 * done with, and page orientation comes from the CSS `@page` rules. But
 * `window.print()` is a no-op inside WKWebView on macOS, so in Tauri we route
 * through a native command that triggers the platform webview's own print
 * operation — and that path ignores the CSS `@page { size: landscape }`, so
 * `landscape` is forwarded to be applied natively there.
 *
 * Callers that print a document they have just put into the DOM (the QR code,
 * the result sheets) may only take it out again once this has resolved, which
 * is why the native command waits for the print operation rather than merely
 * starting it — see `print_window` in `src-tauri/src/lib.rs`.
 */
export async function printPage(landscape = false): Promise<void> {
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("print_window", { landscape });
    return;
  }
  window.print();
}
