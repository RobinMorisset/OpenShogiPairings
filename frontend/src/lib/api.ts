import type {
  BackupInfo,
  Counterfactual,
  CounterfactualMode,
  Handicap,
  HealthStatus,
  NewPlayer,
  NoShow,
  RatedPlayer,
  RoundExplanation,
  SitoutValue,
  Tournament,
  TournamentResponse,
  TournamentSettings,
  TournamentSummary,
  Winner,
} from "./types";
import { isTauri } from "./platform";
import {
  authRequired,
  clearAdminToken,
  clearToken,
  connectionStatus,
  currentTournamentId,
  getAdminToken,
  getToken,
  setAdminToken,
  setToken,
} from "./session";

// Where the API lives.
//   - Tauri: the server is embedded and bound to an OS-assigned port, so we ask
//     the Rust side for the base URL via the `api_base` command.
//   - Browser: a standalone server on the fixed dev address, overridable with
//     `VITE_API_BASE` (e.g. to point referees at a central server).
// Resolved once and cached.
let apiBasePromise: Promise<string> | null = null;

function resolveApiBase(): Promise<string> {
  if (!apiBasePromise) {
    apiBasePromise = (async () => {
      if (isTauri()) {
        const { invoke } = await import("@tauri-apps/api/core");
        return await invoke<string>("api_base");
      }
      return import.meta.env.VITE_API_BASE ?? "http://127.0.0.1:3000";
    })();
  }
  return apiBasePromise;
}

/**
 * Start resolving the API base without waiting for the answer.
 *
 * Under Tauri this costs a dynamic import plus an IPC round-trip, and nothing
 * about it depends on the UI locale — so `main.ts` kicks it off alongside the
 * locale catalogue rather than letting it queue behind the first render. By the
 * time a component issues a request, `apiBasePromise` is already in flight (or
 * settled) and `apiUrl` resolves immediately. Purely an optimisation: every
 * caller still awaits `resolveApiBase`, so skipping this changes nothing but
 * the timing.
 */
export function prewarmApiBase(): void {
  // Errors are deliberately swallowed: the real caller awaits the same promise
  // and reports failure in context. This must never surface as an unhandled
  // rejection just because we asked early.
  void resolveApiBase().catch(() => {});
}

/** Build a full API URL from a path like `/api/health`. */
async function apiUrl(path: string): Promise<string> {
  return `${await resolveApiBase()}${path}`;
}

// The latest tournament version we've seen from the server. Sent back on every
// mutation as `X-Tournament-Version` so the server can reject an edit based on a
// stale view (409), and used to skip the SSE echo of our own changes. `null`
// until we've loaded a tournament.
let knownVersion: number | null = null;

// The tournament currently open, mirrored from the `currentTournamentId`
// store so every request can be scoped without callers threading an id
// through ~30 functions. Switching tournaments (including to none) resets
// `knownVersion` — a different tournament's version counter means nothing here.
// `subscribe` fires immediately with the current value, so this line must come
// after `knownVersion` is declared above.
let currentId: string | null = null;
currentTournamentId.subscribe((id) => {
  currentId = id;
  knownVersion = null;
});

/** Path scoped to the currently open tournament, e.g. `scopedPath("/players")`. */
function scopedPath(suffix: string): string {
  if (!currentId) throw new Error("no tournament is currently open");
  return `/api/tournaments/${currentId}${suffix}`;
}

/**
 * Which password (if any) a request needs, and whose token to attach.
 *
 * - `tournament`: that tournament's own token (defaults to whichever is
 *   currently open) — a 401 clears it and, only for the currently open
 *   tournament, raises the global {@link authRequired} gate.
 * - `admin`: the process-wide admin token — a 401 just clears it; there's no
 *   single overlay for this (callers handle it locally, e.g. the picker's
 *   create form, or a silently-swallowed ratings fetch).
 * - `none`: no token attached, no gate to raise (health, the public tournament list).
 */
type AuthKind = { kind: "tournament"; id?: string } | { kind: "admin" } | { kind: "none" };

const TOURNAMENT_AUTH: AuthKind = { kind: "tournament" };
const ADMIN_AUTH: AuthKind = { kind: "admin" };
const NO_AUTH: AuthKind = { kind: "none" };

function resolveToken(authKind: AuthKind): string | null {
  if (authKind.kind === "none") return null;
  if (authKind.kind === "admin") return getAdminToken();
  const id = authKind.id ?? currentId;
  return id ? getToken(id) : null;
}

