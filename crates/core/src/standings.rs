//! Standings and tie-breaks.
//!
//! The ranked table shown on the Results tab — and, later, the ordering the
//! American grid (cross-table) is built from — is computed here so the server is
//! the single source of truth. It is derived from the **completed** rounds only.
//!
//! Each player's score is their **points** (MacMahon starting points plus
//! victories, counting the *effective* winner of a board — the handicap giver
//! always scores — and a bye as a win). The table is ranked by the
//! referee-chosen criteria (see [`TournamentSettings::tiebreaks`]) in order —
//! points is one of them, normally first — and finally by tournament number so
//! the order is always deterministic.
//!
//! Points plus every one of the twelve tie-break metrics is computed for every
//! player, so the Results tab can show whichever the referee selected without
//! recomputing. Each opponent-sum metric comes in a MacMahon-inclusive (`…M`)
//! and a wins-only (`…W`) flavour, according to how an opponent's "score" is
//! measured.
//!
//! # Relationship to [`crate::scoring`]
//!
//! A [`Standing`] is a **derived view**, not a second copy of the data. The raw
//! per-player accumulation — points, MacMahon start, victories, opponents faced
//! and beaten — is computed once by [`compute_scores`] (into the internal
//! `Scores` / `PlayerScore` substrate), and both this module *and* the pairing
//! engine build on that one result. [`compute_standings`] calls [`compute_scores`]
//! and then layers the twelve ranking tie-breaks and the live ELO estimate on
//! top; the pairing engine ([`crate::pairing`]) reads the same substrate directly.
//!
//! The substrate and this view are kept as separate types on purpose, rather
//! than folded into one:
//!
//! - **Disjoint extra fields.** `PlayerScore` carries pairing-only float history
//!   (`last_ascended` / `last_descended` / `had_bye`) that drives the pairing
//!   rules but is meaningless on a results page; [`Standing`] carries the twelve
//!   tie-break metrics and `estimated_elo`, which the engine never needs. Neither
//!   type should pay for the other's fields.
//! - **Different representation.** The substrate is indexed by dense tournament
//!   *number* and stores opponents as numbers, so the opponent-sum tie-breaks are
//!   plain array walks with no hashing — it runs repeatedly inside the matching
//!   solver. A [`Standing`] instead faces callers by [`Uuid`] and is a flat,
//!   ranked list, the shape a client consumes.
//! - **Dependency direction.** Pairing is the engine, standings is presentation.
//!   Having both depend on the lower-level `scoring` module keeps `pairing` from
//!   depending on `standings` (and from pulling in ranking/ELO logic it never
//!   uses).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::elo::estimate_elos;
use crate::player::Player;
use crate::round::Round;
use crate::scoring::{compute_scores, Cumulative};
use crate::settings::{Tiebreak, TournamentSettings};
use crate::units::{HalfPoints, TournamentId, Wins};

use typed_index_collections::TiVec;

/// One player's standing: score and every tie-break metric. The position in the
/// returned [`compute_standings`] vector is the player's rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Standing {
    pub player_id: Uuid,
    /// Games won (effective winner; a bye counts as a win), in half-wins — a
    /// `0=` sit-out is worth half of one, so this can be odd. See [`Wins`].
    pub victories: Wins,
    /// MacMahon starting points, with any manual point adjustment folded in.
    pub macmahon: HalfPoints,
    /// Total score. **Derived**, not accumulated: exactly `macmahon +
    /// victories`, since the two share the ×2 scale and every way of scoring a
    /// round adds to the start or to the wins and to nothing else (see
    /// [`PlayerScore::points`](crate::scoring::PlayerScore::points)).
    ///
    /// That identity is what the Results tab's "Wins + MacMahon points" tooltip
    /// claims. It held for nobody while a `0=` scored half a point and no win,
    /// and for no adjusted player while the bonus sat outside the MacMahon
    /// column; now it is true by construction rather than by agreement between
    /// two counters.
    pub points: HalfPoints,
    /// Every ranking criterion computed the same way for a player and for a
    /// team — the opponent-sums, the cumulative sums and direct confrontation.
    /// Flattened on the wire, so a client still reads `standing.sosm`.
    #[serde(flatten)]
    pub tiebreaks: Tiebreaks,
    /// Opponents faced, one entry per game, in round order (a rematch appears
    /// twice). Lets the Results tab show how the opponent-sum tie-breaks were
    /// built without re-deriving who played whom.
    pub opponents: Vec<Uuid>,
    /// Opponents defeated (effective winner), one entry per game — the subset of
    /// `opponents` the SODOS tie-breaks sum over.
    pub defeated: Vec<Uuid>,
    /// Running points total after each completed round (the sequence CUSSM sums).
    pub running_points: Vec<HalfPoints>,
    /// Running win total after each completed round (the sequence CUSSW sums).
    pub running_wins: Vec<Wins>,
    /// The player's current estimated ELO (rounded), from the Bayesian estimate
    /// (see [`crate::estimate_elos`]). `None` unless a live estimate is maintained
    /// — [`TournamentSettings::elo_estimate_live`], i.e. ELO pairing *or*
    /// estimate-based MacMahon — since otherwise it isn't computed at all. Also
    /// `None` for a *rated* player while the estimate only applies to unrated ones
    /// ([`TournamentSettings::elo_estimate_rated_pinned`]): they are pinned to
    /// their registration rating, so an "estimate" that never moves off the Rating
    /// column would only read as a broken estimate. When present, a player with no
    /// counted games sits at their prior mean (their registration rating, or 600
    /// if unrated).
    ///
    /// How the Results tab surfaces it depends on the mode: ELO pairing shows it
    /// as its own column (and can rank by it — see
    /// [`TournamentSettings::est_elo_ranks`]), while estimate-based MacMahon shows
    /// it only in the tooltip of the MacMahon points it produced, for the players
    /// whose estimate differs from their registration rating.
    pub estimated_elo: Option<i32>,
}

