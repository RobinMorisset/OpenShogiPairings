import type {
  BackupList,
  Counterfactual,
  CounterfactualMode,
  DeletedTournament,
  Handicap,
  HealthStatus,
  NewPlayer,
  Forfeit,
  PublicationState,
  PublicTournamentResponse,
  RatedPlayer,
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
  // `knownVersion` describes the tournament that is *open*, so only a request
  // aimed at that one may declare it. A request aimed elsewhere (the picker
  // seeding a freshly created entry, deleting another tournament) must send no
  // version rather than one from an unrelated counter.
  const declaresVersion =
    mutating &&
    knownVersion !== null &&
    authKind.kind === "tournament" &&
    (authKind.id ?? currentId) === currentId;
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
        ...(declaresVersion ? { "x-tournament-version": String(knownVersion) } : {}),
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

/** What the picker gets back: the tournaments, and whether that is all of them. */
export interface TournamentListing {
  tournaments: TournamentSummary[];
  /**
   * True when the server has an admin password and we didn't present it, so
   * this list holds only the *published* tournaments. The picker turns it into
   * an admin-password prompt — a short list with no explanation would read as
   * "there is nothing here", which is the wrong answer.
   */
  restricted: boolean;
}

/**
 * List the tournaments the picker may show. Sends the admin token if we have
 * one: on a hosted server that is what distinguishes a referee from a stranger
 * who found the URL (see `docs/public-access.md` §3.1).
 */
export function listTournaments(): Promise<TournamentListing> {
  return request<TournamentListing>("/api/tournaments", undefined, ADMIN_AUTH);
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

/**
 * Create a tournament from a save file, optionally with its own password.
 *
 * One atomic request: the server validates the file and registers nothing
 * unless it passes, so a rejected load leaves no half-created tournament to
 * clean up. Like {@link createTournamentEntry} it needs the admin token when
 * the server has an admin password, and stores the returned tournament token
 * so the caller can select it immediately.
 */
export async function importTournament(
  tournament: Tournament,
  password?: string,
): Promise<CreateTournamentResult> {
  const body: { tournament: Tournament; password?: string } = { tournament };
  if (password) body.password = password;
  const result = await request<CreateTournamentResult>(
    "/api/tournaments/import",
    { method: "POST", body: JSON.stringify(body) },
    ADMIN_AUTH,
  );
  if (result.token) setToken(result.id, result.token);
  return result;
}

/**
 * The tournaments in the bin — deleted, but still holding the backups kept with
 * them — most recently deleted first. Needs the admin token when the server has
 * an admin password, like the other registry-level calls.
 */
export function fetchDeletedTournaments(): Promise<DeletedTournament[]> {
  return request<DeletedTournament[]>("/api/tournaments/deleted", undefined, ADMIN_AUTH);
}

/**
 * Bring a deleted tournament back, from one of the backups kept with it —
 * `backupId` picks which, defaulting to the newest, the one taken as it was
 * deleted.
 *
 * It comes back as *itself*: same id, and the backups kept with it become its
 * own again, so it stops being listed as deleted. Restoring one that is already
 * back is a 409 rather than a second copy.
 *
 * `password` is the password it had, required if it had one, and it becomes the
 * restored tournament's own password: the bin is not a way to strip protection
 * off a tournament. The returned token is stored so the caller can select it
 * immediately.
 */
export async function restoreDeletedTournament(
  id: string,
  options: { backupId?: string; password?: string } = {},
): Promise<CreateTournamentResult> {
  const body: { backup_id?: string; password?: string } = {};
  if (options.backupId) body.backup_id = options.backupId;
  if (options.password) body.password = options.password;
  const result = await request<CreateTournamentResult>(
    `/api/tournaments/deleted/${id}/restore`,
    { method: "POST", body: JSON.stringify(body) },
    ADMIN_AUTH,
  );
  if (result.token) setToken(result.id, result.token);
  return result;
}

/** Delete a tournament: its registry entry and persisted file. Its automatic
 *  backups are kept on the server for a month (a final one is taken as it is
 *  deleted), so it can be restored from the picker's recently-deleted list —
 *  see {@link fetchDeletedTournaments}. */
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

// --- Teams -----------------------------------------------------------------
//
// Roster editing for a team tournament. Every one of these is registration-time
// only and team-mode only; the server owns those rules, so a call made outside
// them comes back as an ordinary domain error.

/** Create a team. The name must be non-empty and unique ignoring case. */
export function addTeam(name: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath("/teams"), {
    method: "POST",
    body: JSON.stringify({ name }),
  });
}

/** Rename a team. */
export function renameTeam(teamId: string, name: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/teams/${teamId}`), {
    method: "PUT",
    body: JSON.stringify({ name }),
  });
}

/** Delete a team; its members return to the unassigned pool, still registered. */
export function removeTeam(teamId: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/teams/${teamId}`), {
    method: "DELETE",
  });
}

