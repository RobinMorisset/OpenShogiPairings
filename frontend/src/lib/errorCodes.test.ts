// Checks that the two ends of the error-localization contract still agree.
//
// The server emits stable `code`s; this frontend maps them to translation keys;
// the catalogues hold the text. A break anywhere along that chain fails
// *silently* — the referee just sees the server's English fallback, which looks
// enough like a real message that nobody reports it. Nothing else in the build
// spans the language boundary, so this test does: it reads the Rust registry
// directly.

import { describe, expect, it } from "vitest";

// Read as text by Vite, so this needs no Node types and breaks loudly at import
// time if the file is ever moved or renamed.
import REGISTRY_SOURCE from "../../../crates/server/src/error_codes.rs?raw";
import { ApiError } from "./api";
import { describeApiError, ERROR_CODE_KEYS } from "./errorCodes";
import en from "./i18n/locales/en.json";

const REGISTRY = "crates/server/src/error_codes.rs";

/**
 * The codes the server can emit.
 *
 * `error_codes.rs` holds nothing but the list precisely so this parse is safe:
 * every string literal in it is a code. A Rust-side test rejects codes that
 * aren't lowercase snake_case, so this pattern can't quietly miss one.
 */
function serverCodes(): string[] {
  const source = REGISTRY_SOURCE;
  const body = source.slice(source.indexOf("LOCALIZED_ERROR_CODES"));
  const codes = [...body.matchAll(/"([a-z0-9_]+)"/g)].map((m) => m[1]);
  if (codes.length === 0) {
    throw new Error(
      `no error codes parsed from ${REGISTRY} — the file's shape changed and ` +
        `this test would silently pass while checking nothing`,
    );
  }
  return codes;
}

/** Look up a dotted key like `serverError.roundHasResults` in a catalogue. */
function lookup(catalogue: unknown, key: string): unknown {
  return key
    .split(".")
    .reduce<unknown>(
      (node, part) =>
        node && typeof node === "object"
          ? (node as Record<string, unknown>)[part]
          : undefined,
      catalogue,
    );
}

describe("server error codes", () => {
  it("all have a translation key", () => {
    const missing = serverCodes().filter((code) => !ERROR_CODE_KEYS[code]);
    expect(
      missing,
      `these codes would fall back to the server's English message`,
    ).toEqual([]);
  });

  it("map only to keys that exist in the catalogue", () => {
    const dangling = Object.entries(ERROR_CODE_KEYS)
      .filter(([, key]) => typeof lookup(en, key) !== "string")
      .map(([code, key]) => `${code} → ${key}`);
    expect(dangling, `these keys are missing from en.json`).toEqual([]);
  });

  it("have no mapping for a code the server never sends", () => {
    const known = new Set(serverCodes());
    const stale = Object.keys(ERROR_CODE_KEYS).filter((c) => !known.has(c));
    expect(stale, `these mappings are dead weight`).toEqual([]);
  });
});

describe("describeApiError", () => {
  // Stand-in for svelte-i18n's `$_`: echoes the key and its values, so these
  // tests assert the routing decision rather than the wording.
  const t = (id: string, opts?: Record<string, unknown>) => {
    const values = (opts?.values ?? {}) as Record<string, string>;
    const args = Object.entries(values)
      .map(([k, v]) => `${k}=${v}`)
      .join(",");
    return args ? `${id}(${args})` : id;
  };

  it("translates a tagged error, passing its interpolation values through", () => {
    // Field order mirrors the wire: the server serializes a BTreeMap, so the
    // values arrive alphabetized.
    const err = new ApiError(
      400,
      "need at least 2 present players (have 0)",
      "not_enough_present_players",
      { have: "0", needed: "2" },
    );
    expect(describeApiError(err, t)).toBe(
      "serverError.notEnoughPresentPlayers(have=0,needed=2)",
    );
  });

  it("shows an untagged error's English message verbatim", () => {
    // Internal errors stay English on purpose: the referee can only reach them
    // through a client bug, and the raw text is what belongs in a bug report.
    const err = new ApiError(404, "no round number 99");
    expect(describeApiError(err, t)).toBe("no round number 99");
  });

  it("falls back to the message when the server sends an unknown code", () => {
    // A code from a newer server than this frontend: better the English
    // sentence than a raw key or a blank banner.
    const err = new ApiError(400, "something new went wrong", "from_the_future");
    expect(describeApiError(err, t)).toBe("something new went wrong");
  });

  it("reports an unreachable server rather than a network stack trace", () => {
    expect(describeApiError(new ApiError(0, "Failed to fetch"), t)).toBe(
      "app.cannotReachServer",
    );
  });
});
