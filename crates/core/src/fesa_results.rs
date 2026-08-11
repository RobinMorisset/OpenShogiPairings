//! Import a FESA post-tournament **result table** into a finished [`Tournament`].
//!
//! Tournaments usually publish the table FESA generates after the event rather
//! than the raw American Grid. It carries the full cross-table (replayed by
//! [`crate::result_import::build_tournament`]) plus, per player, the
//! pre-tournament ELO used for pairing and the points gained/lost — which makes it
//! a precise post-tournament **strength oracle** for simulations.
//!
//! Format (see `crates/core/tests/fixtures/results_WOSC_2024.txt`): a title line,
//! a column header, then one fixed-width row per player —
//! `Nr  Last  First  Nat  Grade  ELO  <round cells…>  Pts  +/-` — and finally a
//! "Promoting …" section that is ignored. Three wrinkles the parser handles:
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
//! - A round cell can carry a parenthesised **handicap** (`30+(-6p)`, `16-(+R)`,
//!   sometimes with an internal space as in `(-r )`, which would otherwise split
//!   the cell into two tokens). It is kept glued to its cell and read as real
//!   odds — see [`normalize_cell_annotations`] and
//!   [`crate::result_import::parse_cell`]. A parenthetical that stands *alone*
//!   annotates no cell (an `(elder)` in a name, say) and is dropped.

use std::collections::HashMap;

use crate::player::Grade;
use crate::result_import::{build_tournament, parse_cell, RawRow, ResultImportError};
use crate::tournament::Tournament;
use crate::units::TournamentId;

/// Fallback last-name column width if detection finds nothing (e.g. an empty
/// table); the real width is [`detect_last_name_width`].
const DEFAULT_LAST_NAME_WIDTH: usize = 18;

