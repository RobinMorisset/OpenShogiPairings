//! The matching step: hand the scored cost matrix to the blossom solver, and
//! turn its answer back into a round's pairs and bye.
//!
//! The weights themselves come from [`super::rules`]; what lives here is the
//! solver shim (including the width dispatch that keeps the hot arithmetic as
//! narrow as the ladder allows) and [`pair_round_weighted`], the real pairing
//! path.

use std::collections::HashSet;

use typed_index_collections::TiSlice;

use crate::matching::{min_weight_perfect_matching, Weight};
use crate::round::PairingSource;
use crate::settings::TournamentSettings;
use crate::units::UnitKey;

use super::model::{PairingModel, PairingUnit};
use super::rules::{accumulate_edge_rule, bye_cost, Rule};

/// Solve a minimum-weight perfect matching, picking the narrowest edge-weight
/// type that comfortably fits the cost matrix. Rule costs are built as `i128`
/// so the ladder's lexicographic multipliers can never overflow while scoring,
/// but most tournaments' actual ladders (few rules, modest gaps) fit easily in
/// `i32` or `i64` — narrower arithmetic the blossom solver runs faster with.
/// Only a ladder that genuinely needs `i128`'s headroom pays for it.
///
/// The `/ 16` margin covers the solver's internal ×4 weight scaling and its
/// `MAX / 4` "infinity" sentinel with room to spare.
pub(super) fn solve_matching(cost: &[i128], n: usize) -> Vec<usize> {
    dump_matching_instance(cost, n);
    let max = cost.iter().copied().max().unwrap_or(0);
    if max <= i32::MAX as i128 / 16 {
        min_weight_perfect_matching(&narrow::<i32>(cost), n)
    } else if max <= i64::MAX as i128 / 16 {
        min_weight_perfect_matching(&narrow::<i64>(cost), n)
    } else {
        // The narrow tiers require `max <= TYPE::MAX / 16` (headroom for the
        // solver's internal ×4 edge-weight scaling and its `MAX / 4` "infinity"
        // sentinel); the i128 tier needs the same guard, or a cost that reaches
        // this range overflows *inside* the solver — silently, in release.
        // There is no wider integer to fall back to, so this is a hard scale
        // limit, checked here rather than left to corrupt the matching.
        assert!(
            max <= i128::MAX / 16,
            "pairing cost {max} exceeds the matching solver's i128 headroom \
             (i128::MAX / 16): field too large or score/MacMahon spread too wide \
             (known scale limit)"
        );
        min_weight_perfect_matching(cost, n)
    }
}

/// Capture hook: when `OSP_MATCHING_DUMP` names a directory, every cost matrix
/// handed to the solver is also written there as a little-endian binary blob
/// (`b"OSPM1"`, `n` as `u64`, then the `n*n` `i128` values) for offline replay
/// by `integer-blossom`'s `examples/bench.rs --replay` — solver benchmarks on
/// the exact graphs this engine produces rather than synthetic families.
///
/// Best-effort by design: I/O errors are swallowed (a failed dump must never
/// affect pairing), and the cost when the variable is unset is one lazily
/// initialized check. File names carry a process-wide sequence number, so
/// multithreaded runs dump safely — but for a *canonical* capture set, run
/// single-threaded with `--runs 1` (the sequence order is then deterministic).
fn dump_matching_instance(cost: &[i128], n: usize) {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let Some(dir) = DIR.get_or_init(|| {
        let dir = std::env::var_os("OSP_MATCHING_DUMP").map(PathBuf::from)?;
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }) else {
        return;
    };
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut buf = Vec::with_capacity(13 + cost.len() * 16);
    buf.extend_from_slice(b"OSPM1");
    buf.extend_from_slice(&(n as u64).to_le_bytes());
    for c in cost {
        buf.extend_from_slice(&c.to_le_bytes());
    }
    let _ = std::fs::write(dir.join(format!("solve_{seq:05}_n{n}.ospm")), buf);
}

/// Convert a row-major `i128` cost matrix down to a narrower [`Weight`] type.
/// Panics if a value doesn't fit — callers must check the bound first (see
/// [`solve_matching`]).
fn narrow<W>(cost: &[i128]) -> Vec<W>
where
    W: Weight + TryFrom<i128>,
    <W as TryFrom<i128>>::Error: std::fmt::Debug,
{
    cost.iter().map(|&c| W::try_from(c).unwrap()).collect()
}

