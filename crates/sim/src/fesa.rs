//! Deriving player "true strength" from a FESA rating list.
//!
//! For a historical tournament, the *next* FESA rating list after the event
//! already reflects its results, so it is a good proxy for each player's true
//! strength. Given the tournament date, resolve the first list that postdates it
//! (FESA publishes one every 1 Jan and 1 Jun), fetch it, and match players by
//! name to build a strength-override map (see `osp_core::sim::sample_strengths`).

use std::collections::HashMap;
use std::io::Read;

use osp_core::sim::StrengthMap;
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

/// The first FESA rating-list date (1 Jan or 1 Jun) strictly after `after`.
pub fn next_list_date(after: Ymd) -> Ymd {
    let (y, _, _) = after;
    // Two lists a year; the window [y, y+1] always contains the next one.
    [(y, 1, 1), (y, 6, 1), (y + 1, 1, 1), (y + 1, 6, 1)]
        .into_iter()
        .find(|&d| d > after)
        .expect("y+1 candidates always exceed a date in year y")
}

/// The FESA URL for a dated rating list.
pub fn list_url(date: Ymd) -> String {
    format!(
        "https://fesashogi.eu/old/ratinglists/{:04}-{:02}-{:02}.txt",
        date.0, date.1, date.2
    )
}

/// Canonical name key for matching: trimmed and lower-cased.
fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Build a strength override by matching base players to FESA entries on
/// `(last, first)` name. Returns the map and how many players matched.
pub fn match_fesa(rated: &[RatedPlayer], base: &Tournament) -> (StrengthMap, usize) {
    let by_name: HashMap<(String, String), u32> = rated
        .iter()
        .map(|r| {
            (
                (normalize(&r.last_name), normalize(&r.first_name)),
                r.rating,
            )
        })
        .collect();
    let mut map = StrengthMap::new();
    for p in &base.players {
        if let Some(&elo) = by_name.get(&(normalize(&p.last_name), normalize(&p.first_name))) {
            map.insert(p.id, f64::from(elo));
        }
    }
    let matched = map.len();
    (map, matched)
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

/// Resolve overrides from the first list after a tournament date. Returns the
/// map, the resolved list URL (for reporting), and the match count.
pub fn overrides_after(
    date: &str,
    base: &Tournament,
) -> Result<(StrengthMap, String, usize), String> {
    let url = list_url(next_list_date(parse_ymd(date)?));
    let rated = parse_rating_list(&fetch_url(&url)?);
    let (map, matched) = match_fesa(&rated, base);
    Ok((map, url, matched))
}

/// Resolve overrides from a specific list (URL or local path). Returns the map
/// and the match count.
pub fn overrides_from_list(
    source: &str,
    base: &Tournament,
) -> Result<(StrengthMap, usize), String> {
    let rated = parse_rating_list(&load_list_text(source)?);
    Ok(match_fesa(&rated, base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use osp_core::NewPlayer;

    #[test]
    fn next_list_date_picks_the_first_jan_or_jun_after() {
        assert_eq!(next_list_date((2025, 11, 15)), (2026, 1, 1));
        assert_eq!(next_list_date((2026, 3, 10)), (2026, 6, 1));
        assert_eq!(next_list_date((2026, 5, 31)), (2026, 6, 1));
        // A date exactly on a list date resolves to the *next* one (strictly after).
        assert_eq!(next_list_date((2026, 1, 1)), (2026, 6, 1));
        assert_eq!(next_list_date((2026, 6, 1)), (2027, 1, 1));
        assert_eq!(next_list_date((2026, 7, 1)), (2027, 1, 1));
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
    fn match_fesa_maps_by_name_case_insensitively_and_counts() {
        let mut base = Tournament::new("T").unwrap();
        let a = base
            .add_player(NewPlayer {
                last_name: "Goix".into(),
                first_name: Some("Nicolas".into()),
                rating: Some(1500),
                ..Default::default()
            })
            .unwrap()
            .id;
        let b = base
            .add_player(NewPlayer {
                last_name: "Cheymol".into(),
                first_name: Some("Jean".into()),
                rating: Some(2000),
                ..Default::default()
            })
            .unwrap()
            .id;

        let rated = vec![
            RatedPlayer {
                last_name: "  goix ".into(), // different case/spacing → still matches
                first_name: "NICOLAS".into(),
                rating: 1834,
                games: 40,
                nationality: "FR".into(),
            },
            RatedPlayer {
                last_name: "Nobody".into(),
                first_name: "Here".into(),
                rating: 1200,
                games: 5,
                nationality: "FR".into(),
            },
        ];

        let (map, matched) = match_fesa(&rated, &base);
        assert_eq!(matched, 1);
        assert_eq!(map[&a], 1834.0); // post-tournament rating used as truth
        assert!(!map.contains_key(&b)); // no FESA entry → falls back to grid rating
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
    fn overrides_from_a_repo_fixture_parse_and_match_without_network() {
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

        let (map, matched) = overrides_from_list(path, &base).unwrap();
        assert_eq!(matched, 2);
        assert_eq!(map[&cheymol], 2011.0);
        assert_eq!(map[&goix], 1720.0);
        assert!(!map.contains_key(&ghost)); // unmatched → keeps its grid rating
    }
}