/// Every ranking criterion that a player's [`Standing`] and a team's
/// [`TeamStanding`](crate::team_scoring::TeamStanding) compute the same way: the
/// ten opponent-sum tie-breaks, the two cumulative sums and direct
/// confrontation.
///
/// The two rows differ in what they rank and in the extra criterion each carries
/// (a team has board wins, a player an ELO estimate), but every metric here is a
/// function of the same three things — who the unit faced, which of them it
/// beat, and what each of those is worth — so it is computed once, by
/// [`compute_tiebreaks`], and answered once, by [`Tiebreaks::value`]. That is
/// what keeps a change to (say) the Buchholz cut from landing in the individual
/// copy only.
///
/// The **score** deliberately stays on each row rather than joining this block:
/// it is the headline figure the cross-table, the FESA export and the result
/// import read, not only a ranking criterion.
///
/// `#[serde(flatten)]` on both rows keeps the wire exactly as flat as it was
/// before this block existed, and ts-rs inlines the fields into `Standing.ts`
/// and `TeamStanding.ts` — which is why this type is deliberately *not* exported
/// on its own: no client ever sees it as a nested object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Tiebreaks {
    /// Sum of opponents' points.
    pub sosm: HalfPoints,
    /// Sum of opponents' wins.
    pub sosw: Wins,
    /// Sum of defeated opponents' points.
    pub sodosm: HalfPoints,
    /// Sum of defeated opponents' wins.
    pub sodosw: Wins,
    /// Sum of opponents' SOSM.
    pub sososm: HalfPoints,
    /// Sum of opponents' SOSW.
    pub sososw: Wins,
    /// SOSM dropping the single lowest-scoring opponent.
    pub sosm1: HalfPoints,
    /// SOSM dropping the two lowest-scoring opponents.
    pub sosm2: HalfPoints,
    /// SOSW dropping the single lowest-scoring opponent.
    pub sosw1: Wins,
    /// SOSW dropping the two lowest-scoring opponents.
    pub sosw2: Wins,
    /// Cumulative sum of the running points total after each round.
    pub cussm: HalfPoints,
    /// Cumulative sum of the running win total after each round.
    pub cussw: Wins,
    /// Direct confrontation: this unit's wins against the other units it was
    /// still tied with once every earlier configured criterion ran out (0 if
    /// that tied group never played a complete round-robin among themselves, or
    /// if `Dc` isn't configured, or wasn't reached). Filled in by the ranking,
    /// which is what decides who is tied — [`rank_groups`].
    pub dc: Wins,
}

impl Tiebreaks {
    /// This block's value for a ranking criterion, as a plain ordinal `u32`, or
    /// `None` for a criterion the block doesn't carry — the score itself and the
    /// per-row extras, which the row answers for itself.
    ///
    /// Ranking only ever compares a metric against the *same* metric, so the
    /// typed point / win distinction that the fields carry collapses to a bare
    /// "bigger is better" integer here — both in raw ×2 units, and never
    /// compared across the two.
    pub fn value(&self, tb: Tiebreak) -> Option<u32> {
        Some(match tb {
            Tiebreak::SosM => self.sosm.halves(),
            Tiebreak::SosW => self.sosw.halves(),
            Tiebreak::SodosM => self.sodosm.halves(),
            Tiebreak::SodosW => self.sodosw.halves(),
            Tiebreak::SososM => self.sososm.halves(),
            Tiebreak::SososW => self.sososw.halves(),
            Tiebreak::SosM1 => self.sosm1.halves(),
            Tiebreak::SosM2 => self.sosm2.halves(),
            Tiebreak::SosW1 => self.sosw1.halves(),
            Tiebreak::SosW2 => self.sosw2.halves(),
            Tiebreak::CussM => self.cussm.halves(),
            Tiebreak::CussW => self.cussw.halves(),
            Tiebreak::Dc => self.dc.halves(),
            // The score, and the two criteria only one kind of row has.
            Tiebreak::Points | Tiebreak::BoardWins | Tiebreak::EstElo => return None,
        })
    }
}

/// One unit's accumulated scoring state, as the tie-break pass reads it: who it
/// faced and beat (by the dense number the score table is keyed by), what it is
/// worth as an opponent, and its cumulative sums. The bridge from a
/// [`PlayerScore`](crate::scoring::PlayerScore) or a
/// [`TeamScore`](crate::team_scoring::TeamScore) to [`compute_tiebreaks`].
pub(crate) struct ScoreRow<'a, K> {
    pub opponents: &'a [K],
    pub defeated: &'a [K],
    pub points: HalfPoints,
    pub victories: Wins,
    pub cumulative: &'a Cumulative,
}

