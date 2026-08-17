//! The engine's behaviour, rule by rule: what each rule makes the matching do,
//! and that the derived ladder really is lexicographic.
//!
//! These tests drive the whole engine (through
//! [`pair_players`](crate::pairing::test_support::pair_players)) rather than
//! calling a rule directly, because a rule's meaning *is* the pairing it
//! produces.

use std::collections::HashSet;
use std::num::NonZeroU32;

use typed_index_collections::TiVec;
use uuid::Uuid;

use crate::pairing::explain::RoundExplanation;
use crate::pairing::model::{player_units, PairingUnit};
use crate::pairing::rules::{active_rules, fold_ranks, scale_ladder, Ctx, Rule};
use crate::pairing::test_support::*;
use crate::player::{Player, PointAdjustment};
use crate::round::{
    Board, GameRecord, Outcome, PairingSource, Round, Sitout, SitoutKind, SitoutValue, Winner,
};
use crate::settings::{
    ClubProtection, FloaterStyle, MacMahonThreshold, NationalityProtection, RatioAtLeastOne,
    TournamentSettings,
};
use crate::units::{TournamentId, UnitKey};

#[test]
fn pairing_is_independent_of_input_order() {
    // A genuinely tied field: two blocks of three equal-rated players in ELO
    // mode round 1. Any single cross-block board costs the same squared gap, so
    // *which* players cross is a real tie the rules can't resolve. Presenting the
    // exact same players (same ids, same tournament numbers) to the engine in a
    // different vector order must not change the pairing — the round is a
    // function of tournament state, not registration/import order.
    let p: Vec<Player> = vec![
        player(1, Some(2000), None),
        player(2, Some(2000), None),
        player(3, Some(2000), None),
        player(4, Some(1000), None),
        player(5, Some(1000), None),
        player(6, Some(1000), None),
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let settings = elo_settings();

    let forward = pair_players(1, &p, &settings, &[], &present);

    // Same players and tournament numbers, reversed order in both the players
    // slice and the present list.
    let mut p_rev = p.clone();
    p_rev.reverse();
    let present_rev: Vec<TournamentId> = present.iter().rev().copied().collect();
    let reversed = pair_players(1, &p_rev, &settings, &[], &present_rev);

    assert_eq!(board_pairs(&forward), board_pairs(&reversed));
    assert_eq!(forward.swiss_bye(), reversed.swiss_bye());
}

#[test]
fn weighted_pairs_by_score_and_avoids_rematch() {
    let p: Vec<Player> = (1..=4)
        .map(|i| player(i, Some(2000 - i * 10), None))
        .collect();
    // Round 1: p0 beat p1, p2 beat p3. Winners now have 1 victory, losers 0.
    let r1 = completed_round(
        1,
        &[
            (
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap(),
                Winner::Player1,
            ),
            (
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap(),
                Winner::Player1,
            ),
        ],
        None,
    );
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let round = pair_players(2, &p, &TournamentSettings::default(), &[r1], &present);

    assert_eq!(round.swiss_bye(), None);
    assert_eq!(round.boards.len(), 2);
    let pairs = board_pairs(&round);
    // Same-score, no rematch: winners together, losers together.
    assert!(pairs.contains(&unord(
        p[0].tournament_id.unwrap(),
        p[2].tournament_id.unwrap()
    )));
    assert!(pairs.contains(&unord(
        p[1].tournament_id.unwrap(),
        p[3].tournament_id.unwrap()
    )));
}

#[test]
fn weighted_avoids_repeat_bye() {
    let p: Vec<Player> = (1..=3).map(|i| player(i, Some(1500), None)).collect();
    // Round 1: p0 beat p1; p2 took the bye (so p0 and p2 have 1 victory).
    let r1 = completed_round(
        1,
        &[(
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap(),
            Winner::Player1,
        )],
        Some(p[2].tournament_id.unwrap()),
    );
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let round = pair_players(2, &p, &TournamentSettings::default(), &[r1], &present);

    // p2 already had a bye, so it must fall elsewhere; giving it to p1 also
    // leaves the same-score board p0 vs p2.
    assert_eq!(round.swiss_bye(), Some(p[1].tournament_id.unwrap()));
    assert_eq!(
        board_pairs(&round),
        HashSet::from([unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )])
    );
}

#[test]
fn weighted_gives_the_bye_to_the_lowest_group() {
    // 5 same-rated players: round 1 pairs p0-p1 and p2-p3 (winners p0, p2);
    // p4 takes the bye. Going into round 2: p0, p2, p4 lead on 1 point, p1
    // and p3 trail on 0. p4 already had the bye, so it must fall to p1 or
    // p3 — never to one of the two leaders, even though that could look
    // like a perfectly valid matching otherwise.
    let p: Vec<Player> = (1..=5).map(|i| player(i, Some(1500), None)).collect();
    let r1 = completed_round(
        1,
        &[
            (
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap(),
                Winner::Player1,
            ),
            (
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap(),
                Winner::Player1,
            ),
        ],
        Some(p[4].tournament_id.unwrap()),
    );
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let round = pair_players(2, &p, &TournamentSettings::default(), &[r1], &present);

    let bye = round.swiss_bye().expect("odd field needs a bye");
    assert!(
        bye == p[1].tournament_id.unwrap() || bye == p[3].tournament_id.unwrap(),
        "bye went to a leader"
    );
}

