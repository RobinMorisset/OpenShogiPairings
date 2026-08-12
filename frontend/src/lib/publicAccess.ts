// Reader mode: is this browser tab a public reader page, and for which
// tournament? See docs/public-access.md §5.
//
// The capability URL the referee hands out is `/t/{id}/public?k=…`. Parsing it
// here — once, from `location`, at module load — is all the routing this app
// has: everything else is a single page with tabs, and adding a router for one
// URL would be a lot of machinery for one branch in `App.svelte`.
//
// The mode is *not* decided by this file alone. It picks the URL apart; the
// server decides what a holder of that key may see, and answers 404 if the key
// is wrong, revoked, or the tournament was never published. A reader who edits
// their JavaScript gets a broken page, not a write.

/** A public reader page's target: which tournament, under which key. */
export interface PublicPage {
  id: string;
  key: string;
}

const PUBLIC_PATH = /^\/t\/([0-9a-fA-F-]{36})\/public\/?$/;

/**
 * The reader page this URL asks for, or `null` for the ordinary referee app.
 *
 * A path that looks like a reader page but carries no `k` is still `null`: a
 * bare `/t/{id}/public` grants nothing, so treating it as reader mode would
 * only produce a page that can never load.
 */
export function readPublicPage(url: URL): PublicPage | null {
  const match = PUBLIC_PATH.exec(url.pathname);
  if (!match) return null;
  const key = url.searchParams.get("k");
  if (!key) return null;
  return { id: match[1], key };
}

/**
 * The reader page this tab was opened at, resolved once at startup.
 *
 * Deliberately a snapshot: the reader page has no in-app navigation away from
 * itself, and nothing in the app pushes history, so re-reading `location` later
 * could only ever give the same answer.
 */
export const publicPage: PublicPage | null =
  typeof window === "undefined" ? null : readPublicPage(new URL(window.location.href));
