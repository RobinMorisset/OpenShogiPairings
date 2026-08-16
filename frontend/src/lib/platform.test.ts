// @vitest-environment jsdom
// `printPage` picks between two ways of printing, and picking wrong is quiet:
// the native command on a webview that has `window.print()` loses its blocking,
// and `window.print()` on WKWebView does nothing at all. Hence a test per case.
import { afterEach, describe, expect, it, vi } from "vitest";
import { printPage } from "./platform";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
const WINDOWS =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/136.0.0.0 Safari/537.36";

const tauri = () => window as unknown as { __TAURI_INTERNALS__?: unknown };

function runningOn(userAgent: string, desktopApp: boolean) {
  vi.stubGlobal("navigator", { userAgent });
  vi.stubGlobal("print", vi.fn());
  if (desktopApp) tauri().__TAURI_INTERNALS__ = {};
}

afterEach(() => {
  delete tauri().__TAURI_INTERNALS__;
  vi.unstubAllGlobals();
  invoke.mockClear();
});

describe("printPage", () => {
  it("prints in the browser the way a page does", async () => {
    runningOn(MAC, false);
    await printPage();
    expect(window.print).toHaveBeenCalledOnce();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("goes native in the desktop app on macOS, orientation and all", async () => {
    runningOn(MAC, true);
    await printPage(true);
    expect(invoke).toHaveBeenCalledWith("print_window", { landscape: true });
    expect(window.print).not.toHaveBeenCalled();
  });

  it("stays with window.print in the desktop app off macOS", async () => {
    // WebView2 implements it, and blocks until the dialog is done with — which
    // is what the sheets and the QR code need, and what the native path can
    // only manage on macOS.
    runningOn(WINDOWS, true);
    await printPage();
    expect(window.print).toHaveBeenCalledOnce();
    expect(invoke).not.toHaveBeenCalled();
  });
});
