// Session tokens + auth-gate state (see docs/multi-tournament.md).
//
// In local/embedded mode the server runs without authentication, so none of
// this ever fires. Against a hosted server, two independent passwords may
// gate things: each tournament has its own optional password (token stored
// per-id), and a separate admin password gates creating tournaments and the
// FESA ratings proxy (one token, process-wide). `currentTournamentId` is
// which tournament (if any) is open; `null` shows the picker.

import { writable } from "svelte/store";

const TOKEN_PREFIX = "osp_auth_token:";
const ADMIN_TOKEN_KEY = "osp_admin_token";
const CURRENT_TOURNAMENT_KEY = "osp_current_tournament";

/**
 * True when the *currently open* tournament has demanded a password we don't
 * (yet) have. Drives the login overlay; cleared on a successful login.
 * Deliberately not raised for the admin password (see `api.ts`) — there's no
 * single overlay for that, since it can be needed from the picker (creating a
 * tournament) or in the background (ratings autocomplete), each with its own
 * handling.
 */
export const authRequired = writable(false);

/** Health of the live-update (SSE) connection to the server. */
export type ConnectionStatus = "online" | "connecting" | "offline";

/**
 * Current live-connection health, driven by the SSE stream in `api.ts` and
 * surfaced by the connection indicator. Starts "connecting" until the first
 * successful open. Matters most against a hosted server on flaky venue wifi.
 */
export const connectionStatus = writable<ConnectionStatus>("connecting");

function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/**
 * Which tournament is open, persisted so a reload doesn't bounce back to the
 * picker. `null` means "show the picker". `api.ts` subscribes to this to
 * scope every request and reset its version tracking on a switch.
 */
export const currentTournamentId = writable<string | null>(
  readStorage(CURRENT_TOURNAMENT_KEY),
);
currentTournamentId.subscribe((id) => {
  try {
    if (id) localStorage.setItem(CURRENT_TOURNAMENT_KEY, id);
    else localStorage.removeItem(CURRENT_TOURNAMENT_KEY);
  } catch {
    /* storage unavailable — the selection just won't survive a reload */
  }
});

/** The stored session token for tournament `id`, or `null` if we've never logged in. */
export function getToken(id: string): string | null {
  return readStorage(TOKEN_PREFIX + id);
}

/** Persist a session token obtained from a successful tournament login. */
export function setToken(id: string, token: string): void {
  try {
    localStorage.setItem(TOKEN_PREFIX + id, token);
  } catch {
    /* storage unavailable — the token just won't survive a reload */
  }
}

/** Forget a tournament's stored token (on logout, or when the server rejects it). */
export function clearToken(id: string): void {
  try {
    localStorage.removeItem(TOKEN_PREFIX + id);
  } catch {
    /* nothing to clear */
  }
}

/** The stored admin session token, or `null` if we've never logged in as admin. */
export function getAdminToken(): string | null {
  return readStorage(ADMIN_TOKEN_KEY);
}

/** Persist an admin session token obtained from a successful admin login. */
export function setAdminToken(token: string): void {
  try {
    localStorage.setItem(ADMIN_TOKEN_KEY, token);
  } catch {
    /* storage unavailable — the token just won't survive a reload */
  }
}

/** Forget the stored admin token. */
export function clearAdminToken(): void {
  try {
    localStorage.removeItem(ADMIN_TOKEN_KEY);
  } catch {
    /* nothing to clear */
  }
}
