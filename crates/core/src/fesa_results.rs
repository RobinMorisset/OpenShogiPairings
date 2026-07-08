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
//!   ELO cell carries a `*` suffix and *is* their assigned post-tournament rating.
//!   A rated player's strength is `ELO + (+/-)`.
//! - Columns are only *mostly* fixed-width: a 3-digit player number or a 3-digit
//!   opponent in a round cell shifts everything to its right. So only the last
//!   name is read positionally (a fixed 18-wide field); everything after it is
//!   tokenised and classified by shape, which is drift-proof.

use std::collections::HashMap;

use uuid::Uuid;

use crate::grid_import::{build_tournament, parse_cell, Cell, GridImportError, RawRow};
use crate::tournament::Tournament;

/// Width of the fixed last-name column (matches the FESA rating list).
const LAST_NAME_WIDTH: usize = 18;

/// Parse a FESA result table, returning the replayed tournament and each player's
/// post-tournament strength (`ELO + (+/-)`, or the `*` rating for a pre-unrated
/// player) keyed by player id.
pub fn import_fesa_results(
    text: &str,
) -> Result<(Tournament, HashMap<Uuid, f64>), GridImportError> {
    let mut title: Option<String> = None;
    let mut rows: Vec<RawRow> = Vec::new();
    let mut strength_by_number: HashMap<u32, f64> = HashMap::new();

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if is_data_row(line) {
            let (row, strength) = parse_results_row(line, i + 1)?;
            strength_by_number.insert(row.number, strength);
            rows.push(row);
        } else if title.is_none() {
            // The first non-empty, non-data line is the title; the column header
            // and the trailing "Promoting …" lines are non-data and ignored.
            title = Some(line.trim().to_string());
        }
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

/// Parse one player row into a [`RawRow`] and the player's strength.
fn parse_results_row(line: &str, line_no: usize) -> Result<(RawRow, f64), GridImportError> {
    let bad = |reason: &str| GridImportError::BadRow {
        line: line_no,
        reason: reason.to_string(),
    };
    let chars: Vec<char> = line.chars().collect();

    // Player number, then skip to the name column.
    let mut i = 0;
    while i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    let num_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    let number: u32 = chars[num_start..i]
        .iter()
        .collect::<String>()
        .parse()
        .map_err(|_| bad("missing player number"))?;
    while i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    let name_start = i;
    if name_start + LAST_NAME_WIDTH >= chars.len() {
        return Err(bad("row too short for the name column"));
    }

    // Last name is the only positional field (its column is left of any drift);
    // everything after it is tokenised.
    let last_name = chars[name_start..name_start + LAST_NAME_WIDTH]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    if last_name.is_empty() {
        return Err(bad("empty last name"));
    }
    let remainder: String = chars[name_start + LAST_NAME_WIDTH..].iter().collect();
    let tokens: Vec<&str> = remainder.split_whitespace().collect();

    // ELO is the first bare 3-4 digit number (optionally `*`): it sits before the
    // round cells (which always end in a sign) and is wider than Pts (1-2 digits).
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

    // After the ELO: round cells, then Pts, then an optional `+/-` delta. Cells end
    // in a sign; Pts and the delta don't, so the first non-cell ends the cells.
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

    // trailing = [Pts, +/-?]. A rated player's +/- feeds the strength; an unrated
    // player has none (the `*` ELO is already their post-tournament rating).
    let delta: i64 = if unrated {
        0
    } else {
        match trailing.get(1) {
            Some(d) => d.parse().map_err(|_| bad("unparseable +/- delta"))?,
            None => 0,
        }
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

/// A 3-4 digit number, optionally with a trailing `*` (pre-unrated marker).
fn is_elo_token(t: &str) -> bool {
    let core = t.strip_suffix('*').unwrap_or(t);
    (3..=4).contains(&core.len()) && core.chars().all(|c| c.is_ascii_digit())
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

    fn wosc() -> (Tournament, HashMap<Uuid, f64>) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/results_WOSC_2024.txt"
        );
        let bytes = std::fs::read(path).expect("fixture present");
        import_fesa_results(&decode_latin1(&bytes)).expect("parses")
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
}
