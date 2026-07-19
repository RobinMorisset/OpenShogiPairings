//! `mm-grades` — generate MacMahon-threshold configs for a result file so that
//! every MacMahon group holds at least `N` players and every threshold sits on a
//! FESA grade boundary.
//!
//! Usage: `mm-grades <result-file> <N> [out-dir]`
//!
//! It parses the FESA result table (reusing the same importer as `osp-sim`, so
//! all the annotation / unrated quirks are handled identically), then walks the
//! grade boundaries from weakest to strongest, cutting a new group as soon as the
//! open segment holds `N` players *and* at least `N` players remain above. The
//! result is the finest grouping whose every band still meets the `N` floor.
//!
//! Thresholds are emitted as ELO criteria whose value is exactly a grade
//! boundary: that keeps the grouping deterministic by rating (every rated player
//! has a rating; not every player has a parsed grade) while still "falling on a
//! grade boundary" as requested.
//!
//! When at least one threshold is selected, it writes settings JSON files (all
//! sharing those thresholds), ready to pass to `osp-sim --configs`:
//!   `a` — the thresholds alone (static MacMahon);
//!   `b` — plus MacMahon-from-estimated-ELO with the rated/provisional K
//!         multiplier pinned to 0 and a Flat prior for unrated players;
//!   `c` — same as `b` but the unrated prior is an asymmetric Huber-Laplace
//!         (centre 700, k 260, upward-looseness 300%);
//!   `d` — same as `b` but rated/provisional players are unpinned (default K
//!         multiplier / default Gaussian prior), so results can move them.
//! If no thresholds are selected (e.g. `N` larger than the field allows), it
//! writes nothing.
//!
//! Configs use the nested `PairingMode` layout `TournamentSettings` deserializes
//! (`pairing.macmahon.{thresholds,source}`); a flat top-level layout is silently
//! ignored, since unknown keys are dropped.

use std::path::Path;
use std::process::ExitCode;

use osp_core::{decode_latin1, import_fesa_results};