/// Compute the shared tie-break block for every unit, indexed by its dense
/// number — players keyed by [`TournamentId`], teams by
/// [`TeamId`](crate::units::TeamId), over the same code.
///
/// Two passes, because the SOSOS metrics are sums *of* SOS: the first fills SOSM
/// and SOSW for every unit, the second reads them back through each unit's
/// opponents. `numbers` lists the units in play; a gap number keeps the all-zero
/// default, which is what an opponent that no longer exists is worth.
///
/// `capacity` sizes the tables, so every number in `numbers` must be below it.
pub(crate) fn compute_tiebreaks<'a, K, F>(
    capacity: usize,
    numbers: &[K],
    row: F,
) -> TiVec<K, Tiebreaks>
where
    K: Copy + From<usize> + Into<usize> + 'a,
    F: Fn(K) -> ScoreRow<'a, K>,
{
    // SOSM and SOSW first — each needed on its own for the SOSOS metrics.
    // Indexed by number so the SOSOS pass (a sum over opponents, which are
    // numbers) is a plain array walk with no hashing.
    let mut sosm: TiVec<K, HalfPoints> = vec![HalfPoints::ZERO; capacity].into();
    let mut sosw: TiVec<K, Wins> = vec![Wins::ZERO; capacity].into();
    for &k in numbers {
        let s = row(k);
        sosm[k] = s.opponents.iter().map(|&o| row(o).points).sum();
        sosw[k] = s.opponents.iter().map(|&o| row(o).victories).sum();
    }

    let mut tiebreaks: TiVec<K, Tiebreaks> = vec![Tiebreaks::default(); capacity].into();
    for &k in numbers {
        let s = row(k);
        // Scoring an opponent is a direct table lookup — `…M` by their points,
        // `…W` by their wins.
        let opp_m: Vec<HalfPoints> = s.opponents.iter().map(|&o| row(o).points).collect();
        let opp_w: Vec<Wins> = s.opponents.iter().map(|&o| row(o).victories).collect();
        tiebreaks[k] = Tiebreaks {
            sosm: sosm[k],
            sosw: sosw[k],
            sodosm: s.defeated.iter().map(|&o| row(o).points).sum(),
            sodosw: s.defeated.iter().map(|&o| row(o).victories).sum(),
            sososm: s.opponents.iter().map(|&o| sosm[o]).sum(),
            sososw: s.opponents.iter().map(|&o| sosw[o]).sum(),
            sosm1: sum_dropping_lowest(opp_m.clone(), 1),
            sosm2: sum_dropping_lowest(opp_m, 2),
            sosw1: sum_dropping_lowest(opp_w.clone(), 1),
            sosw2: sum_dropping_lowest(opp_w, 2),
            cussm: s.cumulative.cuss_m,
            cussw: s.cumulative.cuss_w,
            // Filled in below, while the ranking order is computed, since it
            // depends on which units end up tied on the earlier criteria.
            dc: Wins::ZERO,
        };
    }
    tiebreaks
}

impl Standing {
    /// The value of a given ranking criterion for this player, as a plain
    /// ordinal `u32` (see [`Tiebreaks::value`], which answers every criterion
    /// shared with a team row).
    pub fn tiebreak(&self, tb: Tiebreak) -> u32 {
        if let Some(value) = self.tiebreaks.value(tb) {
            return value;
        }
        match tb {
            Tiebreak::Points => self.points.halves(),
            // Ranking metrics are u32; a (non-negative) estimated ELO fits, and a
            // stray negative estimate floors at 0 for ordering. Only reached when
            // the estimate both is maintained and estimates rated players (see the
            // `EstElo` skip in ranking), so it is `Some` for every player; a
            // missing one sorts as 0.
            // Unreachable in a normalized individual tournament:
            // `TournamentSettings::normalized` drops board wins outside team
            // mode, precisely because a player's board wins *are* their own
            // wins and the criterion could never break a tie. Answered rather
            // than panicked because that is still the honest value for anyone
            // who builds settings by hand and skips normalization — but reaching
            // it means normalization was skipped, so say so in debug (the
            // `EstElo` sibling below states its own invariant the same way).
            Tiebreak::BoardWins => {
                debug_assert!(
                    false,
                    "board_wins ranks no individual player; `normalized` drops it \
                     outside team mode"
                );
                self.victories.halves()
            }
            Tiebreak::EstElo => {
                debug_assert!(
                    self.estimated_elo.is_some(),
                    "ranking by est_elo without an estimate — the `est_elo_ranks` skip should \
                     have kept this criterion out"
                );
                self.estimated_elo.unwrap_or(0).max(0) as u32
            }
            // Every remaining criterion is one `Tiebreaks::value` answers, so it
            // returned above.
            _ => unreachable!("{tb:?} is a shared criterion; `Tiebreaks::value` answers it"),
        }
    }
}

/// Sum a list of opponent scores after dropping the `drop` lowest — the
/// Buchholz-cut family (`drop` = 0 is the plain sum). Generic over the score
/// unit so it serves both the half-point (`…M`) and win-count (`…W`) flavours.
pub(crate) fn sum_dropping_lowest<T: Ord + Copy + std::iter::Sum>(
    mut scores: Vec<T>,
    drop: usize,
) -> T {
    scores.sort_unstable();
    scores.into_iter().skip(drop).sum()
}

