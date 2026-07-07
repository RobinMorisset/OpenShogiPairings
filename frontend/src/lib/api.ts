import type {
  BackupInfo,
  Counterfactual,
  Handicap,
  HealthStatus,
  NewPlayer,
  RatedPlayer,
  RoundExplanation,
  Tournament,
  TournamentResponse,
  TournamentSettings,
  Winner,
} from "./types";
import { isTauri } from "./platform";

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

/** Build a full API URL from a path like `/api/health`. */
async function apiUrl(path: string): Promise<string> {
  return `${await resolveApiBase()}${path}`;
}

/** Error carrying the HTTP status, so callers can special-case e.g. 404. */
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Run a fetch and parse the JSON body, throwing an {@link ApiError} on failure. */
async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response;
  try {
    response = await fetch(await apiUrl(path), {
      ...init,
      headers: { "content-type": "application/json", ...init?.headers },
    });
  } catch (cause) {
    // Network-level failure (server down, CORS, etc.).
    throw new ApiError(0, cause instanceof Error ? cause.message : String(cause));
  }

  if (!response.ok) {
    // The server sends `{ "error": "..." }` for handled errors.
    let message = `${response.status} ${response.statusText}`;
    try {
      const body = await response.json();
      if (body && typeof body.error === "string") message = body.error;
    } catch {
      // Non-JSON error body; keep the status text.
    }
    throw new ApiError(response.status, message);
  }

  return (await response.json()) as T;
}

/** Ask the server whether it is up. */
export function fetchHealth(): Promise<HealthStatus> {
  return request<HealthStatus>("/api/health");
}

/** Fetch the FESA rating list (server-cached) for registration autocomplete. */
export function fetchRatings(): Promise<RatedPlayer[]> {
  return request<RatedPlayer[]>("/api/ratings");
}

/** Force the server to re-download the FESA rating list from the website. */
export function refreshRatings(): Promise<RatedPlayer[]> {
  return request<RatedPlayer[]>("/api/ratings/refresh", { method: "POST" });
}

/**
 * Fetch the current tournament, or `null` if none has been created yet
 * (the server answers 404 in that case).
 */
export async function fetchTournament(): Promise<TournamentResponse | null> {
  try {
    return await request<TournamentResponse>("/api/tournament");
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) return null;
    throw err;
  }
}

/** Create a new, empty tournament with the given name. */
export function createTournament(name: string): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament", {
    method: "POST",
    body: JSON.stringify({ name }),
  });
}

/** Replace the current tournament wholesale (used when loading a saved file). */
export function replaceTournament(tournament: Tournament): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament", {
    method: "PUT",
    body: JSON.stringify(tournament),
  });
}

/** Revert the last player change (linear server-side undo history). */
export function undoTournament(): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/undo", { method: "POST" });
}

/**
 * Fetch the American Grid (cross-table) export as plain text, ready to save for
 * an ELO update. Unlike the other endpoints this returns raw text, not JSON.
 */
