//! `fesa-dump` — dump the benchmarking-relevant shape of a FESA result file as
//! JSON, reusing the same importer as `osp-sim` so the ELO/unrated distinction,
//! the strength (`pre-ELO + (+/-)`) and the reconstructed round structure all
//! match what a simulation would see.
//!
//! Usage: `fesa-dump <result-file>`
//!
//! Prints one JSON object to stdout:
//!
//! ```json
//! {
//!   "n_players": 107,
//!   "n_rounds": 9,
//!   "players": [
//!     {"rating": 2567, "strength": 2582.0, "absent_rounds": []},
//!     {"rating": null, "strength": 2337.0, "absent_rounds": [8, 9]},
//!     …
//!   ]
//! }
//! ```
//!
//! - `players[i].rating` is the pre-tournament pairing ELO, or `null` for a
//!   player who was unrated before the event (their `*` rating).
//! - `players[i].strength` is the post-tournament true strength the oracle uses.
//! - `players[i].absent_rounds` are the 1-based round numbers that player was
//!   marked absent (byes excluded) — kept per player, rather than a per-round
//!   count, so the correlation the count throws away survives: late-round
//!   dropouts, late arrivals, and runs of consecutive missed rounds. With
//!   `n_rounds` it lets the generator map one player's pattern onto a tournament
//!   of a different length (see `scripts/fesa_extract_elo_pairs.py`).
//!
//! Feeds `scripts/fesa_extract_elo_pairs.py`, which pools these across the whole
//! valid corpus into the sampling table the fake-tournament generator draws on.
//! Parse/read errors go to stderr with a non-zero exit.

use std::collections::HashMap;
use std::process::ExitCode;

use osp_core::{decode_latin1, import_fesa_results, TournamentId};
use serde_json::{json, Value};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!(
            "usage: {} <result-file>",
            args.first().map(String::as_str).unwrap_or("fesa-dump")
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
    let (tournament, strengths) = match import_fesa_results(&decode_latin1(&bytes)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("parsing {path}: {e}");
            return ExitCode::from(1);
        }
    };

    // Per player: the 1-based round numbers they were marked absent (byes
    // excluded), gathered by scanning each round's absentee set once.
    let mut absent_rounds: HashMap<TournamentId, Vec<u32>> = HashMap::new();
    for round in &tournament.rounds {
        for id in round.absentees() {
            absent_rounds.entry(id).or_default().push(round.number);
        }
    }

    let players: Vec<Value> = tournament
        .players
        .iter()
        .map(|p| {
            let strength = p.tournament_id.and_then(|id| strengths.get(&id)).copied();
            let absent = p
                .tournament_id
                .and_then(|id| absent_rounds.get(&id))
                .cloned()
                .unwrap_or_default();
            json!({
                "rating": p.rating,
                "strength": strength,
                "absent_rounds": absent,
            })
        })
        .collect();

    let out = json!({
        "n_players": tournament.players.len(),
        "n_rounds": tournament.rounds.len(),
        "players": players,
    });
    println!("{out}");
    ExitCode::SUCCESS
}
