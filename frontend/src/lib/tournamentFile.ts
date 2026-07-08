// Save / load a tournament as a JSON file on the user's machine.
//
// Two backends behind one API:
//   - Tauri (the primary target): native save/open dialogs + file I/O, via the
//     dialog plugin and small `write_text_file`/`read_text_file` Rust commands.
//   - Browser (dev / fallback): a Blob download and a hidden <input type=file>.
//
// Callers use `saveTournament` / `loadTournament` and never branch on platform.
// Both return a "was it done?" signal so the UI can ignore a cancelled dialog.

import type { Tournament, TournamentSettings } from "./types";
import { isTauri } from "./platform";

/** File extension used for saved tournaments. */
const FILE_SUFFIX = ".osp.json";

/** Filters offered in the native dialogs. */
const DIALOG_FILTERS = [{ name: "Tournament", extensions: ["json", "osp"] }];

/** Turn a tournament name into a safe file-name stem. */
function slugify(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "tournament";
}

/** Pretty-printed JSON we write to disk. */
function serialize(tournament: Tournament): string {
  return JSON.stringify(tournament, null, 2);
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
 * Parse tournament JSON text. Only a light shape check here; the server does
 * authoritative validation (including the format version) on upload.
 */
function parseTournament(text: string): Tournament {
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

/**
 * Save the tournament to a user-chosen file.
 *
 * Returns `true` if the file was written, `false` if the user cancelled the
 * dialog (browser downloads always return `true`).
 */
export function saveTournament(tournament: Tournament): Promise<boolean> {
  return isTauri() ? saveViaTauri(tournament) : saveViaBrowser(tournament);
}

/**
 * Load a tournament from a user-chosen file.
 *
 * Returns the parsed tournament, or `null` if the user cancelled.
 */
export function loadTournament(): Promise<Tournament | null> {
  return isTauri() ? loadViaTauri() : loadViaBrowser();
}

/** Suffix used for exported American Grid documents. */
const GRID_SUFFIX = "-american-grid.txt";

/** Filters offered when saving the grid. */
const GRID_FILTERS = [{ name: "American Grid", extensions: ["txt"] }];

/**
 * Save the American Grid text to a user-chosen file. `name` is the tournament
 * name, used only to seed the default file name.
 *
 * Returns `true` if written, `false` if the user cancelled (browser downloads
 * always return `true`).
 */
export function saveAmericanGrid(name: string, contents: string): Promise<boolean> {
  const fileName = `${slugify(name)}${GRID_SUFFIX}`;
  return isTauri()
    ? saveTextViaTauri(fileName, contents, GRID_FILTERS)
    : saveTextViaBrowser(fileName, contents);
}

/** Suffix used for exported settings files. */
const SETTINGS_SUFFIX = "-settings.json";

/** Filters offered when saving settings. */
const SETTINGS_FILTERS = [{ name: "Settings", extensions: ["json"] }];

/**
 * Save the tournament settings as a pretty-printed JSON file — the shape the
 * server's settings endpoint accepts and the simulation CLI's `--configs` reads.
 * `name` seeds the default file name only.
 *
 * Returns `true` if written, `false` if the user cancelled (browser downloads
 * always return `true`).
 */
export function saveSettings(name: string, settings: TournamentSettings): Promise<boolean> {
  const fileName = `${slugify(name)}${SETTINGS_SUFFIX}`;
  const contents = JSON.stringify(settings, null, 2);
  return isTauri()
    ? saveTextViaTauri(fileName, contents, SETTINGS_FILTERS)
    : saveTextViaBrowser(fileName, contents);
}

async function saveTextViaTauri(
  fileName: string,
  contents: string,
  filters: { name: string; extensions: string[] }[],
): Promise<boolean> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  const { invoke } = await import("@tauri-apps/api/core");

  const path = await save({ defaultPath: fileName, filters });
  if (!path) return false; // cancelled

  await invoke("write_text_file", { path, contents });
  return true;
}

function saveTextViaBrowser(fileName: string, contents: string): Promise<boolean> {
  const blob = new Blob([contents], { type: "text/plain" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
  return Promise.resolve(true);
}

// ---------------------------------------------------------------------------
// Tauri backend
// ---------------------------------------------------------------------------

async function saveViaTauri(tournament: Tournament): Promise<boolean> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  const { invoke } = await import("@tauri-apps/api/core");

  const path = await save({
    defaultPath: `${slugify(tournament.name)}${FILE_SUFFIX}`,
    filters: DIALOG_FILTERS,
  });
  if (!path) return false; // cancelled

  await invoke("write_text_file", { path, contents: serialize(tournament) });
  return true;
}

async function loadViaTauri(): Promise<Tournament | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const { invoke } = await import("@tauri-apps/api/core");

  const selected = await open({
    multiple: false,
    directory: false,
    filters: DIALOG_FILTERS,
  });
  if (typeof selected !== "string") return null; // cancelled

  const text = await invoke<string>("read_text_file", { path: selected });
  return parseTournament(text);
}

// ---------------------------------------------------------------------------
// Browser backend
// ---------------------------------------------------------------------------

function saveViaBrowser(tournament: Tournament): Promise<boolean> {
  const blob = new Blob([serialize(tournament)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `${slugify(tournament.name)}${FILE_SUFFIX}`;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
  return Promise.resolve(true);
}

function loadViaBrowser(): Promise<Tournament | null> {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".json,application/json";
    input.style.display = "none";
    document.body.appendChild(input);

    let settled = false;
    const finish = (result: Tournament | null) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(result);
    };

    input.addEventListener("change", async () => {
      const file = input.files?.[0];
      if (!file) return finish(null);
      try {
        const tournament = parseTournament(await file.text());
        finish(tournament);
      } catch (err) {
        if (!settled) {
          settled = true;
          input.remove();
          reject(err);
        }
      }
    });

    // There is no reliable "cancel" event for a file input, so detect it via the
    // window regaining focus without a file having been chosen.
    window.addEventListener(
      "focus",
      () => setTimeout(() => {
        if (!input.files?.length) finish(null);
      }, 300),
      { once: true },
    );

    input.click();
  });
}
