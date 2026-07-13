// Tie-break breakdown helpers.
//
// The tie-break *values* are computed server-side (osp-core), but the Results
// tab shows a per-cell breakdown of how each opponent-sum was built. For the
// Buchholz-cut metrics that means partitioning the opponents into the ones that
// count and the lowest few that are dropped — which must match the ordering of
// osp-core's `sum_dropping_lowest` (sort ascending, skip the first `drop`) so the
// tooltip never disagrees with the number it explains. Kept out of ResultsView
// so the ordering is unit-tested against that invariant.

/**
 * Partition items for a Buchholz-cut breakdown: sort ascending by `value`, then
 * split off the `drop` lowest. `kept` are the contributions the cut sums;
 * `dropped` are the discarded lowest ones. A stable sort keeps input order among
 * equal values. With `drop` ≤ 0 nothing is dropped; with `drop` ≥ the length,
 * everything is dropped and `kept` is empty.
 */
export function partitionDropped<T>(
  items: T[],
  value: (item: T) => number,
  drop: number,
): { kept: T[]; dropped: T[] } {
  const sorted = [...items].sort((a, b) => value(a) - value(b));
  const cut = Math.max(0, drop);
  return { dropped: sorted.slice(0, cut), kept: sorted.slice(cut) };
}

/**
 * Sum a list of opponent scores after dropping the `drop` lowest — the
 * Buchholz-cut family (`drop` = 0 is the plain sum). The scalar the breakdown
 * from {@link partitionDropped} must add up to; mirrors osp-core's
 * `sum_dropping_lowest`.
 */
export function sumDroppingLowest(values: number[], drop: number): number {
  return partitionDropped(values, (v) => v, drop).kept.reduce((sum, v) => sum + v, 0);
}
