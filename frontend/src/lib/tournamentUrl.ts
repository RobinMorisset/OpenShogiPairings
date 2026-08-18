// The open tournament's identity in the tab's own URL: `/t/{id}` for a
// tournament, `/` for the picker.
//
// This is what lets two browser tabs sit on two different tournaments and
// *stay* there across a reload or a browser restart. The previous scheme — a
// single shared localStorage key — worked while the tabs were open (requests
// are scoped per-id) but collapsed on restore: every tab read the one key and
// landed on whichever tournament was opened last anywhere. The URL is the only
// per-tab place a browser reliably restores, so the id lives there, and the
// shared key survives only as a "last opened" seed for a fresh tab at `/`
// (see `session.ts`) — it never overrides an id present in the URL.
//
// Like `publicAccess.ts` (the `/t/{id}/public` reader page, whose path shape
// this deliberately extends), this is all the routing the referee app has; a
// router would be a lot of machinery for one path with one parameter. And as
// with the reader page, the URL only *names* the tournament — the server
// decides whether it exists and what a visitor may see; an unknown id gets a
// visible "no longer exists" banner over the picker, not a quiet shrug
// (`App.svelte`'s `loadInitial`).
//
// None of this runs under Tauri: `main.ts` never wires it there. The desktop
// app is a single window restored from the bundle at its root URL — there are
// no tabs to tell apart and no address bar, so the localStorage seed alone is
// the right behaviour, and pushing paths onto the bundle protocol's URL would
// only create states a webview reload cannot get back to.

import type { Writable } from "svelte/store";

// Same id shape as `publicAccess.ts`'s PUBLIC_PATH, minus the `/public` leaf.
const TOURNAMENT_PATH = /^\/t\/([0-9a-fA-F-]{36})\/?$/;

/**
 * The tournament this URL opens, or `null` for the picker.
 *
 * `null` also for paths that are neither `/` nor `/t/{id}` — including the
 * reader page's `/t/{id}/public`, which must not open a referee session, and
 * any stray path the SPA fallback served `index.html` for. The wiring below
 * then rewrites the URL to the canonical path for whatever is actually open.
 */
export function readTournamentUrl(url: URL): string | null {
  const match = TOURNAMENT_PATH.exec(url.pathname);
  return match ? match[1] : null;
}

/** The canonical path for a given open tournament (`null` = the picker). */
export function tournamentUrlPath(id: string | null): string {
  return id === null ? "/" : `/t/${id}`;
}

/**
 * The slice of `window` the wiring touches — injectable so tests can drive a
 * fake history instead of jsdom's.
 */
export interface UrlHost {
  location: { readonly href: string };
  history: {
    pushState(data: unknown, unused: string, url: string): void;
    replaceState(data: unknown, unused: string, url: string): void;
  };
  addEventListener(type: "popstate", listener: () => void): void;
}

/**
 * Two-way binding between the URL and `currentTournamentId`, for the lifetime
 * of the tab. Called once at startup, before the app mounts — browser referee
 * tabs only (not the reader page, not Tauri; see `main.ts`).
 *
 * - An id in the URL wins outright: it is what this tab *is*, so it overrides
 *   whatever the localStorage seed put in the store.
 * - The first store value then normalises the URL via `replaceState` — a fresh
 *   tab at `/` restoring the last-opened tournament should show `/t/{id}`, but
 *   the visitor didn't navigate, so it earns no history entry.
 * - Every later change (picker choice, back-to-picker, deletion fallback) is a
 *   navigation and pushes one, which is exactly what makes Back/Forward walk
 *   between tournaments and the picker.
 * - Back/Forward themselves fire `popstate`; the store follows the URL, and
 *   the subscriber sees the path already matching, so nothing loops.
 */
export function wireTournamentUrl(store: Writable<string | null>, win: UrlHost): void {
  const urlId = readTournamentUrl(new URL(win.location.href));
  if (urlId !== null) store.set(urlId);

  let restoring = true;
  store.subscribe((id) => {
    const target = tournamentUrlPath(id);
    if (new URL(win.location.href).pathname !== target) {
      if (restoring) win.history.replaceState(null, "", target);
      else win.history.pushState(null, "", target);
    }
    restoring = false;
  });

  win.addEventListener("popstate", () => {
    store.set(readTournamentUrl(new URL(win.location.href)));
  });
}
