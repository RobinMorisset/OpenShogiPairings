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
    pub missing: Vec<Uuid>,
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
    // the names are read.
    let listed = parse_players_csv(csv, &[])?;
    let licensed: HashSet<String> = listed
        .iter()
        .map(|p| name_key(&p.last_name, p.first_name.as_deref().unwrap_or("")))
        .collect();

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
        if !licensed.contains(&name_key(&player.last_name, &player.first_name)) {
            missing.push(player.id);
        }
    }

    Ok(LicenceCheck {
        listed: listed.len(),
        checked,
        missing,
    })
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
            .map(|id| {
                let p = players.iter().find(|p| p.id == *id).expect("known player");
                format!("{} {}", p.last_name, p.first_name)
            })
            .collect()
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
    fn a_nationality_nobody_registered_checks_nobody() {
        let players = vec![player("Alpha", "Ann", Some("FR"))];
        let check = check_licences("Last name,First name\nAlpha,Ann\n", &players, "DE").unwrap();
        assert_eq!(check.checked, 0);
        assert_eq!(check.listed, 1); // the list was still read
        assert!(check.missing.is_empty());
    }
}