/// Compute the ranked standings from the completed rounds.
///
/// Byes and absences contribute no opponent to the tie-breaks. Rounds that are
/// not yet completed are ignored.
pub(crate) fn compute_standings(
    players: &[Player],
    settings: &TournamentSettings,
    rounds: &[Round],
) -> Vec<Standing> {
    // Points, opponents, wins and cumulative sums come from the shared scorer; the
    // opponent-based tie-breaks are then sums over each player's opponents, in two
    // flavours: `…M` scores an opponent by their points, `…W` by their wins.
    let scores = compute_scores(players, settings, rounds);

    // The live Bayesian ELO estimate — computed whenever a live estimate is
    // maintained (ELO pairing *or* estimate-based MacMahon), which is exactly when
    // it is shown and ranks; skip the whole (quadrature-heavy) computation
    // otherwise.
    let elos = settings
        .elo_estimate_live()
        .then(|| estimate_elos(players, settings, rounds));
    // "Apply estimates to unrated players only": a rated player's "estimate" is
    // their registration rating, frozen there for the whole tournament. Reporting
    // it would duplicate the Rating column and read as an estimate that never
    // updates, so those cells stay empty and only the unrated newcomers — the ones
    // actually being estimated — carry a value.
    let rated_pinned = settings.elo_estimate_rated_pinned();

    // The opponent-sum and cumulative tie-breaks, over the same code the team
    // standings use. Every player has a number by the time standings run
    // (registration is finalized) — the score pass above already `expect`s it.
    let numbers: Vec<TournamentId> = players
        .iter()
        .map(|p| p.tournament_id.expect("player has a number"))
        .collect();
    let tiebreaks = compute_tiebreaks(scores.tid_capacity(), &numbers, |t| {
        let s = scores.get_tid(t);
        ScoreRow {
            opponents: &s.opponents,
            defeated: &s.defeated,
            points: s.points(),
            victories: s.victories,
            cumulative: &s.cumulative,
        }
    });

    let mut standings: Vec<Standing> = players
        .iter()
        .zip(&numbers)
        .map(|(p, &t)| {
            let s = scores.get_tid(t);
            Standing {
                player_id: p.id,
                victories: s.victories,
                macmahon: s.macmahon,
                points: s.points(),
                tiebreaks: tiebreaks[t],
                // The output faces callers by id, so translate the numbers back.
                opponents: s.opponents.iter().map(|&o| scores.id_of(o)).collect(),
                defeated: s.defeated.iter().map(|&o| scores.id_of(o)).collect(),
                running_points: s.cumulative.running_points.clone(),
                running_wins: s.cumulative.running_wins.clone(),
                estimated_elo: elos
                    .as_ref()
                    .filter(|_| !(rated_pinned && p.rating.is_some()))
                    .map(|e| {
                        e.get(&p.id)
                            .expect("estimate_elos returns one entry per player")
                            .round() as i32
                    }),
            }
        })
        .collect();

    // Rank by each configured criterion in order (points is one of them, normally
    // first); the tournament number breaks any remaining tie so the order is
    // deterministic (unnumbered players last).
    //
    // NOTE (intentional): there is deliberately no hard-wired "sort by points
    // first" step. Score is modeled as an ordinary, reorderable/removable
    // tie-break rather than a mandatory primary sort, because which quantity
    // should rank first is a tournament choice — e.g. in ELO pairing mode ranking
    // by estimated ELO is the point, not raw points. So a settings list without
    // `Points` (or with it below another criterion) is a valid configuration, not
    // a bug; the default order (`Tiebreak::default_order`) simply puts it first.
    //
    // Most criteria are a plain per-player number, so ranking by them is just a
    // lexicographic sort. Direct confrontation isn't: its value for a player
    // depends on which other players they're still tied with once every earlier
    // criterion has run out, so ranking proceeds by repeatedly splitting the
    // still-tied groups, criterion by criterion, rather than a single sort.
    let rows: Vec<DcRow> = standings
        .iter()
        .map(|s| DcRow {
            id: s.player_id,
            opponents: s.opponents.clone(),
            defeated: s.defeated.clone(),
        })
        .collect();
    // `standings` is in `players` order, so `numbers` indexes it directly.
    let mut seed_order: Vec<usize> = (0..standings.len()).collect();
    seed_order.sort_by_key(|&i| numbers[i]);

    let (order, dc) = rank_groups(
        seed_order,
        &settings.tiebreaks,
        // Estimated ELO only ranks in ELO pairing mode, and only while rated
        // players are actually estimated (see `est_elo_ranks`); skip it otherwise
        // (defends against a loaded save that predates normalization dropping it).
        |tb| tb == Tiebreak::EstElo && !settings.est_elo_ranks(),
        |i, tb| standings[i].tiebreak(tb),
        &rows,
    );
    for (i, wins) in dc.into_iter().enumerate() {
        standings[i].tiebreaks.dc = Wins::from_whole(wins);
    }
    order.into_iter().map(|i| standings[i].clone()).collect()
}

/// Rank rows by the configured criteria, in order, and report each row's
/// direct-confrontation score.
///
/// Shared by the player and team standings, because the subtle part is identical
/// for both: most criteria are a plain per-row number, so ranking by them is a
/// lexicographic sort — but direct confrontation isn't. Its value depends on
/// which *other* rows are still tied once every earlier criterion has run out, so
/// ranking proceeds by repeatedly splitting the still-tied groups, criterion by
/// criterion, rather than by one sort.
///
/// `seed_order` is the starting order (by tournament / team number), which
/// survives as the final tie-break because every split below is stable. `skip`
/// drops a criterion that cannot rank in this configuration. `value` reads a
/// criterion off a row — never [`Tiebreak::Dc`], which is computed here and
/// returned alongside the order, since it isn't known until the ranking runs.
pub(crate) fn rank_groups(
    seed_order: Vec<usize>,
    tiebreaks: &[Tiebreak],
    skip: impl Fn(Tiebreak) -> bool,
    value: impl Fn(usize, Tiebreak) -> u32,
    rows: &[DcRow],
) -> (Vec<usize>, Vec<u32>) {
    let mut dc = vec![0u32; rows.len()];
    let mut groups: Vec<Vec<usize>> = vec![seed_order];
    for &tb in tiebreaks {
        if skip(tb) {
            continue;
        }
        let mut next_groups: Vec<Vec<usize>> = Vec::new();
        for group in groups {
            if group.len() < 2 {
                next_groups.push(group);
                continue;
            }
            if tb == Tiebreak::Dc {
                match direct_confrontation(&group, rows) {
                    Some(scores) => {
                        for &i in &group {
                            dc[i] = scores[&rows[i].id];
                        }
                        next_groups.extend(split_by_key(group, |i| dc[i]));
                    }
                    // Not a complete subgraph: DC stays 0 for everyone here, and
                    // the group stays tied going into the next criterion.
                    None => next_groups.push(group),
                }
                continue;
            }
            next_groups.extend(split_by_key(group, |i| value(i, tb)));
        }
        groups = next_groups;
    }
    (groups.into_iter().flatten().collect(), dc)
}

/// Stable-split a tied group into subgroups sharing the same (descending) key —
/// each subgroup is still internally ordered as it was in `group` (so an outer
/// tie-break already applied, like tournament number, survives untouched).
fn split_by_key(mut group: Vec<usize>, key: impl Fn(usize) -> u32) -> Vec<Vec<usize>> {
    group.sort_by_key(|&i| std::cmp::Reverse(key(i)));
    let mut result: Vec<Vec<usize>> = Vec::new();
    for i in group {
        match result.last_mut() {
            Some(last) if key(last[0]) == key(i) => last.push(i),
            _ => result.push(vec![i]),
        }
    }
    result
}

