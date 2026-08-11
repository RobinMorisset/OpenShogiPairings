//! Enriching base players from a FESA rating list.
//!
//! A result table carries no game counts, so every player would import as
//! provisional. Given the tournament date, resolve the first list that postdates
//! it (FESA publishes one every 1 Jan and 1 Jul), fetch it, and match players by
//! name to recover each one's FESA game count — the established/provisional
//! reliability signal — without touching strengths.

use std::collections::HashMap;
use std::io::Read;

use uuid::Uuid;

use osp_core::{decode_latin1, parse_rating_list, RatedPlayer, Tournament};

/// A calendar date as `(year, month, day)` — ordered so tuple comparison is date
/// comparison.
type Ymd = (i32, u32, u32);

/// Parse a `YYYY-MM-DD` date.
pub fn parse_ymd(s: &str) -> Result<Ymd, String> {
    let parts: Vec<&str> = s.trim().split('-').collect();
    if parts.len() != 3 {
        return Err(format!("date '{s}' must be YYYY-MM-DD"));
    }
    let year = parts[0]
        .parse::<i32>()
        .map_err(|_| format!("bad year in '{s}'"))?;
    let month = parts[1]
        .parse::<u32>()
        .map_err(|_| format!("bad month in '{s}'"))?;
    let day = parts[2]
        .parse::<u32>()
        .map_err(|_| format!("bad day in '{s}'"))?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(format!("out-of-range date '{s}'"));
    }
    Ok((year, month, day))
}

/// FESA's stable "latest list" endpoint — always resolves to the most recent
/// published list (a permanent one, or a more-recent transient one). Mirrors
/// `crates/server/src/ratings.rs`'s `FESA_URL`.
const FESA_LATEST_URL: &str = "https://fesashogi.eu/old/ratinglists/latest.txt";

/// The FESA rating-list **URL** to fetch for a tournament dated `after`, given the
/// current date `today`.
///
/// FESA publishes a **permanent** list twice a year, on 1 January and 1 July, at a
/// dated URL. We target the first permanent list strictly after the tournament,
/// since it already reflects the results. If that list isn't published yet (its
/// date is still in the future), we fall back to the server's stable `latest.txt`
/// endpoint — the most recent list available, whether that is a permanent list or
/// a more-recent transient one. (A transient list has an irregular, uncomputable
/// date, so it can only be reached through that endpoint.)
pub fn list_url_for(after: Ymd, today: Ymd) -> String {
    let scheduled = next_permanent_list(after);
    if scheduled <= today {
        list_url(scheduled)
    } else {
        FESA_LATEST_URL.to_string()
    }
}

/// The first permanent FESA list date (1 Jan or 1 Jul) strictly after `after`.
fn next_permanent_list(after: Ymd) -> Ymd {
    let (y, _, _) = after;
    // Two permanent lists a year; the window [y, y+1] always contains the next one.
    [(y, 1, 1), (y, 7, 1), (y + 1, 1, 1), (y + 1, 7, 1)]
        .into_iter()
        .find(|&d| d > after)
        .expect("y+1 candidates always exceed a date in year y")
}

/// Today's date as `(year, month, day)` in UTC, from the system clock.
pub fn today_ymd() -> Ymd {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    civil_from_days((secs / 86_400) as i64)
}

/// Convert a day count since the Unix epoch to `(year, month, day)` (proleptic
/// Gregorian). Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> Ymd {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

/// The FESA URL for a dated rating list.
pub fn list_url(date: Ymd) -> String {
    format!(
        "https://fesashogi.eu/old/ratinglists/{:04}-{:02}-{:02}.txt",
        date.0, date.1, date.2
    )
}