/// Lower-bound rating of each FESA grade, weakest (20 Kyu) to strongest (5 Dan).
/// Mirrors `GRADE_LB` in `crates/core/src/elo.rs` — keep the two in sync.
const GRADE_LB: [u32; 25] = [
    1, // 20 Kyu
    80, 160, 240, 320, 400, 480, 560, 640, 720, 800, // 19..10 Kyu
    880, 960, 1040, 1120, 1200, 1280, 1360, 1460, 1560, // 9..1 Kyu
    1680, 1800, 1920, 2080, 2240, // 1..5 Dan
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        eprintln!(
            "usage: {} <result-file> <N> [out-dir]",
            args.first().map(String::as_str).unwrap_or("mm-grades")
        );
        eprintln!("  write MacMahon-threshold configs (on grade boundaries) so every group holds >= N players");
        return ExitCode::from(2);
    }
    let path = &args[1];
    let n: usize = match args[2].parse() {
        Ok(v) if v >= 1 => v,
        _ => {
            eprintln!("N must be a positive integer, got {:?}", args[2]);
            return ExitCode::from(2);
        }
    };
    let out_dir = args.get(3).map(String::as_str).unwrap_or(".");

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

    // Collect ratings. Unrated players meet no ELO threshold, so they live in the
    // bottom band: count them as strictly below every candidate boundary.
    let mut rated: Vec<u32> = tournament.players.iter().filter_map(|p| p.rating).collect();
    rated.sort_unstable();
    let unrated = tournament.players.len() - rated.len();
    let total = tournament.players.len();

    // Players strictly below boundary `b` (rating < b). Unrated count for every b.
    let below = |b: u32| unrated + rated.partition_point(|&r| r < b);

    // Greedy left-to-right: cut at the first boundary where the open segment has
    // >= N and >= N players still remain above.
    let mut chosen: Vec<u32> = Vec::new();
    let mut last_below = 0usize;
    for &b in &GRADE_LB {
        let bel = below(b);
        let seg = bel - last_below;
        if seg >= n && total.saturating_sub(bel) >= n {
            chosen.push(b);
            last_below = bel;
        }
    }

    // Human-readable summary of the resulting bands.
    eprintln!(
        "{total} players ({} rated, {unrated} unrated); N = {n}; {} threshold(s) -> {} group(s):",
        rated.len(),
        chosen.len(),
        chosen.len() + 1
    );
    let mut prev_below = 0usize;
    let mut edges: Vec<Option<u32>> = vec![None];
    edges.extend(chosen.iter().map(|&b| Some(b)));
    edges.push(None);
    for w in edges.windows(2) {
        let lo = w[0];
        let hi = w[1];
        let upto = hi.map(&below).unwrap_or(total);
        let size = upto - prev_below;
        prev_below = upto;
        let lo_s = lo
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unrated".into());
        let hi_s = hi
            .map(|v| format!("< {v}"))
            .unwrap_or_else(|| "and up".into());
        eprintln!("  [{lo_s} .. {hi_s}]: {size} players");
    }

    if chosen.is_empty() {
        eprintln!("no thresholds selected (field too small for N = {n}); nothing written.");
        return ExitCode::SUCCESS;
    }

    // Each threshold as an ELO criterion on the boundary; matches the serde shape
    // of `MacMahonThreshold` (drops_after_round omitted == never degresses).
    let thresholds: Vec<serde_json::Value> = chosen
        .iter()
        .map(|&b| serde_json::json!({ "criterion": { "kind": "elo", "value": b } }))
        .collect();

    // The configs, all sharing the same thresholds, in the **nested**
    // `PairingMode` shape `osp-sim` deserializes (Swiss > macmahon > {thresholds,
    // source}). A flat top-level layout (`macmahon_thresholds`, `elo_*`, …) is
    // silently dropped — those were top-level fields before the PairingMode enum
    // refactor, and `TournamentSettings` ignores unknown keys.
    let with_source = |source: serde_json::Value| {
        serde_json::json!({
            "pairing": { "kind": "swiss", "macmahon": {
                "thresholds": thresholds.clone(),
                "source": source,
            } }
        })
    };
    // `a` — static MacMahon (thresholds alone).
    let cfg_a = with_source(serde_json::json!({ "kind": "static" }));
    // `b` — MacMahon from the live estimate; k = 0 pins every rated *and*
    // provisional player (estimating unrated only), unrated on a Flat prior.
    let cfg_b = with_source(serde_json::json!({
        "kind": "from_estimate",
        "estimator": { "k_multiplier": 0, "prior_shape_unrated": "flat" },
    }));
    // `c` — like `b` but the unrated prior is the tuned asymmetric Huber-Laplace
    // (the settings-tab "Tuned Laplace": centre 700, k 260, upward looseness 300%).
    let cfg_c = with_source(serde_json::json!({
        "kind": "from_estimate",
        "estimator": {
            "k_multiplier": 0,
            "prior_shape_unrated": "laplace",
            "unrated_prior_center": 700,
            "unrated_k": 260,
            "upward_looseness_unrated": 300,
        },
    }));
    // `d` — like `b` but rated/provisional players are *not* pinned: the default K
    // multiplier lets their estimated ELO follow the default (Gaussian) prior
    // around their rating, so results can move them. Unrated still use Flat.
    let cfg_d = with_source(serde_json::json!({
        "kind": "from_estimate",
        "estimator": { "prior_shape_unrated": "flat" },
    }));

    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("results");
    let outputs = [
        (format!("{stem}.mm{n}-a.json"), &cfg_a),
        (format!("{stem}.mm{n}-b.json"), &cfg_b),
        (format!("{stem}.mm{n}-c.json"), &cfg_c),
        (format!("{stem}.mm{n}-d.json"), &cfg_d),
    ];

    eprintln!("wrote {} configs to {out_dir}/:", outputs.len());
    let mut written: Vec<String> = Vec::with_capacity(outputs.len());
    for (name, cfg) in &outputs {
        let full = Path::new(out_dir).join(name);
        let text = serde_json::to_string_pretty(cfg).unwrap();
        if let Err(e) = std::fs::write(&full, text + "\n") {
            eprintln!("writing {}: {e}", full.display());
            return ExitCode::from(1);
        }
        eprintln!("  {}", full.display());
        written.push(full.display().to_string());
    }
    eprintln!(
        "run: osp-sim --results {path:?} --configs {}",
        written.join(" ")
    );

    ExitCode::SUCCESS
}
