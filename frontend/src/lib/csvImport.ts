// Import players in bulk from a CSV file.
//
// Column order is not fixed: the first row must be a header naming each
// column, matched case/accent-insensitively against a small set of known
// aliases. "Last name" and "First name" are required; ELO, Grade,
// Nationality, and Club are optional and, when present, matched the same
// way. Unrecognized columns are ignored. When a row is missing ELO or Grade,
// they're filled in from the FESA ratings list by exact last+first name
// match (case/accent-insensitive), the same list used for autocomplete in
// the add-player form.

import type { NewPlayer, RatedPlayer } from "./types";
import { parseGrade } from "./grade";
import { isTauri } from "./platform";

/** Lowercase + strip diacritics + collapse whitespace, for fuzzy matching. */
function normalize(text: string): string {
  return text
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}

type Field = "last_name" | "first_name" | "rating" | "grade" | "nationality" | "club";

const HEADER_ALIASES: Record<Field, string[]> = {
  last_name: ["last name", "lastname", "nom", "surname"],
  first_name: ["first name", "firstname", "prenom", "given name"],
  rating: ["elo", "rating", "classement"],
  grade: ["grade", "dan kyu", "dan/kyu"],
  nationality: ["nationality", "nat", "nat.", "pays", "country"],
  club: ["club"],
};

function matchColumn(header: string): Field | null {
  const norm = normalize(header);
  for (const [field, aliases] of Object.entries(HEADER_ALIASES) as [Field, string[]][]) {
    if (aliases.some((a) => normalize(a) === norm)) return field;
  }
  return null;
}

/** Split a CSV header line to sniff whether it uses "," or ";" (common in French exports). */
function detectDelimiter(headerLine: string): string {
  const commas = (headerLine.match(/,/g) ?? []).length;
  const semicolons = (headerLine.match(/;/g) ?? []).length;
  return semicolons > commas ? ";" : ",";
}

/** Minimal RFC4180-ish parser: quoted fields, embedded delimiters/newlines, "" escapes. */
function parseCsvRows(text: string, delimiter: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let inQuotes = false;
  const s = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");

  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inQuotes) {
      if (c === '"') {
        if (s[i + 1] === '"') {
          field += '"';
          i++;
        } else {
          inQuotes = false;
        }
      } else {
        field += c;
      }
      continue;
    }
    if (c === '"') {
      inQuotes = true;
    } else if (c === delimiter) {
      row.push(field);
      field = "";
    } else if (c === "\n") {
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
    } else {
      field += c;
    }
  }
  if (field !== "" || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows.filter((r) => r.some((cell) => cell.trim() !== ""));
}

/** Exact (normalized) last+first name match against the FESA ratings list. */
function findFesaMatch(
  lastName: string,
  firstName: string,
  ratings: RatedPlayer[],
): RatedPlayer | undefined {
  const key = `${normalize(lastName)} ${normalize(firstName)}`;
  return ratings.find((r) => `${normalize(r.last_name)} ${normalize(r.first_name)}` === key);
}

export interface ParsedCsvPlayers {
  players: NewPlayer[];
  /** Human-readable messages for rows that couldn't be imported (1-based row numbers, header excluded). */
  errors: string[];
}

/**
 * Parse CSV text into players, filling missing ELO/grade from the FESA list.
 * Throws if the file is empty or lacks a last-name/first-name column.
 */
export function parseCsvPlayers(text: string, ratings: RatedPlayer[]): ParsedCsvPlayers {
  const firstLine = text.split(/\r\n|\r|\n/, 1)[0] ?? "";
  const rows = parseCsvRows(text, detectDelimiter(firstLine));
  if (rows.length === 0) {
    throw new Error("The CSV file is empty.");
  }

  const columns = rows[0].map(matchColumn);
  const lastIdx = columns.indexOf("last_name");
  const firstIdx = columns.indexOf("first_name");
  if (lastIdx === -1 || firstIdx === -1) {
    throw new Error("The CSV must have a last-name column and a first-name column.");
  }
  const ratingIdx = columns.indexOf("rating");
  const gradeIdx = columns.indexOf("grade");
  const natIdx = columns.indexOf("nationality");
  const clubIdx = columns.indexOf("club");

  const players: NewPlayer[] = [];
  const errors: string[] = [];

  for (let r = 1; r < rows.length; r++) {
    const row = rows[r];
    const lastName = (row[lastIdx] ?? "").trim();
    const firstName = (row[firstIdx] ?? "").trim();
    if (!lastName) {
      errors.push(`Row ${r + 1}: missing last name`);
      continue;
    }

    const player: NewPlayer = { last_name: lastName };
    if (firstName) player.first_name = firstName;

    let rating: number | undefined;
    if (ratingIdx !== -1) {
      const raw = (row[ratingIdx] ?? "").trim();
      const n = Number(raw);
      if (raw !== "" && Number.isInteger(n) && n >= 0) rating = n;
    }
    let grade = gradeIdx !== -1 ? (parseGrade((row[gradeIdx] ?? "").trim()) ?? undefined) : undefined;

    if (rating === undefined || grade === undefined) {
      const match = findFesaMatch(lastName, firstName, ratings);
      if (match) {
        if (rating === undefined) {
          rating = match.rating;
          player.fesa_games = match.games;
        }
        if (grade === undefined && match.grade) grade = match.grade;
      }
    }
    if (rating !== undefined) player.rating = rating;
    if (grade !== undefined) player.grade = grade;

    if (natIdx !== -1) {
      const nat = (row[natIdx] ?? "").trim();
      if (nat) player.nationality = nat;
    }
    if (clubIdx !== -1) {
      const cl = (row[clubIdx] ?? "").trim();
      if (cl) player.club = cl;
    }

    players.push(player);
  }

  return { players, errors };
}

/** Filters offered in the native/browser file picker. */
const CSV_FILTERS = [{ name: "CSV", extensions: ["csv"] }];

/**
 * Prompt the user to pick a CSV file and return its raw text, or `null` if
 * the dialog was cancelled.
 */
export function pickCsvFile(): Promise<string | null> {
  return isTauri() ? pickCsvViaTauri() : pickCsvViaBrowser();
}

async function pickCsvViaTauri(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const { invoke } = await import("@tauri-apps/api/core");

  const selected = await open({ multiple: false, directory: false, filters: CSV_FILTERS });
  if (typeof selected !== "string") return null; // cancelled

  return invoke<string>("read_text_file", { path: selected });
}

function pickCsvViaBrowser(): Promise<string | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".csv,text/csv";
    input.style.display = "none";
    document.body.appendChild(input);

    let settled = false;
    const finish = (result: string | null) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(result);
    };

    input.addEventListener("change", async () => {
      const file = input.files?.[0];
      if (!file) return finish(null);
      finish(await file.text());
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