/// Canonical name key for matching: lower-cased, diacritics folded to ASCII, and
/// whitespace collapsed.
///
/// The grid and the FESA list don't always spell accents the same way — one may
/// carry them and the other not (e.g. grid "Frederik" vs FESA "Frédérik"), and a
/// UTF-8 grid may store an accent decomposed (`e` + a combining mark) while the
/// Latin-1 list has it precomposed. Folding to a bare-ASCII key makes the match
/// robust to all of these. Names genuinely absent from the list still don't match
/// (they fall back to the grid rating) — folding only removes accent noise.
pub(crate) fn fold_name(s: &str) -> String {
    let folded: String = s
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => Some('a'),
            'ç' => Some('c'),
            'è' | 'é' | 'ê' | 'ë' => Some('e'),
            'ì' | 'í' | 'î' | 'ï' => Some('i'),
            'ñ' => Some('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => Some('o'),
            'ù' | 'ú' | 'û' | 'ü' => Some('u'),
            'ý' | 'ÿ' => Some('y'),
            'ß' => Some('s'),
            // Combining diacritical marks (the tail of a decomposed NFD letter).
            c if ('\u{0300}'..='\u{036F}').contains(&c) => None,
            c => Some(c),
        })
        .collect();
    // Collapse internal runs of whitespace so "Le  Roux" == "Le Roux".
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Match base players to FESA entries by folded `(last, first)` name and return
/// each matched player's FESA **game count** (plus how many matched). A result
/// table carries no game counts, so every player imports as provisional; filling
/// `fesa_games` from a rating list restores the established/provisional distinction
/// *without* touching strengths (the oracle still uses the results' post-ELO).
pub fn match_games(rated: &[RatedPlayer], base: &Tournament) -> (HashMap<Uuid, u32>, usize) {
    let by_name: HashMap<(String, String), u32> = rated
        .iter()
        .map(|r| ((fold_name(&r.last_name), fold_name(&r.first_name)), r.games))
        .collect();
    let mut map = HashMap::new();
    for p in &base.players {
        if let Some(&games) = by_name.get(&(fold_name(&p.last_name), fold_name(&p.first_name))) {
            map.insert(p.id, games);
        }
    }
    let matched = map.len();
    (map, matched)
}

/// Game counts from the first list after a tournament date. Returns the map, the
/// resolved list URL (for reporting), and the match count.
pub fn games_after(
    date: &str,
    base: &Tournament,
) -> Result<(HashMap<Uuid, u32>, String, usize), String> {
    let url = list_url_for(parse_ymd(date)?, today_ymd());
    let rated = parse_rating_list(&fetch_url(&url)?);
    let (map, matched) = match_games(&rated, base);
    Ok((map, url, matched))
}

/// Game counts from a specific list (URL or local path).
pub fn games_from_list(
    source: &str,
    base: &Tournament,
) -> Result<(HashMap<Uuid, u32>, usize), String> {
    let rated = parse_rating_list(&load_list_text(source)?);
    Ok(match_games(&rated, base))
}

/// Fetch (http/https URL) or read (local path) a Latin-1 FESA list into decoded
/// text. The FESA files are Latin-1, so we read raw bytes and decode explicitly.
pub fn load_list_text(source: &str) -> Result<String, String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        fetch_url(source)
    } else {
        let bytes = std::fs::read(source).map_err(|e| format!("reading {source}: {e}"))?;
        Ok(decode_latin1(&bytes))
    }
}

