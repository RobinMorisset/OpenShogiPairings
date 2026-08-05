// `parseTournament` answers one question — "did the referee pick the wrong
// file?" — and leaves every judgement about the contents to the server's import
// endpoint. These tests pin that boundary: the structural rejections, and the
// fact that each one is reported in the referee's own language rather than as
// an English string from this file.

import { describe, expect, it } from "vitest";

import { ERROR_CODE_KEYS } from "./errorCodes";
import { TournamentFileError, parseTournament } from "./tournamentFile";

/** A minimal file that `parseTournament` should accept. */
function saveFile(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    format_version: 5,
    id: "6a1c1e4e-0000-4000-8000-000000000000",
    name: "Paris Open",
    players: [],
    ...overrides,
  });
}

/** Run `parseTournament` and return the error it threw. */
function rejection(text: string): TournamentFileError {
  try {
    parseTournament(text);
  } catch (err) {
    if (err instanceof TournamentFileError) return err;
    throw err;
  }
  throw new Error("parseTournament accepted a file it should have rejected");
}

describe("parseTournament", () => {
  it("accepts a save file", () => {
    expect(parseTournament(saveFile()).name).toBe("Paris Open");
  });

  it("rejects text that is not JSON, and JSON that is not a tournament", () => {
    expect(rejection("not json at all").code).toBe("malformed_save");
    expect(rejection('{"hello": "world"}').code).toBe("malformed_save");
  });

  it("leaves the contents to the server rather than second-guessing it", () => {
    // A format version this build can't read and a blank name are both real
    // rejections — but the server's, made atomically at import, where nothing
    // is registered unless the whole file passes. Pre-judging them here would
    // let this end drift stricter than the authority.
    expect(parseTournament(saveFile({ format_version: 1 })).format_version).toBe(1);
    expect(parseTournament(saveFile({ name: "   " })).name).toBe("   ");
  });

  it("only uses codes the UI can translate", () => {
    // The point of borrowing the server's codes is a translated message; a code
    // with no mapping would quietly fall back to the English `message`.
    const codes = [rejection("not json at all"), rejection('{"hello": "world"}')].map(
      (err) => err.code,
    );
    expect(codes.filter((code) => !ERROR_CODE_KEYS[code])).toEqual([]);
  });
});