/// The head-to-head record the direct-confrontation criterion needs, for one
/// ranked row — a player's or a team's, since the rule is identical either way
/// (who did this unit face, and which of them did it beat).
pub(crate) struct DcRow {
    pub id: Uuid,
    pub opponents: Vec<Uuid>,
    pub defeated: Vec<Uuid>,
}

/// Direct confrontation scores for a tied `group`: each row's wins against
/// the others in the group. Only defined — `Some` — when the group is a
/// complete subgraph (every pair in it has played each other at least once);
/// otherwise `None`, meaning the criterion doesn't apply to this group.
fn direct_confrontation(group: &[usize], rows: &[DcRow]) -> Option<HashMap<Uuid, u32>> {
    let ids: Vec<Uuid> = group.iter().map(|&i| rows[i].id).collect();
    for (pos, &i) in group.iter().enumerate() {
        for &other_id in &ids[(pos + 1)..] {
            if !rows[i].opponents.contains(&other_id) {
                return None;
            }
        }
    }
    let id_set: std::collections::HashSet<Uuid> = ids.iter().copied().collect();
    Some(
        group
            .iter()
            .map(|&i| {
                let wins = rows[i]
                    .defeated
                    .iter()
                    .filter(|d| id_set.contains(d))
                    .count() as u32;
                (rows[i].id, wins)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::RoundExplanation;
    use crate::round::{Board, Outcome, PairingSource, Winner};
    use crate::settings::{MacMahonThreshold, ThresholdCriterion};

    fn player(tid: u32, rating: Option<u32>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: Some(TournamentId(tid)),
            last_name: format!("P{tid}"),
            first_name: String::new(),
            rating,
            pairing_rating: None,
            grade: None,
            fesa_games: None,
            nationality: None,
            club: None,
            eligible: false,
            categories: Vec::new(),
            adjustments: Vec::new(),
        }
    }

    fn board(a: TournamentId, b: TournamentId, winner: Winner) -> Board {
        Board {
            outcome: Outcome::won(winner),
            ..Board::pending(a, b, 0, PairingSource::Swiss)
        }
    }

    fn round(number: u32, boards: Vec<Board>) -> Round {
        Round {
            number,
            explanation: RoundExplanation::empty(number),
            boards,
            sitouts: Vec::new(),
            completed: true,
        }
    }

    #[test]
    fn long_board_opponent_counts_twice_in_sos_and_sodos() {
        // R1: A beats B on a long board; C beats D (normal). R2: B beats C.
        // B ends on 1 point (2 halves). A faced B on a long board, so B is
        // counted *twice* for A's opponent-sums: SOSM(A) = SODOSM(A) = 2·2 = 4,
        // not 2 as it would be for a single game.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let long_ab = Board {
            outcome: Outcome::won(Winner::Player1),
            long: true,
            ..Board::pending(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                0,
                PairingSource::Swiss,
            )
        };
        let rounds = vec![
            round(
                1,
                vec![
                    long_ab,
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                2,
                vec![board(
                    b.tournament_id.unwrap(),
                    c.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let standings = compute_standings(&players, &TournamentSettings::default(), &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // A: 2 points and 2 victories from the long win — both in ×2 units.
        assert_eq!(
            (of(a.id).points.halves(), of(a.id).victories.halves()),
            (4, 4)
        );
        // B: beat C once (1 point = 2 halves).
        assert_eq!(of(b.id).points, 2);
        // B counted twice in A's opponent-sums.
        assert_eq!(of(a.id).tiebreaks.sosm, 4);
        assert_eq!(of(a.id).tiebreaks.sodosm, 4);
        assert_eq!(of(a.id).tiebreaks.sosw, 4); // B has 1 win (2 halves), counted twice
    }

    #[test]
    fn sos_breaks_a_points_tie() {
        // A>C, D>B (upset) in R1; A>D, B>C in R2. Final points A=2, B=1, D=1,
        // C=0. B and D tie on 1 point, but D faced the stronger field (A, B)
        // and B the weaker (D, C), so SOS ranks D above B despite B's lower
        // tournament number.
        let a = player(1, Some(2000));
        let b = player(2, Some(1900));
        let c = player(3, Some(1800));
        let d = player(4, Some(1700));
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats C
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // D beats B
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats D
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // B beats C
                ],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let standings = compute_standings(&players, &TournamentSettings::default(), &rounds);

        let order: Vec<Uuid> = standings.iter().map(|s| s.player_id).collect();
        assert_eq!(order, vec![a.id, d.id, b.id, c.id]);

        // Points and SOSM are in half-point units (×2).
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!(
            (of(a.id).points.halves(), of(a.id).tiebreaks.sosm.halves()),
            (4, 2)
        );
        assert_eq!(
            (of(d.id).points.halves(), of(d.id).tiebreaks.sosm.halves()),
            (2, 6)
        ); // faced A(4)+B(2)
        assert_eq!(
            (of(b.id).points.halves(), of(b.id).tiebreaks.sosm.halves()),
            (2, 2)
        ); // faced D(2)+C(0)
        assert_eq!(of(c.id).points, 0);
    }

    #[test]
    fn macmahon_points_feed_into_scores_and_sos() {
        // One completed round, A beats B, with a 1500 threshold: A rated 2000
        // starts at 1 MacMahon point (now 2 after the win), B at 1.
        let a = player(1, Some(2000));
        let b = player(2, Some(1600));
        let rounds = vec![round(
            1,
            vec![board(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                Winner::Player1,
            )],
        )];
        let settings =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
        let standings = compute_standings(&[a.clone(), b.clone()], &settings, &rounds);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // MacMahon, points and victories are all in ×2 units, so A's single win
        // reads as 2.
        assert_eq!(
            (
                of(a.id).macmahon.halves(),
                of(a.id).victories.halves(),
                of(a.id).points.halves()
            ),
            (2, 2, 4)
        );
        assert_eq!(
            (
                of(b.id).macmahon.halves(),
                of(b.id).victories.halves(),
                of(b.id).points.halves()
            ),
            (2, 0, 2)
        );
        assert_eq!(of(a.id).tiebreaks.sosm, 2); // opponent B has 1 point (2 halves)
        assert_eq!(of(a.id).tiebreaks.sodosm, 2); // defeated B (1 point = 2 halves)
        assert_eq!(standings[0].player_id, a.id); // A ranks first
    }

    #[test]
    fn degressive_macmahon_drops_the_head_start_after_the_scheduled_round() {
        // A (2000) starts one MacMahon point above B (1000) thanks to the 1500
        // threshold, but that group is dropped at the end of round 1. After A
        // beats B in round 1, A's starting bonus is gone: A keeps only the real
        // win, so A is on 1 point rather than 2.
        let a = player(1, Some(2000));
        let b = player(2, Some(1000));
        let rounds = vec![round(
            1,
            vec![board(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                Winner::Player1,
            )],
        )];
        let settings = TournamentSettings::default().with_thresholds(vec![MacMahonThreshold {
            criterion: ThresholdCriterion::Elo { value: 1500 },
            drops_after_round: Some(1),
        }]);
        let standings = compute_standings(&[a.clone(), b.clone()], &settings, &rounds);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // Points in half-point units: A keeps only the win (2 halves), not the
        // dropped MacMahon head start.
        assert_eq!(
            (
                of(a.id).macmahon.halves(),
                of(a.id).victories.halves(),
                of(a.id).points.halves()
            ),
            (0, 2, 2)
        );
        assert_eq!(
            (
                of(b.id).macmahon.halves(),
                of(b.id).victories.halves(),
                of(b.id).points.halves()
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn wins_only_tiebreaks_ignore_macmahon_start() {
        // A 1500 threshold gives the higher-rated players a MacMahon head start,
        // so the M and W flavours of the opponent-sum tie-breaks diverge. After
        // A>B and A>C: A faced B and C. With the threshold, B and C each have 1
        // point (MacMahon) but 0 wins, so A's SOSM = 2 but SOSW = 0. A also
        // defeated both, so SODOSM = 2, SODOSW = 0.
        let a = player(1, Some(2000));
        let b = player(2, Some(1800));
        let c = player(3, Some(1700));
        let rounds = vec![
            round(
                1,
                vec![board(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ),
            round(
                2,
                vec![board(
                    a.tournament_id.unwrap(),
                    c.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ),
        ];
        let settings =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
        let standings = compute_standings(&[a.clone(), b.clone(), c.clone()], &settings, &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // Both flavours are in ×2 units: the M ones sum opponents' MacMahon
        // (2 halves each), the W ones their wins (none here).
        assert_eq!(
            (
                of(a.id).tiebreaks.sosm.halves(),
                of(a.id).tiebreaks.sosw.halves()
            ),
            (4, 0)
        );
        assert_eq!(
            (
                of(a.id).tiebreaks.sodosm.halves(),
                of(a.id).tiebreaks.sodosw.halves()
            ),
            (4, 0)
        );
    }

    /// The Wins column and the Points column describe the same round, so they
    /// must not disagree about it.
    ///
    /// Reported from a live tournament: a player on a MacMahon start of 1 who
    /// lost a game and took a `0=` sit-out showed `Points 1½` beside `Wins 0`.
    /// A half-point sit-out is half a win (the EGF "number of wins"
    /// convention), and everything summed from wins has to inherit that half.
    #[test]
    fn a_half_point_sitout_is_half_a_win_here_and_in_every_sum_built_on_wins() {
        use crate::round::{Sitout, SitoutKind, SitoutValue};
        let a = player(1, Some(1600));
        let b = player(2, Some(1600));
        let half_bye = |tid| Sitout {
            player: TournamentId(tid),
            kind: SitoutKind::Bye,
            value: SitoutValue::Half,
        };
        let rounds = vec![
            // B beats A.
            round(
                1,
                vec![board(TournamentId(1), TournamentId(2), Winner::Player2)],
            ),
            // Both then take a `0=` sit-out.
            Round {
                number: 2,
                explanation: RoundExplanation::empty(2),
                boards: Vec::new(),
                sitouts: vec![half_bye(1), half_bye(2)],
                completed: true,
            },
        ];
        let standings = compute_standings(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &rounds,
        );
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();

        // Everything below is in half-units. A lost their game and took a `0=`:
        // half a point, and half a win — not the `0` the Wins column used to show.
        assert_eq!(of(a.id).points, 1);
        assert_eq!(of(a.id).victories, 1);
        // B won a game and took the same `0=`: 1½ of each.
        assert_eq!(of(b.id).points, 3);
        assert_eq!(of(b.id).victories, 3);

        // The sums built on wins carry the half through rather than rounding it
        // away — each faced the other exactly once.
        assert_eq!(of(a.id).tiebreaks.sosw, 3); // B's 1½ wins
        assert_eq!(of(b.id).tiebreaks.sosw, 1); // A's ½ win
                                                // SODOSW counts only defeated opponents: B beat A, A beat nobody.
        assert_eq!(of(b.id).tiebreaks.sodosw, 1);
        assert_eq!(of(a.id).tiebreaks.sodosw, 0);
    }

    /// The Results tab shows Wins and MacMahon as separate columns and claims
    /// Points is their sum. That has to be true for *every* player, including
    /// one carrying a manual adjustment — which used to land outside the
    /// MacMahon column, so an adjusted player's row visibly did not add up.
    #[test]
    fn points_are_the_starting_score_plus_the_wins_even_with_an_adjustment() {
        use crate::player::PointAdjustment;
        let mut a = player(1, Some(1600));
        a.adjustments.push(PointAdjustment {
            id: Uuid::new_v4(),
            delta: 1,
            reason: "fair play".into(),
        });
        let b = player(2, Some(1600));
        let rounds = vec![round(
            1,
            vec![board(TournamentId(1), TournamentId(2), Winner::Player1)],
        )];
        let standings = compute_standings(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &rounds,
        );
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();

        // A: a +1 bonus and one win — the bonus is *in* the MacMahon column.
        assert_eq!(of(a.id).macmahon, 2);
        assert_eq!(of(a.id).victories, 2);
        assert_eq!(of(a.id).points, 4);

        // The identity the UI promises, for everyone in the table.
        for s in &standings {
            assert_eq!(
                s.points,
                s.macmahon + s.victories.into(),
                "points must be MacMahon + wins for {:?}",
                s.player_id
            );
        }
    }

    #[test]
    fn buchholz_cut_drops_the_lowest_opponents() {
        // A round robin: A wins everything (3 wins); B beats C and D (2 wins); C
        // beats D (1 win); D loses everything (0 wins). A's opponents' win scores
        // are therefore {2, 1, 0}.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                3,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
        ];
        let standings = compute_standings(
            &[a.clone(), b.clone(), c.clone(), d.clone()],
            &TournamentSettings::default(),
            &rounds,
        );
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // A faced B (2 wins), C (1 win), D (0 wins) → {4, 2, 0} in half-wins.
        assert_eq!(of(a.id).tiebreaks.sosw, 6);
        assert_eq!(of(a.id).tiebreaks.sosw1, 6); // drop the 0
        assert_eq!(of(a.id).tiebreaks.sosw2, 4); // drop the 0 and the 1
    }

    #[test]
    fn cumulative_sums_running_totals_over_rounds() {
        // A: win, loss, win → running wins 1, 1, 2 → CUSSW = 4. B mirrors: loss,
        // win, loss → running wins 0, 1, 1 → CUSSW = 2.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                3,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ),
                ],
            ),
        ];
        let standings = compute_standings(
            &[a.clone(), b.clone(), c.clone(), d.clone()],
            &TournamentSettings::default(),
            &rounds,
        );
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!(of(a.id).tiebreaks.cussw, 8); // (1 + 1 + 2) wins, in half-wins
        assert_eq!(of(b.id).tiebreaks.cussw, 4); // 0 + 1 + 1
                                                 // CUSSM sums points; with no MacMahon a win is worth exactly a
                                                 // point, so on the shared ×2 scale the two now agree.
        assert_eq!(of(a.id).tiebreaks.cussm, 8);
    }

    #[test]
    fn estimated_elo_seeds_at_rating_and_tracks_upsets() {
        // No games: each player's estimate is their registration rating. Then a
        // lower-rated player beats a higher one, and their estimate rises above
        // its seed while the loser's falls.
        let a = player(1, Some(1400));
        let b = player(2, Some(1800));

        // The estimate is only computed in ELO mode, so enable it here.
        let elo_mode = TournamentSettings::elo_pairing();
        let none = compute_standings(&[a.clone(), b.clone()], &elo_mode, &[]);
        let of = |st: &[Standing], id| {
            st.iter()
                .find(|s| s.player_id == id)
                .unwrap()
                .estimated_elo
                .expect("estimate present in ELO mode")
        };
        assert_eq!(of(&none, a.id), 1400);
        assert_eq!(of(&none, b.id), 1800);

        let rounds = vec![round(
            1,
            vec![board(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                Winner::Player1,
            )],
        )]; // A upsets B
        let after = compute_standings(&[a.clone(), b.clone()], &elo_mode, &rounds);
        assert!(of(&after, a.id) > 1400, "the winner's estimate rises");
        assert!(of(&after, b.id) < 1800, "the loser's estimate falls");
    }

    #[test]
    fn estimated_elo_is_populated_in_macmahon_from_estimate_mode() {
        // Estimate-based MacMahon maintains a live estimate for scoring, so the
        // standings must expose it too — not only in ELO pairing mode. It gets no
        // column of its own there; the Results tab puts it in the tooltip of the
        // MacMahon points it produced, which needs the value all the same.
        let a = player(1, Some(1400));
        let b = player(2, Some(1800));
        let settings = TournamentSettings::default()
            .with_thresholds(vec![MacMahonThreshold::elo(1500)])
            .with_macmahon_from_estimate();
        assert!(settings.macmahon_from_estimate_active());

        let st = compute_standings(&[a.clone(), b.clone()], &settings, &[]);
        let est = |id| st.iter().find(|s| s.player_id == id).unwrap().estimated_elo;
        // No games → the estimate seeds at the registration rating, and it is present.
        assert_eq!(est(a.id), Some(1400));
        assert_eq!(est(b.id), Some(1800));
    }

    #[test]
    fn estimated_elo_is_empty_for_rated_players_when_only_unrated_are_estimated() {
        // "Apply estimates to unrated players only" (K × 0) pins every rated player
        // to their registration rating, so the standings report no estimate for
        // them — repeating the Rating column would look like an estimate that never
        // updates. The unrated newcomer, who *is* estimated, still gets one, and it
        // moves once they play.
        let a = player(1, Some(1400));
        let b = player(2, Some(1800));
        let c = player(3, None);
        let players = vec![a.clone(), b.clone(), c.clone()];
        let settings = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.k_multiplier = crate::settings::Ratio::from_percent(0));

        let est =
            |st: &[Standing], id| st.iter().find(|s| s.player_id == id).unwrap().estimated_elo;

        let none = compute_standings(&players, &settings, &[]);
        assert_eq!(
            est(&none, a.id),
            None,
            "rated players are pinned, not estimated"
        );
        assert_eq!(est(&none, b.id), None);
        let seed = est(&none, c.id).expect("the unrated newcomer is estimated");

        // C beats the 1800 — their estimate must climb off the unrated prior.
        let rounds = vec![round(
            1,
            vec![board(
                c.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                Winner::Player1,
            )],
        )];
        let after = compute_standings(&players, &settings, &rounds);
        assert_eq!(est(&after, b.id), None, "the pinned loser stays empty");
        assert!(
            est(&after, c.id).expect("still estimated") > seed,
            "the unrated winner's estimate rises off its {seed} seed"
        );

        // With rated players estimated too (the default ×1.0), everyone has one.
        let all = compute_standings(&players, &TournamentSettings::elo_pairing(), &rounds);
        assert!(est(&all, a.id).is_some() && est(&all, b.id).is_some());
    }

    #[test]
    fn est_elo_tiebreak_is_ignored_without_elo_mode() {
        // Equal on everything but rating; no games, so estimates sit at the seeds.
        let a = player(1, Some(1400));
        let b = player(2, Some(1800));
        let players = vec![a.clone(), b.clone()];
        let pos = |st: &[Standing], id| st.iter().position(|s| s.player_id == id).unwrap();

        // Mode off: est_elo can't break the tie, so the tournament number does
        // (A before B) — the stronger estimate does *not* win.
        let off = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::EstElo],
                ..Default::default()
            },
            &[],
        );
        assert!(pos(&off, a.id) < pos(&off, b.id));

        // Mode on: est_elo ranks the stronger estimate first (B above A).
        let on = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::EstElo],
                ..TournamentSettings::elo_pairing()
            },
            &[],
        );
        assert!(pos(&on, b.id) < pos(&on, a.id));
    }

    #[test]
    fn configured_tiebreak_order_decides_the_ranking() {
        // With a 1500 threshold, A and B start on 1 MacMahon point. E and F both
        // finish on 1 point. E faced A (1 pt, 0 wins) and B (3 pts, 2 wins); F
        // faced C (2 pts, 2 wins) and D (1 pt, 1 win). So SOSM(E)=4 > SOSM(F)=3,
        // but SOSW(E)=2 < SOSW(F)=3 — the two flavours order E and F oppositely.
        let a = player(1, Some(2000));
        let b = player(2, Some(2000));
        let c = player(3, Some(1000));
        let d = player(4, Some(1000));
        let e = player(5, Some(1000));
        let f = player(6, Some(1000));
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        e.tournament_id.unwrap(),
                        a.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // E beats A
                    board(
                        f.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // C beats F
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // D beats B
                ],
            ),
            round(
                2,
                vec![
                    board(
                        e.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // B beats E
                    board(
                        f.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // F beats D
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // C beats A
                ],
            ),
        ];
        let players = vec![
            a.clone(),
            b.clone(),
            c.clone(),
            d.clone(),
            e.clone(),
            f.clone(),
        ];
        let base =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
        let pos = |st: &[Standing], id| st.iter().position(|s| s.player_id == id).unwrap();

        let by_m = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::SosM],
                ..base.clone()
            },
            &rounds,
        );
        assert!(pos(&by_m, e.id) < pos(&by_m, f.id)); // E above F by SOSM

        let by_w = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::SosW],
                ..base
            },
            &rounds,
        );
        assert!(pos(&by_w, f.id) < pos(&by_w, e.id)); // F above E by SOSW
    }

    #[test]
    fn direct_confrontation_breaks_a_tie_within_a_complete_subgraph() {
        // A, B and C each finish on 2 points but by different means: A beats
        // both B and C; B beats C plus outsider D; C beats outsiders D and E.
        // A, B and C have played each other (a complete subgraph), so DC ranks
        // them by wins *within that trio*: A beat both (2), B beat only C (1),
        // C beat neither (0) — A > B > C, even though their tournament numbers
        // (3, 2, 1) say the opposite.
        let a = player(3, None);
        let b = player(2, None);
        let c = player(1, None);
        let d = player(4, None);
        let e = player(5, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats B
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // C beats D
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats C
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // B beats D
                ],
            ),
            round(
                3,
                vec![board(
                    b.tournament_id.unwrap(),
                    c.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ), // B beats C
            round(
                4,
                vec![board(
                    c.tournament_id.unwrap(),
                    e.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ), // C beats E
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone(), e.clone()];
        let settings = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::Dc],
            ..Default::default()
        };
        let standings = compute_standings(&players, &settings, &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();

        // Everything in ×2 units: 2 points reads as 4, and so does 2 DC wins.
        assert_eq!(
            (
                of(a.id).points.halves(),
                of(b.id).points.halves(),
                of(c.id).points.halves()
            ),
            (4, 4, 4)
        );
        // Direct confrontation counts played games, so its halves are always
        // even: 2, 1 and 0 wins.
        assert_eq!(
            (
                of(a.id).tiebreaks.dc.halves(),
                of(b.id).tiebreaks.dc.halves(),
                of(c.id).tiebreaks.dc.halves()
            ),
            (4, 2, 0)
        );

        let pos = |id| standings.iter().position(|s| s.player_id == id).unwrap();
        assert!(pos(a.id) < pos(b.id));
        assert!(pos(b.id) < pos(c.id));
    }

    #[test]
    fn direct_confrontation_does_nothing_without_a_complete_subgraph() {
        // A, B, C and D all finish on 1 point, but they haven't all played each
        // other: A faced C and D, B faced C and D, but A never faced B and C
        // never faced D. DC can't apply to a tied group that isn't a complete
        // subgraph, so it stays 0 for everyone and the tournament number alone
        // breaks the tie.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats C
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // B beats D
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // D beats A
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // C beats B
                ],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let settings = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::Dc],
            ..Default::default()
        };
        let standings = compute_standings(&players, &settings, &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();

        for id in [a.id, b.id, c.id, d.id] {
            assert_eq!(of(id).points, 2); // 1 point = 2 half-points
            assert_eq!(of(id).tiebreaks.dc, 0);
        }
        let order: Vec<Uuid> = standings.iter().map(|s| s.player_id).collect();
        assert_eq!(order, vec![a.id, b.id, c.id, d.id]); // tournament number order
    }
}
