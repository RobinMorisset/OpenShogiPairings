// The static HTML export of the public page — docs/public-access.md phase 2.
//
// Phase 1 serves the public projection from the server, which is everything the
// hosted deployment needs and nothing at all for the desktop app: there,
// `osp-server` runs in-process on `127.0.0.1:<random>` and no phone in the room
// can reach it. So the laptop publishes *outward* instead — a folder of plain
// web pages, one per tab, that the referee uploads wherever their club already
// has a website and re-uploads after every round.
//
// Two things make those pages trustworthy rather than merely convenient:
//
//   - **They are rendered by the same components as the live page.** Each body
//     is produced by mounting `PublicSnapshot` and serialising the DOM it
//     builds, so the exported standings cannot disagree with the referee's
//     screen. The alternative — a second, string-building renderer — would be a
//     second thing to keep in sync, and the first time the two drifted the
//     club's website would be quietly wrong (docs/public-access.md §5).
//   - **They are rendered from the server's projection, not from the referee's
//     own view.** `GET /public-snapshot` runs the same exhaustive-destructuring
//     filter the reader endpoint runs, so a field added to `Tournament` cannot
//     reach a public file until someone has decided it belongs there.
//
// The output carries no script at all, so it survives any Content-Security-Policy
// the club's host happens to set, prints, and still shows the standings with
// JavaScript disabled. What a script would have provided is provided otherwise:
// the tab strip becomes real links between **one file per section** (see
// `publicSections`), and the app's floating tooltips become CSS ones.

import { flushSync, mount, unmount } from "svelte";
import { get } from "svelte/store";
import { _, locale } from "svelte-i18n";

import { fetchPublicSnapshot } from "./api";
import PublicSnapshot from "./components/PublicSnapshot.svelte";
import { publicSections, sectionLabel, type PublicSection } from "./publicPage";
import { savePublicPages, slugify, type ExportedFile } from "./tournamentFile";
import type { PublicTournamentResponse } from "./types";

