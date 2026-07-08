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
    /// rating is (the ELO estimator widens the prior for provisional ratings);
    /// 0 when the column was missing or unparseable.
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

/// Decode ISO-8859-1 (Latin-1) bytes to a `String`.
///
/// The FESA file is Latin-1, so it is not valid UTF-8 and can't be read as a
/// `String` directly. Each byte maps to the identically-numbered Unicode code
/// point, which is exactly Latin-1 — and keeps 1 byte = 1 character, so the
/// fixed-width column offsets stay correct.
pub fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Parse the (already Latin-1 decoded) rating list into player entries.
///
/// Malformed rows are skipped rather than failing the whole parse, so a future
/// format tweak degrades gracefully instead of producing garbage.
pub fn parse_rating_list(text: &str) -> Vec<RatedPlayer> {
    let data_lines: Vec<&str> = text.lines().skip(HEADER_LINES).collect();
    // The last-name column width varies between exports, so measure it from the
    // rows before parsing them.
    let width = detect_last_name_width(data_lines.iter().copied());
    data_lines
        .into_iter()
        .filter_map(|line| parse_row(line, width))
        .collect()
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

/// Parse a single data row, or `None` if it doesn't look like a player row.
fn parse_row(line: &str, width: usize) -> Option<RatedPlayer> {
    // Drop the leading rank ("  1 ", "1000 ", …) and anchor to the name start;
    // this absorbs the 4-digit rank column shift.
    let after_rank = strip_rank(line)?;

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
    let nationality = tokens.pop()?.to_uppercase();
    // #games must be present, but tolerate a non-numeric value (→ 0) rather than
    // dropping the whole row, matching the parser's graceful-degradation policy.
    let games: u32 = tokens.pop()?.parse().unwrap_or(0);
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
        parse_row(line, width).expect("row should parse")
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
        let players = parse_rating_list(&text);
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
        let players = parse_rating_list(&text);
        assert_eq!(players.len(), 2);
        assert_eq!(players[0].last_name, "Kobayashi");
        assert_eq!(players[0].first_name, "Taichi");
        assert_eq!(players[1].last_name, "Takita");
        assert_eq!(players[1].first_name, "Hirotaka");
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
        let players = parse_rating_list(&text);
        assert_eq!(players.len(), 2);
        assert_eq!(players[0].last_name, "Kobayashi");
        assert_eq!(players[1].last_name, "Takita");
    }
}