fn fetch_url(url: &str) -> Result<String, String> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| format!("fetching {url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading {url}: {e}"))?;
    Ok(decode_latin1(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use osp_core::NewPlayer;

    #[test]
    fn next_permanent_list_picks_the_first_jan_or_jul_after() {
        assert_eq!(next_permanent_list((2025, 11, 15)), (2026, 1, 1));
        assert_eq!(next_permanent_list((2026, 3, 10)), (2026, 7, 1));
        assert_eq!(next_permanent_list((2026, 6, 30)), (2026, 7, 1));
        // A date exactly on a list date resolves to the *next* one (strictly after).
        assert_eq!(next_permanent_list((2026, 1, 1)), (2026, 7, 1));
        assert_eq!(next_permanent_list((2026, 7, 1)), (2027, 1, 1));
        // WOSC 2024 (August) → the following January list.
        assert_eq!(next_permanent_list((2024, 8, 4)), (2025, 1, 1));
    }

    #[test]
    fn list_url_for_uses_the_dated_list_when_published_else_latest() {
        // Published: the dated URL of the first permanent list after the tournament.
        assert_eq!(
            list_url_for((2024, 8, 4), (2025, 3, 1)),
            "https://fesashogi.eu/old/ratinglists/2025-01-01.txt"
        );
        // The next permanent list is still in the future → the stable latest.txt.
        assert_eq!(list_url_for((2024, 8, 4), (2024, 9, 15)), FESA_LATEST_URL);
        // Mid-year, before 1 Jul: the next permanent (Jul) list isn't out yet, so
        // latest.txt — which also covers any transient list published in between.
        assert_eq!(list_url_for((2025, 2, 1), (2025, 6, 20)), FESA_LATEST_URL);
        // Once the July list is out, back to its dated URL.
        assert_eq!(
            list_url_for((2025, 2, 1), (2025, 7, 5)),
            "https://fesashogi.eu/old/ratinglists/2025-07-01.txt"
        );
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1)); // the Unix epoch
        assert_eq!(civil_from_days(18_993), (2022, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024 is a leap year
        assert_eq!(civil_from_days(19_905), (2024, 7, 1));
    }

    #[test]
    fn list_url_is_zero_padded() {
        assert_eq!(
            list_url((2026, 6, 1)),
            "https://fesashogi.eu/old/ratinglists/2026-06-01.txt"
        );
    }

    #[test]
    fn parse_ymd_rejects_garbage() {
        assert!(parse_ymd("2026-06-01").is_ok());
        assert!(parse_ymd("2026/06/01").is_err());
        assert!(parse_ymd("2026-13-01").is_err());
        assert!(parse_ymd("nope").is_err());
    }

    #[test]
    fn match_games_maps_by_name_case_insensitively_and_counts() {
        let mut base = Tournament::new("T").unwrap();
        let a_id = base
            .add_player(NewPlayer {
                last_name: "Goix".into(),
                first_name: Some("Nicolas".into()),
                rating: Some(1500),
                ..Default::default()
            })
            .unwrap()
            .id;
        let b_id = base
            .add_player(NewPlayer {
                last_name: "Cheymol".into(),
                first_name: Some("Jean".into()),
                rating: Some(2000),
                ..Default::default()
            })
            .unwrap()
            .id;
        base.finalize_registration().unwrap();

        let rated = vec![
            RatedPlayer {
                last_name: "  goix ".into(), // different case/spacing → still matches
                first_name: "NICOLAS".into(),
                rating: 1834,
                games: 40,
                grade: None,
                nationality: "FR".into(),
            },
            RatedPlayer {
                last_name: "Nobody".into(),
                first_name: "Here".into(),
                rating: 1200,
                games: 5,
                grade: None,
                nationality: "FR".into(),
            },
        ];

        // Keyed on the folded name, yielding the FESA game count. The unmatched
        // player is simply left out (so they stay provisional).
        let (games, matched) = match_games(&rated, &base);
        assert_eq!(matched, 1);
        assert_eq!(games[&a_id], 40);
        assert!(!games.contains_key(&b_id));
    }

    fn rated(last: &str, first: &str, games: u32) -> RatedPlayer {
        RatedPlayer {
            last_name: last.into(),
            first_name: first.into(),
            rating: 1500,
            games,
            grade: None,
            nationality: "FR".into(),
        }
    }

    #[test]
    fn folding_matches_across_accents_and_composition() {
        let mut base = Tournament::new("T").unwrap();
        // Base without the accent the list carries.
        let frederik = player(&mut base, "Wietholter", "Frederik");
        // Base with a *decomposed* (NFD) accent: "André" as 'e' + U+0301.
        let andre = player(&mut base, "Muller", "Andre\u{0301}");
        base.finalize_registration().unwrap();

        let list = vec![
            rated("Wietholter", "Frédérik", 42), // precomposed accents in the list
            rated("Müller", "André", 17),        // precomposed, plus an umlaut surname
        ];
        let (games, matched) = match_games(&list, &base);
        assert_eq!(matched, 2, "accents/composition should not block a match");
        assert_eq!(games[&frederik], 42);
        assert_eq!(games[&andre], 17);
    }

    /// A player with the given last/first name.
    fn player(base: &mut Tournament, last: &str, first: &str) -> uuid::Uuid {
        base.add_player(NewPlayer {
            last_name: last.into(),
            first_name: Some(first.into()),
            rating: Some(1500),
            ..Default::default()
        })
        .unwrap()
        .id
    }

    #[test]
    fn games_from_a_repo_fixture_parse_and_match_without_network() {
        // Reads a checked-in, synthetic FESA list through the real fixed-width
        // parser — no download. Guards the parse + name-match path end to end.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/fesa-ratinglist-sample.txt"
        );
        let mut base = Tournament::new("T").unwrap();
        let cheymol = player(&mut base, "Cheymol", "Jean");
        let goix = player(&mut base, "Goix", "Nicolas");
        let ghost = player(&mut base, "Ghost", "Nobody"); // absent from the list
        base.finalize_registration().unwrap();

        let (games, matched) = games_from_list(path, &base).unwrap();
        assert_eq!(matched, 2);
        assert_eq!(games[&cheymol], 60);
        assert_eq!(games[&goix], 45);
        assert!(!games.contains_key(&ghost)); // unmatched → stays provisional
    }
}
