//! `fesa-players` — dump per-player (nationality, rated?) for a FESA result file,
//! reusing the same importer as `osp-sim`/`mm-grades` so nationality parsing and
//! the unrated (`*`) distinction match exactly.
//!
//! Usage: `fesa-players <result-file>`
//!
//! Prints CSV to stdout: `nationality,rated` — one row per player, `rated` = 1
//! if the player had a pre-tournament rating, 0 if unrated. Nationality is the
//! uppercased country code, or empty if the table carried none. Parse/read errors
//! go to stderr with a non-zero exit.

use std::process::ExitCode;

use osp_core::{decode_latin1, import_fesa_results};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!(
            "usage: {} <result-file>",
            args.first().map(String::as_str).unwrap_or("fesa-players")
        );
        return ExitCode::from(2);
    }
    let path = &args[1];
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("reading {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let (tournament, _strengths) = match import_fesa_results(&decode_latin1(&bytes)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("parsing {path}: {e}");
            return ExitCode::from(1);
        }
    };

    let mut out = String::from("nationality,rated\n");
    for p in &tournament.players {
        let nat = p.nationality.as_deref().unwrap_or("").to_uppercase();
        let rated = if p.rating.is_some() { 1 } else { 0 };
        out.push_str(&format!("{nat},{rated}\n"));
    }
    print!("{out}");
    ExitCode::SUCCESS
}
