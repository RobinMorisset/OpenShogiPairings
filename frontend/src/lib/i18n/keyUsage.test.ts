// Checks that the catalogues and the code that reads them still line up.
//
// `scripts/check-i18n-keys.mjs` keeps the locales in step with *each other*;
// nothing checked them against the source, so both kinds of drift were silent:
// a key nobody asks for lingers in every language and gets dutifully
// translated, and a key the code asks for but no catalogue has degrades to a
// fallback that only shows up at runtime, in one view, in one state.
//
// The obstacle is that six sites build their key from a value rather than
// writing it out, so a plain "is this string in the source?" scan would flag
// 36 live keys. Each of those value sets is enumerable — a ts-rs union
// generated from Rust, a runtime array, a literal map — so instead of
// exempting those subtrees, `DYNAMIC_SITES` expands each one into the exact
// keys that site can request, and both directions stay checked. That is what
// catches a new `RuleId` variant landing in Rust with no label to show for it.

import { describe, expect, it } from "vitest";

import CONNECTION_SRC from "../components/ConnectionStatus.svelte?raw";
import RULE_ID_SRC from "../generated/RuleId.ts?raw";
import SCOPE_REASON_SRC from "../generated/ScopeReason.ts?raw";
import SITOUT_VALUE_SRC from "../generated/SitoutValue.ts?raw";
import UNDO_CODE_SRC from "../generated/UndoCode.ts?raw";
import { TRANSLATED_LABELS as TRANSLATED_TIEBREAK_LABELS } from "../tiebreaks";
import { TIEBREAKS } from "../types";
import en from "./locales/en.json";

/**
 * The members of a ts-rs string union, e.g. `export type RuleId = "a" | "b";`.
 *
 * Throws rather than returning nothing if the shape changes: an empty result
 * would silently shrink the set of keys considered used, turning live keys into
 * reported-dead ones — or worse, quietly stop checking that direction.
 */
function unionMembers(source: string, name: string): string[] {
  const declaration = source.match(new RegExp(`export type ${name} =([^;]+);`));
  if (!declaration) {
    throw new Error(`no \`export type ${name} = …\` found — has ts-rs's output changed?`);
  }
  const members = [...declaration[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  if (members.length === 0) {
    throw new Error(`\`${name}\` parsed to zero members — this test would check nothing`);
  }
  return members;
}

/**
 * The `connection.*` keys named in ConnectionStatus's `labelKey` map. The map's
 * values are written out in full, so the scan already sees them; it is the
 * `…Title` sibling built next to them that needs expanding.
 */
function connectionStatusKeys(): string[] {
  const map = CONNECTION_SRC.match(/const labelKey = \{([^}]+)\}/);
  if (!map) throw new Error("no `labelKey` map found in ConnectionStatus.svelte");
  const keys = [...map[1].matchAll(/"([\w.]+)"/g)].map((m) => m[1]);
  if (keys.length === 0) throw new Error("`labelKey` parsed to zero keys");
  return keys.map((key) => `${key}Title`);
}

/**
 * Every key each dynamic site can ask for. A site belongs here when it builds
 * its key from a value; the entry has to name where that value set comes from,
 * so it stays exact instead of becoming a blanket exemption for the subtree.
 */
const DYNAMIC_SITES: { site: string; keys: string[] }[] = [
  {
    site: "tiebreaks.ts tiebreakTitle() — one per TIEBREAKS entry",
    keys: TIEBREAKS.map((t) => `tiebreaks.${t.code}.title`),
  },
  {
    site: "tiebreaks.ts tiebreakLabel() — only the prose labels",
    keys: [...TRANSLATED_TIEBREAK_LABELS].map((code) => `tiebreaks.${code}.label`),
  },
  {
    site: "RoundView.svelte ruleLabel() — one per RuleId (generated from Rust)",
    keys: unionMembers(RULE_ID_SRC, "RuleId").map((r) => `roundView.explanation.rule.${r}`),
  },
  {
    site: "RoundView.svelte probe — one per ScopeReason (generated from Rust)",
    keys: unionMembers(SCOPE_REASON_SRC, "ScopeReason").map(
      (r) => `roundView.probe.scopedOut.${r}`,
    ),
  },
  {
    // Cups and long games are both rejected in team mode, so those reasons have
    // no team wording and fall back to the shared one — see `scopedOutText`.
    site: "RoundView.svelte probe, team mode — one per ScopeReason but `cup` and `mid_long_game`",
    keys: unionMembers(SCOPE_REASON_SRC, "ScopeReason")
      .filter((r) => r !== "cup" && r !== "mid_long_game")
      .map((r) => `roundView.probe.scopedOutTeam.${r}`),
  },
  {
    site: "ResultsView.svelte sit-out cell — one per SitoutValue (generated from Rust)",
    keys: unionMembers(SITOUT_VALUE_SRC, "SitoutValue").map(
      (v) => `resultsView.sitoutWorth.${v}`,
    ),
  },
  {
    site: "ConnectionStatus.svelte — a title beside each state's label",
    keys: connectionStatusKeys(),
  },
  {
    site: "App.svelte undoTitle() — one per UndoCode (generated from Rust)",
    keys: unionMembers(UNDO_CODE_SRC, "UndoCode").map((c) => `undo.${c}`),
  },
];

/** Dotted paths of every leaf string in a catalogue. */
function leafKeys(node: unknown, prefix = "", out: string[] = []): string[] {
  for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object") leafKeys(value, path, out);
    else out.push(path);
  }
  return out;
}

