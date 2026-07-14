//! `cup-detect` — detect whether a FESA result file was actually run as a
//! direct-elimination cup among the top 2^N players of its most common
//! nationality.
//!
//! Usage: `cup-detect <result-file>`
//!
//! Prints `SIZE,NAT` (e.g. `16,FR`) to stdout when the real pairings form a valid
//! single-elimination bracket among the top SIZE rating-seeded players of the most
//! common nationality NAT — SIZE in 8/16/32/64, the largest that matches. Prints
//! nothing (exit 0) when no cup is detected; parse/read errors go to stderr with a
//! non-zero exit.
//!
//! The check reuses osp-core's exact cup seeding (rating-descending, unrated last)
//! and bracket replay: `Cup::podium` returns a champion only when every scheduled
//! bracket match was actually played between the two seeded players, so a positive
//! match is precisely a tournament osp-sim can reproduce as that cup. National
//! championships are usually run this way among the host nation (the most common
//! nationality); multi-nation cups like WOSC (top-32 European) are out of scope
//! here and handled separately by name.
//!
//! Limitation: bracket rounds played as "long" games span two tournament rounds,
//! but a FESA result table carries no long-game flag, so a long-format cup may not
//! be detected. Standard (one-round-per-bracket-round) cups detect fine.

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

use osp_core::sim::cup_eligibility;
use osp_core::{decode_latin1, import_fesa_results, Cup, CUP_SIZES};

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

    // Most common (non-empty) nationality; ties broken alphabetically for
    // determinism.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in &tournament.players {
        if let Some(n) = p.nationality.as_deref() {
            if !n.is_empty() {
                *counts.entry(n.to_uppercase()).or_default() += 1;
            }
        }
    }
    let nat = match counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
    {
        Some((n, _)) => n,
        None => return ExitCode::SUCCESS, // no nationalities -> no cup
    };
    let nations: HashSet<String> = std::iter::once(nat.clone()).collect();

    // The largest bracket size the real pairings actually realise.
    let mut sizes = CUP_SIZES.to_vec();
    sizes.sort_unstable_by(|a, b| b.cmp(a)); // descending
    for size in sizes {
        let cup_rounds = size.trailing_zeros() as usize;
        if tournament.rounds.len() < cup_rounds {
            continue;
        }
        let eligible = cup_eligibility(&tournament, &nations, cup_rounds);
        if eligible.len() < size as usize {
            continue;
        }

        // Seed the cup exactly as osp-core would (finalize assigns rating-descending
        // tournament numbers, then seeds the top `size` eligible players).
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
        let cup: Cup = match sim.cup {
            Some(c) => c,
            None => continue,
        };

        // A champion means every bracket match was found and decided against the
        // real rounds — i.e. the field played this exact seeded knockout.
        if let Some(podium) = cup.podium(&tournament.rounds) {
            if podium.champion.is_some() {
                println!("{size},{nat}");
                return ExitCode::SUCCESS;
            }
        }
    }

    ExitCode::SUCCESS // no cup detected
}