export async function fetchAmericanGrid(): Promise<string> {
  let response: Response;
  try {
    response = await fetch(await apiUrl("/api/tournament/american-grid"));
  } catch (cause) {
    throw new ApiError(0, cause instanceof Error ? cause.message : String(cause));
  }
  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`;
    try {
      const body = await response.json();
      if (body && typeof body.error === "string") message = body.error;
    } catch {
      // Non-JSON error body; keep the status text.
    }
    throw new ApiError(response.status, message);
  }
  return response.text();
}

/** Update tournament settings (MacMahon groups, degressive schedule, …). The
 *  server stores them normalized (sorted, de-duplicated, removals capped). */
export function updateSettings(
  settings: TournamentSettings,
): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/settings", {
    method: "PUT",
    body: JSON.stringify(settings),
  });
}

/** Finalize registration (prerequisite for starting the first round). When the
 *  cup is enabled, pass the chosen bracket size (8/16/32/64). */
export function finalizeRegistration(
  cupSize?: number,
): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/finalize-registration", {
    method: "POST",
    body: JSON.stringify({ cup_size: cupSize ?? null }),
  });
}

/** Complete the current (in-progress) round. */
export function completeRound(): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/complete-round", {
    method: "POST",
  });
}

/** Cancel the last round (or the open draft), stepping back one stage. */
export function cancelRound(): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/cancel-round", {
    method: "POST",
  });
}

/** Begin drafting the next round (enters the round-draft state). */
export function prepareRound(): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/rounds/prepare", {
    method: "POST",
  });
}

/** The draft customization sent to the server. */
export interface DraftUpdate {
  absent: string[];
  forced_boards: { player1: string; player2: string }[];
  forced_bye?: string | null;
}

/** Edit the current draft (absent set, forced pairings, forced bye). */
export function updateDraft(update: DraftUpdate): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/draft", {
    method: "PUT",
    body: JSON.stringify(update),
  });
}

/** Confirm the draft: pair the remaining players and start the round. */
export function confirmRound(): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/rounds", { method: "POST" });
}

/** Explain a round's Swiss pairings: per-board rule ledger and round report. */
export function fetchRoundExplanation(
  roundNumber: number,
): Promise<RoundExplanation> {
  return request<RoundExplanation>(
    `/api/tournament/rounds/${roundNumber}/explanation`,
  );
}

/** Explain what forcing the pairing `a`–`b` in a round would cost. */
export function fetchCounterfactual(
  roundNumber: number,
  a: string,
  b: string,
): Promise<Counterfactual> {
  return request<Counterfactual>(
    `/api/tournament/rounds/${roundNumber}/counterfactual`,
    { method: "POST", body: JSON.stringify({ mode: "force", a, b }) },
  );
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
    `/api/tournament/rounds/${roundNumber}/boards/${boardIndex}/result`,
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
    `/api/tournament/rounds/${roundNumber}/boards/${boardIndex}/drawn`,
    { method: "POST", body: JSON.stringify({ drawn }) },
  );
}

/** Set or clear a board's handicap (the server freezes the giver from ratings). */
export function setBoardHandicap(
  roundNumber: number,
  boardIndex: number,
  handicap: Handicap | null,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(
    `/api/tournament/rounds/${roundNumber}/boards/${boardIndex}/handicap`,
    { method: "PUT", body: JSON.stringify({ handicap }) },
  );
}

/** Register a player in the current tournament. */
export function addPlayer(player: NewPlayer): Promise<TournamentResponse> {
  return request<TournamentResponse>("/api/tournament/players", {
    method: "POST",
    body: JSON.stringify(player),
  });
}

/** Edit an existing player's fields in place. */
export function editPlayer(id: string, player: NewPlayer): Promise<TournamentResponse> {
  return request<TournamentResponse>(`/api/tournament/players/${id}`, {
    method: "PUT",
    body: JSON.stringify(player),
  });
}

/** Remove a player from the current tournament by id. */
export function removePlayer(id: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(`/api/tournament/players/${id}`, {
    method: "DELETE",
  });
}

/** Set whether a player is eligible for the direct-elimination cup. */
export function setPlayerEligible(
  id: string,
  eligible: boolean,
): Promise<TournamentResponse> {
  return request<TournamentResponse>(`/api/tournament/players/${id}/eligible`, {
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
  return request<TournamentResponse>(`/api/tournament/players/${id}/adjustments`, {
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
    `/api/tournament/players/${id}/adjustments/${adjustmentId}`,
    { method: "DELETE" },
  );
}

/** List automatic server-side backups for the current tournament, newest first. */
export function fetchBackups(): Promise<BackupInfo[]> {
  return request<BackupInfo[]>("/api/tournament/backups");
}

/** Restore a backup as the current tournament (like loading a file, but from
 *  the server's own rotating backup store). Resets undo history. */
export function restoreBackup(id: string): Promise<TournamentResponse> {
  return request<TournamentResponse>(`/api/tournament/backups/${id}/restore`, {
    method: "POST",
  });
}
