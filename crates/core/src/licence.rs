//! Cross-checking the registered field against a federation's licence list.
//!
//! A referee running, say, a French tournament exports from their federation the
//! list of players whose yearly licence is paid up, and drops it here: every
//! registered player of that nationality who is **not** on the list comes back,
//! so the ones who forgot can be chased before the first round.
//!
//! Read-only — this answers a question about the roster, it never edits it. The
//! decision of what to do about an unlicensed player (chase them, remove them,
//! let them play) stays with the referee.
//!
//! The list is parsed by [`parse_players_csv`], the same code path as a roster
//! import, so a file that imports also checks: same column aliases, same `,`/`;`
//! sniffing, same quoting, same all-or-nothing rejection of a row with no last
//! name. Only `Last name` / `First name` are read; a licence number, a club or an
//! expiry date alongside them is ignored like any other unrecognized column.
//!
//! Matching is by accent-folded `last + first` name ([`name_key`]), the same key
//! the importer uses to find a player in the FESA list. It is deliberately
//! strict: a player whose name is spelled differently in the two places is
//! reported as missing rather than quietly matched, because the failure the
//! referee must not have is a player who *looks* covered and isn't.
//!
//! # Near misses
//!
//! Federation exports are typed by hand and misspellings in them are common, so
//! a player reported as missing also carries every list entry whose name is
//! [one edit](within_one_edit) from theirs — as the *list* spells it, which is
//! the spelling the referee has to look at to judge.
//!
//! A near miss annotates, it never resolves: the player stays in the missing
//! list. One edit apart is routinely two different people (`Nguyen Anh` and
//! `Nguyen Ana`, and short names generally), so this can only mean "look at this
//! one" — treating it as payment would reintroduce exactly the silent false
//! clear the strict match exists to avoid.

use std::collections::HashSet;

use serde::Serialize;
use ts_rs::TS;
use uuid::Uuid;

use crate::csv_import::{name_key, normalize, parse_players_csv, CsvImportError};
use crate::player::Player;

/// The outcome of checking one nationality's registered players against a
/// licence list. The counts are what tells a referee they loaded the file they
/// meant to: an empty `missing` next to `listed: 3` is a wrong file, not a
/// well-behaved club.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct LicenceCheck {
    /// How many rows the licence list carried (as given, duplicates included).
    pub listed: usize,
    /// How many registered players had the nationality being checked.
    pub checked: usize,
    /// Those of them with no entry in the list, in registration order.
    pub missing: Vec<UnlicensedPlayer>,
}

/// A registered player the licence list does not carry, and the entries it does
/// carry that are nearly their name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct UnlicensedPlayer {
    /// The registered player, by [`Player::id`].
    pub id: Uuid,
    /// Entries one edit from their name, **as the list spells them** — the
    /// referee compares the two spellings, so ours would tell them nothing.
    /// Usually empty; more than one entry is itself worth seeing, so none are
    /// dropped. A near miss is a prompt to check, never a licence found.
    pub near_misses: Vec<String>,
}

/// Check every registered player of `nationality` against the licence list in
/// `csv`, returning the ones it doesn't carry.
///
/// `nationality` is matched against each player's own the way the CSV names are
/// — accent-folded and case-insensitively — so `fr` finds the `FR` the server
/// stored. Returns a [`CsvImportError`] (and no answer at all) if the list is
/// empty, lacks the two name columns, or has a row with no last name.
///
/// Players with **no** nationality are never checked, whatever is asked for: an
/// unset field is not a claim to be French. Callers that want them looked at
/// have to say so by showing them; passing an empty `nationality` to mean "the
/// ones with none" is a caller bug, not an API.
pub fn check_licences(
    csv: &str,
    players: &[Player],
    nationality: &str,
) -> Result<LicenceCheck, CsvImportError> {
    debug_assert!(
        !nationality.trim().is_empty(),
        "check_licences needs a nationality to check; callers validate this at their boundary",
    );

    // No FESA list: the ELO/grade enrichment it drives is irrelevant here, only
    // the names are read. Each entry is kept as (folded key, spelling as given):
    // the key answers "is this player on the list?", the spelling is what a near
    // miss shows the referee.
    let listed = parse_players_csv(csv, &[])?;
    let entries: Vec<(String, String)> = listed
        .iter()
        .map(|p| {
            let first = p.first_name.as_deref().unwrap_or("");
            (
                name_key(&p.last_name, first),
                format!("{} {}", p.last_name, first).trim().to_string(),
            )
        })
        .collect();
    let licensed: HashSet<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();

    let wanted = normalize(nationality);
    let mut checked = 0;
    let mut missing = Vec::new();
    for player in players {
        let Some(nat) = player.nationality.as_deref() else {
            continue;
        };
        if nat.trim().is_empty() || normalize(nat) != wanted {
            continue;
        }
        checked += 1;
        let key = name_key(&player.last_name, &player.first_name);
        if licensed.contains(key.as_str()) {
            continue;
        }
        // Only the players already known to be missing are scanned against the
        // list, and `within_one_edit` walks the two names once rather than
        // filling a distance matrix — so the whole scan stays a rounding error
        // next to parsing the file.
        let mut near_misses: Vec<String> = Vec::new();
        for (entry_key, spelling) in &entries {
            if within_one_edit(&key, entry_key) && !near_misses.contains(spelling) {
                near_misses.push(spelling.clone());
            }
        }
        missing.push(UnlicensedPlayer {
            id: player.id,
            near_misses,
        });
    }

    Ok(LicenceCheck {
        listed: listed.len(),
        checked,
        missing,
    })
}

