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

/**
 * A file we could not even read as a tournament, rejected before the server was
 * asked.
 *
 * `code` and `values` are the *same* stable codes the server answers with (see
 * `crates/server/src/error_codes.rs`), so `describeApiError` renders a
 * translated sentence rather than an English-only one. `message` is the fallback
 * used only if the code ever loses its mapping.
 */
export class TournamentFileError extends Error {
  constructor(
    message: string,
    readonly code: string,
    readonly values: Record<string, string> = {},
  ) {
    super(message);
    this.name = "TournamentFileError";
  }
}

/** Filters offered in the native dialogs. */
const DIALOG_FILTERS = [{ name: "Tournament", extensions: ["json", "osp"] }];

/**
 * Turn a tournament name into a safe file-name stem.
 *
 * Exported because the public-page export names several files from it *and*
 * makes them link to each other by those names — so the naming has to be the
 * one this module will actually write, not a second guess at it.
 */
export function slugify(name: string): string {
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
 * Parse tournament JSON text.
 *
 * Deliberately only a structural check — "did you pick the wrong file?" — with
 * no attempt to judge the contents. `POST /api/tournaments/import` is a single
 * atomic request that validates the file (format version, then the tournament's
 * own invariants) and registers nothing unless it passes, so there is no
 * half-created state for a client-side pre-check to save us from. Duplicating
 * the server's rules here would buy one round-trip and risk this end drifting
 * *stricter* than the authority, rejecting files that are perfectly good.
 */
export function parseTournament(text: string): Tournament {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (err) {
    throw new TournamentFileError("That file is not valid JSON.", "malformed_save", {
      details: err instanceof Error ? err.message : String(err),
    });
  }
  if (!isTournamentLike(parsed)) {
    throw new TournamentFileError(
      "That file does not look like a saved tournament.",
      "malformed_save",
      { details: "no id, name and players fields" },
    );
  }
  return parsed;
}

/** Minimal structural check that a parsed value looks like exported settings. */
function isSettingsLike(value: unknown): value is TournamentSettings {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.pairing === "object" && v.pairing !== null && Array.isArray(v.tiebreaks)
  );
}

/**
 * Parse settings JSON text. Only a light shape check here; the server does
 * authoritative validation when the settings are applied.
 */
function parseSettings(text: string): TournamentSettings {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error("That file is not valid JSON.");
  }
  if (!isSettingsLike(parsed)) {
    throw new Error("That file does not look like exported settings.");
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
  return isTauri()
    ? loadViaTauri(parseTournament, DIALOG_FILTERS)
    : loadViaBrowser(parseTournament);
}

/**
 * Load tournament settings from a user-chosen JSON file (the shape
 * `saveSettings` writes). Returns the parsed settings, or `null` if the user
 * cancelled. Throws if the file isn't valid JSON / doesn't look like settings.
 */
export function loadSettings(): Promise<TournamentSettings | null> {
  return isTauri()
    ? loadViaTauri(parseSettings, SETTINGS_FILTERS)
    : loadViaBrowser(parseSettings);
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

/** One file of a multi-file export: its name, and what goes in it. */
export interface ExportedFile {
  /** Name only, never a path — every file of one export shares a directory. */
  fileName: string;
  contents: string;
}

/**
 * Save the exported public pages (`docs/public-access.md` phase 2) side by side
 * in one directory. `name` is the tournament name, for the dialog's title.
 *
 * Returns how many files were written, `0` if the user cancelled.
 *
 * The names come from the tournament, not from the user, and are deliberately
 * **stable across exports** — no round number in the standings page, no
 * timestamp anywhere. Two reasons. The pages link to each other by bare file
 * name, so a rename would break the navigation between them; and the referee
 * re-exports after every round and uploads over the same place, so the club's
 * link has to keep working. Names that changed each time would leave a trail of
 * stale pages behind, each looking exactly as authoritative as the current one.
 * Re-exporting therefore overwrites, which is the intent.
 */
export function savePublicPages(name: string, files: ExportedFile[]): Promise<number> {
  return isTauri() ? savePagesViaTauri(name, files) : savePagesViaBrowser(files);
}

async function savePagesViaTauri(name: string, files: ExportedFile[]): Promise<number> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const { invoke } = await import("@tauri-apps/api/core");

  // A directory, not a file: an export is several files that have to land
  // together, and asking once beats one save dialog per round.
  const directory = await open({ directory: true, multiple: false, title: name });
  if (typeof directory !== "string") return 0; // cancelled

  for (const file of files) {
    // Forward slashes are accepted by the Windows APIs underneath too, so this
    // needs no per-platform separator.
    await invoke("write_text_file", {
      path: `${directory}/${file.fileName}`,
      contents: file.contents,
    });
  }
  return files.length;
}

/**
 * The browser has no directory picker it can rely on, so each page is an
 * ordinary download. They land in the same folder with the right names, which
 * is all the links between them need — but the browser will ask once whether to
 * allow several downloads. The referee running the hosted deployment has the
 * live link anyway; this transport is for the desktop app.
 */
async function savePagesViaBrowser(files: ExportedFile[]): Promise<number> {
  for (const file of files) {
    await saveTextViaBrowser(file.fileName, file.contents, "text/html");
  }
  return files.length;
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

function saveTextViaBrowser(
  fileName: string,
  contents: string,
  mimeType = "text/plain",
): Promise<boolean> {
  const blob = new Blob([contents], { type: mimeType });
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

async function loadViaTauri<T>(
  parse: (text: string) => T,
  filters: { name: string; extensions: string[] }[],
): Promise<T | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const { invoke } = await import("@tauri-apps/api/core");

  const selected = await open({
    multiple: false,
    directory: false,
    filters,
  });
  if (typeof selected !== "string") return null; // cancelled

  const text = await invoke<string>("read_text_file", { path: selected });
  return parse(text);
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

function loadViaBrowser<T>(parse: (text: string) => T): Promise<T | null> {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".json,application/json";
    input.style.display = "none";
    document.body.appendChild(input);

    let settled = false;
    const finish = (result: T | null) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(result);
    };

    input.addEventListener("change", async () => {
      const file = input.files?.[0];
      if (!file) return finish(null);
      try {
        finish(parse(await file.text()));
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
