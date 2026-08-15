// Test-only fixture helper: build a `Board` from the shape tests want to talk
// about.
//
// A board's result and its longness live in one tagged union on the wire
// (`GameRecord` in `crates/core/src/round.rs`), because a carried long game must
// have nowhere to put an outcome. That is the right shape for the model and the
// wrong shape for a test fixture, where `{ outcome, long }` is what the case is
// actually about. This folds the latter into the former so the tests stay
// readable.
//
// `withRecord` builds the two states a board can be *created* in: an ordinary
// board, and a long game in its starting round. The other two are made by
// carrying a game forward, which is what `carriedLongGame` does — as one
// operation producing both halves, because that is the only way they occur.
// Nothing should hand-write a lone `long_carried` or `long_end`: half a carried
// game is a state the server refuses to load, so a fixture that can express it
// is a fixture that can test a tournament that cannot exist.

import type { Board, GameRecord, Outcome } from "./types";

/** `Partial<Board>` with the record spelled out as the tests think of it. */
export type BoardFields = Omit<Partial<Board>, "record"> & {
  outcome?: Outcome;
  long?: boolean;
};

/** Fold `outcome`/`long` into the `record` the wire carries. */
export function withRecord(fields: BoardFields): Partial<Board> {
  const { outcome, long, ...rest } = fields;
  if (!long && !outcome) return rest;
  const record: GameRecord = long
    ? { kind: "long_start", outcome: outcome ?? { kind: "pending" } }
    : { kind: "short", outcome: outcome ?? { kind: "pending" } };
  return { ...rest, record };
}

/**
 * A long game as it looks *after* the carry: the inert record left in the round
 * it began, and the live one in the round it finished in.
 *
 * Returned as a pair because that is what the carry writes and what
 * `validate_long_games` insists on — one of them alone is a save the server
 * rejects. `outcome` is the game's result and belongs to the second half only;
 * omit it for a game still being played in its second round.
 *
 * Mirrors `confirm_round_inner`'s carry (`crates/core/src/tournament.rs`), down
 * to the source becoming `Carried`. The handicap needs no handling of its own:
 * it lives inside the outcome, so placing that on the live half places it too.
 */
export function carriedLongGame(
  fields: BoardFields,
): { started: Partial<Board>; ended: Partial<Board> } {
  // `long` is dropped: a carried game is long by construction, so a fixture
  // passing `long: false` here would be describing something that cannot exist.
  const { outcome, long, ...rest } = fields;
  void long;
  return {
    started: { ...rest, record: { kind: "long_carried" } },
    ended: {
      ...rest,
      source: { kind: "carried" },
      record: { kind: "long_end", outcome: outcome ?? { kind: "pending" } },
    },
  };
}
