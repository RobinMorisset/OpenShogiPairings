//! Import a FESA post-tournament **result table** into a finished [`Tournament`].
//!
//! Tournaments usually publish the table FESA generates after the event rather
//! than the raw American Grid. It carries the same cross-table (so the rounds can
//! be replayed exactly like [`crate::import_american_grid`]) plus, per player, the
//! pre-tournament ELO used for pairing and the points gained/lost — which makes it
//! a precise post-tournament **strength oracle** for simulations.
//!
//! Format (see `crates/core/tests/fixtures/results_WOSC_2024.txt`): a title line,
//! a column header, then one fixed-width row per player —
//! `Nr  Last  First  Nat  Grade  ELO  <round cells…>  Pts  +/-` — and finally a
//! "Promoting …" section that is ignored. Two wrinkles the parser handles:
//!
//! - A player **unrated** before the tournament has no grade and no `+/-`; their
//!   ELO cell carries a `*` suffix and *is* their assigned post-tournament rating
//!   (which can be as low as `1`). A rated player's strength is `ELO + (+/-)`.
//! - Columns are only *mostly* fixed-width: a 3-digit player number or a 3-digit
//!   opponent in a round cell shifts everything to its right. So only the last
//!   name is read positionally — and its column width, which differs between
//!   exports, is *detected* per file rather than hardcoded — while everything to
//!   its right is tokenised and classified by shape, which is drift-proof (and
//!   copes with an optional `MMS` column some tables carry before `Pts`).

use std::collections::HashMap;

use uuid::Uuid;

use crate::grid_import::{build_tournament, parse_cell, Cell, GridImportError, RawRow};
use crate::tournament::Tournament;

/// Fallback last-name column width if detection finds nothing (e.g. an empty
/// table); the real width is [`detect_last_name_width`].
const DEFAULT_LAST_NAME_WIDTH: usize = 18;

/// Parse a FESA result table, returning the replayed tournament and each player's
/// post-tournament strength (`ELO + (+/-)`, or the `*` rating for a pre-unrated
/// player) keyed by player id.
pub fn import_fesa_results(
    text: &str,
) -> Result<(Tournament, HashMap<Uuid, f64>), GridImportError> {
    let mut title: Option<String> = None;
    let mut data: Vec<(usize, &str)> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if is_data_row(line) {
            data.push((i + 1, line));
        } else if title.is_none() {
            // The first non-empty, non-data line is the title; the column header
            // and the trailing "Promoting …" lines are non-data and ignored.
            title = Some(line.trim().to_string());
        }
    }

    // The last-name column width varies between exports, so measure it from the
    // rows before parsing them.
    let width = detect_last_name_width(data.iter().map(|(_, l)| *l));

    let mut rows: Vec<RawRow> = Vec::new();
    let mut strength_by_number: HashMap<u32, f64> = HashMap::new();
    for (line_no, line) in data {
        let (row, strength) = parse_results_row(line, line_no, width)?;
        strength_by_number.insert(row.number, strength);
        rows.push(row);
    }

    demote_extra_byes(&mut rows);

    let (tournament, id_of) = build_tournament(title.as_deref(), rows)?;
    let strengths = strength_by_number
        .into_iter()
        .filter_map(|(number, s)| id_of.get(&number).map(|&id| (id, s)))
        .collect();
    Ok((tournament, strengths))
}

/// The tournament model allows at most one bye per round, but a FESA table can
/// show several `0+` in a round (a real bye plus forfeit wins against no-shows).
/// Keep the first as the round's bye and demote the rest to absences. Those few
/// players lose that `+1` in the *reconstructed* standings — a rare, tail-only
/// rounding in the observed data; the real games (and the mismatch metric) are
/// unaffected, and the simulation ignores these rounds entirely.
fn demote_extra_byes(rows: &mut [RawRow]) {
    let round_count = rows.first().map_or(0, |r| r.cells.len());
    for r in 0..round_count {
        let mut seen_bye = false;
        for row in rows.iter_mut() {
            if matches!(row.cells.get(r), Some(Cell::Bye)) {
                if seen_bye {
                    row.cells[r] = Cell::Absent;
                } else {
                    seen_bye = true;
                }
            }
        }
    }
}

/// A data row starts (after leading spaces) with a run of digits then a space —
/// the player number. The title, column header and "Promoting …" lines don't.
fn is_data_row(line: &str) -> bool {
    let mut seen_digit = false;
    for c in line.trim_start().chars() {
        if c.is_ascii_digit() {
            seen_digit = true;
        } else {
            return seen_digit && c == ' ';
        }
    }
    false
}

/// The player number and the index where the name column starts (past the number
/// and its trailing spaces), or `None` if the line has no leading number.
fn split_number(chars: &[char]) -> Option<(u32, usize)> {
    let mut i = 0;
    while i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    let start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    let number = chars[start..i].iter().collect::<String>().parse().ok()?;
    while i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    Some((number, i))
}

