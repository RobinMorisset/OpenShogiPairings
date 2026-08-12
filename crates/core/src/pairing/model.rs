//! What the engine is handed, and the per-round model it derives from it.
//!
//! A [`PairingUnit`] is one pairable side — a player in an individual
//! tournament, a team in a team one — and is all the rules ever read, which is
//! what lets a single engine serve both modes. [`PairingModel`] is one round's
//! scoring context, built once and reused for pairing, explanation and the
//! counterfactuals, so all three are scored against the identical construction.
//!
//! [`player_units`] is the individual-mode wrapper that builds the table; the
//! team-mode one lives in [`crate::team`].

use std::collections::HashSet;

use typed_index_collections::{TiSlice, TiVec};

use crate::elo::{estimate_elos, estimate_or_prior};
use crate::player::Player;
use crate::round::Round;
use crate::scoring::compute_scores;
use crate::settings::{FloaterStyle, TournamentSettings};
use crate::units::{HalfPoints, TournamentId, UnitKey};

use super::rules::{
    active_rules, bye_cost, edge_cost, fold_ranks, scale_ladder, Ctx, FoldInfo, Rule,
};

/// Everything the engine needs to know about one **pairable unit**, whoever it
/// is: a player in an individual tournament, a team in a team tournament. The
/// rules read nothing else, so one engine serves both modes — see [`UnitKey`].
///
/// Defaultable so the table can leave a gap at any key no unit holds (key 0, the
/// phantom, and any number freed by a pre-play removal), exactly as
/// [`Scores`](crate::scoring::Scores) does.
///
/// Built by [`player_units`] for the individual path.
#[derive(Debug, Default, Clone)]
pub(crate) struct PairingUnit {
    /// Total score entering the round: MacMahon start, adjustments and results.
    pub points: HalfPoints,
    /// MacMahon starting points alone (the airtight-groups rule's quantity).
    pub macmahon: HalfPoints,
    /// Units faced so far, one entry per game (a long board counts twice).
    pub opponents: Vec<UnitKey>,
    /// Whether this unit has already had a bye (or the free point an opponent's
    /// no-show hands it).
    pub had_bye: bool,
    /// Round of the most recent up / down float, `None` if never.
    pub last_ascended: Option<u32>,
    pub last_descended: Option<u32>,
    /// What the fold sorts on: a player's rating, a team's average pairing
    /// rating. `None` for unrated, which sorts last (as rating 1).
    pub rating: Option<u32>,
    /// Normalized clubs (see [`TournamentSettings::normalize_club`]) in **board
    /// order** — one entry for a player, one per member for a team. The club rule
    /// compares aligned positions, since board `k` of one team only ever meets
    /// board `k` of the other.
    pub clubs: Vec<Option<String>>,
    /// Normalized nationalities (see
    /// [`TournamentSettings::normalize_nationality`]) in **board order**, read
    /// by [`Rule::Nationality`] exactly as `clubs` is read by [`Rule::Club`] —
    /// so the two vectors always have the same length (one entry per board).
    pub nationalities: Vec<Option<String>>,
    /// Whether this unit is a **pre-qualified** cup entrant this round (see
    /// [`Rule::CupPrequalified`]). Always false outside the qualifier cup's
    /// qualification round, where the rule is filtered out of the set entirely,
    /// and in team mode, where the cup is rejected.
    pub prequalified: bool,
    /// (ELO mode) Rounded live ELO estimate; zero in every other mode, which
    /// rejects ELO pairing.
    pub elo: i64,
}

impl PairingUnit {
    /// What the fold sorts by: the rating, with unrated as 1 so it sorts last.
    pub(super) fn fold_rating(&self) -> u32 {
        self.rating.unwrap_or(1)
    }
}

/// One round's Swiss scoring context, built once from the pairing inputs and
/// reused for both pairing and explanation. It owns the derived per-round data
/// (scores, fold ranks, ELO estimates, the multiplier ladder) and lends a [`Ctx`]
/// on demand, so an explanation is scored against the *identical* construction
/// the pairing used — no risk of the two drifting apart.
pub(super) struct PairingModel<'u> {
    /// The units being paired, indexed by key so the O(k²) cost loop indexes
    /// rather than hashes. Borrowed: the caller owns the table and reuses it (a
    /// pairing and its explanation are scored against the very same units).
    units: &'u TiSlice<UnitKey, PairingUnit>,
    /// Derived per-round data the rules read, also key-indexed. See [`Ctx`].
    fold: TiVec<UnitKey, Option<FoldInfo>>,
    exempt_clubs: HashSet<String>,
    exempt_nationalities: HashSet<String>,
    elo_rank: TiVec<UnitKey, i128>,
    round: u32,
    floater_style: FloaterStyle,
    edges: i128,
    max_gap: i128,
    min_points: i128,
    max_mm_gap: i128,
    max_group: i128,
    free_count: i128,
    max_boards: i128,
    max_elo_gap: i128,
    pub(super) rules: Vec<Rule>,
    pub(super) mult: Vec<i128>,
}

