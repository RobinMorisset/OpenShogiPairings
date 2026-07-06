// Save / load a tournament as a JSON file on the user's machine.
//
// "Save" is a pure client-side download; "load" reads a file and hands back a
// parsed tournament, which the caller then sends to the server (the source of
// truth) via `replaceTournament`.

import type { Tournament } from "./types";

/** File extension used for saved tournaments. */
const FILE_SUFFIX = ".osp.json";

/** Turn a tournament name into a safe file-name stem. */
function slugify(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "tournament";
}

/** Trigger a browser download of the tournament as pretty-printed JSON. */
export function downloadTournament(tournament: Tournament): void {
  const json = JSON.stringify(tournament, null, 2);
  const blob = new Blob([json], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `${slugify(tournament.name)}${FILE_SUFFIX}`;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

/** Minimal structural check that a parsed value looks like a tournament. */
function isTournamentLike(value: unknown): value is Tournament {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    Array.isArray(v.players)
  );
}

/**
 * Read and parse a tournament file chosen by the user.
 *
 * Only does a light shape check here; the server does authoritative validation
 * (including the format version) when the tournament is uploaded.
 */
export async function readTournamentFile(file: File): Promise<Tournament> {
  const text = await file.text();
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error("That file is not valid JSON.");
  }
  if (!isTournamentLike(parsed)) {
    throw new Error("That file does not look like a saved tournament.");
  }
  return parsed;
}