#[test]
fn pairing_freezes_the_points_diff_on_each_board() {
    // After round 1 (A, C on 1 point; B, D on 0), force A vs D in round 2.
    // The board should record the float A had going in: 1 − 0 = 1.
    let p: Vec<Player> = (1..=4)
        .map(|i| player(i, Some(2000 - i * 10), None))
        .collect();
    let r1 = completed_round(
        1,
        &[
            (
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap(),
                Winner::Player1,
            ), // A beats B
            (
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap(),
                Winner::Player1,
            ), // C beats D
        ],
        None,
    );
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    // A (1 pt) vs D (0 pt), forced.
    let forced = vec![Board::pending(
        p[0].tournament_id.unwrap(),
        p[3].tournament_id.unwrap(),
        0,
        PairingSource::Swiss,
    )];

    let round = pair_players_forced(
        2,
        &p,
        &TournamentSettings::default(),
        &[r1],
        &present,
        &forced,
    );

    // The forced A-vs-D board freezes the float A had going in: +2 half-points
    // (A on 1 point = 2 halves, D on 0). Only its sign matters to the float
    // history.
    let ad = round
        .boards
        .iter()
        .find(|b| {
            b.player1 == p[0].tournament_id.unwrap() && b.player2 == p[3].tournament_id.unwrap()
        })
        .expect("forced board present");
    assert_eq!(ad.points_diff, 2);
}

#[test]
fn a_half_point_reaches_pairing_at_half_point_granularity() {
    // C sits out round 1 for half a point (a `0=`, 1 half-unit) and carries it
    // into round 2, while A has a full point (2). The engine sees the scores at
    // half-point granularity: A and C are one half-point apart, so their round-2
    // board freezes an *odd* points_diff (±1) — a float whole-point scoring
    // could never produce. B (lowest) takes the bye.
    let a = player(1, Some(2000), None);
    let b = player(2, Some(1500), None);
    let c = player(3, Some(1000), None);
    let players = vec![a.clone(), b.clone(), c.clone()];
    let settings = TournamentSettings::default();

    // Round 1: A beats B; C is absent for half a point.
    let r1 = Round {
        number: 1,
        explanation: RoundExplanation::empty(1),
        pairing_explanation_valid: true,
        boards: vec![Board {
            record: GameRecord::Short(Outcome::won(Winner::Player1)),
            ..Board::pending(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                0,
                PairingSource::Swiss,
            )
        }],
        sitouts: vec![Sitout {
            player: c.tournament_id.unwrap(),
            kind: SitoutKind::Absent,
            value: SitoutValue::Half,
        }],
        completed: true,
    };

    let present = vec![
        a.tournament_id.unwrap(),
        b.tournament_id.unwrap(),
        c.tournament_id.unwrap(),
    ];
    let round = pair_players(2, &players, &settings, std::slice::from_ref(&r1), &present);

    assert_eq!(round.swiss_bye(), Some(b.tournament_id.unwrap())); // lowest score takes the bye
    let ac = round
        .boards
        .iter()
        .find(|bd| {
            (bd.player1 == a.tournament_id.unwrap() && bd.player2 == c.tournament_id.unwrap())
                || (bd.player1 == c.tournament_id.unwrap()
                    && bd.player2 == a.tournament_id.unwrap())
        })
        .expect("A vs C paired");
    assert_eq!(ac.points_diff.abs(), 1); // A(2) vs C(1) — an odd float
}