/// One round's pairing in **unit** terms: who plays whom, and who sits out. The
/// caller turns each pair into the board(s) it stands for — one for a player
/// pairing, `size` for a team match — and each bye into the sit-out(s) it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitPairing {
    /// The matched pairs: the referee's forced ones first (in draft order), then
    /// the engine's own.
    pub pairs: Vec<PairedUnits>,
    /// The bye the engine chose, if the free field was left odd. There is at most
    /// one by construction — a matching has a single phantom.
    pub swiss_bye: Option<UnitKey>,
}

/// One matched pair, with the facts of the pairing the boards must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PairedUnits {
    pub a: UnitKey,
    pub b: UnitKey,
    /// How the pairing was decided (engine or referee).
    pub source: PairingSource,
    /// `points(a) − points(b)` at pairing time, frozen onto every board of the
    /// pairing: it is a fact of *this* pairing, and the float history replays
    /// from it.
    pub points_diff: i32,
}

/// Pair the `present` units by minimizing the total rule penalty, honoring
/// referee-forced pairs and forced byes. This is the real pairing path used by
/// [`crate::Tournament::confirm_round`], for players and teams alike; the rules
/// and their priority are described in the module docs.
///
/// The forced byes are simply taken out of the pool (and not echoed back — the
/// caller already knows them); if what remains is odd the engine still picks a
/// bye of its own, so any number of them is consistent.
///
/// Preconditions (validated by the caller): every forced unit is present and
/// appears at most once.
pub(crate) fn pair_round_weighted(
    number: u32,
    settings: &TournamentSettings,
    units: &TiSlice<UnitKey, PairingUnit>,
    present: &[UnitKey],
    forced_pairs: &[(UnitKey, UnitKey)],
    forced_byes: &[UnitKey],
) -> UnitPairing {
    // The float frozen onto each board: points(a) − points(b) now.
    let diff =
        |a: UnitKey, b: UnitKey| units[a].points.halves() as i32 - units[b].points.halves() as i32;

    let mut placed: HashSet<UnitKey> = HashSet::new();
    for &(a, b) in forced_pairs {
        placed.insert(a);
        placed.insert(b);
    }
    placed.extend(forced_byes.iter().copied());
    let mut free: Vec<UnitKey> = present
        .iter()
        .copied()
        .filter(|key| !placed.contains(key))
        .collect();

    // A phantom vertex absorbs the bye when an odd number of units remain
    // after the forced byes are taken out; whoever the matching pairs with it
    // sits out.
    let need_phantom = free.len() % 2 == 1;
    let k = free.len();
    let vcount = k + usize::from(need_phantom);

    let mut pairs: Vec<PairedUnits> = forced_pairs
        .iter()
        .map(|&(a, b)| PairedUnits {
            a,
            b,
            source: PairingSource::Forced,
            points_diff: diff(a, b),
        })
        .collect();
    let mut swiss_bye = None;

    if vcount >= 2 {
        // The pairing model owns the per-round derived data and the multiplier
        // ladder; the same construction backs `explain_pairing`.
        let model = PairingModel::build(number, settings, units, &free, need_phantom);

        // Order the matching's vertices canonically by seed (the unit key is the
        // seed), so which of several equally-optimal pairings the solver returns
        // depends only on tournament state — not on the order players were
        // registered, imported, or reloaded (the model itself is order-independent,
        // so this affects ties only).
        free.sort_unstable();

        // `free` is the unit keys in matching-vertex order, so the edge/bye scoring
        // indexes the model's per-key tables directly — no per-edge or per-vertex
        // cast. `ctx` is built once and shared across every edge.
        let ctx = model.ctx();
        // Row-major `vcount × vcount` cost matrix in one flat allocation (entry
        // `(i, j)` at `i * vcount + j`), rather than a `Vec<Vec>`: one allocation
        // and no per-row pointer-chase feeding the solver.
        let mut cost = vec![0i128; vcount * vcount];
        // Fill by rule, not by edge: each rule adds its contribution to every edge
        // in its own monomorphized O(k²) loop, so the per-edge `match self` in
        // `edge_units` folds away (the rule is a constant here). The dispatch on
        // which rule runs once per rule. Bye-only rules are 0 on every real edge, so
        // they are skipped. Only the upper triangle is written — the solver reads
        // the matrix as symmetric — so there is no symmetric store or mirror pass.
        macro_rules! fill {
            ($rule:expr, $m:expr) => {
                accumulate_edge_rule(&mut cost, vcount, k, &free, &ctx, $m, |ctx, a, b| {
                    $rule.edge_units(ctx, a, b)
                })
            };
        }
        for (&rule, &m) in model.rules.iter().zip(&model.mult) {
            match rule {
                Rule::Rematch => fill!(Rule::Rematch, m),
                Rule::CupPrequalified => fill!(Rule::CupPrequalified, m),
                Rule::AirtightGroups => fill!(Rule::AirtightGroups, m),
                Rule::ScoreGap => fill!(Rule::ScoreGap, m),
                Rule::FloatRepeat => fill!(Rule::FloatRepeat, m),
                Rule::FloaterSelection => fill!(Rule::FloaterSelection, m),
                Rule::Club => fill!(Rule::Club, m),
                Rule::Nationality => fill!(Rule::Nationality, m),
                Rule::Fold => fill!(Rule::Fold, m),
                Rule::EloGap => fill!(Rule::EloGap, m),
                // Bye-only rules: 0 on every real edge (they act on the bye edge).
                Rule::ByeGroup | Rule::ByeSelection => {}
            }
        }
        if need_phantom {
            let p = k;
            for i in 0..k {
                let c = bye_cost(&ctx, &model.rules, &model.mult, free[i]);
                cost[i * vcount + p] = c;
                cost[p * vcount + i] = c;
            }
        }
        let mate = solve_matching(&cost, vcount);
        let mut seen = vec![false; vcount];
        for i in 0..vcount {
            if seen[i] {
                continue;
            }
            let j = mate[i];
            seen[i] = true;
            seen[j] = true;
            if need_phantom && (i == k || j == k) {
                let real = if i == k { j } else { i };
                swiss_bye = Some(free[real]);
            } else {
                pairs.push(PairedUnits {
                    a: free[i],
                    b: free[j],
                    source: PairingSource::Swiss,
                    points_diff: diff(free[i], free[j]),
                });
            }
        }
    }

    UnitPairing { pairs, swiss_bye }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_matching_matches_i128_regardless_of_chosen_width() {
        // One instance per width tier: small values (fits i32), values above
        // i32::MAX/16 but below i64::MAX/16 (fits i64), and values above that
        // (needs the i128 fallback). All three should agree with a direct
        // i128 solve on the same instance.
        let small = vec![
            vec![0, 1, 10, 10],
            vec![1, 0, 10, 10],
            vec![10, 10, 0, 1],
            vec![10, 10, 1, 0],
        ];
        let medium_unit = i32::MAX as i128 / 16 + 1;
        let medium = vec![
            vec![0, medium_unit, 10, 10],
            vec![medium_unit, 0, 10, 10],
            vec![10, 10, 0, medium_unit],
            vec![10, 10, medium_unit, 0],
        ];
        let huge_unit = i64::MAX as i128 / 16 + 1;
        let huge = vec![
            vec![0, huge_unit, 10, 10],
            vec![huge_unit, 0, 10, 10],
            vec![10, 10, 0, huge_unit],
            vec![10, 10, huge_unit, 0],
        ];
        for cost in [small, medium, huge] {
            let n = cost.len();
            let flat: Vec<i128> = cost.iter().flatten().copied().collect();
            assert_eq!(
                solve_matching(&flat, n),
                min_weight_perfect_matching(&flat, n)
            );
        }
    }

    #[test]
    #[should_panic(expected = "i128 headroom")]
    fn solve_matching_rejects_costs_beyond_i128_headroom() {
        // A 2×2 cost matrix whose off-diagonal cost is above the solver's i128
        // headroom (MAX/16): the solver's internal doubling would overflow, so the
        // dispatch must reject it rather than hand it over to be silently mangled.
        let big = i128::MAX / 2;
        let _ = solve_matching(&[0, big, big, 0], 2);
    }
}