/**
 * Error carrying the HTTP status, so callers can special-case e.g. 404.
 *
 * `code` and `values` are set when the server tags an error for client-side
 * localization (e.g. a CSV import failure): `code` is a stable machine string
 * the UI maps to a translation key, and `values` holds its interpolation
 * arguments. Both are absent for plain message-only errors — `message` is then
 * the (server-provided, English) text to show directly.
 */
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public code?: string,
    public values?: Record<string, string>,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/**
 * Run a fetch and return the raw {@link Response}, throwing an {@link ApiError}
 * on a network failure or a non-2xx status. Callers decode the body (JSON or
 * text) themselves.
 */
async function fetchOk(
  path: string,
  init?: RequestInit,
  authKind: AuthKind = TOURNAMENT_AUTH,
): Promise<Response> {
  let response: Response;
  const token = resolveToken(authKind);
  const method = (init?.method ?? "GET").toUpperCase();
  const mutating = method === "POST" || method === "PUT" || method === "DELETE";
  try {
    response = await fetch(await apiUrl(path), {
      ...init,
      headers: {
        "content-type": "application/json",
        // Replay the session token in remote mode; harmless when the server
        // doesn't require it.
        ...(token ? { authorization: `Bearer ${token}` } : {}),
        // Declare the version this edit is based on, so the server can reject it
        // if another referee has since changed the tournament (409).
        ...(mutating && authKind.kind === "tournament" && knownVersion !== null
          ? { "x-tournament-version": String(knownVersion) }
          : {}),
        ...init?.headers,
      },
    });
  } catch (cause) {
    // Network-level failure (server down, CORS, etc.).
    throw new ApiError(0, cause instanceof Error ? cause.message : String(cause));
  }

  if (response.status === 401) {
    if (authKind.kind === "tournament") {
      const id = authKind.id ?? currentId;
      if (id) clearToken(id);
      // Only raise the overlay for the tournament actually open right now — a
      // 401 against some *other* tournament (e.g. deleting one from the
      // picker) has no overlay to raise; that caller handles its own error.
      if (id && id === currentId) authRequired.set(true);
    } else if (authKind.kind === "admin") {
      clearAdminToken();
    }
    throw new ApiError(401, "authentication required");
  }

  if (!response.ok) {
    // The server sends `{ "error": "..." }`, optionally with a `code` (+ `values`)
    // for errors the client localizes.
    let message = `${response.status} ${response.statusText}`;
    let code: string | undefined;
    let values: Record<string, string> | undefined;
    try {
      const body = await response.json();
      if (body && typeof body.error === "string") message = body.error;
      if (body && typeof body.code === "string") code = body.code;
      if (body && body.values && typeof body.values === "object") values = body.values;
    } catch {
      // Non-JSON error body; keep the status text.
    }
    throw new ApiError(response.status, message, code, values);
  }

  return response;
}

/** Run a fetch and parse the JSON body, throwing an {@link ApiError} on failure. */
async function request<T>(
  path: string,
  init?: RequestInit,
  authKind: AuthKind = TOURNAMENT_AUTH,
): Promise<T> {
  const body = (await (await fetchOk(path, init, authKind)).json()) as T;
  // Track the tournament version from any envelope that carries one, so the next
  // mutation can declare the state it was based on.
  const versioned = body as { version?: unknown };
  if (versioned && typeof versioned.version === "number") {
    // Never move the known version *backward*. A slow GET (a background resync
    // fired on focus/visibility) can resolve after a newer mutation already
    // advanced the version; adopting its stale number would make the next edit
    // send a too-low `X-Tournament-Version` and get a spurious 409. The version
    // only ever legitimately drops on a tournament switch or an SSE (re)connect
    // — both of which reset `knownVersion` to `null` explicitly, so the guard
    // below re-adopts whatever the server reports next.
    if (knownVersion === null || versioned.version >= knownVersion) {
      knownVersion = versioned.version;
    }
  }
  return body;
}

/**
 * Subscribe to server-pushed change notifications (SSE) for the currently
 * open tournament, calling `onChange` whenever it's modified by *another*
 * client, and on every (re)connect so a client that missed events while
 * offline resyncs. Our own edits are skipped (their version echoes what we
 * already hold). Also drives {@link connectionStatus}. Returns an unsubscribe
 * function; the browser's EventSource reconnects on its own if the stream
 * drops. Callers must resubscribe (unsubscribe + call again) when the open
 * tournament changes — this snapshots the id at call time.
 *
 * `reason` tells the caller *why* it fired: `"reconnect"` on (re)connect — the
 * server may have restarted and reset its version counter, so the caller should
 * adopt whatever it now reports unconditionally — versus `"changed"` for a live
 * edit, where a stale resync should not overwrite newer local state.
 */