/**
 * Every key-shaped string literal written out in the source.
 *
 * Deliberately generous — any quoted dotted word counts, not just `$_("…")`
 * — because keys legitimately travel as data (`ERROR_CODE_KEYS` maps codes to
 * them). Over-counting can only hide a dead key, never invent one, so the
 * failure mode stays on the safe side. Tests are excluded: a key named in an
 * assertion is not a use.
 */
function appSources(): [string, string][] {
  const modules = import.meta.glob("../../**/*.{ts,svelte}", {
    query: "?raw",
    import: "default",
    eager: true,
  }) as Record<string, string>;

  const sources = Object.entries(modules).filter(([path]) => !path.endsWith(".test.ts"));
  if (sources.length === 0) {
    throw new Error("the source glob matched nothing — this test would check nothing");
  }
  return sources;
}

function staticKeyLiterals(): Set<string> {
  const literals = new Set<string>();
  for (const [, source] of appSources()) {
    for (const match of source.matchAll(/["'`]([\w]+(?:\.[\w]+)+)["'`]/g)) {
      literals.add(match[1]);
    }
  }
  return literals;
}

/**
 * Keys passed to a translate call as a plain literal — `$_("a.b")` and the
 * injected-`t` form the plain-TS helpers use. Narrow on purpose, unlike
 * {@link staticKeyLiterals}: every hit really is a key being looked up, so a
 * hit the catalogue lacks is a typo rather than a false alarm.
 */
function translateCallKeys(): { key: string; file: string }[] {
  const calls: { key: string; file: string }[] = [];
  for (const [path, source] of appSources()) {
    for (const match of source.matchAll(/(?:\$_|\bt)\(\s*"([\w]+(?:\.[\w]+)+)"/g)) {
      calls.push({ key: match[1], file: path.replace(/^\.\.\/\.\.\//, "src/") });
    }
  }
  return calls;
}

describe("i18n key usage", () => {
  const catalogue = new Set(leafKeys(en));
  const requested = new Set([
    ...staticKeyLiterals(),
    ...DYNAMIC_SITES.flatMap((s) => s.keys),
  ]);

  it("has no key the code never asks for", () => {
    const dead = [...catalogue].filter((key) => !requested.has(key));
    expect(dead, "dead keys — remove them from every locale").toEqual([]);
  });

  // The other direction, and the one a human would never notice: a value set
  // grew (a new pairing rule, a new sit-out value) and nobody added its string.
  it.each(DYNAMIC_SITES)("has every key $site can request", ({ keys }) => {
    const missing = keys.filter((key) => !catalogue.has(key));
    expect(missing, "keys this site builds but en.json lacks").toEqual([]);
  });

  // A mistyped key renders as the key itself, in whatever view happens to be
  // showing — cheap to catch here, tedious to notice by hand.
  it("has every key a translate call names outright", () => {
    const calls = translateCallKeys();
    expect(calls.length, "no translate calls matched — the scan is broken").toBeGreaterThan(0);

    const unknown = calls
      .filter(({ key }) => !catalogue.has(key))
      .map(({ key, file }) => `${key} (${file})`);
    expect(unknown, "translate calls naming a key en.json lacks").toEqual([]);
  });
});