/** Add a registered player to a team, at the end of its board order. */
export function addTeamMember(
  teamId: string,
  playerId: string,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/teams/${teamId}/members`), {
    method: "POST",
    body: JSON.stringify({ player_id: playerId }),
  });
}

/** Take a player out of a team, back into the unassigned pool. */
export function removeTeamMember(
  teamId: string,
  playerId: string,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/teams/${teamId}/members/${playerId}`),
    { method: "DELETE" },
  );
}

/** Set a team's board order (index 0 = board 1). Must be a permutation of the
 *  team's current members. */
export function setTeamBoardOrder(
  teamId: string,
  order: string[],
): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/teams/${teamId}/board-order`), {
    method: "PUT",
    body: JSON.stringify({ order }),
  });
}

/** Reset a team's board order to descending pairing rating (unrated last). */
export function sortTeamByRating(teamId: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/teams/${teamId}/sort-by-rating`),
    { method: "POST" },
  );
}

/** Apply a manual point bonus (positive `delta`) or malus to a team. Unlike the
 *  roster calls this stays available after finalization. */
export function addTeamAdjustment(
  teamId: string,
  delta: number,
  reason: string,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/teams/${teamId}/adjustments`), {
    method: "POST",
    body: JSON.stringify({ delta, reason }),
  });
}

/** Remove a previously applied team adjustment. */
export function removeTeamAdjustment(
  teamId: string,
  adjustmentId: string,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/teams/${teamId}/adjustments/${adjustmentId}`),
    { method: "DELETE" },
  );
}

