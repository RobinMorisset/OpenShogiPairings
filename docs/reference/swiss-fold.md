# Swiss fold

Within a score group (players on equal points), OpenShogiPairings prefers the
**fold** pairing: sort the group by rating (descending; unrated = 1), split it in
half, and pair the Nth player of the top half against the Nth of the bottom half —
for a group of eight, `1-5, 2-6, 3-7, 4-8`. This is the standard Swiss "slip", and
it is the analog of FIDE's S1-vs-S2 bracket pairing.

The fold is the **lowest-priority tier** of the pairing ladder
([`crates/core/src/pairing`](../../crates/core/src/pairing)), below rematch,
score-gap, float, and club. It only decides *which* alternative to use when the
ideal fold is blocked by a higher rule (a rematch, a needed float, …); when the
ideal fold is achievable it costs nothing.

## The penalty: squared deviation

Each in-group edge is scored by how far it deviates from the ideal fold, as a **sum
of squared deviations**. With `ideal(rank)` the fold partner's rank, the cost of
pairing `a` against `b` is `(rank_b − ideal(rank_a))² + (rank_a − ideal(rank_b))²`
(0 for the ideal fold, growing with the mismatch).

We square rather than take the absolute value for three reasons:

- **It spreads an unavoidable deviation** across boards instead of concentrating it
  on one. Given a fixed total displacement, quadratic prefers many small mismatches
  over one large one — so no single player faces an opponent far from what the fold
  intends (the most visible unfairness). A linear `Σ|·|` is indifferent to that.
- **It matches the rest of the engine.** `ScoreGap` and `EloGap` are already squared
  penalties; a linear fold would be the lone exception.
- **Prior art.** pairgoth (a mature Go pairing engine built on the same weighted-
  matching idea) scores its slip/fold seeding with a quadratic shape.

Both forms give the *same* result whenever deviations are 0 or 1 (the common case);
the difference only appears in larger or heavily-constrained groups. The bound stays
polynomial (`≤ 2·(group_size − 1)²` per edge), so it never stresses the `i128` ladder.

Odd groups need no special handling: `ideal_rank` maps every rank continuously, so
the middle player simply carries a small unavoidable deviation, and *which* player
floats up or down is left to the dedicated `FloaterSelection` / `FloatRepeat` tiers —
there is no discrete "the middle player is the floater" rule to get wrong.

## Why we don't closely approximate the FIDE rules

We deliberately stop at "prefer the fold, softly" rather than reproducing the FIDE
(Dutch) system's exact pairing procedure, for two structural reasons:

1. **FIDE's transposition/exchange order isn't a sum of edge weights.** When the ideal
   fold is blocked, FIDE prescribes a precise sequence of bottom-half transpositions
   and cross-half exchanges to try. Reproducing that order exactly inside a min-weight
   matching requires positional weights that grow exponentially with group size — and
   even then only approximate the deeper tie-breaks.
2. **FIDE brackets are formed *after* floaters are chosen.** FIDE resolves who floats,
   *then* builds the bracket to pair. A single global weighted matching does the
   opposite: it works on equal-score groups and lets floaters emerge from the overall
   optimum. So the FIDE bracket the exact ordering is defined over never actually
   exists in this model.

pairgoth reaches the same conclusion — it also works on equal-score groups with soft
additive weights and does not replicate FIDE's bracket sequencing. A detailed attempt
at the exact-ordering approach (a three-tier count/exchange/transposition fold with a
runtime overflow cutoff) was explored and kept on the **`fide-fold-approximation`**
branch for reference, but not merged: the added complexity buys an approximation that
is still not faithful, for the reasons above. The quadratic fold here is the honest
ceiling for a matching-based engine — a smooth preference for the ideal fold, with the
float tiers handling cross-group movement.