/// Parse a FESA result table, returning the replayed tournament and each player's
/// post-tournament strength (`ELO + (+/-)`, or the `*` rating for a pre-unrated
/// player) keyed by player id.
pub fn import_fesa_results(
    text: &str,
) -> Result<(Tournament, HashMap<TournamentId, f64>), ResultImportError> {
    let mut title: Option<String> = None;
    let mut data: Vec<(usize, &str)> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if title.is_none() {
            // The very first non-empty line is always the title. It is taken
            // unconditionally — before the data-row test — because a title can
            // begin with a year (e.g. "2026 British Shogi Championships"), which
            // otherwise looks like a leading player number and would be misparsed
            // as a data row. The column header and trailing "Promoting …" lines
            // come after it and are non-data, so they are ignored.
            title = Some(line.trim().to_string());
        } else if is_data_row(line) {
            data.push((i + 1, line));
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

    let (tournament, id_of) = build_tournament(title.as_deref(), rows)?;
    let strengths = strength_by_number
        .into_iter()
        .filter_map(|(number, s)| id_of.get(&number).map(|&id| (id, s)))
        .collect();
    Ok((tournament, strengths))
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
) -> Result<(RawRow, f64), ResultImportError> {
    let bad = |reason: &str| ResultImportError::BadRow {
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
    // Normalize the parenthetical marks: a handicap glued to its cell (e.g.
    // `30+(-r )`) keeps its group, minus any internal space that would otherwise
    // split the cell across two whitespace tokens; a free-standing parenthetical
    // (a "(elder)" in a name, say) is dropped.
    let remainder: String = chars[name_start + width..].iter().collect();
    let remainder = normalize_cell_annotations(&remainder);
    let tokens: Vec<&str> = remainder.split_whitespace().collect();

    // ELO sits immediately left of the round cells (each of which ends in a
    // result sign). Anchoring it there — rather than to "the first number-shaped
    // token" — lets a rating be as short as 1-2 digits without colliding with a
    // grade level like the `2` in `2 Dan`, which also precedes the ELO. The
    // anchored token is still shape-checked so a malformed row fails cleanly.
    let first_cell = tokens
        .iter()
        .position(|t| is_cell_token(t))
        .ok_or_else(|| bad("no round cells"))?;
    let elo_idx = match first_cell.checked_sub(1) {
        Some(i) if is_elo_token(tokens[i]) => i,
        _ => return Err(bad("no ELO column found")),
    };
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

/// Normalize the parenthetical groups in a row's remainder.
///
/// A group **glued** to the token before it is a round-cell annotation — the
/// handicap marks `(-r )`, `(+b )`, `(-rl)`, `(-4p)`, … that trail a cell like
/// `30+(-r )`. It is kept (so [`parse_cell`] can read the odds and which side
/// conceded them) with any internal whitespace removed, which would otherwise
/// split one cell across two whitespace tokens.
///
/// A group that stands on its own — `(elder)` in a name, `(U18)` in a title —
/// annotates no cell, so it is dropped as before rather than left to shift the
/// column tokenisation. An unclosed `(` runs to the end of the string.
/// Everything outside the parentheses is preserved verbatim.
fn normalize_cell_annotations(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut group = String::new();
    let mut depth: u32 = 0;
    // Whether the `(` that opened the current group was glued to a preceding
    // non-space character, i.e. attached to a cell rather than standing alone.
    let mut attached = false;
    for c in s.chars() {
        match c {
            '(' if depth == 0 => {
                depth = 1;
                attached = out.chars().next_back().is_some_and(|p| p != ' ');
                group.clear();
                group.push('(');
            }
            '(' => depth += 1,
            ')' if depth > 0 => {
                depth -= 1;
                group.push(')');
                if depth == 0 && attached {
                    out.push_str(&group);
                }
            }
            // Inside a group: collect it (dropping spaces) until it closes.
            _ if depth > 0 => {
                if c != ' ' {
                    group.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// A rating token: a 1-4 digit number, or any digits with a trailing `*` (the
/// pre-unrated marker). Ratings can be as low as 1-2 digits; the token is only
/// tested at its anchored position (just left of the round cells), so allowing
/// the short forms here does not let a grade level be mistaken for an ELO.
fn is_elo_token(t: &str) -> bool {
    match t.strip_suffix('*') {
        Some(core) => !core.is_empty() && core.chars().all(|c| c.is_ascii_digit()),
        None => (1..=4).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit()),
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
/// trailing handicap annotation and floater mark. Distinguishes cells from Pts
/// (no sign) and the signed `+/-` delta (sign at the front, not the back).
fn is_cell_token(t: &str) -> bool {
    // The annotation is only shape-checked here; `parse_cell` validates it.
    let t = match t.split_once('(') {
        Some((cell, annotation)) if annotation.ends_with(')') => cell,
        Some(_) => return false, // an unclosed group is not a cell
        None => t,
    };
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
    use crate::result_import::Cell;
    use crate::round::{Handicap, Winner};

    fn load(fixture: &str) -> (Tournament, HashMap<TournamentId, f64>) {
        let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).expect("fixture present");
        import_fesa_results(&decode_latin1(&bytes)).expect("parses")
    }

    fn wosc() -> (Tournament, HashMap<TournamentId, f64>) {
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
        assert_eq!(strengths[&kob.tournament_id.unwrap()], 2582.0);
        // A negative delta: Leiter 2326 - 27 = 2299.
        let leiter = find(&t, "Leiter", "Thomas");
        assert_eq!(strengths[&leiter.tournament_id.unwrap()], 2299.0);
    }

    #[test]
    fn pre_unrated_players_have_no_pairing_rating_and_the_star_is_their_strength() {
        let (t, strengths) = wosc();
        // Hayakawa Akio: "2337*" — unrated before, no +/-. Pairing rating None,
        // strength = 2337.
        let haya = find(&t, "Hayakawa", "Akio");
        assert_eq!(haya.rating, None);
        assert_eq!(strengths[&haya.tournament_id.unwrap()], 2337.0);
    }

    #[test]
    fn multiword_names_and_offset_rows_parse() {
        let (t, strengths) = wosc();
        // Multi-word last name.
        let vdl = find(&t, "van der Lubbe", "Lex");
        assert_eq!(vdl.rating, Some(1929));
        assert_eq!(strengths[&vdl.tournament_id.unwrap()], 1916.0); // 1929 - 13
                                                                    // Multi-word first name.
        let nguyen = find(&t, "Nguyen", "Anh Tuan");
        assert_eq!(strengths[&nguyen.tournament_id.unwrap()], 1861.0); // 1881 - 20
                                                                       // An 18-char last name splits from its first name.
        find(&t, "Fernandez Nogueira", "Anna");
        // A 3-digit-number (offset) row still parses.
        let ozal = find(&t, "\u{d6}zal", "Berke"); // "Özal", row 100
        assert_eq!(strengths[&ozal.tournament_id.unwrap()], 1496.0); // 1499 - 3
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
        assert_eq!(strengths[&nguyen.tournament_id.unwrap()], 1869.0); // 1862 + 7 (delta past the MMS "1")

        // A pre-unrated player with a single-digit `*` rating ("1*").
        let anais = find(&t, "Massis", "Ana\u{ef}s"); // "Anaïs"
        assert_eq!(anais.rating, None);
        assert_eq!(strengths[&anais.tournament_id.unwrap()], 1.0);
    }

    #[test]
    fn cdf_2025_parses_with_its_own_detected_width() {
        let (t, strengths) = load("results_CdF_2025.txt");
        assert_eq!(t.rounds.len(), 6);
        let pucher = find(&t, "Pucher", "Olivier");
        assert_eq!(strengths[&pucher.tournament_id.unwrap()], 1873.0); // 1887 - 14
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
        let beta = find(&t, "Beta", "Bob");
        assert!(t.rounds[1]
            .absentees()
            .any(|id| id == beta.tournament_id.unwrap()));
        let points = |id| {
            t.standings()
                .into_iter()
                .find(|s| s.player_id == id)
                .unwrap()
                .points
        };
        assert_eq!(points(beta.id), 0);
        assert_eq!(points(find(&t, "Alpha", "Ann").id), 4); // win + bye = 2 pts = 4 halves
    }

    #[test]
    fn imports_a_fesa_half_point_bye_and_scores_it() {
        // A FESA table with an odd field where one player sits out a round as a
        // half-point bye (`0=`), like the all-`0=` Campionato Italiano. The cell
        // states the ½ itself, so it scores without touching any setting.
        let mut text = String::from("Half Open : 2026\nNr Name Nat Grade ELO R1 Pts +/-\n");
        let row = |nr: u32, last: &str, first: &str, c1: &str, pts: &str| {
            format!("{nr:>2} {last:<12}{first} IT 1 Dan 1500 {c1} {pts} +0\n")
        };
        text.push_str(&row(1, "Alpha", "Ann", "2+", "1")); // beats Beta
        text.push_str(&row(2, "Beta", "Bob", "1-", "0")); // loses to Alpha
        text.push_str(&row(3, "Gamma", "Cid", "0=", "1/2")); // half-point bye

        let (t, _) = import_fesa_results(&text).unwrap();
        assert!(!t.settings.half_point_absences, "the setting is untouched");
        let points = |last, first| {
            t.standings()
                .into_iter()
                .find(|s| s.player_id == find(&t, last, first).id)
                .unwrap()
                .points
        };
        assert_eq!(points("Alpha", "Ann"), 2); // a full win = 2 halves
        assert_eq!(points("Beta", "Bob"), 0); // a loss
        assert_eq!(points("Gamma", "Cid"), 1); // the half-point bye = 1 half
    }

    #[test]
    fn normalizes_parenthetical_round_cell_annotations() {
        // An annotation glued to its cell is kept, with its internal space
        // removed so the cell stays a single whitespace token.
        assert_eq!(normalize_cell_annotations("30+(-r ) 32+"), "30+(-r) 32+");
        assert_eq!(
            normalize_cell_annotations("34+(-rl) 26+(+b )"),
            "34+(-rl) 26+(+b)"
        );
        assert_eq!(normalize_cell_annotations("48-(+4p)"), "48-(+4p)");
        // No annotation: untouched.
        assert_eq!(normalize_cell_annotations("14- 32+ 8-"), "14- 32+ 8-");
        // A free-standing group annotates no cell (a name suffix, say) and is
        // dropped, so it can't shift the column tokenisation.
        assert_eq!(
            normalize_cell_annotations("Mykhaylo (elder) UA 691 13-"),
            "Mykhaylo  UA 691 13-"
        );
        // Unclosed parens run to the end rather than panicking — dropped when
        // free-standing, and never emitted when attached (they close nothing).
        assert_eq!(normalize_cell_annotations("10+ (-r"), "10+ ");
        assert_eq!(normalize_cell_annotations("10+(-r"), "10+");
    }

    #[test]
    fn handicap_games_carry_their_odds_and_conceding_side() {
        // Two handicap games. Alpha (rated) concedes 5 pieces to Delta and wins;
        // Gamma — the *lower*-rated side — concedes sente to Beta, which a
        // rating-derived giver would get backwards.
        let mut text = String::from("Handicap Open : 2026\nNr Name Nat Grade ELO R1 Pts +/-\n");
        let row = |nr: u32, last: &str, first: &str, elo: &str, c1: &str, pts: u32| {
            format!("{nr:>2} {last:<12}{first} PL {elo} {c1} {pts} +0\n")
        };
        text.push_str(&row(1, "Alpha", "Ann", "1939", "4+(-5p)", 1));
        text.push_str(&row(2, "Beta", "Bob", "1633", "3+(+m)", 1));
        text.push_str(&row(3, "Gamma", "Cid", "691*", "2-(-m)", 0));
        text.push_str(&row(4, "Delta", "Dan", "1296", "1-(+5p)", 0));

        let (t, _) = import_fesa_results(&text).unwrap();
        let tid = |last: &str| {
            t.players
                .iter()
                .find(|p| p.last_name == last)
                .unwrap()
                .tournament_id
                .unwrap()
        };
        let board = |a: &str, b: &str| {
            let (x, y) = (tid(a), tid(b));
            t.rounds[0]
                .boards
                .iter()
                .find(|bd| {
                    (bd.player1 == x && bd.player2 == y) || (bd.player1 == y && bd.player2 == x)
                })
                .expect("board exists")
        };
        let giver_side = |bd: &crate::Board, last: &str| {
            if bd.player1 == tid(last) {
                Winner::Player1
            } else {
                Winner::Player2
            }
        };

        let ad = board("Alpha", "Delta");
        let game = ad.handicap.expect("handicap set");
        assert_eq!(game.handicap, Handicap::FivePiece);
        assert_eq!(game.giver, giver_side(ad, "Alpha"));

        // `m` is the older spelling of the sente handicap, and the conceding side
        // is the one the cell marks `-` — here the *weaker*, pre-unrated player.
        let bg = board("Beta", "Gamma");
        let game = bg.handicap.expect("handicap set");
        assert_eq!(game.handicap, Handicap::Sente);
        assert_eq!(game.giver, giver_side(bg, "Gamma"));
        assert_eq!(find(&t, "Gamma", "Cid").rating, None); // unrated, yet the giver
    }

    #[test]
    fn rejects_a_handicap_only_one_side_records() {
        let mut text = String::from("Half-marked : 2026\nNr Name Nat Grade ELO R1 Pts +/-\n");
        let row = |nr: u32, last: &str, first: &str, c1: &str, pts: u32| {
            format!("{nr:>2} {last:<12}{first} PL 1500 {c1} {pts} +0\n")
        };
        text.push_str(&row(1, "Alpha", "Ann", "2+(-r)", 1));
        text.push_str(&row(2, "Beta", "Bob", "1-", 0));
        assert!(matches!(
            import_fesa_results(&text),
            Err(ResultImportError::InconsistentHandicap {
                round: 1,
                a: 1,
                b: 2
            })
        ));
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
        let beta = find(&t, "Beta", "Bob").tournament_id.unwrap();
        let gamma = find(&t, "Gamma", "Cid").tournament_id.unwrap();
        let win = t.rounds[2]
            .boards
            .iter()
            .find(|b| {
                (b.player1 == beta && b.player2 == gamma)
                    || (b.player1 == gamma && b.player2 == beta)
            })
            .expect("Beta vs Gamma paired in round 3");
        assert!(win.outcome.winner().is_some());
    }

    #[test]
    fn a_title_that_begins_with_a_year_is_not_misparsed_as_a_player_row() {
        // FESA titles often lead with the year (e.g. "2026 British Shogi
        // Championships"), which looks like a leading player number. The first
        // line must be taken as the title regardless, or it is fed to the row
        // parser and fails with "no ELO column found".
        let mut text = String::from("2026 British Shogi Championships : 2026-02-14/15\n");
        text.push_str("Nr Name Nat Grade ELO R1 R2 Pts +/-\n");
        text.push_str(" 1 Lamb            Stephen GB 4 Dan 1955 2+ 2+ 2 +4\n");
        text.push_str(" 2 Ikeda           Masahiro JP 2 Dan 1912 1- 1- 0 -4\n");

        let (t, _) = import_fesa_results(&text).unwrap();
        assert_eq!(t.name, "2026 British Shogi Championships : 2026-02-14/15");
        assert_eq!(t.players.len(), 2);
        assert_eq!(t.rounds.len(), 2);
    }

    #[test]
    fn one_and_two_digit_elos_parse_without_a_grade_level_being_mistaken_for_them() {
        use crate::Grade;
        // A rated player can have a very low ELO (1-2 digits). The rating is
        // anchored to the token just left of the round cells, so the `2`/`3` of a
        // "2 Dan" / "3 Kyu" grade — which precedes the ELO — is never picked up.
        let mut text = String::from("Low ELO Open : 2026\nNr Name Nat Grade ELO R1 R2 Pts +/-\n");
        let row = |nr: u32,
                   last: &str,
                   first: &str,
                   grade: &str,
                   elo: &str,
                   cells: &str,
                   pts: u32,
                   dz: &str| {
            format!("{nr:>2} {last:<12}{first} SK {grade} {elo} {cells} {pts} {dz}\n")
        };
        // R1: 1 beats 2, 3 beats 4. R2: 1 beats 3, 2 beats 4.
        text.push_str(&row(1, "Senaj", "Aurel", "3 Kyu", "88", "2+ 3+", 2, "+8")); // 2-digit
        text.push_str(&row(2, "Novak", "Jan", "2 Dan", "9", "1- 4+", 1, "-3")); // 1-digit
        text.push_str(&row(3, "Horak", "Ivo", "1 Kyu", "1234", "4+ 1-", 1, "+0"));
        text.push_str(&row(4, "Fiala", "Petr", "5 Kyu", "800", "3- 2-", 0, "+1"));

        let (t, strengths) = import_fesa_results(&text).unwrap();
        let senaj = find(&t, "Senaj", "Aurel");
        assert_eq!(senaj.rating, Some(88));
        assert_eq!(senaj.grade, Some(Grade::kyu(3)));
        assert_eq!(strengths[&senaj.tournament_id.unwrap()], 96.0); // 88 + 8
        let novak = find(&t, "Novak", "Jan");
        assert_eq!(novak.rating, Some(9));
        assert_eq!(novak.grade, Some(Grade::dan(2)));
        assert_eq!(strengths[&novak.tournament_id.unwrap()], 6.0); // 9 - 3
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
