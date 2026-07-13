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
use crate::player::Grade;
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
    // Strip FESA round-cell annotations — parenthetical forfeit/bye/handicap
    // marks like `(-r )`, `(+b )`, `(-rl)`, `(-4p)` that trail a cell (e.g.
    // `30+(-r )`). They carry no cross-table information (the cell's own sign
    // already records the result), and some contain an internal space, which
    // would otherwise split one cell across two whitespace tokens.
    let remainder: String = chars[name_start + width..].iter().collect();
    let remainder = strip_cell_annotations(&remainder);
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
    // grade (kept as the player's rank) and Nat off the right; the remainder is the
    // first name.
    let mut pre = tokens[..elo_idx].to_vec();
    let mut grade = None;
    if pre.len() >= 2 {
        if let Some(g) = parse_grade(pre[pre.len() - 2], pre[pre.len() - 1]) {
            grade = Some(g);
            pre.truncate(pre.len() - 2);
        }
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
            grade,
            cells,
        },
        strength,
    ))
}

/// Parse a grade column pair like `("4", "Dan")` or `("1", "Kyu")` into a
/// [`Grade`]; `None` if the unit isn't dan/kyu or the level isn't a number.
fn parse_grade(level: &str, unit: &str) -> Option<Grade> {
    let level: u32 = level.parse().ok()?;
    if unit.eq_ignore_ascii_case("dan") {
        Some(Grade::dan(level))
    } else if unit.eq_ignore_ascii_case("kyu") {
        Some(Grade::kyu(level))
    } else {
        None
    }
}

/// Remove parenthetical round-cell annotations (`(-r )`, `(+b )`, `(-rl)`,
/// `(-4p)`, …) from a row's remainder. Each spans from a `(` to the next `)`
/// inclusive; an unclosed `(` drops to the end of the string. Everything outside
/// the parentheses — cells, ELO, Pts, `+/-` — is preserved verbatim, so cells
/// still separate on their existing whitespace.
fn strip_cell_annotations(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth: u32 = 0;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
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

    #[test]
    fn recognizes_and_imports_a_no_show_cell() {
        // `is_cell_token` must treat `0#` as a round cell (not the Pts/delta
        // column), and it parses as a zero-score absence like `0-`.
        assert!(is_cell_token("0#"));
        assert_eq!(parse_cell("0#", 1).unwrap(), Cell::Absent);

        // End to end: a small table where Beta is a no-show in round 2. The
        // last-name column is positional, so build it with a fixed width.
        let mut text = String::from("No-Show Open : 2026\nNr Name Nat Grade ELO R1 R2 Pts +/-\n");
        let row = |nr: u32, last: &str, first: &str, c1: &str, c2: &str, pts: u32| {
            format!("{nr:>2} {last:<15}{first} FR 1 Dan 1500 {c1} {c2} {pts} +0\n")
        };
        text.push_str(&row(1, "Alpha", "Ann", "2+", "0+", 2)); // beats Beta, then a bye
        text.push_str(&row(2, "Beta", "Bob", "1-", "0#", 0)); // loses, then no-show
        text.push_str(&row(3, "Gamma", "Cid", "4+", "4+", 2));
        text.push_str(&row(4, "Delta", "Dan", "3-", "3-", 0));

        let (t, _) = import_fesa_results(&text).unwrap();
        assert_eq!(t.rounds.len(), 2);
        assert!(t.rounds.iter().all(|r| r.completed));
        // Beta's round-2 no-show landed them in the absent set and scored nothing.
        let beta = find(&t, "Beta", "Bob").id;
        assert!(t.rounds[1].absent.contains(&beta));
        let points = |id| {
            t.standings()
                .into_iter()
                .find(|s| s.player_id == id)
                .unwrap()
                .points
        };
        assert_eq!(points(beta), 0);
        assert_eq!(points(find(&t, "Alpha", "Ann").id), 2); // win + bye
    }

    #[test]
    fn strips_parenthetical_round_cell_annotations() {
        // Attached annotation removed, cell kept; internal space collapses so the
        // cell stays a single whitespace token.
        assert_eq!(strip_cell_annotations("30+(-r ) 32+"), "30+ 32+");
        assert_eq!(strip_cell_annotations("34+(-rl) 26+(+b )"), "34+ 26+");
        assert_eq!(strip_cell_annotations("48-(+4p)"), "48-");
        // No annotation: untouched.
        assert_eq!(strip_cell_annotations("14- 32+ 8-"), "14- 32+ 8-");
        // Unclosed paren drops to the end rather than panicking.
        assert_eq!(strip_cell_annotations("10+(-r"), "10+");
    }

    #[test]
    fn round_cell_annotations_do_not_break_the_round_count() {
        // A cross-table where one player's round-3 cell carries a forfeit
        // annotation with an internal space (`3+(-r )`). Without stripping, the
        // cell would split into `3+(-r` and `)`, truncating the row to 2 cells and
        // making the rows disagree on the round count.
        let mut text =
            String::from("Annotated Open : 2026\nNr Name Nat Grade ELO R1 R2 R3 Pts +/-\n");
        let row = |nr: u32, last: &str, first: &str, cells: &str, pts: u32| {
            format!("{nr:>2} {last:<15}{first} FR 1 Dan 1500 {cells} {pts} +0\n")
        };
        text.push_str(&row(1, "Alpha", "Ann", "2+      3+       4+", 3));
        text.push_str(&row(2, "Beta", "Bob", "1-      4+       3+(-r )", 2));
        text.push_str(&row(3, "Gamma", "Cid", "4+      1-       2-(+r )", 1));
        text.push_str(&row(4, "Delta", "Dan", "3-      2-       1-", 0));

        let (t, _) = import_fesa_results(&text).unwrap();
        assert_eq!(t.rounds.len(), 3);
        assert!(t.rounds.iter().all(|r| r.completed));
        // Beta beat Gamma in round 3 despite the annotation.
        let beta = find(&t, "Beta", "Bob").id;
        let gamma = find(&t, "Gamma", "Cid").id;
        let win = t.rounds[2]
            .boards
            .iter()
            .find(|b| {
                (b.player1 == beta && b.player2 == gamma)
                    || (b.player1 == gamma && b.player2 == beta)
            })
            .expect("Beta vs Gamma paired in round 3");
        assert!(win.result.is_some());
    }

    #[test]
    fn the_grade_column_is_parsed_into_a_rank() {
        use crate::Grade;
        let (t, _) = wosc();
        assert_eq!(find(&t, "Kobayashi", "Taichi").grade, Some(Grade::dan(4)));
        assert_eq!(find(&t, "Kamo", "Dan").grade, Some(Grade::kyu(1)));
        // A pre-unrated player has no grade column.
        assert_eq!(find(&t, "Hayakawa", "Akio").grade, None);
    }
}