#[test]
fn macmahon_points_group_the_pairings() {
    // Round 1, everyone on 0 victories. With no MacMahon the four form one
    // score group and the fold pairs p0-p2 / p1-p3. A 1500 threshold splits
    // them into a top group {p0,p1} and a bottom group {p2,p3}, and rule 2
    // (equal points) keeps the groups apart instead.
    let p = vec![
        player(1, Some(2000), None),
        player(2, Some(1900), None),
        player(3, Some(1000), None),
        player(4, Some(900), None),
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let settings =
        TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

    let round = pair_players(1, &p, &settings, &[], &present);

    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap()
        )),
        "top MacMahon group paired within itself"
    );
    assert!(
        pairs.contains(&unord(
            p[2].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "bottom MacMahon group paired within itself"
    );
}

/// Round 1, one score group of four, rating order p0>p1>p2>p3 so the fold
/// ideal is p0-p2 and p1-p3 — which are club-mates (X and Y). Used by the club
/// tests below to see whether protection overrides the fold.
fn two_clubs_where_fold_pairs_mates() -> Vec<Player> {
    vec![
        player(1, Some(2000), Some("X")),
        player(2, Some(1900), Some("Y")),
        player(3, Some(1800), Some("X")),
        player(4, Some(1700), Some("Y")),
    ]
}

#[test]
fn weighted_avoids_pairing_club_mates_when_protection_on() {
    let p = two_clubs_where_fold_pairs_mates();
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let settings = TournamentSettings::default().with_club(ClubProtection::On {
        rounds: None,
        exempt_clubs: Vec::new(),
    });

    let round = pair_players(1, &p, &settings, &[], &present);

    assert_eq!(round.boards.len(), 2);
    let club_of = |id: TournamentId| {
        p.iter()
            .find(|q| q.tournament_id.unwrap() == id)
            .unwrap()
            .club
            .clone()
    };
    for b in &round.boards {
        assert_ne!(
            club_of(b.player1),
            club_of(b.player2),
            "club-mates were paired despite protection"
        );
    }
}

/// The club rule counts the same-club *games* an edge would create, over
/// aligned board positions — the form that serves a multi-board unit (a team)
/// as well as a player. Positions are aligned because board `k` only ever
/// meets board `k`, so a shared club on different boards costs nothing.
#[test]
fn the_club_rule_counts_aligned_same_club_boards() {
    let unit = |clubs: &[Option<&str>]| PairingUnit {
        clubs: clubs
            .iter()
            .map(|c| c.map(TournamentSettings::normalize_club))
            .collect(),
        ..PairingUnit::default()
    };
    // Board 1 shares club X, board 2 differs, board 3 shares club Z — but the
    // Y on a's board 2 also appears on b's board 3, on a board it never plays.
    let units: TiVec<UnitKey, PairingUnit> = vec![
        PairingUnit::default(), // key 0, the phantom's gap slot
        unit(&[Some("X"), Some("Y"), Some("Z")]),
        unit(&[Some("x"), Some("W"), Some("Z")]), // case-insensitive on board 1
        unit(&[None, None, None]),
    ]
    .into();
    let free = [UnitKey(1), UnitKey(2), UnitKey(3)];
    let exempt_clubs = HashSet::new();
    let exempt_nationalities = HashSet::new();
    let empty_rank: TiVec<UnitKey, i128> = vec![0i128; units.len()].into();
    let fold = fold_ranks(&units, &free);
    let ctx = Ctx {
        units: &units,
        fold: &fold,
        round: 1,
        floater_style: FloaterStyle::Classic,
        exempt_clubs: &exempt_clubs,
        exempt_nationalities: &exempt_nationalities,
        edges: 1,
        max_gap: 0,
        min_points: 0,
        max_mm_gap: 0,
        max_group: 3,
        free_count: 3,
        max_boards: 3,
        elo_rank: &empty_rank,
        max_elo_gap: 0,
    };
    // Two aligned clashes (boards 1 and 3), not three: the Y/W board differs,
    // and a's Y never meets b's Y because they sit on different boards.
    assert_eq!(Rule::Club.edge_units(&ctx, UnitKey(1), UnitKey(2)), 2);
    // An unknown club is never a clash.
    assert_eq!(Rule::Club.edge_units(&ctx, UnitKey(1), UnitKey(3)), 0);
    // ...and the ladder bound is read off the instance, so it still covers the
    // worst edge — the check that keeps the lexicographic separation exact
    // once an edge can emit more than one unit.
    let bound = Rule::Club.max_total_units(&ctx);
    assert!(Rule::Club.edge_units(&ctx, UnitKey(1), UnitKey(2)) * ctx.edges <= bound);
    assert_eq!(bound, 3);
}

#[test]
fn club_protection_off_by_default_pairs_the_fold() {
    // With protection off (the default), the club rule is silent, so the fold
    // ideal wins and club-mates X-X / Y-Y are paired.
    let p = two_clubs_where_fold_pairs_mates();
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let round = pair_players(1, &p, &TournamentSettings::default(), &[], &present);

    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "fold pairs the X club-mates"
    );
    assert!(
        pairs.contains(&unord(
            p[1].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "fold pairs the Y club-mates"
    );
}

#[test]
fn exempt_club_members_may_be_paired() {
    // Fold ideal is p0-p2 (both "Home") and p1-p3 (both unclubbed). With
    // protection on but "Home" exempt (spelled differently to prove the match
    // is case-insensitive), the Home pair is allowed and the fold wins; without
    // the exemption the club rule breaks that pairing up.
    let p = vec![
        player(1, Some(2000), Some("Home")),
        player(2, Some(1900), None),
        player(3, Some(1800), Some("Home")),
        player(4, Some(1700), None),
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let exempt = TournamentSettings::default().with_club(ClubProtection::On {
        rounds: None,
        exempt_clubs: vec!["  HOME ".into()],
    });
    let round = pair_players(1, &p, &exempt, &[], &present);
    assert!(
        board_pairs(&round).contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "exempt club-mates should be paired by the fold"
    );

    let protected = TournamentSettings::default().with_club(ClubProtection::On {
        rounds: None,
        exempt_clubs: Vec::new(),
    });
    let round = pair_players(1, &p, &protected, &[], &present);
    assert!(
        !board_pairs(&round).contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "non-exempt club-mates should not be paired"
    );
}

#[test]
fn club_protection_only_applies_within_its_round_window() {
    // Protection limited to round 1: round 2 must ignore clubs, so the fold
    // ideal (club-mate pairs) wins again.
    let p = two_clubs_where_fold_pairs_mates();
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let settings = TournamentSettings::default().with_club(ClubProtection::On {
        rounds: NonZeroU32::new(1),
        exempt_clubs: Vec::new(),
    });

    // Pair round 2 directly (no completed rounds needed to exercise the window).
    let round = pair_players(2, &p, &settings, &[], &present);
    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "past the window, fold pairs X-X"
    );
    assert!(
        pairs.contains(&unord(
            p[1].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "past the window, fold pairs Y-Y"
    );
}

/// Round 1, one score group of four, rating order p0>p1>p2>p3 so the fold
/// ideal is p0-p2 and p1-p3 — which are compatriots (JP and FR). The
/// nationality twin of [`two_clubs_where_fold_pairs_mates`].
fn two_nationalities_where_fold_pairs_compatriots() -> Vec<Player> {
    vec![
        player_nat(1, Some(2000), None, Some("JP")),
        player_nat(2, Some(1900), None, Some("FR")),
        player_nat(3, Some(1800), None, Some("JP")),
        player_nat(4, Some(1700), None, Some("FR")),
    ]
}

#[test]
fn weighted_avoids_pairing_compatriots_when_protection_on() {
    let p = two_nationalities_where_fold_pairs_compatriots();
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let settings = TournamentSettings::default().with_nationality(NationalityProtection::On {
        rounds: None,
        exempt_nationalities: Vec::new(),
    });

    let round = pair_players(1, &p, &settings, &[], &present);

    assert_eq!(round.boards.len(), 2);
    let nationality_of = |id: TournamentId| {
        p.iter()
            .find(|q| q.tournament_id.unwrap() == id)
            .unwrap()
            .nationality
            .clone()
    };
    for b in &round.boards {
        assert_ne!(
            nationality_of(b.player1),
            nationality_of(b.player2),
            "compatriots were paired despite protection"
        );
    }
}

#[test]
fn nationality_protection_off_by_default_pairs_the_fold() {
    // With protection off (the default), the nationality rule is silent, so
    // the fold ideal wins and the compatriots JP-JP / FR-FR are paired.
    let p = two_nationalities_where_fold_pairs_compatriots();
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let round = pair_players(1, &p, &TournamentSettings::default(), &[], &present);

    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "fold pairs the JP compatriots"
    );
    assert!(
        pairs.contains(&unord(
            p[1].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "fold pairs the FR compatriots"
    );
}

#[test]
fn exempt_nationality_members_may_be_paired() {
    // Fold ideal is p0-p2 (both "JP", the host country) and p1-p3 (both of
    // unknown nationality). With protection on but JP exempt (spelled
    // differently to prove the match is case-insensitive), the JP pair is
    // allowed and the fold wins; without the exemption it is broken up.
    let p = vec![
        player_nat(1, Some(2000), None, Some("JP")),
        player_nat(2, Some(1900), None, None),
        player_nat(3, Some(1800), None, Some("JP")),
        player_nat(4, Some(1700), None, None),
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let exempt = TournamentSettings::default().with_nationality(NationalityProtection::On {
        rounds: None,
        exempt_nationalities: vec![" jp ".into()],
    });
    let round = pair_players(1, &p, &exempt, &[], &present);
    assert!(
        board_pairs(&round).contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "exempt compatriots should be paired by the fold"
    );

    let protected = TournamentSettings::default().with_nationality(NationalityProtection::On {
        rounds: None,
        exempt_nationalities: Vec::new(),
    });
    let round = pair_players(1, &p, &protected, &[], &present);
    assert!(
        !board_pairs(&round).contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "non-exempt compatriots should not be paired"
    );
}

#[test]
fn nationality_protection_only_applies_within_its_round_window() {
    // Protection limited to round 1: round 2 must ignore nationalities, so
    // the fold ideal (compatriot pairs) wins again.
    let p = two_nationalities_where_fold_pairs_compatriots();
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let settings = TournamentSettings::default().with_nationality(NationalityProtection::On {
        rounds: NonZeroU32::new(1),
        exempt_nationalities: Vec::new(),
    });

    // Pair round 2 directly (no completed rounds needed to exercise the window).
    let round = pair_players(2, &p, &settings, &[], &present);
    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "past the window, fold pairs JP-JP"
    );
    assert!(
        pairs.contains(&unord(
            p[1].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "past the window, fold pairs FR-FR"
    );
}

/// The point of the whole rule: nationality protection is *weaker* than club
/// protection, so when only one of the two can be honoured the club clash is
/// the one avoided.
///
/// Four players in one score group, with the fold-ideal matching (p0-p2,
/// p1-p3) ruled out as a double rematch. That leaves two candidates:
///   * p0-p1 / p2-p3 — one nationality clash (p0, p1 are both JP), fold 20;
///   * p0-p3 / p1-p2 — one club clash (p1, p2 are both "A"), fold 4.
///
/// The second is much the better fold and the only nationality-clean one, so
/// it is what any ordering *other* than club-above-nationality would pick.
#[test]
fn club_protection_outranks_nationality_protection() {
    let p = vec![
        player_nat(1, Some(2000), Some("B"), Some("JP")),
        player_nat(2, Some(1900), Some("A"), Some("JP")),
        player_nat(3, Some(1800), Some("A"), Some("FR")),
        player_nat(4, Some(1700), Some("C"), Some("DE")),
    ];
    let id = |i: usize| p[i].tournament_id.unwrap();
    let present: Vec<TournamentId> = (0..4).map(id).collect();
    // Round 1 played the fold ideal, so round 2 cannot repeat it. The losers'
    // adjustments put all four back on one point, keeping a single score
    // group — otherwise the score-gap rule, far above both protections,
    // would decide the round on its own.
    let r1 = [completed_round(
        1,
        &[
            (id(0), id(2), Winner::Player1),
            (id(1), id(3), Winner::Player1),
        ],
        None,
    )];
    let mut p = p.clone();
    for i in [2, 3] {
        p[i].adjustments.push(PointAdjustment {
            id: Uuid::new_v4(),
            delta: 1,
            reason: "level the score group".into(),
        });
    }

    // With nationality protection alone, the club clash is free and the
    // better fold wins — so the two candidates really are in tension, and
    // the assertion below is about the priority and not about one matching
    // being better on every count.
    let nationality_only =
        TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: None,
            exempt_nationalities: Vec::new(),
        });
    let round = pair_players(2, &p, &nationality_only, &r1, &present);
    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(id(0), id(3))) && pairs.contains(&unord(id(1), id(2))),
        "without club protection the nationality-clean, better-fold matching \
         should win; got {pairs:?}"
    );

    let both = nationality_only.with_club(ClubProtection::On {
        rounds: None,
        exempt_clubs: Vec::new(),
    });
    let round = pair_players(2, &p, &both, &r1, &present);
    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(id(0), id(1))) && pairs.contains(&unord(id(2), id(3))),
        "the engine should accept the nationality clash (JP-JP) to avoid the \
         club clash (A-A), even at a worse fold; got {pairs:?}"
    );
}

/// Two MacMahon groups of 4 (threshold 1500), each already paired internally
/// in round 1 with an upset, so round 2's tied scores admit a cheaper
/// cross-group matching under score-gap alone: the group boundary is
/// otherwise avoidable (both groups have a same-group, non-rematch partner
/// available), so this isolates the airtight-groups rule rather than a
/// parity-forced float.
#[test]
fn airtight_groups_keeps_macmahon_groups_apart_when_scores_would_cross() {
    let p: Vec<Player> = vec![
        player(1, Some(2000), None),
        player(2, Some(1950), None),
        player(3, Some(1900), None),
        player(4, Some(1850), None),
        player(5, Some(1000), None),
        player(6, Some(950), None),
        player(7, Some(900), None),
        player(8, Some(850), None),
    ];
    let settings_base =
        TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let r1 = completed_round(
        1,
        &[
            (
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap(),
                Winner::Player2,
            ), // p1 beats p0 (top group upset)
            (
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap(),
                Winner::Player1,
            ), // p2 beats p3
            (
                p[4].tournament_id.unwrap(),
                p[5].tournament_id.unwrap(),
                Winner::Player2,
            ), // p5 beats p4 (bottom group upset)
            (
                p[6].tournament_id.unwrap(),
                p[7].tournament_id.unwrap(),
                Winner::Player1,
            ), // p6 beats p7
        ],
        None,
    );

    // Without airtight groups, score-gap alone finds a cheaper matching that
    // crosses the MacMahon boundary twice.
    let round_off = pair_players(2, &p, &settings_base, std::slice::from_ref(&r1), &present);
    let top: HashSet<TournamentId> = [
        p[0].tournament_id.unwrap(),
        p[1].tournament_id.unwrap(),
        p[2].tournament_id.unwrap(),
        p[3].tournament_id.unwrap(),
    ]
    .into_iter()
    .collect();
    let crosses = round_off
        .boards
        .iter()
        .filter(|b| top.contains(&b.player1) != top.contains(&b.player2))
        .count();
    assert_eq!(crosses, 2, "score-gap alone crosses the MacMahon boundary");

    // With airtight groups active for round 2, every board stays within its
    // MacMahon group.
    let settings_on = settings_base.with_airtight(NonZeroU32::new(2));
    let round_on = pair_players(2, &p, &settings_on, &[r1], &present);
    for b in &round_on.boards {
        assert_eq!(
            top.contains(&b.player1),
            top.contains(&b.player2),
            "airtight groups should keep every board within its MacMahon group"
        );
    }
}

#[test]
fn airtight_groups_only_applies_within_its_round_window() {
    // Same setup as above, but the window only covers round 1: round 2 must
    // ignore MacMahon groups again, so score-gap crosses the boundary.
    let p: Vec<Player> = vec![
        player(1, Some(2000), None),
        player(2, Some(1950), None),
        player(3, Some(1900), None),
        player(4, Some(1850), None),
        player(5, Some(1000), None),
        player(6, Some(950), None),
        player(7, Some(900), None),
        player(8, Some(850), None),
    ];
    let settings = TournamentSettings::default()
        .with_thresholds(vec![MacMahonThreshold::elo(1500)])
        .with_airtight(NonZeroU32::new(1));
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let r1 = completed_round(
        1,
        &[
            (
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap(),
                Winner::Player2,
            ),
            (
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap(),
                Winner::Player1,
            ),
            (
                p[4].tournament_id.unwrap(),
                p[5].tournament_id.unwrap(),
                Winner::Player2,
            ),
            (
                p[6].tournament_id.unwrap(),
                p[7].tournament_id.unwrap(),
                Winner::Player1,
            ),
        ],
        None,
    );

    let round = pair_players(2, &p, &settings, &[r1], &present);
    let top: HashSet<TournamentId> = [
        p[0].tournament_id.unwrap(),
        p[1].tournament_id.unwrap(),
        p[2].tournament_id.unwrap(),
        p[3].tournament_id.unwrap(),
    ]
    .into_iter()
    .collect();
    let crosses = round
        .boards
        .iter()
        .filter(|b| top.contains(&b.player1) != top.contains(&b.player2))
        .count();
    assert_eq!(crosses, 2, "past the window, score-gap crosses again");
}

// --- ELO (non-Swiss) mode ---------------------------------------------

fn elo_settings() -> TournamentSettings {
    // Neutralize the provisional-rating widening so these pairing tests
    // exercise the base drift; reliability is covered in elo.rs tests.
    TournamentSettings::elo_pairing()
        .map_estimator(|e| e.provisional_multiplier = RatioAtLeastOne::from_percent(100))
}

#[test]
fn elo_mode_pairs_adjacent_ratings_round_one() {
    // Round 1 (no games yet): every estimate sits at its registration rating,
    // so minimizing the squared ELO gap pairs neighbours: 2000-1950, 1500-1450.
    let p = vec![
        player(1, Some(2000), None),
        player(2, Some(1950), None),
        player(3, Some(1500), None),
        player(4, Some(1450), None),
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let round = pair_players(1, &p, &elo_settings(), &[], &present);

    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap()
        )),
        "closest pair 2000-1950"
    );
    assert!(
        pairs.contains(&unord(
            p[2].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "closest pair 1500-1450"
    );
}

#[test]
fn elo_mode_gives_the_bye_to_the_weakest() {
    // Five players, no forced bye: the lowest-rated (and, round 1, lowest
    // estimate) should sit out, and the other four pair by adjacency.
    let p: Vec<Player> = (1..=5)
        .map(|i| player(i, Some(2000 - (i - 1) * 300), None))
        .collect();
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let round = pair_players(1, &p, &elo_settings(), &[], &present);

    assert_eq!(
        round.swiss_bye(),
        Some(p[4].tournament_id.unwrap()),
        "the weakest player takes the bye"
    );
    assert_eq!(round.boards.len(), 2);
}

#[test]
fn elo_mode_reacts_to_results_and_avoids_rematch() {
    // Round 1 paired 2000-1950 and 1500-1450. Say the two lower-rated players
    // (1950, 1450) win. Their estimates rise; round 2 must not rematch, and the
    // squared-ELO-gap rule now pairs the two round-1 winners together and the
    // two losers together.
    let p = vec![
        player(1, Some(2000), None),
        player(2, Some(1950), None),
        player(3, Some(1500), None),
        player(4, Some(1450), None),
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let settings = elo_settings();

    // 1950 (p1) beat 2000 (p0); 1450 (p3) beat 1500 (p2).
    let r1 = completed_round(
        1,
        &[
            (
                p[1].tournament_id.unwrap(),
                p[0].tournament_id.unwrap(),
                Winner::Player1,
            ),
            (
                p[3].tournament_id.unwrap(),
                p[2].tournament_id.unwrap(),
                Winner::Player1,
            ),
        ],
        None,
    );

    let round = pair_players(2, &p, &settings, &[r1], &present);
    let pairs = board_pairs(&round);
    // No rematch of the round-1 boards.
    assert!(!pairs.contains(&unord(
        p[0].tournament_id.unwrap(),
        p[1].tournament_id.unwrap()
    )));
    assert!(!pairs.contains(&unord(
        p[2].tournament_id.unwrap(),
        p[3].tournament_id.unwrap()
    )));
    // Winners (raised estimates) meet, losers meet.
    assert!(
        pairs.contains(&unord(
            p[1].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "the two winners are paired"
    );
    assert!(
        pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "the two losers are paired"
    );
}

/// The float-repeat rule's arithmetic, at the one place it is decided.
///
/// Driven directly rather than through a pairing because the *shape* of the
/// penalty — full strength the round after a float, decaying with distance,
/// never above the bound the ladder is built from — is what the engine's
/// separation depends on, and a pairing only ever shows one point of it.
#[test]
fn float_units_decay_with_distance_and_stay_within_the_ladder_bound() {
    use crate::pairing::rules::{float_units, FLOAT_BASE};

    // Never floated that way: nothing to repeat.
    assert_eq!(float_units(None, 5), 0);
    // Floated in the round just gone: the full penalty.
    assert_eq!(float_units(Some(4), 5), FLOAT_BASE);
    // …and it decays as the float recedes, always strictly downwards.
    let mut previous = FLOAT_BASE;
    for round in 6..12u32 {
        let units = float_units(Some(4), round);
        assert!(units < previous, "round {round}: {units} vs {previous}");
        assert!(units > 0, "a float never stops counting entirely");
        previous = units;
    }
    // The bound `max_total_units` is derived from: one direction can never
    // exceed FLOAT_BASE, so the two-direction edge term can never exceed twice
    // it. A term above this would break the ladder's lexicographic separation.
    for round in 5..40u32 {
        assert!(float_units(Some(4), round) <= FLOAT_BASE);
    }
}

#[test]
#[should_panic(expected = "does not predate")]
fn a_float_recorded_in_the_round_being_paired_is_a_bug_not_a_division_by_zero() {
    // `round - k` would be 0 here. The history is replayed from *completed*
    // rounds, so this can only mean the replay and the round number disagree —
    // a bug in this crate, hence a debug assertion rather than a silent 0.
    let _ = crate::pairing::rules::float_units(Some(5), 5);
}

/// Direction matters: a player who floated *up* is only penalised for floating
/// up again, and the rule outranks floater selection, which would send the same
/// player up a second time.
#[test]
fn a_repeat_float_is_avoided_even_when_floater_selection_asks_for_it() {
    // MacMahon by *grade*, so the starting groups and the rating order can be
    // set independently — which is what lets the two rules disagree.
    let dan = |tid, rating| Player {
        grade: Some(crate::player::Grade::dan(1)),
        ..player(tid, Some(rating), None)
    };
    let kyu = |tid, rating| Player {
        grade: Some(crate::player::Grade::kyu(5)),
        ..player(tid, Some(rating), None)
    };
    // A and C start on 1 MacMahon point, B and D on 0. D is the *stronger* of
    // the two who will end up on 1 point.
    let p = vec![dan(1, 2000), kyu(2, 1000), dan(3, 1500), kyu(4, 1900)];
    let id = |i: usize| p[i].tournament_id.unwrap();
    let settings = TournamentSettings::default()
        .with_thresholds(vec![MacMahonThreshold::grade(crate::player::Grade::dan(1))]);

    // Round 1 is two cross-group boards — the ordinary shape of a MacMahon
    // round 1. A and C float down (`points_diff` +2 = one point, frozen at
    // pairing time), B and D float up. A wins; D wins.
    let float_board = |a, b, winner| Board {
        record: GameRecord::Short(Outcome::won(winner)),
        ..Board::pending(a, b, 2, PairingSource::Swiss)
    };
    let r1 = Round {
        number: 1,
        explanation: RoundExplanation::empty(1),
        pairing_explanation_valid: true,
        boards: vec![
            float_board(id(0), id(1), Winner::Player1),
            float_board(id(2), id(3), Winner::Player2),
        ],
        sitouts: Vec::new(),
        completed: true,
    };

    // Going into round 2: A on 2, C and D on 1, B on 0 — so one of C/D must
    // float up to A and the other down to B.
    let units = player_units(&p, &settings, std::slice::from_ref(&r1), &[]);
    let points = |i: usize| units[UnitKey::from(id(i))].points.halves();
    assert_eq!((points(0), points(1), points(2), points(3)), (4, 0, 2, 2));
    // C has only ever floated down, D only up.
    assert_eq!(units[UnitKey::from(id(2))].last_descended, Some(1));
    assert_eq!(units[UnitKey::from(id(2))].last_ascended, None);
    assert_eq!(units[UnitKey::from(id(3))].last_ascended, Some(1));
    assert_eq!(units[UnitKey::from(id(3))].last_descended, None);

    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let round = pair_players(2, &p, &settings, &[r1], &present);
    let pairs = board_pairs(&round);
    assert!(
        pairs.contains(&unord(id(0), id(2))),
        "C, who has never floated up, should be the one to float up: {pairs:?}"
    );
    assert!(
        pairs.contains(&unord(id(3), id(1))),
        "D, who has never floated down, should be the one to float down: {pairs:?}"
    );
    // Not a restatement of floater selection: that rule (classic) wants the
    // *strongest* of the group to float up, which is D — the very player the
    // float-repeat rule refuses to send up again. It outranks it, so C goes.
    assert!(
        units[UnitKey::from(id(3))].fold_rating() > units[UnitKey::from(id(2))].fold_rating(),
        "D must be the stronger, or the two rules would simply agree"
    );
}

#[test]
fn floater_selection_floats_down_the_weakest() {
    // Upper group (1 MacMahon point): X0>X1>X2 by rating. Lower group (0): a
    // single Y. One X must float down; it should be X2, the weakest of X, so
    // X0-X1 stay together.
    let p = vec![
        player(1, Some(2000), None), // X0 (rank 0 of the upper group)
        player(2, Some(1900), None), // X1
        player(3, Some(1800), None), // X2 (weakest)
        player(4, Some(1000), None), // Y (lower group)
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    // X0..X2 on 1 point, Y on 0
    let settings =
        TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

    let round = pair_players(1, &p, &settings, &[], &present);
    assert!(
        board_pairs(&round).contains(&unord(
            p[2].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "the weakest of the upper group should float down"
    );
}

#[test]
fn floater_selection_classic_vs_median_pick_different_ascenders() {
    // Upper group (1 point): a single H. Lower group (0 points): L0>L1>L2 by
    // rating. One L floats up: classic sends the first (L0), median the middle
    // (L1). The fold is indifferent between those two outcomes, so the floater
    // rule decides.
    let p = vec![
        player(1, Some(2000), None), // H  (1 MacMahon point)
        player(2, Some(1400), None), // L0 (rank 0 of the lower group)
        player(3, Some(1300), None), // L1 (median)
        player(4, Some(1200), None), // L2
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let base = TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

    let classic = base.clone().with_floater(FloaterStyle::Classic);
    let round = pair_players(1, &p, &classic, &[], &present);
    assert!(
        board_pairs(&round).contains(&unord(
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap()
        )),
        "classic Swiss floats up the strongest of the group (L0)"
    );

    let median = base.with_floater(FloaterStyle::Median);
    let round = pair_players(1, &p, &median, &[], &present);
    assert!(
        board_pairs(&round).contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )),
        "median Swiss floats up the median of the group (L1)"
    );
}

#[test]
fn floater_selection_median_descends_the_median_not_the_weakest() {
    // Upper group (1 point): X0>X1>X2 by rating. Lower group (0 points): a
    // single Y. One X must float down: classic sends the weakest (X2),
    // median sends the middle (X1).
    let p = vec![
        player(1, Some(2000), None), // X0 (strongest)
        player(2, Some(1900), None), // X1 (median)
        player(3, Some(1800), None), // X2 (weakest)
        player(4, Some(1000), None), // Y (lower group)
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
    let base = TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

    let classic = base.clone().with_floater(FloaterStyle::Classic);
    let round = pair_players(1, &p, &classic, &[], &present);
    assert!(
        board_pairs(&round).contains(&unord(
            p[2].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "classic Swiss floats down the weakest of the group (X2)"
    );

    let median = base.with_floater(FloaterStyle::Median);
    let round = pair_players(1, &p, &median, &[], &present);
    assert!(
        board_pairs(&round).contains(&unord(
            p[1].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )),
        "median Swiss floats down the median of the group (X1)"
    );
}

#[test]
fn floater_selection_median_gives_the_bye_to_the_median_of_the_group() {
    // 5 players, all in the same (single) score group in round 1, ranked
    // P0 > P1 > P2 > P3 > P4 by rating. Classic sends the bye to the
    // weakest (P4); median sends it to the middle of the group (P2).
    let p = vec![
        player(1, Some(2000), None), // P0
        player(2, Some(1800), None), // P1
        player(3, Some(1600), None), // P2 (median)
        player(4, Some(1400), None), // P3
        player(5, Some(1200), None), // P4 (weakest)
    ];
    let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

    let classic = TournamentSettings::default().with_floater(FloaterStyle::Classic);
    let round = pair_players(1, &p, &classic, &[], &present);
    assert_eq!(
        round.swiss_bye(),
        Some(p[4].tournament_id.unwrap()),
        "classic Swiss gives the bye to the weakest of the group (P4)"
    );

    let median = TournamentSettings::default().with_floater(FloaterStyle::Median);
    let round = pair_players(1, &p, &median, &[], &present);
    assert_eq!(
        round.swiss_bye(),
        Some(p[2].tournament_id.unwrap()),
        "median Swiss gives the bye to the median of the group (P2)"
    );
}

#[test]
fn scale_ladder_tiers_are_disjoint() {
    // Arbitrary per-rule worst-case totals, in priority order (highest first).
    let max_total = [7i128, 40, 13, 21, 5, 9];
    let mult = scale_ladder(&max_total);
    // Each rule's multiplier is exactly 1 plus the most every lower-priority
    // rule can contribute, so one of its units strictly dominates them all —
    // a correct lexicographic ordering.
    for i in 0..max_total.len() {
        let lower_max: i128 = ((i + 1)..max_total.len())
            .map(|j| mult[j] * max_total[j])
            .sum();
        assert_eq!(mult[i], 1 + lower_max);
        assert!(mult[i] > lower_max);
    }
    assert_eq!(mult[max_total.len() - 1], 1); // the lowest-priority rule is the unit
}

#[test]
#[should_panic(expected = "overflowed i128")]
fn scale_ladder_aborts_on_overflow_instead_of_wrapping() {
    // Two astronomically large worst-case totals push the running product past
    // i128. Without the checked arithmetic this would wrap silently in release
    // and corrupt the lexicographic ordering; it must abort loudly instead.
    let _ = scale_ladder(&[i128::MAX / 2, i128::MAX / 2]);
}

#[test]
fn rule_bounds_are_valid_upper_bounds() {
    // A field with rematches, a bye, floats, club-mates and a spread of scores
    // so every rule can fire; then assert no single edge (or bye) can exceed
    // the per-edge share of its rule's `max_total_units`.
    let p = vec![
        player(1, Some(2000), Some("X")),
        player(2, Some(1800), Some("X")),
        player(3, Some(1600), Some("Y")),
        player(4, Some(1400), Some("Y")),
        player(5, Some(1200), None),
    ];
    let id = |i: usize| p[i].tournament_id.unwrap();
    let r1 = completed_round(
        1,
        &[
            (id(0), id(1), Winner::Player1),
            (id(2), id(3), Winner::Player1),
        ],
        Some(id(4)), // p5 took a bye
    );
    let settings =
        TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
    let units = player_units(&p, &settings, &[r1], &[]);
    let free: Vec<UnitKey> = p
        .iter()
        .map(|q| UnitKey::from(q.tournament_id.unwrap()))
        .collect();
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    let (mut mm_lo, mut mm_hi) = (u32::MAX, 0u32);
    for &key in &free {
        let s = &units[key];
        lo = lo.min(s.points.halves());
        hi = hi.max(s.points.halves());
        mm_lo = mm_lo.min(s.macmahon.halves());
        mm_hi = mm_hi.max(s.macmahon.halves());
    }
    let edges = 3i128; // 5 free + phantom bye = 6 vertices → 3 edges
    let exempt_clubs = HashSet::new();
    let exempt_nationalities = HashSet::new();
    let fold = fold_ranks(&units, &free);
    let empty_rank: TiVec<UnitKey, i128> = vec![0i128; units.len()].into();
    let ctx = Ctx {
        units: &units,
        fold: &fold,
        round: 2,
        floater_style: FloaterStyle::Median, // exercise the floater-selection bound
        exempt_clubs: &exempt_clubs,
        exempt_nationalities: &exempt_nationalities,
        edges,
        max_gap: (hi - lo) as i128,
        min_points: lo as i128,
        max_mm_gap: (mm_hi - mm_lo) as i128,
        max_group: fold
            .iter()
            .flatten()
            .map(|f| f.group_size)
            .max()
            .unwrap_or(0) as i128,
        free_count: free.len() as i128,
        max_boards: free
            .iter()
            .map(|&k| units[k].clubs.len())
            .max()
            .unwrap_or(0) as i128,
        elo_rank: &empty_rank,
        max_elo_gap: 0,
    };

    // Check the Swiss rules (the ones active in the default mode). Bye-only
    // rules fire once per round, not once per edge, so their bound isn't
    // scaled by `edges` (see `max_total_units`).
    for &rule in active_rules(&TournamentSettings::default()) {
        let bound = rule.max_total_units(&ctx);
        let bye_scale = match rule {
            Rule::ByeSelection | Rule::ByeGroup => 1,
            _ => edges,
        };
        for i in 0..free.len() {
            for j in (i + 1)..free.len() {
                assert!(
                    rule.edge_units(&ctx, free[i], free[j]) * edges <= bound,
                    "an edge exceeded the rule's total-units bound"
                );
            }
            assert!(
                rule.bye_units(&ctx, free[i]) * bye_scale <= bound,
                "a bye exceeded the rule's total-units bound"
            );
        }
    }
}
