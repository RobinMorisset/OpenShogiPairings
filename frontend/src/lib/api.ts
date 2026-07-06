import type { HealthStatus } from "./types";

// Where the API lives. In browser dev and Tauri the frontend is served from a
// different origin than the server, so we default to the server's dev address.
// Override with `VITE_API_BASE` (e.g. to point referees at a central server).
const API_BASE = import.meta.env.VITE_API_BASE ?? "http://127.0.0.1:3000";

/** Build a full API URL from a path like `/api/health`. */
function apiUrl(path: string): string {
  return `${API_BASE}${path}`;
}

/** Ask the server whether it is up. Throws on network or non-2xx errors. */
export async function fetchHealth(): Promise<HealthStatus> {
  const response = await fetch(apiUrl("/api/health"));
  if (!response.ok) {
    throw new Error(`server responded ${response.status} ${response.statusText}`);
  }
  return (await response.json()) as HealthStatus;
}