/// Whether `a` and `b` are at most one edit apart: one substitution, one
/// insertion, one deletion, or one swap of adjacent characters — the optimal
/// string alignment distance, bounded at 1. (Identical strings are zero edits
/// apart and so also "within one"; the caller has already excluded them.)
///
/// A swap counts as one edit rather than the two plain Levenshtein charges it,
/// because transposed letters are one of the commonest hand-typing errors and
/// `Damein` for `Damien` is exactly the case this is here to catch.
///
/// Bounded at 1 by construction rather than by computing a distance and
/// comparing: at most three differing positions are ever examined, so this walks
/// the names once instead of filling an `n × m` matrix.
fn within_one_edit(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    match a.len().abs_diff(b.len()) {
        // Same length: equal, one substitution, or one adjacent swap.
        0 => {
            let mut differing = a.iter().zip(&b).enumerate().filter(|(_, (x, y))| x != y);
            let Some((i, _)) = differing.next() else {
                return true; // identical
            };
            let Some((j, _)) = differing.next() else {
                return true; // one substitution
            };
            // Two differences are a single edit only as a swap: adjacent, and
            // each character found where the other one is.
            differing.next().is_none() && j == i + 1 && a[i] == b[j] && a[j] == b[i]
        }
        // One longer: an insertion in the shorter, i.e. the longer with one
        // character dropped is the shorter. Everything after the first
        // difference must line up once that character is skipped.
        1 => {
            let (short, long) = if a.len() < b.len() {
                (&a, &b)
            } else {
                (&b, &a)
            };
            let skip = short
                .iter()
                .zip(long.iter())
                .position(|(x, y)| x != y)
                .unwrap_or(short.len());
            short[skip..] == long[skip + 1..]
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registered player, with the fields this check reads.
    fn player(last: &str, first: &str, nationality: Option<&str>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: None,
            last_name: last.to_string(),
            first_name: first.to_string(),
            rating: None,
            pairing_rating: None,
            grade: None,
            fesa_games: None,
            nationality: nationality.map(str::to_string),
            club: None,
            eligible: false,
            categories: Vec::new(),
            adjustments: Vec::new(),
        }
    }

    /// The missing players' names, which read better in an assertion than uuids.
    fn missing_names(check: &LicenceCheck, players: &[Player]) -> Vec<String> {
        check
            .missing
            .iter()
            .map(|m| {
                let p = players.iter().find(|p| p.id == m.id).expect("known player");
                format!("{} {}", p.last_name, p.first_name)
            })
            .collect()
    }

    /// The near misses reported for the single missing player.
    fn only_near_misses(check: &LicenceCheck) -> &[String] {
        assert_eq!(
            check.missing.len(),
            1,
            "expected exactly one missing player"
        );
        &check.missing[0].near_misses
    }

    #[test]
    fn reports_the_registered_players_absent_from_the_list() {
        let players = vec![
            player("Alpha", "Ann", Some("FR")),
            player("Beta", "Bo", Some("FR")),
            player("Gamma", "Gil", Some("FR")),
        ];
        let csv = "Last name,First name\nAlpha,Ann\nGamma,Gil\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(check.listed, 2);
        assert_eq!(check.checked, 3);
        assert_eq!(missing_names(&check, &players), ["Beta Bo"]);
    }

    #[test]
    fn other_nationalities_are_left_alone() {
        // The Japanese player is on nobody's French licence list, and is not
        // reported: the check is scoped to the nationality asked for.
        let players = vec![
            player("Alpha", "Ann", Some("FR")),
            player("Kobayashi", "Taichi", Some("JP")),
        ];
        let csv = "Last name,First name\nAlpha,Ann\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(check.checked, 1);
        assert!(check.missing.is_empty());
    }

    #[test]
    fn the_nationality_is_matched_case_insensitively() {
        // The UI offers the stored spelling, but a hand-built request may not.
        let players = vec![player("Alpha", "Ann", Some("FR"))];
        let csv = "Last name,First name\nBeta,Bo\n";
        let check = check_licences(csv, &players, "fr").unwrap();
        assert_eq!(check.checked, 1);
        assert_eq!(check.missing.len(), 1);
    }

    #[test]
    fn players_with_no_nationality_are_never_checked() {
        // Neither the missing field nor a blank one counts as the nationality
        // asked for — they are reported to the referee separately, by the client
        // that can see the whole roster, rather than folded in here.
        let players = vec![
            player("Alpha", "Ann", None),
            player("Beta", "Bo", Some("  ")),
            player("Gamma", "Gil", Some("FR")),
        ];
        let csv = "Last name,First name\nGamma,Gil\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(check.checked, 1);
        assert!(check.missing.is_empty());
    }

    #[test]
    fn names_match_across_accents_punctuation_and_case() {
        let players = vec![
            player("Róvekamp", "Frédéric", Some("FR")),
            player("Le Roux", "Jean-Pierre", Some("FR")),
        ];
        let csv = "Last name,First name\nROVEKAMP,Frederic\nLe-Roux,Jean Pierre\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert!(
            check.missing.is_empty(),
            "{:?}",
            missing_names(&check, &players)
        );
    }

    #[test]
    fn a_different_first_name_is_reported_rather_than_matched() {
        // Same family name, different given name: two different people as far as
        // this check is concerned, and the referee is told so.
        let players = vec![player("Alpha", "Ann", Some("FR"))];
        let csv = "Last name,First name\nAlpha,Bob\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(check.missing.len(), 1);
    }

    #[test]
    fn a_french_export_with_semicolons_and_extra_columns_works() {
        // The shape a federation's back-office actually exports: French headers,
        // `;` separated, with licence-specific columns the check ignores.
        let players = vec![
            player("Dupont", "Jean", Some("FR")),
            player("Martin", "Marie", Some("FR")),
        ];
        let csv = "Licence;Nom;Prénom;Validité\n\
                   12345;Dupont;Jean;2026-08-31\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(check.listed, 1);
        assert_eq!(missing_names(&check, &players), ["Martin Marie"]);
    }

    #[test]
    fn a_player_registered_without_a_first_name_needs_one_in_the_list_too() {
        // The list's own blank first name matches; a spelled-out one does not.
        // Reporting the mismatch is the deliberate choice: a name the referee
        // has to glance at beats a match that isn't one.
        let players = vec![player("Alpha", "", Some("FR"))];
        assert!(
            check_licences("Last name,First name\nAlpha,\n", &players, "FR")
                .unwrap()
                .missing
                .is_empty()
        );
        assert_eq!(
            check_licences("Last name,First name\nAlpha,Ann\n", &players, "FR")
                .unwrap()
                .missing
                .len(),
            1
        );
    }

    #[test]
    fn a_malformed_list_is_an_error_not_an_empty_answer() {
        // Nothing partial comes back: a file with no name columns would
        // otherwise read as "nobody is licensed", which is the one wrong answer
        // that looks like a real one.
        let players = vec![player("Alpha", "Ann", Some("FR"))];
        assert_eq!(
            check_licences("Licence,Club\n12345,Paris\n", &players, "FR"),
            Err(CsvImportError::MissingNameColumns)
        );
        assert_eq!(
            check_licences("", &players, "FR"),
            Err(CsvImportError::Empty)
        );
        assert_eq!(
            check_licences("Last name,First name\nAlpha,Ann\n,Bo\n", &players, "FR"),
            Err(CsvImportError::RowsMissingLastName { rows: vec![3] })
        );
    }

    #[test]
    fn a_misspelling_in_the_list_is_reported_as_a_near_miss() {
        // The federation typed DUPOND for DUPONT. The player is *still* missing
        // — nobody is marked paid on the strength of a guess — but the list's
        // own spelling comes back with them, which is what the referee needs to
        // see to judge it.
        let players = vec![player("Dupont", "Jean", Some("FR"))];
        let csv = "Last name,First name\nDUPOND,Jean\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(only_near_misses(&check), ["DUPOND Jean"]);
    }

    #[test]
    fn every_single_edit_shape_counts_as_a_near_miss() {
        let players = vec![player("Martin", "Damien", Some("FR"))];
        // In turn: a substitution, a dropped letter, an extra letter, and two
        // letters swapped — the last being the one plain Levenshtein would
        // charge two edits for and miss.
        for typo in [
            "Martin,Damian",
            "Martin,Damen",
            "Martin,Damiien",
            "Martin,Damein",
        ] {
            let csv = format!("Last name,First name\n{typo}\n");
            let check = check_licences(&csv, &players, "FR").unwrap();
            assert_eq!(
                only_near_misses(&check).len(),
                1,
                "{typo} should be one edit from Martin Damien"
            );
        }
    }

    #[test]
    fn two_edits_away_is_not_a_near_miss() {
        // The line has to be somewhere, and past one edit the suggestions are
        // noise the referee has to dismiss one by one.
        let players = vec![player("Martin", "Damien", Some("FR"))];
        let csv = "Last name,First name\nMartin,Dominic\nBernard,Damien\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert!(only_near_misses(&check).is_empty());
    }

    #[test]
    fn a_near_miss_is_found_across_the_two_names_and_the_folding() {
        // The key is the folded `last + first`, so a typo lands wherever it
        // lands — and a difference the fold erases (an accent, a hyphen) is not
        // an edit at all: that player matches outright and is not missing.
        let players = vec![
            player("Le Roux", "Jean-Pierre", Some("FR")),
            player("Róvekamp", "Frédéric", Some("FR")),
        ];
        let csv = "Last name,First name\nLe Rous,Jean Pierre\nROVEKAMP,Frederic\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(missing_names(&check, &players), ["Le Roux Jean-Pierre"]);
        assert_eq!(only_near_misses(&check), ["Le Rous Jean Pierre"]);
    }

    #[test]
    fn several_near_misses_are_all_reported() {
        // Two candidates is itself the answer — the referee needs to see both
        // to tell a typo from a genuinely different player — so neither is
        // dropped in favour of a "best" one.
        let players = vec![player("Li", "Bo", Some("FR"))];
        let csv = "Last name,First name\nLi,Ba\nLi,Ho\nWang,Wei\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert_eq!(only_near_misses(&check), ["Li Ba", "Li Ho"]);
    }

    #[test]
    fn a_player_on_the_list_is_never_a_near_miss_of_anything() {
        // Exact match wins outright: no entry is scanned for someone who is on
        // the list, so a near-twin cannot cast doubt on a player who has paid.
        let players = vec![player("Martin", "Damien", Some("FR"))];
        let csv = "Last name,First name\nMartin,Damien\nMartin,Damian\n";
        let check = check_licences(csv, &players, "FR").unwrap();
        assert!(check.missing.is_empty());
    }

    #[test]
    fn within_one_edit_holds_its_line() {
        // The helper on its own, including the ends of the strings, which the
        // walk handles separately from the middle.
        assert!(within_one_edit("dupont jean", "dupont jean")); // identical
        assert!(within_one_edit("abc", "abd")); // substitution at the end
        assert!(within_one_edit("abc", "xbc")); // substitution at the start
        assert!(within_one_edit("abc", "abcd")); // insertion at the end
        assert!(within_one_edit("abc", "xabc")); // insertion at the start
        assert!(within_one_edit("abcd", "abc")); // deletion, arguments the other way
        assert!(within_one_edit("ab", "ba")); // swap of the whole string
        assert!(within_one_edit("", "a")); // one empty side
        assert!(!within_one_edit("abc", "acb x")); // two edits: swap plus insert
        assert!(!within_one_edit("abcd", "badc")); // two swaps
        assert!(!within_one_edit("abc", "abcde")); // two insertions
        assert!(!within_one_edit("abab", "baba")); // same letters, four positions apart
    }

    #[test]
    fn a_nationality_nobody_registered_checks_nobody() {
        let players = vec![player("Alpha", "Ann", Some("FR"))];
        let check = check_licences("Last name,First name\nAlpha,Ann\n", &players, "DE").unwrap();
        assert_eq!(check.checked, 0);
        assert_eq!(check.listed, 1); // the list was still read
        assert!(check.missing.is_empty());
    }
}
