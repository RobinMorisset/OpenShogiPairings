/**
 * The public read-only page (docs/public-access.md), and the two things the app
 * prints for the wall.
 *
 * Publication is per tournament and off by default. Turning it on mints a
 * capability key; the URL built from it is what goes on the wall, and publishing
 * again rotates the key, revoking every link already handed out.
 *
 * The *live link* is not offered in the desktop app: its server listens on a
 * random loopback port, so nobody outside the laptop could reach it. What serves
 * that deployment is the other half of the panel — the static export (phase 2),
 * a folder of plain web pages the referee uploads to wherever the club already
 * has a website. That half is offered everywhere, so the panel itself is too.
 */
import { tick } from "svelte";
import { fetchPublication, setPublication } from "./api";
import { isTauri, printPage } from "./platform";
import { exportPublicPage } from "./publicExport";
import type { SheetPlayer } from "./resultSheets";
import type { PublicationState } from "./types";

interface Deps {
  /** Run an async action with the app's shared busy/error handling. */
  run: (action: () => Promise<void>) => void;
  /** Translate a key (read at call time, so it follows the current locale). */
  t: (key: string) => string;
  /** Ask the referee to confirm an irreversible step. */
  confirm: (message: string) => boolean;
}

export class Publication {
  /** Whether a live link can be reached at all — see the module docs. */
  readonly canPublish = !isTauri();
  /** Whether the panel is open. */
  open = $state(false);
  /** The server's publication state, `null` until fetched. */
  state = $state<PublicationState | null>(null);
  /** Set after a successful copy, so the button can confirm it happened. */
  copiedLink = $state(false);
  /** How many pages the last export wrote, so the button can confirm it
   *  happened — and say how many files to upload. `0` means never, or
   *  cancelled. */
  exportedPages = $state(0);

  #deps: Deps;

  constructor(deps: Deps) {
    this.#deps = deps;
  }

  /** Forget this tournament's publication entirely: showing one tournament's
   *  capability key under another's name is exactly how a link ends up on the
   *  wrong wall. */
  reset() {
    this.state = null;
    this.open = false;
    this.copiedLink = false;
    this.exportedPages = 0;
  }

  /** Pull the state without opening the panel: it is not only the panel's, it
   *  decides whether the point-adjustment form warns that its reason is now read
   *  by players — and a warning that only appears after you have opened an
   *  unrelated panel is no warning at all. */
  async load() {
    this.state = await fetchPublication();
  }

  toggle() {
    this.open = !this.open;
    if (!this.open || !this.canPublish) return;
    this.#deps.run(async () => {
      this.state = await fetchPublication();
    });
  }

  setPublished(published: boolean) {
    // Rotating invalidates links already on the wall, and unpublishing takes
    // the page away from a room that may be reading it — neither is something
    // to discover by having clicked the wrong button.
    const confirmKey = !published
      ? "app.confirmUnpublish"
      : this.state?.published
        ? "app.confirmRotateKey"
        : null;
    if (confirmKey && !this.#deps.confirm(this.#deps.t(confirmKey))) return;
    this.copiedLink = false;
    this.#deps.run(async () => {
      this.state = await setPublication(published);
    });
  }

  copyLink(url: string | null) {
    if (!url) return;
    this.#deps.run(async () => {
      await navigator.clipboard.writeText(url);
      this.copiedLink = true;
    });
  }

  exportPages() {
    this.exportedPages = 0;
    this.#deps.run(async () => {
      this.exportedPages = await exportPublicPage();
    });
  }
}

/** What the result sheets need, built by the caller (see {@link PrintJobs}). */
export interface SheetPrint {
  players: SheetPlayer[];
  rounds: number;
  blanks: number;
}

/**
 * The two "print something other than the page" modes. Each is set only for the
 * duration of its print: the flag switches the print stylesheet over, and — for
 * the sheets — supplies the slips themselves.
 */
export class PrintJobs {
  qr = $state(false);
  sheets = $state<SheetPrint | null>(null);

  #run: Deps["run"];

  constructor(run: Deps["run"]) {
    this.#run = run;
  }

  printQr() {
    this.#run(async () => {
      this.qr = true;
      // `window.print()` snapshots the DOM synchronously, so the class has to
      // have landed before it is called — without this the first print comes
      // out as the ordinary page.
      await tick();
      try {
        await printPage();
      } finally {
        this.qr = false;
      }
    });
  }

  /**
   * `build` is called inside the shared error handling rather than by the
   * caller, so that a player without a tournament number or a standing — which
   * cannot happen once registration is finalized, and the round tabs only exist
   * then — surfaces as an error banner instead of taking the round tab down
   * with it.
   */
  printSheets(build: () => SheetPrint) {
    this.#run(async () => {
      this.sheets = build();
      // As for the QR code: `window.print()` snapshots the DOM synchronously, so
      // the sheets have to be in it before the dialog opens.
      await tick();
      try {
        await printPage();
      } finally {
        this.sheets = null;
      }
    });
  }
}