/// The last-name column width (chars from the name start to the first name),
/// measured as the most common first-2-space-gap position across the rows. The
/// width varies between exports, so it must be measured, not hardcoded; rows
/// whose last name doesn't fill the column reveal the boundary, and the mode is
/// robust to the exact-fill minority.
fn detect_last_name_width<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        let Some((_, name_start)) = split_number(&chars) else {
            continue;
        };
        // First run of >= 2 spaces after the name start (single spaces are within
        // a multi-word last name), then the first non-space: the first name.
        let mut j = name_start;
        while j + 1 < chars.len() && !(chars[j] == ' ' && chars[j + 1] == ' ') {
            j += 1;
        }
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        if j < chars.len() && j > name_start {
            *counts.entry(j - name_start).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map_or(DEFAULT_LAST_NAME_WIDTH, |(w, _)| w)
}

/// Parse one player row into a [`RawRow`] and the player's strength, given the
/// detected last-name column `width`.
fn parse_results_row(
    line: &str,
    line_no: usize,
    width: usize,
) -> Result<(RawRow, f64), GridImportError> {
    let bad = |reason: &str| GridImportError::BadRow {
        line: line_no,
        reason: reason.to_string(),
    };
    let chars: Vec<char> = line.chars().collect();
    let (number, name_start) = split_number(&chars).ok_or_else(|| bad("missing player number"))?;
    if name_start + width > chars.len() {
        return Err(bad("row too short for the name column"));
    }

    // Last name is the only positional field (its column is left of any drift);
    // everything after it is tokenised, which is drift-proof.
    let last_name = chars[name_start..name_start + width]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    if last_name.is_empty() {
        return Err(bad("empty last name"));
    }
    let remainder: String = chars[name_start + width..].iter().collect();
    let tokens: Vec<&str> = remainder.split_whitespace().collect();

    // ELO is the first 3-4 digit number, or any digits with a `*` (an unrated
    // player's assigned rating, which can be a single digit). It precedes the
    // round cells (which end in a sign) and is distinct from Pts/MMS (1-2 digits).
    let elo_idx = tokens
        .iter()
        .position(|t| is_elo_token(t))
        .ok_or_else(|| bad("no ELO column found"))?;
    let elo_tok = tokens[elo_idx];
    let unrated = elo_tok.ends_with('*');
    let elo: u32 = elo_tok
        .trim_end_matches('*')
        .parse()
        .map_err(|_| bad("unparseable ELO"))?;

    // Before the ELO: first-name words, then Nat, then an optional grade. Peel the
    // grade and Nat off the right; the remainder is the first name.
    let mut pre = tokens[..elo_idx].to_vec();
    if pre.len() >= 2
        && (pre[pre.len() - 1].eq_ignore_ascii_case("dan")
            || pre[pre.len() - 1].eq_ignore_ascii_case("kyu"))
        && pre[pre.len() - 2].chars().all(|c| c.is_ascii_digit())
    {
        pre.truncate(pre.len() - 2);
    }
    let nationality = pre.pop().map(str::to_string);
    let first_name = pre.join(" ");

    // After the ELO: round cells (each ends in a sign), then Pts (and, in some
    // tables, an MMS column), then an optional `+/-` delta. The first non-cell
    // ends the cells; the delta is the trailing *signed* token, so an MMS column
    // between the cells and Pts doesn't fool it.
    let post = &tokens[elo_idx + 1..];
    let mut cells = Vec::new();
    let mut trailing = Vec::new();
    for &t in post {
        if trailing.is_empty() && is_cell_token(t) {
            cells.push(parse_cell(t, line_no)?);
        } else {
            trailing.push(t);
        }
    }
    if cells.is_empty() {
        return Err(bad("no round cells"));
    }

    // A rated player's +/- feeds the strength; an unrated player has none (the `*`
    // ELO is already their post-tournament rating).
    let delta: i64 = match trailing.last() {
        Some(t) if !unrated && is_delta_token(t) => {
            t.parse().map_err(|_| bad("unparseable +/- delta"))?
        }
        _ => 0,
    };

    let strength = f64::from(elo) + delta as f64;
    let rating = if unrated { None } else { Some(elo) };

    Ok((
        RawRow {
            number,
            last_name,
            first_name,
            nationality,
            rating,
            cells,
        },
        strength,
    ))
}

/// A rating token: a 3-4 digit number, or any digits with a trailing `*` (the
/// pre-unrated marker — an assigned rating that can be a single digit).
fn is_elo_token(t: &str) -> bool {
    match t.strip_suffix('*') {
        Some(core) => !core.is_empty() && core.chars().all(|c| c.is_ascii_digit()),
        None => (3..=4).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit()),
    }
}

/// A signed integer like `+15` or `-6` — the `+/-` delta column (sign leads,
/// unlike a round cell where the sign trails).
fn is_delta_token(t: &str) -> bool {
    matches!(t.as_bytes().first(), Some(b'+' | b'-'))
        && t.len() > 1
        && t[1..].chars().all(|c| c.is_ascii_digit())
}

