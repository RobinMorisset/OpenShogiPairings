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
//! - **Fixed width, not delimited.** The last-name field is [`LAST_NAME_WIDTH`]
//!   characters; the family name can contain spaces (`Ågren Thuné`) or fill the
//!   field with no trailing space (`Imamura-Cornuejols` abutting `Toru`), so
//!   whitespace-splitting cannot recover it — we slice the fixed column.
//! - **Latin-1 encoded**, not UTF-8 (`Rövekamp`), so callers decode with
//!   [`decode_latin1`] first (1 byte = 1 char = 1 column).
//! - **Variable grades** (0, 1 or 2 of `N Dan` / `N Kyu`) sit between the given
//!   name and the Elo; we ignore them by parsing the trailing Elo / #games /
//!   nationality tokens from the right and stripping any grade tokens.
//! - The rank field **overflows at 4 digits**, shifting later columns, so we
//!   anchor to the name start (after removing the rank) rather than to absolute
//!   positions.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Width, in characters, of the last-name column (after the rank).
const LAST_NAME_WIDTH: usize = 18;

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
    text.lines()
        .skip(HEADER_LINES)
        .filter_map(parse_row)
        .collect()
}

/// Parse a single data row, or `None` if it doesn't look like a player row.
fn parse_row(line: &str) -> Option<RatedPlayer> {
    // Drop the leading rank ("  1 ", "1000 ", …) and anchor to the name start;
    // this absorbs the 4-digit rank column shift.
    let after_rank = strip_rank(line)?;

    let chars: Vec<char> = after_rank.chars().collect();
    if chars.len() <= LAST_NAME_WIDTH {
        return None;
    }

    // Last name is the fixed-width column; the family name may contain spaces or
    // fill the field, so it can only be read positionally.
    let last_name = chars[..LAST_NAME_WIDTH]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    if last_name.is_empty() {
        return None;
    }

    // Everything after the last-name column: first name + grades + trailing
    // Elo / #games / nationality.
    let rest: String = chars[LAST_NAME_WIDTH..].iter().collect();
    let mut tokens: Vec<&str> = rest.split_whitespace().collect();

    // The last three tokens are always nationality, #games, Elo (right to left).
    let nationality = tokens.pop()?.to_uppercase();
    // #games must be present, but tolerate a non-numeric value (→ 0) rather than
    // dropping the whole row, matching the parser's graceful-degradation policy.
    let games: u32 = tokens.pop()?.parse().unwrap_or(0);
    let rating: u32 = tokens.pop()?.parse().ok()?;

    // Whatever grades remain sit at the end of the given-name region; drop them.
    strip_trailing_grades(&mut tokens);
    let first_name = tokens.join(" ");

    Some(RatedPlayer {
        last_name,
        first_name,
        rating,
        games,
        nationality,
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

/// Strip up to two trailing grade tokens (`N Dan` / `N Kyu`, case-insensitive).
fn strip_trailing_grades(tokens: &mut Vec<&str>) {
    fn is_grade_unit(word: &str) -> bool {
        word.eq_ignore_ascii_case("dan") || word.eq_ignore_ascii_case("kyu")
    }
    for _ in 0..2 {
        let n = tokens.len();
        if n >= 2
            && is_grade_unit(tokens[n - 1])
            && !tokens[n - 2].is_empty()
            && tokens[n - 2].chars().all(|c| c.is_ascii_digit())
        {
            tokens.truncate(n - 2);
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pad `name` to the fixed last-name column width (by character count).
    fn pad_last(name: &str) -> String {
        let mut out = name.to_string();
        while out.chars().count() < LAST_NAME_WIDTH {
            out.push(' ');
        }
        out
    }

    /// Build a data row: right-justified rank, one space, fixed last-name
    /// column, then the free-form remainder (single-spaced is fine — the parser
    /// splits on whitespace past the last-name column).
    fn row(rank: u32, last: &str, rest: &str) -> String {
        format!("{rank:>3} {}{rest}", pad_last(last))
    }

    fn parse_one(line: &str) -> RatedPlayer {
        parse_row(line).expect("row should parse")
    }

    #[test]
    fn decode_latin1_maps_high_bytes() {
        // 0xC5 = 'Å', 0xE9 = 'é', 0xF6 = 'ö'
        assert_eq!(decode_latin1(&[0x52, 0xF6, 0x64]), "Röd");
        assert_eq!(decode_latin1(&[0xC5, 0x67]), "Åg");
    }

    #[test]
    fn two_grades() {
        let p = parse_one(&row(1, "Kobayashi", "Taichi 5 Dan 5 Dan 2556 55 JP"));
        assert_eq!(p.last_name, "Kobayashi");
        assert_eq!(p.first_name, "Taichi");
        assert_eq!(p.rating, 2556);
        assert_eq!(p.games, 55);
        assert_eq!(p.nationality, "JP");
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

        let none = parse_one(&row(3, "Takita", "Hirotaka 2462 7 JP"));
        assert_eq!((none.first_name.as_str(), none.rating), ("Hirotaka", 2462));
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
        // "Imamura-Cornuejols" is exactly LAST_NAME_WIDTH chars and abuts "Toru".
        let p = parse_one(&row(50, "Imamura-Cornuejols", "Toru 3 Dan 2100 20 JP"));
        assert_eq!(p.last_name, "Imamura-Cornuejols");
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
