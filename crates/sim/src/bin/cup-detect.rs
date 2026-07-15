//! `cup-detect` — detect whether a FESA result file was actually run as a national
//! championship's hybrid direct-elimination cup (a top-2^N seeded knockout among
//! the host nation beside a Swiss open — the French Championship format).
//!
//! Usage: `cup-detect <result-file>`
//!
//! Prints `SIZE:NAT` (e.g. `16:FR`) to stdout — the cup size and the eligible
//! nationality code, for osp-sim's `--cup-nations`. Prints nothing (exit 0) when no
//! cup is detected; parse/read errors go to stderr with a non-zero exit.
//!
//! The cup is taken to be among the single most common nationality (the host of a
//! national championship). For each candidate size (largest first) it seeds the
//! top-`size` such players exactly as osp-core would (rating-descending, unrated
//! last) and asks [`knockout_champion`] whether those players actually played a
//! single-elimination knockout over the recorded rounds. That check is
//! **fold-/seeding-robust**: it verifies the knockout *structure* (each round a
//! closed internal matching halving the field to one champion) rather than a
//! specific seeded fold, and skips long-game gap rounds.
//!
//! The individual WOSC / European Championship is *not* handled here — its exact
//! roster (stale seed ratings, declined entries, borderline "European"
//! nationalities) is only reliably recovered from who reached the final, so it is
//! reconstructed backward from the medal table via osp-sim's `--cup-final` instead
//! (see `scripts/esc_finals.json` and `mm_study.py`).

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

use osp_core::sim::cup_eligibility;
use osp_core::{decode_latin1, import_fesa_results, knockout_champion, CUP_SIZES};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!(
            "usage: {} <result-file>",
            args.first().map(String::as_str).unwrap_or("cup-detect")
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

    // The cup is among the single most common nationality (the host of a national
    // championship); ties broken alphabetically for determinism.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for n in tournament
        .players
        .iter()
        .filter_map(|p| p.nationality.as_deref())
        .filter(|n| !n.is_empty())
    {
        *counts.entry(n).or_default() += 1;
    }
    let nations: HashSet<String> = match counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
    {
        Some((n, _)) => std::iter::once(n.to_uppercase()).collect(),
        None => return ExitCode::SUCCESS, // no nationalities -> no cup
    };

    // Largest size first. WOSC and a national championship are the *same* hybrid
    // format — a top-`size` seeded knockout among the eligible nationalities beside
    // a Swiss open — so both are verified the same way: do the top-`size` eligible
    // players (seeded exactly as osp-core would) actually play a single-elimination
    // knockout? The check is fold-/seeding-robust, so it survives the near-tie seed
    // perturbations real events accumulate from slightly stale rating lists.
    let mut sizes = CUP_SIZES.to_vec();
    sizes.sort_unstable_by(|a, b| b.cmp(a)); // descending
    for size in sizes {
        let cup_rounds = size.trailing_zeros() as usize;
        if tournament.rounds.len() < cup_rounds {
            continue;
        }
        // A cup entrant is defined by playing the round-of-`size` (round 1), not by
        // attending every cup round: a first-round loser drops to the Swiss and may
        // miss a later round, but was still in the bracket. So gate entry on round-1
        // presence only — a broader window would drop real entrants and break the
        // seed set (and hence the knockout check).
        let eligible = cup_eligibility(&tournament, &nations, 1);
        if eligible.len() < size as usize {
            continue;
        }

        // Seed the top `size` eligible exactly as osp-core would (finalize assigns
        // rating-descending tournament numbers, then seeds the top `size`).
        let mut sim = tournament.clone();
        sim.rounds.clear();
        sim.cup = None;
        sim.registration_finalized = false;
        sim.settings.cup_enabled = true;
        for p in &mut sim.players {
            p.tournament_id = None;
            p.eligible = eligible.contains(&p.id);
        }
        if sim.finalize_registration_with(Some(size)).is_err() {
            continue;
        }
        let seeds = match sim.cup {
            Some(c) => c.seed_order,
            None => continue,
        };

        if knockout_champion(&tournament.rounds, &seeds).is_some() {
            emit(size, &nations);
            return ExitCode::SUCCESS;
        }
    }

    ExitCode::SUCCESS // no cup detected
}

/// Print the detection as `SIZE:NAT[,NAT...]`, nationalities sorted for a stable,
/// deduplicated `--cup-nations` list.
fn emit(size: u32, nations: &HashSet<String>) {
    let mut nats: Vec<&str> = nations.iter().map(String::as_str).collect();
    nats.sort_unstable();
    println!("{size}:{}", nats.join(","));
}
