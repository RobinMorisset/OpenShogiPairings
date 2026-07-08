// Session token + auth-gate state for remote mode.
//
// In local/embedded mode the server runs without authentication, so none of
// this ever fires. Against a hosted server (see docs/multi-referee-internet.md)
// the API answers 401 until we present a session token obtained from
// `POST /api/login`; we stash that token here and replay it on every request.

import { writable } from "svelte/store";

const TOKEN_KEY = "osp_auth_token";

/**
 * True when the server has demanded a password we don't (yet) have — i.e. a
 * request came back 401. Drives the login overlay; cleared on a successful
 * login. localStorage may be unavailable in some webviews, hence the guards.
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

/** The stored session token, or `null` if we've never logged in. */
export function getToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

/** Persist a session token obtained from a successful login. */
export function setToken(token: string): void {
  try {
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    /* storage unavailable — the token just won't survive a reload */
  }
}

/** Forget the stored token (on logout, or when the server rejects it). */
export function clearToken(): void {
  try {
    localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* nothing to clear */
  }
}