export function subscribeToChanges(
  onChange: (reason: "reconnect" | "changed") => void,
): () => void {
  const id = currentId;
  if (!id) return () => {};

  let source: EventSource | null = null;
  let closed = false;
  connectionStatus.set("connecting");
  void resolveApiBase().then((base) => {
    if (closed) return;
    source = new EventSource(`${base}/api/tournaments/${id}/events`);
    source.onopen = () => {
      connectionStatus.set("online");
      // Re-adopt the server's version on (re)connect: a restarted server resets
      // its counter, so a stale-high knownVersion would otherwise make us ignore
      // valid updates. Clearing it means the resync below sets it afresh from the
      // server's response (or leaves it null if the server has no tournament).
      knownVersion = null;
      // Resync: catch up on anything missed while disconnected. Cheap and safe —
      // refetch preserves any local "unsaved" state.
      onChange("reconnect");
    };
    source.onerror = () => {
      // EventSource auto-reconnects (readyState CONNECTING) unless it has given
      // up (CLOSED). Reflect that as reconnecting vs. offline.
      connectionStatus.set(
        source?.readyState === EventSource.CLOSED ? "offline" : "connecting",
      );
    };
    source.addEventListener("changed", (event) => {
      const version = Number((event as MessageEvent).data);
      // Refetch when the change is newer than what we hold, or on a resync
      // signal ("reload" → NaN). Skip the echo of our own edits.
      if (!Number.isFinite(version) || knownVersion === null || version > knownVersion) {
        onChange("changed");
      }
    });
  });
  return () => {
    closed = true;
    connectionStatus.set("offline");
    source?.close();
  };
}

/** Ask the server whether it is up. */
export function fetchHealth(): Promise<HealthStatus> {
  return request<HealthStatus>("/api/health", undefined, NO_AUTH);
}

/** List every tournament known to the server, for the picker. Never requires auth. */
export function listTournaments(): Promise<TournamentSummary[]> {
  return request<TournamentSummary[]>("/api/tournaments", undefined, NO_AUTH);
}

/** Result of creating a tournament: its id, and a session token if it has a password. */
export interface CreateTournamentResult {
  id: string;
  token?: string;
}

/**
 * Create a new, empty tournament, optionally with its own password. Requires
 * the admin token if the server has an admin password configured — callers
 * should catch a 401 and prompt for it (see `loginAdmin`). Stores the
 * returned tournament token, if any, so the caller can select it immediately.
 */
export async function createTournamentEntry(
  name: string,
  password?: string,
): Promise<CreateTournamentResult> {
  const body: { name: string; password?: string } = { name };
  if (password) body.password = password;
  const result = (await request<CreateTournamentResult>(
    "/api/tournaments",
    { method: "POST", body: JSON.stringify(body) },
    ADMIN_AUTH,
  )) as CreateTournamentResult;
  if (result.token) setToken(result.id, result.token);
  return result;
}

/** Delete a tournament: its registry entry, persisted file, and backups. */
export function deleteTournamentEntry(id: string): Promise<void> {
  return fetchOk(`/api/tournaments/${id}`, { method: "DELETE" }, { kind: "tournament", id }).then(
    () => undefined,
  );
}

/**
 * Exchange the currently-open tournament's password for a session token. On
 * success the token is stored and the login gate lowered. Deliberately
 * bypasses {@link fetchOk}: a 401 here means "wrong password" for the caller
 * to show, not a reason to re-raise the very gate we're trying to clear.
 */
export async function loginTournament(password: string): Promise<void> {
  if (!currentId) throw new Error("no tournament is currently open");
  let response: Response;
  try {
    response = await fetch(await apiUrl(`/api/tournaments/${currentId}/login`), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password }),
    });
  } catch (cause) {
    throw new ApiError(0, cause instanceof Error ? cause.message : String(cause));
  }
  if (!response.ok) {
    throw new ApiError(response.status, `${response.status} ${response.statusText}`);
  }
  const { token } = (await response.json()) as { token: string };
  setToken(currentId, token);
  authRequired.set(false);
}

