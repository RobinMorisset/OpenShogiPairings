// The nationalities actually present in a roster.
//
// Two controls pick a nationality — the Players tab's bulk cup-eligibility
// toggle and the licence check — and neither offers a free-text field: a
// nationality nobody registered is never what the referee means, and one typed
// with a different case or spelling than the roster's would silently match
// nobody. So both are driven by the roster itself, from here rather than from a
// copy in each component.
//
// Kept as a standalone module (out of the two components) so it is unit-tested,
// and so the two pickers cannot drift apart on what counts as a nationality.

import type { Player } from "./types";

/**
 * The distinct nationalities among `players`, each with how many carry it,
 * sorted alphabetically.
 *
 * A blank or whitespace-only nationality is not one: those players belong to no
 * country's list and are deliberately unreachable through these pickers (the
 * licence check says how many there are instead, since a roster where the field
 * was never filled in would otherwise check clean).
 *
 * Values are compared exactly as stored — the server uppercases a nationality on
 * registration, so `FR` and `fr` cannot both be in the roster to begin with.
 */
export function registeredNationalities(players: Player[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const p of players) {
    const nat = p.nationality?.trim();
    if (nat) counts.set(nat, (counts.get(nat) ?? 0) + 1);
  }
  return [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0]));
}

/** How many of `players` have no nationality at all, so belong to no list. */
export function withoutNationality(players: Player[]): number {
  return players.filter((p) => !p.nationality?.trim()).length;
}