/// A round cell: digits then a result sign (`+`, `-`, `=`, `#`), tolerating a
/// trailing floater mark. Distinguishes cells from Pts (no sign) and the signed
/// `+/-` delta (sign at the front, not the back).
fn is_cell_token(t: &str) -> bool {
    let t = t.trim_end_matches(['^', 'v']);
    match t.chars().last() {
        Some('+' | '-' | '=' | '#') => {}
        _ => return false,
    }
    let digits = &t[..t.len() - 1];
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_latin1;

    fn load(fixture: &str) -> (Tournament, HashMap<Uuid, f64>) {
        let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).expect("fixture present");
        import_fesa_results(&decode_latin1(&bytes)).expect("parses")
    }

    fn wosc() -> (Tournament, HashMap<Uuid, f64>) {
        load("results_WOSC_2024.txt")
    }

    fn find<'a>(t: &'a Tournament, last: &str, first: &str) -> &'a crate::Player {
        t.players
            .iter()
            .find(|p| p.last_name == last && p.first_name == first)
            .unwrap_or_else(|| panic!("player {last} {first} not found"))
    }

    #[test]
    fn imports_players_rounds_and_the_title() {
        let (t, _) = wosc();
        assert_eq!(t.players.len(), 107);
        assert_eq!(t.rounds.len(), 9);
        assert!(t.rounds.iter().all(|r| r.completed));
        assert!(t.name.starts_with("World Open Shogi Championship"));
    }

    #[test]
    fn rated_strength_is_elo_plus_delta_and_pairing_uses_the_pre_elo() {
        let (t, strengths) = wosc();
        // Kobayashi Taichi: pre-ELO 2567 (for pairing) + 15 = 2582 strength.
        let kob = find(&t, "Kobayashi", "Taichi");
        assert_eq!(kob.rating, Some(2567));
        assert_eq!(strengths[&kob.id], 2582.0);
        // A negative delta: Leiter 2326 - 27 = 2299.
        let leiter = find(&t, "Leiter", "Thomas");
        assert_eq!(strengths[&leiter.id], 2299.0);
    }

    #[test]
    fn pre_unrated_players_have_no_pairing_rating_and_the_star_is_their_strength() {
        let (t, strengths) = wosc();
        // Hayakawa Akio: "2337*" — unrated before, no +/-. Pairing rating None,
        // strength = 2337.
        let haya = find(&t, "Hayakawa", "Akio");
        assert_eq!(haya.rating, None);
        assert_eq!(strengths[&haya.id], 2337.0);
    }

    #[test]
    fn multiword_names_and_offset_rows_parse() {
        let (t, strengths) = wosc();
        // Multi-word last name.
        let vdl = find(&t, "van der Lubbe", "Lex");
        assert_eq!(vdl.rating, Some(1929));
        assert_eq!(strengths[&vdl.id], 1916.0); // 1929 - 13
                                                // Multi-word first name.
        let nguyen = find(&t, "Nguyen", "Anh Tuan");
        assert_eq!(strengths[&nguyen.id], 1861.0); // 1881 - 20
                                                   // An 18-char last name splits from its first name.
        find(&t, "Fernandez Nogueira", "Anna");
        // A 3-digit-number (offset) row still parses.
        let ozal = find(&t, "\u{d6}zal", "Berke"); // "Özal", row 100
        assert_eq!(strengths[&ozal.id], 1496.0); // 1499 - 3
    }

    #[test]
    fn cdf_files_with_a_narrower_name_column_and_an_mms_column_parse() {
        // CdF 2024: last-name column is narrower than WOSC's, and there is an MMS
        // column between the rounds and Pts — the delta must still be the trailing
        // signed token, not the column after the rounds.
        let (t, strengths) = load("results_CdF_2024.txt");
        assert_eq!(t.rounds.len(), 6);
        assert!(t.rounds.iter().all(|r| r.completed));
        let nguyen = find(&t, "Nguyen", "Anh Tuan");
        assert_eq!(nguyen.rating, Some(1862));
        assert_eq!(strengths[&nguyen.id], 1869.0); // 1862 + 7 (delta past the MMS "1")

        // A pre-unrated player with a single-digit `*` rating ("1*").
        let anais = find(&t, "Massis", "Ana\u{ef}s"); // "Anaïs"
        assert_eq!(anais.rating, None);
        assert_eq!(strengths[&anais.id], 1.0);
    }

    #[test]
    fn cdf_2025_parses_with_its_own_detected_width() {
        let (t, strengths) = load("results_CdF_2025.txt");
        assert_eq!(t.rounds.len(), 6);
        let pucher = find(&t, "Pucher", "Olivier");
        assert_eq!(strengths[&pucher.id], 1873.0); // 1887 - 14
    }
}