/**
 * Exchange the admin password for a session token, storing it. Used before
 * creating a tournament or reaching the ratings proxy on a server that has
 * `OSP_ADMIN_PASSWORD` set. A 401 here means "wrong password".
 */
export async function loginAdmin(password: string): Promise<void> {
  let response: Response;
  try {
    response = await fetch(await apiUrl("/api/admin/login"), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password }),
    });
  } catch (cause) {
    throw new ApiError(0, cause instanceof Error ? cause.message : String(cause));
  }
  if (!response.ok) {
    throw new ApiError(response.status, `${response.status} ${response.statusText}`);
  }
  const { token } = (await response.json()) as { token: string };
  setAdminToken(token);
}

/** Fetch the FESA rating list (server-cached) for registration autocomplete. */
export function fetchRatings(): Promise<RatedPlayer[]> {
  return request<RatedPlayer[]>("/api/ratings", undefined, ADMIN_AUTH);
}

/** Force the server to re-download the FESA rating list from the website. */
export function refreshRatings(): Promise<RatedPlayer[]> {
  return request<RatedPlayer[]>("/api/ratings/refresh", { method: "POST" }, ADMIN_AUTH);
}

/**
 * Fetch the currently open tournament, or `null` if it no longer exists (the
 * server answers 404 in that case).
 */
export async function fetchTournament(): Promise<TournamentResponse | null> {
  try {
    return await request<TournamentResponse>(scopedPath(""));
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) return null;
    throw err;
  }
}

/** Replace the current tournament wholesale (used when loading a saved file). */
export function replaceTournament(tournament: Tournament): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(""), {
    method: "PUT",
    body: JSON.stringify(tournament),
  });
}

/** Revert the last player change (linear server-side undo history). */
export function undoTournament(): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/undo"), { method: "POST" });
}

/**
 * Fetch the American Grid (cross-table) export as plain text, ready to save for
 * an ELO update. Unlike the other endpoints this returns raw text, not JSON.
 */
export async function fetchAmericanGrid(): Promise<string> {
  return (await fetchOk(scopedPath("/american-grid"))).text();
}

/** Update tournament settings (MacMahon groups, degressive schedule, …). The
 *  server stores them normalized (sorted, de-duplicated, removals capped). */
export function updateSettings(
  settings: TournamentSettings,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/settings"), {
    method: "PUT",
    body: JSON.stringify(settings),
  });
}

/** Cancel the last round (or the open draft), stepping back one stage. */
export function cancelRound(): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/cancel-round"), {
    method: "POST",
  });
}

/** Begin drafting the next round (enters the round-draft state). For the first
 *  round this also finalizes registration in the same step; when the cup is
 *  enabled, pass the chosen bracket size (8/16/32/64). */
export function prepareRound(cupSize?: number): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/rounds/prepare"), {
    method: "POST",
    body: JSON.stringify({ cup_size: cupSize ?? null }),
  });
}

/** The draft customization sent to the server. */
export interface DraftUpdate {
  absent: number[];
  forced_boards: { player1: number; player2: number }[];
  /** Players forced onto a bye. The engine still byes one more of its own if
   *  what's left over is odd, so any number of these is consistent. */
  forced_byes: number[];
}

/** Edit the current draft (absent set, forced pairings, forced byes). */
export function updateDraft(update: DraftUpdate): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/draft"), {
    method: "PUT",
    body: JSON.stringify(update),
  });
}

/** Confirm the draft: pair the remaining players and start the round. */
export function confirmRound(): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/rounds"), { method: "POST" });
}

/** Explain a round's Swiss pairings: per-board rule ledger and round report. */
export function fetchRoundExplanation(
  roundNumber: number,
): Promise<RoundExplanation> {
  return request<RoundExplanation>(scopedPath(`/rounds/${roundNumber}/explanation`));
}

/** Explain what forcing (or forbidding) the pairing `a`–`b` in a round would cost. */
export function fetchCounterfactual(
  roundNumber: number,
  a: number,
  b: number,
  mode: CounterfactualMode = "force",
): Promise<Counterfactual> {
  return request<Counterfactual>(
    scopedPath(`/rounds/${roundNumber}/counterfactual`),
    { method: "POST", body: JSON.stringify({ mode, a, b }) },
  );
}

/** Force the pairing `a`–`b` onto the current round (re-pairs it around that
 *  board). Fails if the round already has recorded results. */
export function forcePairing(a: number, b: number): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/rounds/force-pairing"), {
    method: "POST",
    body: JSON.stringify({ a, b }),
  });
}

