// Small platform helpers shared across the app.

/** True when running inside the Tauri desktop shell (vs a plain browser). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * True on macOS — the one webview whose `window.print()` does nothing, so the
 * only place [`printPage`] has to leave the web platform. Every webview that
 * runs this code says "Macintosh" there exactly when it is one: WebView2 says
 * "Windows NT", WebKitGTK "Linux".
 */
function isMac(): boolean {
  return typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");
}

/**
 * Open the print dialog, and resolve when the print is over.
 *
 * Resolving late is the point: the QR code and the result sheets print a
 * document put into the DOM for the occasion and taken out again as soon as
 * this resolves, so a promise that means "the dialog is open" rather than "the
 * print is done" prints the ordinary page instead.
 *
 * `window.print()` gives us exactly that, everywhere it works: it blocks until
 * the dialog is done with, and takes the page orientation from the CSS `@page`
 * rules. WKWebView on macOS is where it does not work — it is a no-op there —
 * so that one platform routes through a native command instead, which reports
 * back when the print operation has run and which needs `landscape` handed to
 * it because it does not read `@page { size: landscape }`. See `print_window`
 * in `src-tauri/src/lib.rs`.
 */
export async function printPage(landscape = false): Promise<void> {
  if (isTauri() && isMac()) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("print_window", { landscape });
    return;
  }
  window.print();
}
