//! Parser for the FESA (European Shogi Federation) rating list.
//!
//! The list is published as a fixed-width text file, e.g.
//! <https://fesashogi.eu/old/ratinglists/2026-06-01.txt>:
//!
//! ```text
//! Elo list for 2026-06-01
//!
//!     Name                               Grades         Elo #games Nationality
//!   1 Kobayashi         Taichi           5 Dan   5 Dan  2556   55    JP
//!   3 Takita            Hirotaka                        2462    7    JP
//! ```
//!
//! Three header lines, then one row per player. Quirks that drive the parsing
//! strategy:
//!
//! - **Fixed width, not delimited.** The last-name field is a fixed number of
//!   characters; the family name can contain spaces (`Ågren Thuné`) or fill the
//!   field with no trailing space (`Imamura-Cornuejols` abutting `Toru`), so
//!   whitespace-splitting cannot recover it — we slice the fixed column. That
//!   width isn't universal (FESA has widened it before), so it is *detected*
//!   per file — the mode of the first 2-space gap across rows — rather than
//!   hardcoded, falling back to [`DEFAULT_LAST_NAME_WIDTH`] if detection finds
//!   nothing (e.g. an empty list).
//! - **Latin-1 encoded**, not UTF-8 (`Rövekamp`), so callers decode with
//!   [`decode_latin1`] first (1 byte = 1 char = 1 column).
//! - **Variable grades** (0, 1 or 2 of `N Dan` / `N Kyu`) sit between the given
//!   name and the Elo; we parse the trailing Elo / #games / nationality tokens
//!   from the right, then strip any grade tokens the same way, keeping the
//!   first (leftmost) grade when two are given.
//! - The rank field **overflows at 4 digits**, shifting later columns, so we
//!   anchor to the name start (after removing the rank) rather than to absolute
//!   positions.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::csv_import::name_key;
use crate::player::Grade;

/// Fallback last-name column width if detection finds nothing (e.g. an empty
/// list); the real width is [`detect_last_name_width`].
const DEFAULT_LAST_NAME_WIDTH: usize = 18;

/// Number of header lines before the player rows begin.
const HEADER_LINES: usize = 3;

/// One entry from the FESA rating list. Used to autocomplete registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RatedPlayer {
    pub last_name: String,
    pub first_name: String,
    pub rating: u32,
    /// Number of rated games behind this rating. Used to judge how established a
    /// rating is (the ELO estimator widens the prior for provisional ratings).
    /// Always the value the list carried: a row whose `#games` column is missing
    /// or unreadable is reported as malformed, never defaulted to 0 (which would
    /// silently demote an established player to provisional).
    pub games: u32,
    /// Country code, uppercased (e.g. `JP`, `FR`).
    pub nationality: String,
    /// The player's dan/kyu grade, if the row listed one. Some rows list two
    /// (e.g. a national and a local grade) — the first (leftmost) is kept.
    /// `None` when the row had no grade column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub grade: Option<Grade>,
}

/// The FESA file is Latin-1, so it is not valid UTF-8 and can't be read as a
/// `String` directly. Each byte maps to the identically-numbered Unicode code
/// point, which is exactly Latin-1 — and keeps 1 byte = 1 character, so the
/// fixed-width column offsets stay correct.
pub fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// A rating list that could not be read as published.
///
/// Rows that don't *look* like player rows at all (blank lines, a trailing
/// footer) are skipped silently — they always were. A row that starts with a
/// rank but doesn't parse is different: it means the columns moved under us, and
/// dropping it would show the referee a list with players silently missing, or
/// established players silently demoted to provisional. So it fails the parse
/// and names the lines.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "the FESA rating list has {count} unreadable player row(s) (line {}) — the format may have changed",
    format_lines(.lines, *.count)
)]
pub struct RatingListError {
    /// How many rows failed, however many are named in `lines`.
    pub count: usize,
    /// 1-based line numbers of the first [`MAX_REPORTED_LINES`] of them.
    pub lines: Vec<usize>,
}

/// How many bad line numbers an error names before giving up on listing them —
/// a wholesale format change would otherwise put a thousand numbers in a message.
const MAX_REPORTED_LINES: usize = 10;