/** Set (or clear, with `null`) a player's referee-assigned pairing ELO. */
export function setPairingRating(
  playerId: string,
  pairingRating: number | null,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/players/${playerId}/pairing-rating`),
    { method: "PUT", body: JSON.stringify({ pairing_rating: pairingRating }) },
  );
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
  /** Absent *players*, in either mode — in a team tournament a member can be
   *  absent without their team being, and their board is forfeited for them. */
  absent: number[];
  forced_boards: { player1: number; player2: number }[];
  /** Players forced onto a bye. The engine still byes one more of its own if
   *  what's left over is odd, so any number of these is consistent. */
  forced_byes: number[];
  /** Forced team matches — team mode, where teams are what get paired. */
  forced_matches: { team1: number; team2: number }[];
  /** Teams forced onto a bye, likewise. */
  forced_team_byes: number[];
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
  absent: Forfeit | null,
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

/**
 * The same for a whole team's sit-out, writing the value to every member's
 * entry at once — a team sits out together, and its score for the round is read
 * from entries that have to agree.
 */
export function setTeamSitoutValue(
  roundNumber: number,
  teamId: string,
  value: SitoutValue,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    scopedPath(`/rounds/${roundNumber}/team-sitouts/${teamId}`),
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

/** Add or remove a player's membership in a referee-defined category. */
export function setPlayerCategory(
  id: string,
  categoryId: string,
  member: boolean,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/players/${id}/category`), {
    method: "POST",
    body: JSON.stringify({ category_id: categoryId, member }),
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

/** The current tournament's automatic server-side backups: the directory the
 *  server keeps them in (so the referee can reach the files themselves), and
 *  the backups it holds, newest first. */
export function fetchBackups(): Promise<BackupList> {
  return request<BackupList>(scopedPath("/backups"));
}

/** Restore a backup as the current tournament (like loading a file, but from
 *  the server's own rotating backup store). Resets undo history. */
export function restoreBackup(id: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(scopedPath(`/backups/${id}/restore`), {
    method: "POST",
  });
}

// --- Public read-only access (docs/public-access.md, phase 1) ---------------
//
// Two audiences share this section. The referee half (`fetchPublication`,
// `setPublication`) is ordinary authenticated API. The reader half takes a
// capability key instead of a token and never touches the session machinery
// above — a reader has no tournament "open", no version to declare, and no
// login to be sent to.

/** Whether the currently open tournament is published, and under which key. */
export function fetchPublication(): Promise<PublicationState> {
  return request<PublicationState>(scopedPath("/publication"));
}

/**
 * Publish the currently open tournament, or unpublish it. Publishing always
 * mints a **fresh** key, so calling it again is how a key is rotated —
 * revoking every link already handed out.
 */
export function setPublication(published: boolean): Promise<PublicationState> {
  return request<PublicationState>(scopedPath("/publication"), {
    method: "PUT",
    body: JSON.stringify({ published }),
  });
}

/**
 * The public projection of the currently open tournament, for the static HTML
 * export (`docs/public-access.md` phase 2).
 *
 * The referee's own credentials, not a capability key: the export exists for
 * the desktop deployment, where the reader endpoint listens on a loopback port
 * nobody can reach, so requiring publication first would mean minting a key
 * that points at nothing. Saving the file *is* the act of publishing there.
 *
 * It goes to the server rather than projecting the open tournament here, so the
 * one fail-closed filter decides what a public file may contain.
 */
export function fetchPublicSnapshot(): Promise<PublicTournamentResponse> {
  return request<PublicTournamentResponse>(scopedPath("/public-snapshot"));
}

/**
 * The reader page's own URL, for the referee to print or hand around. Built
 * from the page's origin because that is where the SPA is served from — the
 * API may sit elsewhere in dev, but the link is for a browser, not for fetch.
 */
export function publicPageUrl(id: string, key: string): string {
  return `${window.location.origin}/t/${id}/public?k=${encodeURIComponent(key)}`;
}

function publicApiPath(id: string, key: string, suffix = ""): string {
  return `/api/tournaments/${id}/public${suffix}?k=${encodeURIComponent(key)}`;
}

/**
 * Fetch the public projection of a tournament with a capability key. A wrong
 * or revoked key — like an unknown tournament — is a 404, deliberately: the
 * endpoint must not tell a stranger which ids are real.
 */
export async function fetchPublicTournament(
  id: string,
  key: string,
): Promise<PublicTournamentResponse> {
  const url = await apiUrl(publicApiPath(id, key));
  let response: Response;
  try {
    response = await fetch(url);
  } catch (cause) {
    throw new ApiError(0, cause instanceof Error ? cause.message : String(cause));
  }
  if (!response.ok) {
    throw new ApiError(response.status, `${response.status} ${response.statusText}`);
  }
  return (await response.json()) as PublicTournamentResponse;
}

/** First reconnect delay for a reader's dropped stream, and the ceiling. */
const READER_RECONNECT_MS = 1000;
const READER_RECONNECT_MAX_MS = 30_000;

/** Callbacks for {@link subscribeToPublicTournament}. */
export interface PublicSubscription {
  /** A fresh whole-state payload — sent on connect and on every change. */
  onState: (state: PublicTournamentResponse) => void;
  /** The tournament is gone (deleted while the page was open). */
  onGone: () => void;
  /**
   * This link has been revoked — the referee issued a new one or stopped
   * publishing. Terminal: the subscription stops, and reconnecting would only
   * be refused.
   */
  onRevoked: () => void;
  /** Something arrived that we could not make sense of. */
  onError: (message: string) => void;
}

/**
 * Subscribe a reader to a published tournament.
 *
 * Unlike the referee stream this carries the **whole projection** in each
 * event, not just the version: the payload is identical for every reader and
 * the server serializes it once, so pushing it costs one serialization and
 * zero refetches per change, where "push the version, everyone refetches"
 * costs a hundred of each.
 *
 * Reconnection is handled here rather than left to `EventSource`, which retries
 * immediately and in lockstep. The real burst is not steady state: a server
 * restart or a wifi blip drops every phone in the room at the same instant, and
 * they would all come back at the same instant too. Hence exponential backoff
 * with jitter.
 */
export function subscribeToPublicTournament(
  id: string,
  key: string,
  { onState, onGone, onRevoked, onError }: PublicSubscription,
): () => void {
  let source: EventSource | null = null;
  let retry: ReturnType<typeof setTimeout> | null = null;
  let attempt = 0;
  let closed = false;

  function scheduleReconnect() {
    attempt += 1;
    const ceiling = Math.min(
      READER_RECONNECT_MAX_MS,
      READER_RECONNECT_MS * 2 ** (attempt - 1),
    );
    // Full jitter: anywhere in (0, ceiling], so a herd of clients that dropped
    // together comes back spread out instead of in one thundering instant.
    retry = setTimeout(
      () => {
        retry = null;
        void connect();
      },
      Math.random() * ceiling,
    );
  }

  async function connect() {
    if (closed) return;
    connectionStatus.set("connecting");
    const base = await resolveApiBase();
    if (closed) return;
    const stream = new EventSource(`${base}${publicApiPath(id, key, "/events")}`);
    source = stream;
    stream.onopen = () => {
      attempt = 0;
      connectionStatus.set("online");
    };
    stream.addEventListener("state", (event) => {
      try {
        onState(JSON.parse((event as MessageEvent).data) as PublicTournamentResponse);
      } catch (cause) {
        // The server built this payload itself, so a parse failure is a bug,
        // not weather. Say so rather than sitting on stale standings.
        onError(cause instanceof Error ? cause.message : String(cause));
      }
    });
    stream.addEventListener("gone", () => onGone());
    stream.addEventListener("revoked", () => {
      // Terminal, so stop for good: reconnecting with a key the server has
      // already refused would just be a retry loop nobody can win, and the
      // backoff would keep the page looking like it was merely offline.
      stop();
      onRevoked();
    });
    stream.onerror = () => {
      // Take the retry away from EventSource and back it off ourselves. Also
      // the path for a 503 when the tournament is at its reader cap.
      stream.close();
      if (source === stream) source = null;
      if (closed) return;
      connectionStatus.set("connecting");
      scheduleReconnect();
    };
  }

  /** Close the stream and stop reconnecting. Idempotent. */
  function stop() {
    closed = true;
    connectionStatus.set("offline");
    if (retry) clearTimeout(retry);
    source?.close();
    source = null;
  }

  void connect();
  return stop;
}
