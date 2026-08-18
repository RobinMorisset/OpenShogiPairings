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
//! The whole import is **all-or-nothing**, and it refuses anything it cannot
//! read rather than guessing: a row without a last name, a row whose cell count
//! doesn't match the header, a non-empty ELO or grade cell that doesn't parse,
//! a name matching several FESA entries, an empty file, missing name columns —
//! each aborts the import with a [`CsvImportError`] naming the rows, so a
//! half-imported or quietly-rewritten roster never lands. That strictness is the
//! point around the FESA fill-in: an unreadable cell treated as "missing" would
//! be *replaced* by the federation's value, and the result looks exactly like a
//! correct import. The parser is pure — it takes the FESA list
//! as an argument rather than fetching it — so the server can run it against its
//! own cache and it stays trivially testable.
//!
//! This lives in `osp-core` (not the browser client) so the parsing rules have a
//! single, tested implementation shared by every client.

use crate::fesa::{self, RatedPlayer};
use crate::player::{Grade, NewPlayer};

/// A fatal problem that aborts the whole CSV import.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsvImportError {
    /// The file had no data rows (empty, or a header with nothing under it).
    #[error("the CSV file has no players to import")]
    Empty,
    #[error("the CSV must have a last-name column and a first-name column")]
    MissingNameColumns,
    /// One or more data rows had a blank last name (1-based row numbers, the
    /// header being row 1). The whole import is rejected so nothing half-lands.
    #[error("these rows are missing a last name: {}", format_rows(.rows))]
    RowsMissingLastName { rows: Vec<usize> },
    /// A quoted cell was never closed, so the rest of the file was being read as
    /// part of it — turning a roster into one player with an enormous name.
    #[error("a quoted cell in the CSV is never closed")]
    UnterminatedQuote,
    /// One or more data rows had a different number of cells than the header.
    /// Reading them anyway would take each column from whatever happened to sit
    /// at that index — an unquoted delimiter inside a name shifts every cell to
    /// its right, so the ELO column can quietly be read out of the club column.
    #[error("these rows don't have the same number of columns as the header: {}", format_rows(.rows))]
    RaggedRows { rows: Vec<usize> },
    /// One or more data rows had a non-empty ELO cell that isn't a plain number.
    /// Treating it as "no rating" would let the FESA list fill in a *different*
    /// rating, indistinguishable from a correct import — `2,100` and `2 100` are
    /// routine spreadsheet output, so this is not a rare case.
    #[error("these rows have an ELO that isn't a plain number: {}", format_rows(.rows))]
    RowsWithBadRating { rows: Vec<usize> },
    /// One or more data rows had a non-empty grade cell that isn't a dan/kyu
    /// grade. As with the ELO, the FESA list would otherwise fill in its own.
    #[error("these rows have an unreadable grade: {}", format_rows(.rows))]
    RowsWithBadGrade { rows: Vec<usize> },
    /// One or more data rows needed the FESA list to fill in a missing ELO,
    /// grade or nationality, but their name matches several entries in it. There
    /// is no way to tell which one the referee meant, so we ask rather than pick.
    #[error(
        "these rows match more than one player in the FESA rating list; fill in their ELO and grade by hand: {}",
        format_rows(.rows)
    )]
    RowsMatchingSeveralRatedPlayers { rows: Vec<usize> },
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
///
/// Shared with [`crate::licence`], which folds names and nationalities the same
/// way so a list that imports also checks.
pub(crate) fn normalize(text: &str) -> String {
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

/// The accent-folded `last + first` key used to look a player up in the FESA
/// list — and, in [`crate::licence`], in a federation's licence list.
pub(crate) fn name_key(last: &str, first: &str) -> String {
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
///
/// A quote that is never closed is refused rather than read to the end of the
/// file: an unclosed quote in the last text column swallows every row below it
/// into one cell, so a roster of forty imports as one player with a very long
/// first name — no error, no empty result, nothing to notice.
fn parse_rows(text: &str, delimiter: char) -> Result<Vec<Vec<String>>, CsvImportError> {
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
    if in_quotes {
        return Err(CsvImportError::UnterminatedQuote);
    }
    // Flush a trailing field/row not terminated by a newline.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    rows.retain(|r| r.iter().any(|cell| !cell.trim().is_empty()));
    Ok(rows)
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
    let rows = parse_rows(text, detect_delimiter(first_line))?;
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

    let cell = |row: &[String], idx: Option<usize>| -> String {
        idx.and_then(|i| row.get(i))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    let mut players = Vec::new();
    let mut bad_rows = Vec::new();
    let mut ragged_rows = Vec::new();
    let mut bad_rating_rows = Vec::new();
    let mut bad_grade_rows = Vec::new();
    let mut ambiguous_rows = Vec::new();
    for (offset, row) in data.iter().enumerate() {
        // 1-based row number in the file (header is row 1).
        let row_number = offset + 2;
        // A row that doesn't line up with the header is refused rather than read
        // column-by-index anyway: too few cells makes a typed ELO look absent
        // (and get overwritten from the FESA list), too many means a delimiter
        // slipped into a field and every column past it is someone else's data.
        if row.len() != header.len() {
            ragged_rows.push(row_number);
            continue;
        }
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
        // A cell the referee *did* fill in but we can't read is an error, not a
        // `None` to be quietly refilled from the FESA list: the whole point of
        // typing it was to override what the federation has.
        let mut rating = match rating_idx
            .map(|_| cell(row, rating_idx))
            .filter(|raw| !raw.is_empty())
        {
            // ELO must be a plain non-negative integer to count.
            Some(raw) => match raw.parse::<u32>() {
                Ok(rating) => Some(rating),
                Err(_) => {
                    bad_rating_rows.push(row_number);
                    continue;
                }
            },
            None => None,
        };
        let mut grade = match grade_idx
            .map(|_| cell(row, grade_idx))
            .filter(|raw| !raw.is_empty())
        {
            Some(raw) => match Grade::parse(&raw) {
                Some(grade) => Some(grade),
                None => {
                    bad_grade_rows.push(row_number);
                    continue;
                }
            },
            None => None,
        };
        let nat_cell = cell(row, nat_idx);
        let mut nationality = (!nat_cell.is_empty()).then_some(nat_cell);

        // Fill anything the row didn't carry — ELO (with its game count), grade,
        // and nationality — from the FESA list, matched on the exact accent-folded
        // name. The lookup runs whenever *any* of them is missing, so e.g. a row
        // that gave an ELO but no nationality still gets the nationality filled.
        if rating.is_none() || grade.is_none() || nationality.is_none() {
            match fesa::lookup(ratings, &player.last_name, first_name.as_str()) {
                fesa::Lookup::One(m) => {
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
                // Homonyms are real in a European-wide list. Picking the first
                // match would hand this player someone else's rating, and nothing
                // downstream could tell. Refuse and let the referee type it.
                fesa::Lookup::Ambiguous(_) => {
                    ambiguous_rows.push(row_number);
                    continue;
                }
                fesa::Lookup::None => {}
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

    // Structural problems first: a ragged row explains most of what follows it.
    if !ragged_rows.is_empty() {
        return Err(CsvImportError::RaggedRows { rows: ragged_rows });
    }
    if !bad_rows.is_empty() {
        return Err(CsvImportError::RowsMissingLastName { rows: bad_rows });
    }
    if !bad_rating_rows.is_empty() {
        return Err(CsvImportError::RowsWithBadRating {
            rows: bad_rating_rows,
        });
    }
    if !bad_grade_rows.is_empty() {
        return Err(CsvImportError::RowsWithBadGrade {
            rows: bad_grade_rows,
        });
    }
    if !ambiguous_rows.is_empty() {
        return Err(CsvImportError::RowsMatchingSeveralRatedPlayers {
            rows: ambiguous_rows,
        });
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
    fn a_negative_or_bogus_rating_cell_is_refused() {
        // Not silently dropped to "unrated": the referee typed something, and
        // treating it as absent would let the FESA list overwrite it.
        let csv = "Last name,First name,ELO\nAlpha,Ann,-5\nBeta,Bo,abc\n";
        assert_eq!(
            parse_players_csv(csv, &[]),
            Err(CsvImportError::RowsWithBadRating { rows: vec![2, 3] })
        );
    }

    #[test]
    fn a_thousands_separator_in_the_elo_is_refused_not_refilled_from_fesa() {
        // The case that motivates the rule: `2 100` is routine spreadsheet output,
        // and reading it as "no ELO" would hand Ann the FESA list's 1500 instead.
        let csv = "Last name,First name,ELO\nAlpha,Ann,2 100\n";
        assert_eq!(
            parse_players_csv(csv, &[rated("Alpha", "Ann", 1500, 40, None)]),
            Err(CsvImportError::RowsWithBadRating { rows: vec![2] })
        );
    }

    #[test]
    fn an_unreadable_grade_cell_is_refused() {
        let csv = "Last name,First name,Grade\nAlpha,Ann,green belt\n";
        assert_eq!(
            parse_players_csv(csv, &[]),
            Err(CsvImportError::RowsWithBadGrade { rows: vec![2] })
        );
    }

    #[test]
    fn rows_that_dont_match_the_header_width_are_refused() {
        // Row 2 is short (a missing ELO cell would look like a blank one); row 3
        // is long, which is what an unquoted delimiter inside a name produces —
        // there, every column past it belongs to the wrong field.
        let csv = "Last name,First name,ELO\nAlpha,Ann\nGamma,Cid,1700,extra\n";
        assert_eq!(
            parse_players_csv(csv, &[]),
            Err(CsvImportError::RaggedRows { rows: vec![2, 3] })
        );
    }

    #[test]
    fn a_name_matching_two_fesa_entries_is_refused_rather_than_guessed() {
        // Homonyms are real in a European-wide list; picking the first would give
        // this player someone else's rating with nothing to show for it.
        let csv = "Last name,First name\nAlpha,Ann\n";
        let list = [
            rated("Alpha", "Ann", 1500, 40, None),
            rated("Alpha", "Ann", 2100, 80, None),
        ];
        assert_eq!(
            parse_players_csv(csv, &list),
            Err(CsvImportError::RowsMatchingSeveralRatedPlayers { rows: vec![2] })
        );
        // A row that needs nothing from the list is unaffected by the ambiguity.
        let complete = "Last name,First name,ELO,Grade,Nationality\nAlpha,Ann,1800,2d,FR\n";
        let players = parse_players_csv(complete, &list).unwrap();
        assert_eq!(players[0].rating, Some(1800));
    }

    #[test]
    fn an_unterminated_quote_is_refused_not_read_to_the_end_of_the_file() {
        // The dangerous shape: the quote opens in the last text column, so it
        // eats every row below it. This used to import as one player called
        // "Alpha" whose first name was the rest of the file — three players
        // silently down to one, with an `Ok` to say it went fine.
        let csv = "Last name,First name\nAlpha,\"Ann\nBeta,Bo\nGamma,Cid\n";
        assert_eq!(
            parse_players_csv(csv, &[]),
            Err(CsvImportError::UnterminatedQuote)
        );
        // A properly closed quote still carries a delimiter and a newline.
        let quoted = "Last name,First name\n\"Le Roux, aine\",\"Jean\nPaul\"\n";
        let players = parse_players_csv(quoted, &[]).unwrap();
        assert_eq!(players[0].last_name, "Le Roux, aine");
        assert_eq!(players[0].first_name.as_deref(), Some("Jean\nPaul"));
    }

    #[test]
    fn a_utf8_bom_does_not_hide_the_first_column() {
        // Excel writes one on every CSV it exports. It lands on the first header
        // cell, so without folding it away the last-name column simply isn't
        // found and the whole file is refused.
        let csv = "\u{feff}Last name,First name,ELO\nAlpha,Ann,2000\n";
        let players = parse_players_csv(csv, &[]).unwrap();
        assert_eq!(players[0].last_name, "Alpha");
        assert_eq!(players[0].rating, Some(2000));
    }

    #[test]
    fn cr_only_and_crlf_line_endings_parse_like_lf() {
        // Classic-Mac CR-only endings, and the CRLF that any Windows export has.
        let expected = |csv: &str| {
            let players = parse_players_csv(csv, &[]).unwrap();
            assert_eq!(players.len(), 2, "{csv:?}");
            assert_eq!(players[1].last_name, "Beta", "{csv:?}");
            assert_eq!(players[1].rating, Some(1700), "{csv:?}");
        };
        expected("Last name,First name,ELO\rAlpha,Ann,2000\rBeta,Bo,1700\r");
        expected("Last name,First name,ELO\r\nAlpha,Ann,2000\r\nBeta,Bo,1700\r\n");
        expected("Last name,First name,ELO\nAlpha,Ann,2000\nBeta,Bo,1700\n");
    }

    #[test]
    fn grade_kind_round_trips_through_parse() {
        assert_eq!(Grade::parse("1d").map(|g| g.kind), Some(GradeKind::Dan));
        assert_eq!(Grade::parse("1k").map(|g| g.kind), Some(GradeKind::Kyu));
    }
}