/// Render the reported line numbers as `4, 9, 27`, with a `…` when `count` says
/// there were more than the ones named.
fn format_lines(lines: &[usize], count: usize) -> String {
    let joined = lines
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if count > lines.len() {
        format!("{joined}, …")
    } else {
        joined
    }
}

/// Parse the (already Latin-1 decoded) rating list into player entries.
///
/// Fails — rather than returning a partial list — as soon as any row that looks
/// like a player row doesn't parse: a silently shortened list reads exactly like
/// a correct one, and half the field vanishing from registration autocomplete is
/// not something a referee can be expected to notice.
pub fn parse_rating_list(text: &str) -> Result<Vec<RatedPlayer>, RatingListError> {
    let data_lines: Vec<&str> = text.lines().skip(HEADER_LINES).collect();
    // The last-name column width varies between exports, so measure it from the
    // rows before parsing them.
    let width = detect_last_name_width(data_lines.iter().copied());
    let mut players = Vec::new();
    let mut lines = Vec::new();
    let mut count = 0;
    for (offset, line) in data_lines.into_iter().enumerate() {
        match parse_row(line, width) {
            Row::Player(p) => players.push(p),
            Row::Malformed => {
                count += 1;
                if lines.len() < MAX_REPORTED_LINES {
                    // 1-based line number in the file, past the header lines.
                    lines.push(HEADER_LINES + offset + 1);
                }
            }
            Row::Other => {}
        }
    }
    if count == 0 {
        Ok(players)
    } else {
        Err(RatingListError { count, lines })
    }
}

/// Where a name lookup in the rating list landed.
///
/// `Ambiguous` exists because a European-wide list really does carry homonyms,
/// and the two callers used to resolve them differently (first match here, last
/// match in the simulator) — so the same name silently meant two different
/// ratings depending on who asked. Now neither picks: the caller is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup<'a> {
    /// No entry with this name.
    None,
    /// Exactly one entry.
    One(&'a RatedPlayer),
    /// Several entries share this name; how many.
    Ambiguous(usize),
}

/// Look a player up in the rating list by accent-folded `last + first` name —
/// the single matching rule shared by every consumer of the list.
pub fn lookup<'a>(list: &'a [RatedPlayer], last: &str, first: &str) -> Lookup<'a> {
    let key = name_key(last, first);
    let mut found: Option<&RatedPlayer> = None;
    let mut count = 0;
    for entry in list {
        if name_key(&entry.last_name, &entry.first_name) == key {
            count += 1;
            found.get_or_insert(entry);
        }
    }
    match (count, found) {
        (0, _) | (_, None) => Lookup::None,
        (1, Some(entry)) => Lookup::One(entry),
        (n, Some(_)) => Lookup::Ambiguous(n),
    }
}