impl<'u> PairingModel<'u> {
    /// Build the model for the given `free` set (the units the matching will
    /// pair). `need_phantom` is whether a bye vertex participates, so the edge
    /// count — and hence the derived multipliers — match the matching that was or
    /// will be solved.
    pub(super) fn build(
        number: u32,
        settings: &TournamentSettings,
        units: &'u TiSlice<UnitKey, PairingUnit>,
        free: &[UnitKey],
        need_phantom: bool,
    ) -> Self {
        let fold = fold_ranks(units, free);

        let (mut lo, mut hi) = (u32::MAX, 0u32);
        let (mut mm_lo, mut mm_hi) = (u32::MAX, 0u32);
        for &key in free {
            let s = &units[key];
            lo = lo.min(s.points.halves());
            hi = hi.max(s.points.halves());
            mm_lo = mm_lo.min(s.macmahon.halves());
            mm_hi = mm_hi.max(s.macmahon.halves());
        }
        let exempt_clubs = settings.exempt_clubs_normalized();
        let exempt_nationalities = settings.exempt_nationalities_normalized();

        // ELO mode: the ascending ELO rank of each free unit (0 = weakest, for the
        // bye-selection rule) and the widest gap (for the ladder bound). The
        // estimate itself is on the unit; all of this is zero in Swiss mode.
        let (elo_rank, max_elo_gap): (TiVec<UnitKey, i128>, i128) =
            if settings.elo_estimate_needed() {
                // Ascending ELO; ties by key.
                let mut order = free.to_vec();
                order.sort_by(|&x, &y| units[x].elo.cmp(&units[y].elo).then(x.cmp(&y)));
                let mut elo_rank: TiVec<UnitKey, i128> = vec![0i128; units.len()].into();
                for (rank, &key) in order.iter().enumerate() {
                    elo_rank[key] = rank as i128;
                }
                let (elo_lo, elo_hi) = free
                    .iter()
                    .map(|&key| units[key].elo)
                    .fold((i64::MAX, i64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
                (elo_rank, (elo_hi - elo_lo).max(0) as i128)
            } else {
                (vec![0i128; units.len()].into(), 0)
            };

        let k = free.len();
        let vcount = k + usize::from(need_phantom);
        let max_group = fold
            .iter()
            .flatten()
            .map(|f| f.group_size)
            .max()
            .unwrap_or(0) as i128;
        // The affiliation rules' per-edge ceiling, read off the instance rather
        // than assumed: one board per player, `size` boards per team. The club and
        // nationality vectors are built together from the same members, so one
        // count bounds both — an invariant worth stating rather than trusting.
        debug_assert!(
            free.iter()
                .all(|&key| units[key].clubs.len() == units[key].nationalities.len()),
            "a unit's clubs and nationalities must be one per board"
        );
        let max_boards = free
            .iter()
            .map(|&key| units[key].clubs.len())
            .max()
            .unwrap_or(0) as i128;
        // The active rules, minus the whole-round no-ops that contribute 0 to every
        // edge and bye — and (having max-total 0) leave every other rule's
        // multiplier unchanged, so dropping them here is exact and spares the O(k²)
        // cost loop a per-edge branch and call each:
        //   - `AirtightGroups` with its window closed, and `Club` / `Nationality`
        //     with their protection off;
        //   - the bye-only rules (`ByeGroup`, `ByeSelection`) when no phantom is in
        //     play: on an even field there is no bye vertex for them to fire on, so
        //     they would only reserve a ladder tier (and eat overflow headroom) for
        //     nothing.
        let club_active = settings.club_protection_active(number);
        let nationality_active = settings.nationality_protection_active(number);
        let airtight_active = settings.airtight_groups_active(number);
        let rules: Vec<Rule> = active_rules(settings)
            .iter()
            .copied()
            .filter(|r| match r {
                Rule::AirtightGroups => airtight_active,
                Rule::Club => club_active,
                Rule::Nationality => nationality_active,
                Rule::CupPrequalified => free.iter().any(|&key| units[key].prequalified),
                Rule::ByeGroup | Rule::ByeSelection => need_phantom,
                _ => true,
            })
            .collect();

        let mut model = PairingModel {
            units,
            fold,
            exempt_clubs,
            exempt_nationalities,
            elo_rank,
            round: number,
            floater_style: settings.floater_style(),
            edges: (vcount / 2) as i128,
            max_gap: hi.saturating_sub(lo) as i128,
            min_points: lo as i128,
            max_mm_gap: mm_hi.saturating_sub(mm_lo) as i128,
            max_group,
            free_count: k as i128,
            max_boards,
            max_elo_gap,
            rules,
            mult: Vec::new(),
        };
        // The multipliers depend on the per-rule bounds, which need a Ctx — so
        // build the ladder in a second pass, once the rest of the model exists.
        let max_total: Vec<i128> = {
            let ctx = model.ctx();
            model
                .rules
                .iter()
                .map(|r| r.max_total_units(&ctx))
                .collect()
        };
        model.mult = scale_ladder(&max_total);
        model
    }

    /// A scoring context borrowing this model's data.
    pub(super) fn ctx(&self) -> Ctx<'_> {
        Ctx {
            units: self.units,
            fold: &self.fold,
            round: self.round,
            floater_style: self.floater_style,
            exempt_clubs: &self.exempt_clubs,
            exempt_nationalities: &self.exempt_nationalities,
            edges: self.edges,
            max_gap: self.max_gap,
            min_points: self.min_points,
            max_mm_gap: self.max_mm_gap,
            max_group: self.max_group,
            free_count: self.free_count,
            max_boards: self.max_boards,
            elo_rank: &self.elo_rank,
            max_elo_gap: self.max_elo_gap,
        }
    }

    /// Scalar edge weight for pairing unit `a` against unit `b`.
    pub(super) fn edge_cost(&self, a: UnitKey, b: UnitKey) -> i128 {
        edge_cost(&self.ctx(), &self.rules, &self.mult, a, b)
    }

    /// Scalar edge weight for giving `unit` the bye.
    pub(super) fn bye_cost(&self, unit: UnitKey) -> i128 {
        bye_cost(&self.ctx(), &self.rules, &self.mult, unit)
    }

    /// Per-rule penalty units (pre-multiplier) for pairing `a` against `b`, in
    /// priority order (aligned with [`Self::rules`]).
    pub(super) fn edge_units(&self, a: UnitKey, b: UnitKey) -> Vec<i128> {
        let ctx = self.ctx();
        self.rules
            .iter()
            .map(|r| r.edge_units(&ctx, a, b))
            .collect()
    }

    /// Per-rule penalty units (pre-multiplier) for giving `unit` the bye.
    pub(super) fn bye_units(&self, unit: UnitKey) -> Vec<i128> {
        let ctx = self.ctx();
        self.rules.iter().map(|r| r.bye_units(&ctx, unit)).collect()
    }

    pub(super) fn rules(&self) -> &[Rule] {
        &self.rules
    }
}

/// Build the engine's input for an **individual** tournament: one unit per
/// player, keyed by their tournament number, from the replayed scores plus the
/// registration data the rules read (rating, club, nationality) and the
/// per-round cup and ELO context.
///
/// Gap keys (number 0, and any number freed by a pre-play removal) hold a default
/// unit, exactly as [`Scores`](crate::scoring::Scores) leaves gaps — the free set
/// never names them.
pub(crate) fn player_units(
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    prequalified: &[TournamentId],
) -> TiVec<UnitKey, PairingUnit> {
    let scores = compute_scores(players, settings, completed_rounds);
    let cap = scores.tid_capacity();
    let mut units: TiVec<UnitKey, PairingUnit> = vec![PairingUnit::default(); cap].into();

    // A live ELO estimate is only computed for the mode that pairs on it —
    // it replays every game, so it is far too expensive to take speculatively.
    let estimates = settings
        .elo_estimate_needed()
        .then(|| estimate_elos(players, settings, completed_rounds));

    for p in players {
        let Some(tid) = p.tournament_id else {
            continue; // not finalized yet, so on no board either
        };
        let s = scores.get_tid(tid);
        let key = UnitKey::from(tid);
        units[key] = PairingUnit {
            points: s.points(),
            macmahon: s.macmahon,
            opponents: s.opponents.iter().copied().map(UnitKey::from).collect(),
            had_bye: s.had_bye,
            last_ascended: s.last_ascended,
            last_descended: s.last_descended,
            rating: p.rating,
            // One board, so the affiliation rules' aligned-position count
            // degenerates to the individual mode's 0/1.
            clubs: vec![p
                .club
                .as_ref()
                .map(|c| TournamentSettings::normalize_club(c))],
            nationalities: vec![p
                .nationality
                .as_ref()
                .map(|n| TournamentSettings::normalize_nationality(n))],
            prequalified: false,
            elo: estimates
                .as_ref()
                .map(|est| estimate_or_prior(est, p.id).round() as i64)
                .unwrap_or(0),
        };
    }
    for &tid in prequalified {
        units[UnitKey::from(tid)].prequalified = true;
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::test_support::*;

    #[test]
    fn bye_only_rules_are_dropped_from_the_ladder_on_an_even_field() {
        let settings = TournamentSettings::default();
        let has_bye_rule = |need_phantom: bool, n: u32| {
            let p: Vec<Player> = (1..=n).map(|i| player(i, Some(1500), None)).collect();
            let free: Vec<UnitKey> = (1..=n).map(UnitKey).collect();
            let units = player_units(&p, &settings, &[], &[]);
            let model = PairingModel::build(1, &settings, &units, &free, need_phantom);
            model
                .rules
                .iter()
                .any(|r| matches!(r, Rule::ByeGroup | Rule::ByeSelection))
        };
        // No phantom (even field) → the bye-only rules can never fire, so they must
        // not reserve a ladder tier. A phantom (odd field) → they stay.
        assert!(
            !has_bye_rule(false, 4),
            "an even field must not reserve a bye-rule tier"
        );
        assert!(has_bye_rule(true, 3), "an odd field keeps the bye rules");
    }
}