/** Escape text for use in HTML element content or a double-quoted attribute. */
function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Every stylesheet the running app has loaded, as text, in document order.
 *
 * Taken from the live document rather than from an import list, so a rule added
 * to any component is in the export automatically — the same reason the body is
 * rendered by the real components. `<style>` elements (the dev server, and
 * Svelte's injected styles) give their text directly; a built `<link>` is
 * fetched, which is same-origin and already in the browser's cache.
 *
 * The whole app's CSS goes in, not just the selectors this page uses: ~50 kB of
 * mostly-unused rules is a cheaper mistake than a missing one, and "which rules
 * does the public page need?" is exactly the question that would go stale.
 */
export async function collectStyles(doc: Document): Promise<string> {
  const texts = await Promise.all(Array.from(doc.styleSheets).map(readStyleSheet));
  return texts.join("\n");
}

/**
 * One stylesheet's text.
 *
 * A `<style>` element gives it exactly (that is the dev server, and any style
 * Svelte injected). Everything else is read through the CSSOM — no network, and
 * it works from `tauri://` as readily as from `https://`, which matters because
 * the desktop app is the deployment this whole export exists for. Fetching the
 * href is the fallback for the one case the CSSOM refuses, a cross-origin
 * sheet; if that fails too the export stops, rather than quietly writing a file
 * with half its styling that nobody looks at before uploading it.
 */
async function readStyleSheet(sheet: CSSStyleSheet): Promise<string> {
  const node = sheet.ownerNode;
  if (node instanceof HTMLStyleElement) return node.textContent ?? "";
  try {
    return Array.from(sheet.cssRules)
      .map((rule) => rule.cssText)
      .join("\n");
  } catch (err) {
    if (!sheet.href) {
      throw new Error(`could not read a stylesheet with no URL to fall back to: ${err}`);
    }
    const response = await fetch(sheet.href);
    if (!response.ok) {
      throw new Error(`could not read the stylesheet ${sheet.href}: ${response.status}`);
    }
    return await response.text();
  }
}

/**
 * Render the public page to an HTML fragment, by mounting the real component
 * and reading back the DOM it produced.
 *
 * The host is off-screen rather than `display: none`, so anything that ever
 * measures itself gets real numbers; it is removed again whatever happens.
 * `flushSync` is what makes the read safe — Svelte 5 runs effects on a
 * microtask, so `innerHTML` straight after `mount` would miss anything an
 * effect fills in.
 */
export function renderSnapshotBody(
  view: PublicTournamentResponse,
  sections: PublicSection[],
  current: PublicSection,
  generatedAt: string,
): string {
  const host = document.createElement("div");
  host.setAttribute("style", "position:fixed;left:-99999px;top:0");
  document.body.appendChild(host);
  try {
    const app = mount(PublicSnapshot, {
      target: host,
      props: { view, sections, current, generatedAt },
    });
    flushSync();
    // Serialise a *copy*, so the rewrite below cannot disturb the DOM Svelte
    // still owns and is about to unmount.
    const copy = host.cloneNode(true) as HTMLElement;
    anchorTooltips(copy);
    const html = copy.innerHTML;
    void unmount(app);
    return html;
  } finally {
    host.remove();
  }
}

/**
 * Decide, per element, which way its CSS tooltip should open.
 *
 * The tables explain themselves through `data-tip` — who the opponent on a
 * result cell was, how a tie-break was built, why a sit-out scored what it did —
 * and in the app a mousemove handler turns that into a floating panel it clamps
 * to the viewport. {@link TOOLTIP_CSS} replaces the panel; this replaces the
 * clamping, which CSS cannot do on its own. A tooltip on the last column would
 * otherwise hang off the right of the page and give the whole document a
 * horizontal scrollbar, so cells near either end of their row are told to open
 * inwards. It is decided here because *here* the position in the row is known,
 * and in a file nothing ever moves again.
 */
function anchorTooltips(root: HTMLElement): void {
  for (const element of root.querySelectorAll<HTMLElement>("[data-tip]")) {
    if (!element.dataset.tip) {
      element.removeAttribute("data-tip");
      continue;
    }
    element.classList.add("osp-tip");
    const row = element.closest("tr");
    const cells = row ? Array.from(row.children) : [];
    const index = cells.indexOf(element.closest("td, th") ?? element);
    if (index >= 0 && cells.length > 1) {
      const position = index / (cells.length - 1);
      if (position < 0.25) element.classList.add("osp-tip-start");
      else if (position > 0.75) element.classList.add("osp-tip-end");
    }
  }
}

/**
 * The tooltip, in CSS — the app's floating panel, minus the script.
 *
 * Native `title` would be the zero-effort answer and was the first one, but a
 * browser shows it only after a second of hovering and in the OS's own styling,
 * which reads as "the tooltips are gone" next to the app's instant panel. This
 * is the same box (`.cell-tip`) drawn by `:hover::after` instead.
 *
 * Appended after the app's own stylesheet so it wins on equal specificity, and
 * dropped from print, where a tooltip cannot be hovered.
 */
const TOOLTIP_CSS = `
/* --- Static-export tooltips (docs/public-access.md phase 2) --- */
.osp-tip { position: relative; }
.osp-tip::after {
  content: attr(data-tip);
  display: none;
  position: absolute;
  z-index: 1000;
  top: 100%;
  left: 50%;
  transform: translateX(-50%);
  width: max-content;
  max-width: min(21rem, 60vw);
  padding: 0.35rem 0.55rem;
  border-radius: 0.4rem;
  background: var(--text);
  color: var(--bg-surface);
  font-size: 0.78rem;
  font-weight: 400;
  line-height: 1.4;
  text-align: left;
  white-space: pre-line;
  pointer-events: none;
  box-shadow: 0 4px 14px var(--shadow-dropdown);
}
.osp-tip.osp-tip-start::after { left: 0; transform: none; }
.osp-tip.osp-tip-end::after { left: auto; right: 0; transform: none; }
.osp-tip:hover::after { display: block; }
@media print {
  .osp-tip::after { display: none !important; }
}
`;

/** What {@link buildDocument} needs beyond the rendered body. */
export interface DocumentOptions {
  /** The tournament name — the `<title>`, and so the browser tab and bookmark. */
  title: string;
  /** BCP-47 tag for `<html lang>`: the language the body was rendered in. */
  lang: string;
  css: string;
  body: string;
}

/**
 * Assemble the complete, self-contained document.
 *
 * Two choices in the head are deliberate:
 *
 * - **`noindex`.** The file will outlive the tournament by years on somebody's
 *   web server, and it carries forty people's names, clubs and ratings. Putting
 *   it on the club's site is the referee's decision; leaving it in a search
 *   index for good is a different one, and this is the conservative default
 *   (the same reasoning as the capability URL in docs/public-access.md §3.2).
 * - **No `data-theme`.** The app's dark theme is opt-in via that attribute, so
 *   its absence pins the export to light — like the QR code, and for the same
 *   sort of reason: a dark page dropped into a club's white website looks
 *   broken, and this one gets printed.
 */
export function buildDocument({ title, lang, css, body }: DocumentOptions): string {
  return `<!doctype html>
<html lang="${escapeHtml(lang)}">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <meta name="robots" content="noindex" />
    <title>${escapeHtml(title)}</title>
    <style>
${css}
${TOOLTIP_CSS}
    </style>
  </head>
  <body>
${body}
  </body>
</html>
`;
}

/** When the snapshot was taken, in the language the page is rendered in. */
function formatGeneratedAt(when: Date, lang: string): string {
  return new Intl.DateTimeFormat(lang, { dateStyle: "long", timeStyle: "short" }).format(
    when,
  );
}

/**
 * Every page of the export, rendered and named, ready to be written.
 *
 * Split out from {@link exportPublicPage} so it can be tested without a server
 * or a file dialog — and because "what does one export consist of?" is the
 * question this feature is really about.
 */
export function renderPublicPages(
  view: PublicTournamentResponse,
  lang: string,
  css: string,
  when: Date,
): ExportedFile[] {
  const t = get(_);
  const sections = publicSections(view, `${slugify(view.tournament.name)}-public`);
  const generatedAt = formatGeneratedAt(when, lang);
  return sections.map((current) => {
    const label = sectionLabel(current, t);
    // The standings page is the entry point and carries the bare tournament
    // name; the others say which page they are, since that is what a browser
    // tab, a bookmark and a printed header will show.
    const title =
      current.kind === "standings" ? view.tournament.name : `${view.tournament.name} — ${label}`;
    return {
      fileName: current.file,
      contents: buildDocument({
        title,
        lang,
        css,
        body: renderSnapshotBody(view, sections, current, generatedAt),
      }),
    };
  });
}

/**
 * Fetch the projection, render every page, and write them to a folder the
 * referee chooses.
 *
 * Returns how many files were written — `0` if they cancelled the dialog. They
 * all go in one directory because they link to each other by bare file name;
 * splitting them up would leave a navigation strip of dead links.
 *
 * Throws — loudly, for the caller to show — if the projection could not be
 * fetched or the styles could not be read: an export that silently produced an
 * unstyled or half-rendered page would be worse than none, because it would be
 * uploaded before anyone looked at it.
 */
export async function exportPublicPage(when: Date = new Date()): Promise<number> {
  const view = await fetchPublicSnapshot();
  const lang = get(locale) ?? "en";
  const css = await collectStyles(document);
  return await savePublicPages(view.tournament.name, renderPublicPages(view, lang, css, when));
}