/**
 * Register a click on a board's player: toggles that player as the winner
 * (clicking the current winner clears the result).
 */
export function setBoardWinner(
  roundNumber: number,
  boardIndex: number,
  clicked: Winner,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/rounds/${roundNumber}/boards/${boardIndex}/result`),
    { method: "POST", body: JSON.stringify({ clicked }) },
  );
}

/** Set (or clear) a board's "a draw occurred" flag. */
export function setBoardDrawn(
  roundNumber: number,
  boardIndex: number,
  drawn: boolean,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/rounds/${roundNumber}/boards/${boardIndex}/drawn`),
    { method: "POST", body: JSON.stringify({ drawn }) },
  );
}

/**
 * Mark (or clear) a board as a no-show. `absent` names the side(s) that failed
 * to appear (one player or `"both"`); `null` clears the flag back to a normal
 * unplayed board.
 */
export function setBoardNoShow(
  roundNumber: number,
  boardIndex: number,
  absent: NoShow | null,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/rounds/${roundNumber}/boards/${boardIndex}/no-show`),
    { method: "POST", body: JSON.stringify({ absent }) },
  );
}

/**
 * Flag (or unflag) a board as a two-round "long game" (double time control, two
 * points for the winner). Allowed only on the current round; unflagging is
 * allowed even after a result (to demote a game that finished in one round).
 */
export function setBoardLong(
  roundNumber: number,
  boardIndex: number,
  long: boolean,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/rounds/${roundNumber}/boards/${boardIndex}/long`),
    { method: "POST", body: JSON.stringify({ long }) },
  );
}

/** Set or clear a board's handicap (the server freezes the giver from ratings). */
export function setBoardHandicap(
  roundNumber: number,
  boardIndex: number,
  handicap: Handicap | null,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/rounds/${roundNumber}/boards/${boardIndex}/handicap`),
    { method: "PUT", body: JSON.stringify({ handicap }) },
  );
}

/**
 * Set what a round scored a player who sat it out — the `0+` / `0=` / `0−` in
 * their cross-table cell. Past (completed) rounds are fair game: only the score
 * moves, never why the player sat out.
 */
export function setSitoutValue(
  roundNumber: number,
  player: number,
  value: SitoutValue,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/rounds/${roundNumber}/sitouts/${player}`),
    { method: "PUT", body: JSON.stringify({ value }) },
  );
}

/** Register a player in the current tournament. */
export function addPlayer(player: NewPlayer): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/players"), {
    method: "POST",
    body: JSON.stringify(player),
  });
}

/**
 * Import players from a raw CSV file. The file's text is sent as-is to the
 * server, which parses it (column detection, FESA enrichment) and registers the
 * roster as a single all-or-nothing mutation — so one undo reverts the whole
 * import. A malformed file comes back as a 400 {@link ApiError} whose message is
 * shown to the referee.
 */
export function importPlayersCsv(csv: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/players/import-csv"), {
    method: "POST",
    headers: { "content-type": "text/plain" },
    body: csv,
  });
}

/** Edit an existing player's fields in place. */
export function editPlayer(id: string, player: NewPlayer): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/players/${id}`), {
    method: "PUT",
    body: JSON.stringify(player),
  });
}

/** Remove a player from the current tournament by id. */
export function removePlayer(id: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/players/${id}`), {
    method: "DELETE",
  });
}

/** Set whether a player is eligible for the direct-elimination cup. */
export function setPlayerEligible(
  id: string,
  eligible: boolean,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/players/${id}/eligible`), {
    method: "POST",
    body: JSON.stringify({ eligible }),
  });
}

/** Apply a manual point bonus (positive delta) or malus (negative) to a player. */
export function addPointAdjustment(
  id: string,
  delta: number,
  reason: string,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/players/${id}/adjustments`), {
    method: "POST",
    body: JSON.stringify({ delta, reason }),
  });
}

/** Remove a previously applied point adjustment. */
export function removePointAdjustment(
  id: string,
  adjustmentId: string,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/players/${id}/adjustments/${adjustmentId}`),
    { method: "DELETE" },
  );
}

/** List automatic server-side backups for the current tournament, newest first. */
export function fetchBackups(): Promise<BackupInfo[]> {
  return request<BackupInfo[]>(scopedPath("/backups"));
}

/** Restore a backup as the current tournament (like loading a file, but from
 *  the server's own rotating backup store). Resets undo history. */
export function restoreBackup(id: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/backups/${id}/restore`), {
    method: "POST",
  });
}