/// The last-name column width (chars from the name start to the first name),
/// measured as the most common first-2-space-gap position across the rows. The
/// width varies between exports, so it must be measured, not hardcoded; rows
/// whose last name doesn't fill the column reveal the boundary, and the mode is
/// robust to the exact-fill minority.
fn detect_last_name_width<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for line in lines {
        let Some(after_rank) = strip_rank(line) else {
            continue;
        };
        let chars: Vec<char> = after_rank.chars().collect();
        // First run of >= 2 spaces (single spaces are within a multi-word last
        // name), then the first non-space: the first name. A row whose last
        // name exactly fills the column (no gap at all, e.g. abutting the first
        // name) contributes nothing rather than a spurious position.
        let mut j = 0;
        let mut found_gap = false;
        while j + 1 < chars.len() {
            if chars[j] == ' ' && chars[j + 1] == ' ' {
                found_gap = true;
                break;
            }
            j += 1;
        }
        if !found_gap {
            continue;
        }
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        if j < chars.len() && j > 0 {
            *counts.entry(j).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map_or(DEFAULT_LAST_NAME_WIDTH, |(w, _)| w)
}

/// What one line of the list turned out to be.
enum Row {
    /// A player row that parsed.
    Player(RatedPlayer),
    /// A player row (it carries a leading rank) that did *not* parse.
    Malformed,
    /// Not a player row at all: a blank line or a footer. Skipped silently.
    Other,
}

fn parse_row(line: &str, width: usize) -> Row {
    // Drop the leading rank ("  1 ", "1000 ", …) and anchor to the name start;
    // this absorbs the 4-digit rank column shift. No rank, no player row.
    let Some(after_rank) = strip_rank(line) else {
        return Row::Other;
    };
    match parse_ranked_row(after_rank, width) {
        Some(player) => Row::Player(player),
        None => Row::Malformed,
    }
}

/// Parse the part of a player row after its rank, or `None` if it doesn't parse
/// — which, the rank having already identified it as a player row, is a
/// malformed row rather than a line to skip.
fn parse_ranked_row(after_rank: &str, width: usize) -> Option<RatedPlayer> {
    let chars: Vec<char> = after_rank.chars().collect();
    if chars.len() <= width {
        return None;
    }

    // Last name is the fixed-width column; the family name may contain spaces or
    // fill the field, so it can only be read positionally.
    let last_name = chars[..width].iter().collect::<String>().trim().to_string();
    if last_name.is_empty() {
        return None;
    }

    // Everything after the last-name column: first name + grades + trailing
    // Elo / #games / nationality.
    let rest: String = chars[width..].iter().collect();
    let mut tokens: Vec<&str> = rest.split_whitespace().collect();

    // The last three tokens are always nationality, #games, Elo (right to left).
    // All three must parse: a `#games` that quietly became 0 would demote an
    // established player to provisional and widen their ELO prior, which is
    // invisible in the UI and changes what the estimator returns.
    let nationality = tokens.pop()?.to_uppercase();
    let games: u32 = tokens.pop()?.parse().ok()?;
    let rating: u32 = tokens.pop()?.parse().ok()?;

    // Whatever grades remain sit at the end of the given-name region; strip
    // them, keeping the first (leftmost) one.
    let grade = strip_trailing_grades(&mut tokens);
    let first_name = tokens.join(" ");

    Some(RatedPlayer {
        last_name,
        first_name,
        rating,
        games,
        nationality,
        grade,
    })
}

/// Remove the leading rank number and return the text starting at the name.
///
/// Returns `None` for lines that don't begin with a rank (blank lines, footers).
fn strip_rank(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let digits_end = trimmed.find(|c: char| !c.is_ascii_digit())?;
    if digits_end == 0 {
        return None; // no leading rank number
    }
    let rest = &trimmed[digits_end..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some(rest.trim_start())
}

/// Strip up to two trailing grade tokens (`N Dan` / `N Kyu`, case-insensitive),
/// returning the *first* (leftmost) one parsed, if any. The loop strips
/// right-to-left, so the second iteration — if it matches — parses the
/// leftmost pair and overwrites whatever the first iteration found, which is
/// exactly the "use the first rank when two are given" rule.
fn strip_trailing_grades(tokens: &mut Vec<&str>) -> Option<Grade> {
    fn is_grade_unit(word: &str) -> bool {
        word.eq_ignore_ascii_case("dan") || word.eq_ignore_ascii_case("kyu")
    }
    fn parse_grade(level: &str, unit: &str) -> Option<Grade> {
        let level: u32 = level.parse().ok()?;
        if unit.eq_ignore_ascii_case("dan") {
            Some(Grade::dan(level))
        } else {
            Some(Grade::kyu(level))
        }
    }

    let mut leftmost = None;
    for _ in 0..2 {
        let n = tokens.len();
        if n >= 2
            && is_grade_unit(tokens[n - 1])
            && !tokens[n - 2].is_empty()
            && tokens[n - 2].chars().all(|c| c.is_ascii_digit())
        {
            leftmost = parse_grade(tokens[n - 2], tokens[n - 1]);
            tokens.truncate(n - 2);
        } else {
            break;
        }
    }
    leftmost
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pad `name` to a fixed last-name column `width` (by character count).
    fn pad_last(name: &str, width: usize) -> String {
        let mut out = name.to_string();
        while out.chars().count() < width {
            out.push(' ');
        }
        out
    }

    /// Build a data row: right-justified rank, one space, fixed last-name
    /// column ([`DEFAULT_LAST_NAME_WIDTH`] wide), then the free-form remainder
    /// (single-spaced is fine — the parser splits on whitespace past the
    /// last-name column).
    fn row(rank: u32, last: &str, rest: &str) -> String {
        format!(
            "{rank:>3} {}{rest}",
            pad_last(last, DEFAULT_LAST_NAME_WIDTH)
        )
    }

    fn parse_one(line: &str) -> RatedPlayer {
        let width = detect_last_name_width(std::iter::once(line));
        match parse_row(line, width) {
            Row::Player(p) => p,
            Row::Malformed => panic!("row should parse"),
            Row::Other => panic!("row should be recognized as a player row"),
        }
    }

    #[test]
    fn decode_latin1_maps_high_bytes() {
        // 0xC5 = 'Å', 0xE9 = 'é', 0xF6 = 'ö'
        assert_eq!(decode_latin1(&[0x52, 0xF6, 0x64]), "Röd");
        assert_eq!(decode_latin1(&[0xC5, 0x67]), "Åg");
    }

    #[test]
    fn two_grades() {
        // Distinct grades so the assertion actually proves which one is kept.
        let p = parse_one(&row(1, "Kobayashi", "Taichi 5 Dan 4 Dan 2556 55 JP"));
        assert_eq!(p.last_name, "Kobayashi");
        assert_eq!(p.first_name, "Taichi");
        assert_eq!(p.rating, 2556);
        assert_eq!(p.games, 55);
        assert_eq!(p.nationality, "JP");
        assert_eq!(p.grade, Some(Grade::dan(5))); // the first (leftmost) of the two
    }

    #[test]
    fn parses_the_number_of_games() {
        // A provisional rating (few games) vs an established one.
        assert_eq!(parse_one(&row(3, "Takita", "Hirotaka 2462 7 JP")).games, 7);
        assert_eq!(
            parse_one(&row(1000, "Tkachenko", "Vladimir 17 Kyu 326 18 BY")).games,
            18
        );
    }

    #[test]
    fn one_grade_and_zero_grades() {
        let one = parse_one(&row(2, "Tanyan", "Vincent 5 Dan 2469 1224 BY"));
        assert_eq!(
            (
                one.first_name.as_str(),
                one.rating,
                one.nationality.as_str()
            ),
            ("Vincent", 2469, "BY")
        );
        assert_eq!(one.grade, Some(Grade::dan(5)));

        let none = parse_one(&row(3, "Takita", "Hirotaka 2462 7 JP"));
        assert_eq!((none.first_name.as_str(), none.rating), ("Hirotaka", 2462));
        assert_eq!(none.grade, None);
    }

    #[test]
    fn multi_word_last_name_with_accents() {
        let p = parse_one(&row(103, "Ågren Thuné", "Anders 2 Dan 1877 103 SE"));
        assert_eq!(p.last_name, "Ågren Thuné");
        assert_eq!(p.first_name, "Anders");
        assert_eq!(p.nationality, "SE");
    }

    #[test]
    fn last_name_fills_field_no_separating_space() {
        // "Imamura-Cornuejols" is exactly DEFAULT_LAST_NAME_WIDTH chars and abuts
        // "Toru", with no gap to reveal the column boundary on its own — so this
        // row is parsed alongside a normal one, whose gap lets detection find the
        // width (the exact-fill row is the minority the mode is robust to).
        let text = format!(
            "Elo list for 2026-06-01\n\n{header}\n{a}\n{b}\n",
            header = "    Name                               Grades         Elo #games Nationality",
            a = row(50, "Imamura-Cornuejols", "Toru 3 Dan 2100 20 JP"),
            b = row(3, "Takita", "Hirotaka 2462 7 JP"),
        );
        let players = parse_rating_list(&text).expect("every row parses");
        let p = players
            .iter()
            .find(|p| p.last_name == "Imamura-Cornuejols")
            .expect("row should parse");
        assert_eq!(p.first_name, "Toru");
    }

    #[test]
    fn multi_word_first_name() {
        let p = parse_one(&row(60, "Smith", "Kristian Leonard 1 Dan 1500 5 GB"));
        assert_eq!(p.first_name, "Kristian Leonard");
    }

    #[test]
    fn four_digit_rank_still_aligns() {
        let p = parse_one(&row(1000, "Tkachenko", "Vladimir 17 Kyu 326 18 BY"));
        assert_eq!(p.last_name, "Tkachenko");
        assert_eq!(p.first_name, "Vladimir");
        assert_eq!(p.rating, 326);
    }

    #[test]
    fn grade_matching_is_case_insensitive() {
        let p = parse_one(&row(70, "Foo", "Grigoriy 20 kyu 300 5 UA"));
        assert_eq!(p.first_name, "Grigoriy");
        assert_eq!(p.rating, 300);
        assert_eq!(p.grade, Some(Grade::kyu(20)));
    }

    #[test]
    fn a_narrower_export_is_detected_and_parses() {
        // A hypothetical export with a 12-char last-name column, narrower than
        // the default 18 — the width must be detected per file, not hardcoded.
        let width = 12;
        let a = format!(
            "{:>3} {}{}",
            1,
            pad_last("Kobayashi", width),
            "Taichi 5 Dan 2556 55 JP"
        );
        let b = format!(
            "{:>3} {}{}",
            3,
            pad_last("Takita", width),
            "Hirotaka 2462 7 JP"
        );
        let text = format!(
            "Elo list for 2026-06-01\n\
             \n\
             {header}\n\
             {a}\n\
             {b}\n",
            header = "    Name             Grades         Elo #games Nationality",
        );
        let players = parse_rating_list(&text).expect("every row parses");
        assert_eq!(players.len(), 2);
        assert_eq!(players[0].last_name, "Kobayashi");
        assert_eq!(players[0].first_name, "Taichi");
        assert_eq!(players[1].last_name, "Takita");
        assert_eq!(players[1].first_name, "Hirotaka");
    }

    #[test]
    fn a_ranked_row_that_doesnt_parse_fails_the_whole_list() {
        // The row carries a rank, so it is a player row — its #games column has
        // just gone missing. Skipping it would drop the player from autocomplete
        // (or, worse, keep them with 0 games and a provisional prior) with the
        // list still looking perfectly healthy.
        let text = format!(
            "Elo list for 2026-06-01\n\n{header}\n{a}\n{b}\n",
            header = "    Name                               Grades         Elo #games Nationality",
            a = row(1, "Kobayashi", "Taichi 5 Dan 2556 55 JP"),
            b = row(2, "Takita", "Hirotaka 2462 JP"),
        );
        let err = parse_rating_list(&text).expect_err("the second row is malformed");
        assert_eq!(err.lines, vec![5]);
        assert!(err.to_string().contains("format may have changed"));
    }

    #[test]
    fn a_non_numeric_games_column_is_malformed_not_zero() {
        // `unwrap_or(0)` here would silently demote an established player to
        // provisional and widen their ELO prior.
        let text = format!(
            "Elo list for 2026-06-01\n\n{header}\n{a}\n",
            header = "    Name                               Grades         Elo #games Nationality",
            a = row(1, "Kobayashi", "Taichi 5 Dan 2556 n/a JP"),
        );
        assert!(parse_rating_list(&text).is_err());
    }

    #[test]
    fn lookup_reports_homonyms_instead_of_picking_one() {
        // Published lists really do carry these (two in the 2024-01-01 list).
        let entry = |last: &str, first: &str, rating: u32| RatedPlayer {
            last_name: last.to_string(),
            first_name: first.to_string(),
            rating,
            games: 30,
            nationality: "FR".to_string(),
            grade: None,
        };
        let list = [
            entry("Popovich", "Alexander", 1700),
            entry("Takita", "Hirotaka", 2462),
            entry("Popovich", "Alexander", 2100),
        ];
        assert_eq!(lookup(&list, "Popovich", "Alexander"), Lookup::Ambiguous(2));
        assert_eq!(lookup(&list, "Nobody", "Here"), Lookup::None);
        // Matching is accent- and case-folded, like every other name match here.
        match lookup(&list, "takita", "Hirotaka") {
            Lookup::One(p) => assert_eq!(p.rating, 2462),
            other => panic!("expected exactly one match, got {other:?}"),
        }
    }

    #[test]
    fn header_and_blank_lines_are_skipped() {
        let text = format!(
            "Elo list for 2026-06-01\n\
             \n\
             {header}\n\
             {a}\n\
             \n\
             {b}\n",
            header = "    Name                               Grades         Elo #games Nationality",
            a = row(1, "Kobayashi", "Taichi 5 Dan 5 Dan 2556 55 JP"),
            b = row(2, "Takita", "Hirotaka 2462 7 JP"),
        );
        let players = parse_rating_list(&text).expect("every row parses");
        assert_eq!(players.len(), 2);
        assert_eq!(players[0].last_name, "Kobayashi");
        assert_eq!(players[1].last_name, "Takita");
    }
}
