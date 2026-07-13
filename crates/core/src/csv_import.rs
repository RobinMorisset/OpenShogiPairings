//! Bulk player import from a CSV file.
//!
//! The referee exports a roster from a spreadsheet and imports it in one step.
//! Column order is **not** fixed: the first row must be a header naming each
//! column, matched case/accent-insensitively against a small set of aliases.
//! `Last name` and `First name` are required; `ELO`, `Grade`, `Nationality` and
//! `Club` are optional and matched the same way; unrecognized columns are
//! ignored. When a row is missing its ELO, grade, or nationality — anything the
//! federation's list also carries — it is filled in from the FESA rating list by
//! exact accent-folded `last + first` name match (the same list that backs
//! registration autocomplete). Club is never in that list, so it stays whatever
//! the CSV gave.
//!
//! The whole import is **all-or-nothing**: any row without a last name (or an
//! empty file / missing name columns) aborts it with a [`CsvImportError`], so a
//! half-imported roster never lands. The parser is pure — it takes the FESA list
//! as an argument rather than fetching it — so the server can run it against its
//! own cache and it stays trivially testable.
//!
//! This lives in `osp-core` (not the browser client) so the parsing rules have a
//! single, tested implementation shared by every client.

use crate::fesa::RatedPlayer;
use crate::player::{Grade, NewPlayer};

/// A fatal problem that aborts the whole CSV import.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsvImportError {
    /// The file had no data rows (empty, or a header with nothing under it).
    #[error("the CSV file has no players to import")]
    Empty,
    /// The header lacked a last-name and/or first-name column.
    #[error("the CSV must have a last-name column and a first-name column")]
    MissingNameColumns,
    /// One or more data rows had a blank last name (1-based row numbers, the
    /// header being row 1). The whole import is rejected so nothing half-lands.
    #[error("these rows are missing a last name: {}", format_rows(.rows))]
    RowsMissingLastName { rows: Vec<usize> },
}

/// Render a list of 1-based row numbers as `2, 5, 7` for the error message.
fn format_rows(rows: &[usize]) -> String {
    rows.iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The columns the importer understands. Anything else in the header is ignored.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    LastName,
    FirstName,
    Rating,
    Grade,
    Nationality,
    Club,
}

/// The header aliases for each field, matched after [`normalize`] on both sides
/// (so `"Nat."` matches `"nat"`, `"Dan/Kyu"` matches `"dan kyu"`, and accents /
/// casing don't matter).
const ALIASES: &[(Field, &[&str])] = &[
    (
        Field::LastName,
        &["last name", "lastname", "nom", "surname"],
    ),
    (
        Field::FirstName,
        &["first name", "firstname", "prenom", "given name"],
    ),
    (Field::Rating, &["elo", "rating", "classement"]),
    (Field::Grade, &["grade", "dan kyu", "dan/kyu"]),
    (
        Field::Nationality,
        &["nationality", "nat", "nat.", "pays", "country"],
    ),
    (Field::Club, &["club"]),
];

/// Which known field a header cell names, if any.
fn match_column(header: &str) -> Option<Field> {
    let norm = normalize(header);
    ALIASES
        .iter()
        .find(|(_, aliases)| aliases.iter().any(|a| normalize(a) == norm))
        .map(|(field, _)| *field)
}

