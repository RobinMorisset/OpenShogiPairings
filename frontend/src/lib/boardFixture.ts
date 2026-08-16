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
// Only the two states a fixture can legitimately start in are constructible
// here: an ordinary board, and a long game in its starting round. `long_carried`
// and `long_end` are produced by carrying a game forward, never written by hand.

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
