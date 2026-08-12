//! Round pairing.
//!
//! A round is modeled as a **minimum-weight perfect matching** over a complete
//! graph of the present players (solved by [`crate::matching`]). Each candidate
//! pairing is scored by a fixed set of rules; a rule emits a small number of
//! penalty *units*, and the edge weight is `Σ multiplier[rule] · units`. The
//! multipliers are **derived** from each rule's worst-case units per round rather
//! than hand-tuned, so one unit of any rule always outweighs the largest possible
//! sum of every lower-priority rule combined — i.e. the scalar weight is a correct
//! lexicographic ordering by rule priority, by construction. The rules, most
//! important first:
//!
//! 1. **No rematch / no repeat bye** — two players never meet twice, and no one
//!    takes the bye twice.
//!    Immediately below it, and only in a hybrid cup's qualification round:
//!    **no pre-qualified clash** — the cup's pre-qualified players spend that
//!    round in the Swiss pool, but must not be paired with each other. Absent
//!    from the rule set in every other round and every other format, so it costs
//!    nothing to have.
//! 2. **Bye in the lowest group** — the bye should go to the lowest-scoring free
//!    player, not (say) the tournament leader; the penalty grows with the
//!    *square* of the points gap to the lowest score among free players. A
//!    bye-only rule, sitting right below no-rematch so it's decided before any
//!    other pairing preference.
//! 3. **Airtight groups** (optional, off by default) — for the first N
//!    configured rounds, avoid pairing players with a different number of
//!    MacMahon points; the penalty grows with the *square* of the gap, same as
//!    the score-gap rule below. Costs nothing when unset.
//! 4. **Equal scores** — prefer opponents with the same number of points (each
//!    player's MacMahon starting points plus their victories); the penalty grows
//!    with the *square* of the points gap.
//! 5. **No repeated float** — avoid making a player an ascending floater (meeting
//!    someone with more points) or a descending floater (fewer points, or a
//!    bye) twice; the penalty fades with the number of rounds since the last such
//!    float.
//! 6. **Floater selection** — when a group has to pair across score groups, choose
//!    *who* floats: in classic Swiss, the descending floater should be the last
//!    (weakest) of the upper group and the ascending floater the first of the
//!    lower group; in median Swiss, both floaters should be the median of their
//!    respective group. The penalty rises with the
//!    distance from that ideal in-group rank.
//! 7. **Different clubs** — avoid pairing club-mates (ignored when a club is
//!    unknown).
//! 8. **Different nationalities** (optional, off by default) — the same idea one
//!    notch weaker: avoid pairing compatriots (ignored when a nationality is
//!    unknown). Below the club rule, so when the two disagree — one pairing
//!    shares a club, the other a nationality — the club clash is the one avoided.
//! 9. **Fold within a score group** — sort a group (equal points) by rating
//!    (unrated = 1), descending; the Nth player of the top half should meet the
//!    Nth of the bottom half, penalized by the *squared* deviation from that ideal.
//!
//! Priority lives in exactly one place — the order of [`rules::active_rules`] — and
//! the separation between tiers is proven by construction (see
//! [`rules::scale_ladder`]), so
//! adding or reordering rules stays sound with no magic numbers to retune.
//!
//! [`pair_round_weighted`] is the real pairing path; the bye is modeled as a
//! phantom vertex.
//!
//! The engine is split along those lines: [`model`] holds what it is given (one
//! [`PairingUnit`] per pairable side) and the per-round context derived from it,
//! [`rules`] scores an edge and builds the multiplier ladder, [`matching`] hands
//! the resulting cost matrix to the solver, and [`explain`] and [`counterfactual`]
//! answer for the result afterwards — both against the very same model the round
//! was paired from.
//!
//! ## Determinism and tie-breaking
//!
//! Rule costs are coarse integers, so the minimum-weight matching is frequently
//! achieved by several distinct pairings at once — e.g. two interchangeable
//! players, or which of the equal-lowest scorers takes the bye. Because the scalar
//! weight is an injective lexicographic encoding of the per-rule unit totals (see
//! [`rules::scale_ladder`]), two pairings tie on total cost *exactly* when they emit the
//! same units on every rule — i.e. when the rules are genuinely indifferent
//! between them.
//!
//! The choice among tied pairings is made deterministically and as a pure function
//! of tournament *state*: the matching's vertices are ordered by tournament number
//! (see the `free.sort_unstable()` in [`pair_round_weighted`]), so the same field
//! pairs identically no matter how it was registered, imported, or reloaded, and
//! within that
//! canonical order the blossom solver returns one fixed optimum. No lower-priority
//! "seniority" preference is layered on top — a tie means the rules do not care,
//! and the seed order only fixes *a* stable, reproducible representative.
//!
//! An ILP/CP-SAT backend is still planned (see TODO.md) for very large fields and
//! for formats needing hard constraints a plain matching can't express.

mod counterfactual;
mod explain;
mod matching;
mod model;
mod rules;
#[cfg(test)]
mod test_support;

pub use counterfactual::{
    AffectedCycle, Counterfactual, CounterfactualMode, RuleDelta, ScopeReason,
};
pub use explain::{BoardLedger, RoundExplanation, RuleContribution, RuleTotal};
pub use rules::RuleId;

pub(crate) use counterfactual::{counterfactual_forbid, counterfactual_force, PHANTOM};
pub(crate) use explain::explain_pairing;
pub(crate) use matching::pair_round_weighted;
pub(crate) use model::{player_units, PairingUnit};
