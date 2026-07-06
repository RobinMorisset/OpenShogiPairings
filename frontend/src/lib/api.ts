import type { HealthStatus, NewPlayer, Tournament } from "./types";

// Where the API lives. In browser dev and Tauri the frontend is served from a
// different origin than the server, so we default to the server's dev address.
// Override with `VITE_API_BASE` (e.g. to point referees at a central server).
const API_BASE = import.meta.env.VITE_API_BASE ?? "http://127.0.0.1:3000";

/** Build a full API URL from a path like `/api/health`. */
function apiUrl(path: string): string {
  return `${API_BASE}${path}`;
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
    response = await fetch(apiUrl(path), {
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

/**
 * Fetch the current tournament, or `null` if none has been created yet
 * (the server answers 404 in that case).
 */
export async function fetchTournament(): Promise<Tournament | null> {
  try {
    return await request<Tournament>("/api/tournament");
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) return null;
    throw err;
  }
}

/** Create a new, empty tournament with the given name. */
export function createTournament(name: string): Promise<Tournament> {
  return request<Tournament>("/api/tournament", {
    method: "POST",
    body: JSON.stringify({ name }),
  });
}

/** Replace the current tournament wholesale (used when loading a saved file). */
export function replaceTournament(tournament: Tournament): Promise<Tournament> {
  return request<Tournament>("/api/tournament", {
    method: "PUT",
    body: JSON.stringify(tournament),
  });
}

/** Register a player in the current tournament. */
export function addPlayer(player: NewPlayer): Promise<Tournament> {
  return request<Tournament>("/api/tournament/players", {
    method: "POST",
    body: JSON.stringify(player),
  });
}

/** Remove a player from the current tournament by id. */
export function removePlayer(id: string): Promise<Tournament> {
  return request<Tournament>(`/api/tournament/players/${id}`, {
    method: "DELETE",
  });
}