/// Fold to a bare, lower-case, accent-free key with punctuation collapsed to
/// single spaces — used for both header-alias and FESA-name matching, so
/// `"Frédéric"` matches `"Frederic"` and `"Le-Roux"` matches `"Le Roux"`.
fn normalize(text: &str) -> String {
    let folded: String = text
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
            // Combining diacritical marks (the tail of a decomposed NFD letter)
            // are dropped, so "e" + ´ folds to "e" rather than splitting the word.
            c if ('\u{0300}'..='\u{036F}').contains(&c) => None,
            // A bare ASCII alphanumeric is kept; anything else is a separator.
            c if c.is_ascii_alphanumeric() => Some(c),
            _ => Some(' '),
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The accent-folded `last + first` key used to look a player up in the FESA list.
fn name_key(last: &str, first: &str) -> String {
    format!("{} {}", normalize(last), normalize(first))
}

/// Sniff `,` vs `;` (French exports often use `;`) from the header line: whichever
/// is more frequent, defaulting to `,` on a tie.
fn detect_delimiter(header_line: &str) -> char {
    let commas = header_line.chars().filter(|&c| c == ',').count();
    let semicolons = header_line.chars().filter(|&c| c == ';').count();
    if semicolons > commas {
        ';'
    } else {
        ','
    }
}

/// A minimal RFC-4180-ish parser: quoted fields, embedded delimiters/newlines,
/// and `""` escapes. Rows that are entirely blank are dropped.
fn parse_rows(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let chars: Vec<char> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_quotes {
            if c == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    field.push('"');
                    i += 1;
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delimiter {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(c);
        }
        i += 1;
    }
    // Flush a trailing field/row not terminated by a newline.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    rows.retain(|r| r.iter().any(|cell| !cell.trim().is_empty()));
    rows
}

/// Parse CSV `text` into a list of players, filling missing ELO/grade from the
/// FESA `ratings` list. All-or-nothing: returns a [`CsvImportError`] (and no
/// players) if the file is empty, lacks the required name columns, or has any
/// row without a last name.
pub fn parse_players_csv(
    text: &str,
    ratings: &[RatedPlayer],
) -> Result<Vec<NewPlayer>, CsvImportError> {
    let first_line = text.split('\n').next().unwrap_or("");
    let rows = parse_rows(text, detect_delimiter(first_line));
    let Some((header, data)) = rows.split_first() else {
        return Err(CsvImportError::Empty);
    };

    let columns: Vec<Option<Field>> = header.iter().map(|h| match_column(h)).collect();
    let index_of = |field: Field| columns.iter().position(|c| *c == Some(field));
    let (Some(last_idx), Some(first_idx)) = (index_of(Field::LastName), index_of(Field::FirstName))
    else {
        return Err(CsvImportError::MissingNameColumns);
    };
    let rating_idx = index_of(Field::Rating);
    let grade_idx = index_of(Field::Grade);
    let nat_idx = index_of(Field::Nationality);
    let club_idx = index_of(Field::Club);

    // FESA lookup by accent-folded name, for filling missing ELO/grade.
    let cell = |row: &[String], idx: Option<usize>| -> String {
        idx.and_then(|i| row.get(i))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    let mut players = Vec::new();
    let mut bad_rows = Vec::new();
    for (offset, row) in data.iter().enumerate() {
        // 1-based row number in the file (header is row 1).
        let row_number = offset + 2;
        let last_name = cell(row, Some(last_idx));
        let first_name = cell(row, Some(first_idx));
        if last_name.is_empty() {
            bad_rows.push(row_number);
            continue;
        }

        let mut player = NewPlayer {
            last_name,
            first_name: (!first_name.is_empty()).then(|| first_name.clone()),
            ..Default::default()
        };

        // What the row itself carries (a missing column or a blank cell is `None`).
        // ELO must be a non-negative integer to count.
        let mut rating = rating_idx
            .map(|_| cell(row, rating_idx))
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| raw.parse::<u32>().ok());
        let mut grade = grade_idx
            .map(|_| cell(row, grade_idx))
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| Grade::parse(&raw));
        let nat_cell = cell(row, nat_idx);
        let mut nationality = (!nat_cell.is_empty()).then_some(nat_cell);

        // Fill anything the row didn't carry — ELO (with its game count), grade,
        // and nationality — from the FESA list, matched on the exact accent-folded
        // name. The lookup runs whenever *any* of them is missing, so e.g. a row
        // that gave an ELO but no nationality still gets the nationality filled.
        if rating.is_none() || grade.is_none() || nationality.is_none() {
            let key = name_key(&player.last_name, first_name.as_str());
            if let Some(m) = ratings
                .iter()
                .find(|r| name_key(&r.last_name, &r.first_name) == key)
            {
                if rating.is_none() {
                    rating = Some(m.rating);
                    player.fesa_games = Some(m.games);
                }
                if grade.is_none() {
                    grade = m.grade;
                }
                if nationality.is_none() && !m.nationality.trim().is_empty() {
                    nationality = Some(m.nationality.clone());
                }
            }
        }
        player.rating = rating;
        player.grade = grade;
        player.nationality = nationality;

        // The FESA list carries no club, so this stays whatever the row gave.
        let club = cell(row, club_idx);
        player.club = (!club.is_empty()).then_some(club);

        players.push(player);
    }

    if !bad_rows.is_empty() {
        return Err(CsvImportError::RowsMissingLastName { rows: bad_rows });
    }
    if players.is_empty() {
        return Err(CsvImportError::Empty);
    }
    Ok(players)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::GradeKind;

    /// A FESA entry, for enrichment tests.
    fn rated(
        last: &str,
        first: &str,
        rating: u32,
        games: u32,
        grade: Option<Grade>,
    ) -> RatedPlayer {
        RatedPlayer {
            last_name: last.to_string(),
            first_name: first.to_string(),
            rating,
            games,
            nationality: "FR".to_string(),
            grade,
        }
    }

    #[test]
    fn parses_a_basic_roster_with_reordered_columns() {
        let csv = "First name,Last name,ELO,Club\n\
                   Ann,Alpha,2000,Paris\n\
                   Bo,Beta,,Lyon\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players.len(), 2);
        assert_eq!(players[0].last_name, "Alpha");
        assert_eq!(players[0].first_name.as_deref(), Some("Ann"));
        assert_eq!(players[0].rating, Some(2000));
        assert_eq!(players[0].club.as_deref(), Some("Paris"));
        // Empty ELO cell with no FESA match stays unrated.
        assert_eq!(players[1].rating, None);
        assert_eq!(players[1].club.as_deref(), Some("Lyon"));
    }

    #[test]
    fn accent_insensitive_headers_and_semicolon_delimiter() {
        // French headers, semicolon-separated, accents on the column names.
        let csv = "Nom;Prénom;Classement;Nat.\n\
                   Dupont;Jean;1500;FR\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players.len(), 1);
        assert_eq!(players[0].last_name, "Dupont");
        assert_eq!(players[0].first_name.as_deref(), Some("Jean"));
        assert_eq!(players[0].rating, Some(1500));
        assert_eq!(players[0].nationality.as_deref(), Some("FR"));
    }

    #[test]
    fn quoted_fields_with_commas_and_newlines() {
        let csv = "Last name,First name,Club\n\
                   \"Van der Berg\",\"Jan, Jr.\",\"Club\nwith newline\"\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players[0].last_name, "Van der Berg");
        assert_eq!(players[0].first_name.as_deref(), Some("Jan, Jr."));
        assert_eq!(players[0].club.as_deref(), Some("Club\nwith newline"));
    }

    #[test]
    fn escaped_double_quotes() {
        let csv = "Last name,First name\n\"O\"\"Brien\",Sean\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players[0].last_name, "O\"Brien");
    }

    #[test]
    fn grade_column_is_parsed() {
        let csv = "Last name,First name,Grade\nAlpha,Ann,3d\nBeta,Bo,5 kyu\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players[0].grade, Some(Grade::dan(3)));
        assert_eq!(players[1].grade, Some(Grade::kyu(5)));
    }

    #[test]
    fn fesa_fills_missing_rating_and_grade_by_folded_name() {
        // The row has neither ELO nor grade; the FESA list supplies both, matched
        // across an accent difference ("Frederic" row vs "Frédéric" list).
        let ratings = vec![rated(
            "Rô\u{0301}vekamp",
            "Frederic",
            1800,
            42,
            Some(Grade::dan(2)),
        )];
        // Row name is the plain-ASCII spelling with a different accent style.
        let csv = "Last name,First name\nRóvekamp,Frédéric\n";
        let players = parse_players_csv(csv, &ratings).unwrap();
        assert_eq!(players[0].rating, Some(1800));
        assert_eq!(players[0].fesa_games, Some(42));
        assert_eq!(players[0].grade, Some(Grade::dan(2)));
    }

    #[test]
    fn an_explicit_rating_is_not_overridden_by_fesa() {
        let ratings = vec![rated("Alpha", "Ann", 1800, 42, Some(Grade::dan(2)))];
        // Row carries its own ELO; only the (missing) grade is filled from FESA.
        let csv = "Last name,First name,ELO\nAlpha,Ann,2100\n";
        let players = parse_players_csv(csv, &ratings).unwrap();
        assert_eq!(players[0].rating, Some(2100)); // kept, not 1800
        assert_eq!(players[0].fesa_games, None); // not a FESA rating
        assert_eq!(players[0].grade, Some(Grade::dan(2))); // grade still filled
    }

    #[test]
    fn fesa_fills_a_missing_nationality_even_when_elo_and_grade_are_present() {
        // The row carries its own ELO and grade but leaves the nationality cell
        // blank; the FESA list supplies it — so the lookup runs for nationality
        // alone, not only when the ELO/grade are missing.
        let ratings = vec![rated("Alpha", "Ann", 1800, 42, Some(Grade::dan(2)))];
        let csv = "Last name,First name,ELO,Grade,Nat\nAlpha,Ann,2100,3d,\n";
        let players = parse_players_csv(csv, &ratings).unwrap();
        assert_eq!(players[0].nationality.as_deref(), Some("FR")); // from FESA
        assert_eq!(players[0].rating, Some(2100)); // row's own values untouched
        assert_eq!(players[0].grade, Some(Grade::dan(3)));
    }

    #[test]
    fn fesa_fills_nationality_when_the_column_is_absent() {
        let ratings = vec![rated("Alpha", "Ann", 1800, 42, None)];
        let csv = "Last name,First name\nAlpha,Ann\n";
        let players = parse_players_csv(csv, &ratings).unwrap();
        assert_eq!(players[0].nationality.as_deref(), Some("FR"));
    }

    #[test]
    fn an_explicit_nationality_is_not_overridden_by_fesa() {
        // The row says JP, the FESA list says FR — the row wins.
        let ratings = vec![rated("Alpha", "Ann", 1800, 42, None)];
        let csv = "Last name,First name,Nationality\nAlpha,Ann,jp\n";
        let players = parse_players_csv(csv, &ratings).unwrap();
        assert_eq!(players[0].nationality.as_deref(), Some("jp"));
    }

    #[test]
    fn empty_file_and_header_only_are_empty_errors() {
        assert_eq!(parse_players_csv("", &[]), Err(CsvImportError::Empty));
        assert_eq!(
            parse_players_csv("   \n\n", &[]),
            Err(CsvImportError::Empty)
        );
        assert_eq!(
            parse_players_csv("Last name,First name\n", &[]),
            Err(CsvImportError::Empty)
        );
    }

    #[test]
    fn missing_name_columns_is_an_error() {
        let csv = "ELO,Club\n2000,Paris\n";
        assert_eq!(
            parse_players_csv(csv, &[]),
            Err(CsvImportError::MissingNameColumns)
        );
    }

    #[test]
    fn a_row_without_a_last_name_aborts_the_whole_import() {
        // Row 3 (the second data row) has a blank last name.
        let csv = "Last name,First name\nAlpha,Ann\n,Bo\nGamma,Gil\n";
        let err = parse_players_csv(csv, &[]).unwrap_err();
        assert_eq!(err, CsvImportError::RowsMissingLastName { rows: vec![3] });
        // The message names the offending row.
        assert!(err.to_string().contains('3'));
    }

    #[test]
    fn blank_rows_are_skipped_not_counted() {
        // A fully-blank middle row is dropped, so it isn't reported as missing a
        // last name and doesn't shift the players.
        let csv = "Last name,First name\nAlpha,Ann\n\nBeta,Bo\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players.len(), 2);
        assert_eq!(players[1].last_name, "Beta");
    }

    #[test]
    fn a_negative_or_bogus_rating_cell_is_ignored() {
        let csv = "Last name,First name,ELO\nAlpha,Ann,-5\nBeta,Bo,abc\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players[0].rating, None);
        assert_eq!(players[1].rating, None);
    }

    #[test]
    fn grade_kind_round_trips_through_parse() {
        // Guard the enum wiring the importer relies on.
        assert_eq!(Grade::parse("1d").map(|g| g.kind), Some(GradeKind::Dan));
        assert_eq!(Grade::parse("1k").map(|g| g.kind), Some(GradeKind::Kyu));
    }
}
