//! `osp-sim` — Monte-Carlo comparison of pairing-settings variants.
//!
//! Loads a base tournament (an `.osp` save or an American Grid), then for each
//! settings variant runs many simulated tournaments and reports how the variants
//! differ on game mismatch (are there fewer foregone-conclusion games?) and on
//! who tends to win / how faithfully the final ranking tracks true strength. See
//! `docs/simulation-cli.md` for the design.

mod fesa;
mod stats;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Parser;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;
use serde::Serialize;
use uuid::Uuid;

use osp_core::sim::{
    cup_eligibility, game_elo_diffs, sample_strengths, simulate_run, CupConfig, RunOutcome,
    StrengthMap,
};
use osp_core::{
    decode_latin1, estimate_elos, import_fesa_results, Player, Tournament, TournamentSettings,
};

use stats::{diff_stats, mean, spearman, wilson, DiffStats, Proportion};

/// Compare pairing-settings variants by simulating a tournament many times.
#[derive(Debug, Parser)]
#[command(name = "osp-sim", version)]
struct Args {
    /// Base tournament as an `.osp` save file (JSON). One base source is required.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["grid", "results"])]
    base: Option<PathBuf>,

    /// Base tournament as an American Grid text export.
    #[arg(long, value_name = "FILE", conflicts_with = "results")]
    grid: Option<PathBuf>,

    /// Base tournament as a FESA post-tournament result table. Also supplies each
    /// player's true strength (pre-ELO + points gained, or the assigned rating for
    /// a pre-unrated player), so no separate --strength* is needed.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["strength", "strength_fesa_after", "strength_fesa_list"])]
    results: Option<PathBuf>,

    /// One or more settings variants (each a full TournamentSettings JSON). If
    /// omitted, the base tournament's own settings are the single variant.
    #[arg(long = "configs", value_name = "FILE", num_args = 1..)]
    configs: Vec<PathBuf>,

    /// Number of simulated tournaments per variant.
    #[arg(long, default_value_t = 1000)]
    runs: u64,

    /// Rounds per simulated tournament. Defaults to the base's round count.
    #[arg(long)]
    rounds: Option<u32>,

    /// Master seed. Run i uses seed+i, identical across variants (common random
    /// numbers), so a variant's effect is measured on the same simulated worlds.
    #[arg(long, default_value_t = 0)]
    seed: u64,

    /// Optional per-player true-strength overrides: a JSON object mapping
    /// tournament number (as a string) to an ELO. Overridden players are treated
    /// as known truth (never jittered).
    #[arg(long, value_name = "FILE")]
    strength: Option<PathBuf>,

    /// Use the first FESA rating list published after this tournament date
    /// (YYYY-MM-DD) as each player's true strength — matched by name, fetched from
    /// fesashogi.eu. The post-tournament list already reflects the results.
    #[arg(long, value_name = "YYYY-MM-DD", conflicts_with_all = ["strength", "strength_fesa_list"])]
    strength_fesa_after: Option<String>,

    /// Use a specific FESA rating list (https URL or local file path) as true
    /// strength, matched by name.
    #[arg(long, value_name = "URL|PATH", conflicts_with = "strength")]
    strength_fesa_list: Option<String>,

    /// Multiplier on each (non-overridden) player's prior width: 0 pins true
    /// strength to the rating, 1 samples from the estimator's own prior.
    #[arg(long, default_value_t = 0.0)]
    jitter: f64,

    /// Run the hybrid direct-elimination cup with this bracket size (8/16/32/64).
    /// Requires --cup-nations.
    #[arg(long, value_name = "N", requires = "cup_nations")]
    cup_size: Option<u32>,

    /// Comma-separated nationalities eligible for the cup (e.g. FR,BE,CH). A player
    /// absent in any of the cup rounds (the first log2(size) real rounds) is
    /// excluded.
    #[arg(
        long,
        value_name = "CODES",
        value_delimiter = ',',
        requires = "cup_size"
    )]
    cup_nations: Vec<String>,

    /// One or more |ELO diff| thresholds T for reporting P(|diff| > T).
    #[arg(long = "threshold", value_name = "T", default_values_t = [400.0_f64])]
    thresholds: Vec<f64>,

    /// Optional output directory for report.json and per-variant CSV histograms.
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let (base, results_strengths) = load_base(&args)?;
    let rounds = args.rounds.unwrap_or(base.rounds.len() as u32);
    if rounds == 0 {
        return Err("no rounds to simulate: pass --rounds (the base has none)".into());
    }

    let overrides = if let Some(strengths) = results_strengths {
        eprintln!(
            "true strength from the results table — {}/{} players (elo + points won)",
            strengths.len(),
            base.players.len()
        );
        strengths
    } else if let Some(date) = &args.strength_fesa_after {
        let (map, url, matched) = fesa::overrides_after(date, &base)?;
        eprintln!(
            "true strength from FESA list {url} — matched {matched}/{} players (unmatched keep their grid rating)",
            base.players.len()
        );
        map
    } else if let Some(source) = &args.strength_fesa_list {
        let (map, matched) = fesa::overrides_from_list(source, &base)?;
        eprintln!(
            "true strength from FESA list {source} — matched {matched}/{} players (unmatched keep their grid rating)",
            base.players.len()
        );
        map
    } else if let Some(path) = &args.strength {
        load_overrides(path, &base)?
    } else {
        StrengthMap::new()
    };

    // Cup format: compute eligibility (nationality + attendance) from the base's
    // real rounds, once, and apply it across all variants.
    let cup = if let Some(size) = args.cup_size {
        if !osp_core::CUP_SIZES.contains(&size) {
            return Err(format!(
                "--cup-size must be one of {:?}",
                osp_core::CUP_SIZES
            ));
        }
        let nations: HashSet<String> = args
            .cup_nations
            .iter()
            .map(|n| n.trim().to_uppercase())
            .filter(|n| !n.is_empty())
            .collect();
        // A cup player must have played every cup round — the first log2(size).
        let cup_rounds = size.trailing_zeros() as usize;
        let eligible = cup_eligibility(&base, &nations, cup_rounds);
        eprintln!(
            "cup: size {size}, {} eligible ({} nationalities, present through round {cup_rounds})",
            eligible.len(),
            nations.len(),
        );
        Some(CupConfig { eligible, size })
    } else {
        None
    };

    // Resolve the variants: named settings files, or the base's own settings.
    let variants: Vec<(String, TournamentSettings)> = if args.configs.is_empty() {
        vec![("base".to_string(), base.settings.clone())]
    } else {
        args.configs
            .iter()
            .map(|p| load_settings(p).map(|s| (stem(p), s)))
            .collect::<Result<_, _>>()?
    };

    let names = player_names(&base);
    let observed = observed_report(&base, &overrides, &args.thresholds);
    let mut reports = Vec::new();
    for (name, settings) in &variants {
        let outcomes = simulate_variant(
            &base,
            settings,
            &overrides,
            args.jitter,
            rounds,
            cup.as_ref(),
            args.runs,
            args.seed,
        )
        .map_err(|e| format!("variant '{name}': {e}"))?;
        reports.push(aggregate(name.clone(), &outcomes, &args.thresholds));
    }

    print_report(
        &reports,
        observed.as_ref(),
        &names,
        rounds,
        args.runs,
        args.seed,
        args.jitter,
    );

    if let Some(dir) = &args.out {
        write_outputs(
            dir,
            &reports,
            observed.as_ref(),
            &base,
            args.jitter,
            args.seed,
            rounds,
            args.runs,
        )
        .map_err(|e| format!("writing outputs: {e}"))?;
        println!(
            "\nwrote report.json and per-variant CSV histograms to {}",
            dir.display()
        );
    }
    Ok(())
}

// --- loading ---------------------------------------------------------------

/// Load the base tournament. A `--results` table also yields each player's true
/// strength (the second element); the other sources return `None` for it.
fn load_base(args: &Args) -> Result<(Tournament, Option<StrengthMap>), String> {
    match (&args.base, &args.grid, &args.results) {
        (Some(path), None, None) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            let t = serde_json::from_str(&text)
                .map_err(|e| format!("parsing {} as a tournament: {e}", path.display()))?;
            Ok((t, None))
        }
        (None, Some(path), None) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            let t = osp_core::import_american_grid(&text)
                .map_err(|e| format!("parsing {} as an American Grid: {e}", path.display()))?;
            Ok((t, None))
        }
        (None, None, Some(path)) => {
            // FESA result tables are Latin-1, like the rating lists.
            let bytes =
                std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
            let (t, strengths) = import_fesa_results(&decode_latin1(&bytes))
                .map_err(|e| format!("parsing {} as a FESA result table: {e}", path.display()))?;
            Ok((t, Some(strengths)))
        }
        _ => Err("provide exactly one of --base, --grid, or --results".into()),
    }
}

fn load_settings(path: &Path) -> Result<TournamentSettings, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parsing {} as settings: {e}", path.display()))
}

/// Parse `{ "<tournament_number>": <elo>, ... }` and resolve to player ids.
fn load_overrides(path: &Path, base: &Tournament) -> Result<StrengthMap, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let by_number: HashMap<String, f64> = serde_json::from_str(&text)
        .map_err(|e| format!("parsing {} as a strength map: {e}", path.display()))?;
    let id_of: HashMap<u32, Uuid> = base
        .players
        .iter()
        .filter_map(|p| p.tournament_id.map(|n| (n, p.id)))
        .collect();
    let mut map = StrengthMap::new();
    for (key, elo) in by_number {
        let number: u32 = key
            .parse()
            .map_err(|_| format!("strength key '{key}' is not a tournament number"))?;
        match id_of.get(&number) {
            Some(&id) => {
                map.insert(id, elo);
            }
            None => {
                eprintln!("warning: strength override for #{number} matches no player; ignored")
            }
        }
    }
    Ok(map)
}

// --- simulation ------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn simulate_variant(
    base: &Tournament,
    settings: &TournamentSettings,
    overrides: &StrengthMap,
    jitter: f64,
    rounds: u32,
    cup: Option<&CupConfig>,
    runs: u64,
    seed: u64,
) -> Result<Vec<RunOutcome>, String> {
    (0..runs)
        .into_par_iter()
        .map(|i| {
            // Run i uses `seed + i`, so `--seed` actually changes the simulated
            // worlds (not just the printed label), while every variant still sees
            // the *same* seed per run — common random numbers, for a fair A/B.
            let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_add(i));
            simulate_run(base, settings, overrides, jitter, rounds, cup, &mut rng)
                .map_err(|e| e.to_string())
        })
        .collect()
}

// --- aggregation -----------------------------------------------------------

/// Aggregated metrics for one variant.
struct VariantReport {
    name: String,
    #[allow(dead_code)]
    runs: usize,
    diff: DiffStats,
    /// Pooled absolute ELO diffs across all runs, sorted — retained for the CSV
    /// histogram so outputs need no re-simulation.
    pooled: Vec<f64>,
    /// Mean Spearman(final standings, true-strength order) over runs.
    fidelity_score: f64,
    /// Mean Spearman(ELO-estimate order, true-strength order) over runs.
    fidelity_estimate: f64,
    /// Per-player top-1 probability (with CI) and top-3 rate, sorted best first.
    players: Vec<PlayerProb>,
}

struct PlayerProb {
    player: Uuid,
    top1: Proportion,
    top3: f64,
}

/// Strength order (strongest first) for a strength map.
fn strength_order(strengths: &StrengthMap) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = strengths.keys().copied().collect();
    ids.sort_by(|a, b| {
        strengths[b]
            .partial_cmp(&strengths[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ids
}

/// True-strength order (strongest first) for one run.
fn true_order(out: &RunOutcome) -> Vec<Uuid> {
    strength_order(&out.strengths)
}

/// Players ordered by descending ELO estimate, ties broken by tournament number.
fn estimate_order(players: &[Player], estimate: &HashMap<Uuid, f64>) -> Vec<Uuid> {
    let mut ids: Vec<&Player> = players.iter().collect();
    ids.sort_by(|a, b| {
        let ea = estimate.get(&a.id).copied().unwrap_or(f64::MIN);
        let eb = estimate.get(&b.id).copied().unwrap_or(f64::MIN);
        eb.partial_cmp(&ea)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tournament_id.cmp(&b.tournament_id))
    });
    ids.into_iter().map(|p| p.id).collect()
}

fn aggregate(name: String, outcomes: &[RunOutcome], thresholds: &[f64]) -> VariantReport {
    let runs = outcomes.len();

    let mut pooled: Vec<f64> = outcomes
        .iter()
        .flat_map(|o| o.game_diffs.iter().copied())
        .collect();
    let diff = diff_stats(&pooled, thresholds);
    pooled.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let score_rhos: Vec<f64> = outcomes
        .iter()
        .map(|o| spearman(&o.final_order, &true_order(o)))
        .collect();
    let est_rhos: Vec<f64> = outcomes
        .iter()
        .map(|o| spearman(&o.estimated_order, &true_order(o)))
        .collect();

    // Victory counts.
    let mut top1: HashMap<Uuid, usize> = HashMap::new();
    let mut top3: HashMap<Uuid, usize> = HashMap::new();
    for o in outcomes {
        if let Some(&w) = o.final_order.first() {
            *top1.entry(w).or_default() += 1;
        }
        for &id in o.final_order.iter().take(3) {
            *top3.entry(id).or_default() += 1;
        }
    }
    let mut players: Vec<PlayerProb> = top1
        .keys()
        .chain(top3.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .map(|id| PlayerProb {
            player: id,
            top1: wilson(top1.get(&id).copied().unwrap_or(0), runs),
            top3: top3.get(&id).copied().unwrap_or(0) as f64 / runs.max(1) as f64,
        })
        .collect();
    players.sort_by(|a, b| {
        b.top1
            .p
            .partial_cmp(&a.top1.p)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    VariantReport {
        name,
        runs,
        diff,
        pooled,
        fidelity_score: mean(&score_rhos),
        fidelity_estimate: mean(&est_rhos),
        players,
    }
}

/// Metrics from the base tournament's *actual* played rounds — a real-world
/// yardstick beside the simulated variants (design §3.3). Strengths are nominal
/// (override else registration rating, no jitter), since a real event is a single
/// realisation with no sampling.
struct ObservedReport {
    rounds: u32,
    diff: DiffStats,
    pooled: Vec<f64>,
    fidelity_score: f64,
    fidelity_estimate: f64,
    /// Finishing rank (0-based) of each player in the real standings.
    rank_of: HashMap<Uuid, usize>,
    winner: Option<Uuid>,
}

/// Build the observed report from the base's real results, or `None` if the base
/// has no played games (e.g. a synthetic, never-played base).
fn observed_report(
    base: &Tournament,
    overrides: &StrengthMap,
    thresholds: &[f64],
) -> Option<ObservedReport> {
    let played = base
        .rounds
        .iter()
        .flat_map(|r| &r.boards)
        .any(|b| b.result.is_some());
    if !played {
        return None;
    }

    // Nominal strengths: jitter 0 → override else registration rating. The rng is
    // unused at jitter 0 but the signature needs one.
    let mut rng = ChaCha8Rng::seed_from_u64(0);
    let strengths = sample_strengths(&base.players, &base.settings, overrides, 0.0, &mut rng);

    let mut pooled = game_elo_diffs(base, &strengths);
    let diff = diff_stats(&pooled, thresholds);
    pooled.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let final_order: Vec<Uuid> = base.standings().into_iter().map(|s| s.player_id).collect();
    let true_ord = strength_order(&strengths);
    let estimate = estimate_elos(&base.players, &base.settings, &base.rounds);
    let est_ord = estimate_order(&base.players, &estimate);

    Some(ObservedReport {
        rounds: base.rounds.len() as u32,
        diff,
        pooled,
        fidelity_score: spearman(&final_order, &true_ord),
        fidelity_estimate: spearman(&est_ord, &true_ord),
        rank_of: final_order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect(),
        winner: final_order.first().copied(),
    })
}

// --- output ----------------------------------------------------------------

fn player_names(base: &Tournament) -> HashMap<Uuid, String> {
    base.players
        .iter()
        .map(|p| {
            let label = match p.tournament_id {
                Some(n) => format!("#{n} {}", p.last_name),
                None => p.last_name.clone(),
            };
            (p.id, label)
        })
        .collect()
}

fn print_report(
    reports: &[VariantReport],
    observed: Option<&ObservedReport>,
    names: &HashMap<Uuid, String>,
    rounds: u32,
    runs: u64,
    seed: u64,
    jitter: f64,
) {
    println!("osp-sim: {runs} runs × {rounds} rounds, seed {seed}, jitter {jitter}");
    println!("(no draws modelled; byes excluded from game stats)\n");

    // Row formatter for the diff table, shared by variants and the observed line.
    let diff_row = |label: &str, d: &DiffStats| {
        let exceed: Vec<String> = d
            .exceed
            .iter()
            .map(|(t, p)| format!("{}:{:.1}%", *t as i64, p * 100.0))
            .collect();
        println!(
            "{:<14} {:>7.0} {:>7.0} {:>7.0} {:>7.0}   {}",
            trunc(label, 14),
            d.mean,
            d.median,
            d.p90,
            d.p95,
            exceed.join("  "),
        );
    };

    // Headline table: one row per variant, then the observed reality.
    println!(
        "{:<14} {:>7} {:>7} {:>7} {:>7}   P(|d|>T)",
        "variant", "mean|d|", "median", "p90", "p95"
    );
    for r in reports {
        diff_row(&r.name, &r.diff);
    }
    if let Some(o) = observed {
        diff_row("observed*", &o.diff);
    }

    println!(
        "\n{:<14} {:>16} {:>16}",
        "variant", "fidelity(score)", "fidelity(est)"
    );
    for r in reports {
        println!(
            "{:<14} {:>16.3} {:>16.3}",
            trunc(&r.name, 14),
            r.fidelity_score,
            r.fidelity_estimate
        );
    }
    if let Some(o) = observed {
        println!(
            "{:<14} {:>16.3} {:>16.3}",
            "observed*", o.fidelity_score, o.fidelity_estimate
        );
    }

    // Victory probabilities: union of the top few players across variants, with an
    // extra column for each player's real finishing rank.
    let mut shown: Vec<Uuid> = Vec::new();
    for r in reports {
        for pp in r.players.iter().take(6) {
            if !shown.contains(&pp.player) {
                shown.push(pp.player);
            }
        }
    }
    println!(
        "\nvictory probability (top-1, 95% CI) — showing {} players",
        shown.len()
    );
    print!("{:<20}", "player");
    for r in reports {
        print!("  {:>22}", trunc(&r.name, 22));
    }
    if observed.is_some() {
        print!("  {:>8}", "obs.rank");
    }
    println!();
    for id in &shown {
        print!(
            "{:<20}",
            trunc(names.get(id).map(String::as_str).unwrap_or("?"), 20)
        );
        for r in reports {
            match r.players.iter().find(|pp| pp.player == *id) {
                Some(pp) => print!(
                    "  {:>6.1}% [{:>4.1},{:>4.1}]",
                    pp.top1.p * 100.0,
                    pp.top1.lo * 100.0,
                    pp.top1.hi * 100.0
                ),
                None => print!("  {:>22}", "-"),
            }
        }
        if let Some(o) = observed {
            let rank = o
                .rank_of
                .get(id)
                .map(|r| format!("#{}", r + 1))
                .unwrap_or_else(|| "-".into());
            print!("  {rank:>8}");
        }
        println!();
    }

    if let Some(o) = observed {
        let winner = o
            .winner
            .and_then(|id| names.get(&id))
            .map(String::as_str)
            .unwrap_or("?");
        println!(
            "\n* observed: the base's real {}-round result; winner {winner}.",
            o.rounds
        );
        if jitter > 0.0 {
            println!(
                "  (observed uses registration ratings; simulated diffs use jittered strengths — compare loosely)"
            );
        }
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

// --- machine-readable outputs ----------------------------------------------

#[derive(Serialize)]
struct JsonReport {
    runs: u64,
    rounds: u32,
    seed: u64,
    jitter: f64,
    variants: Vec<JsonVariant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed: Option<JsonObserved>,
}

#[derive(Serialize)]
struct JsonObserved {
    rounds: u32,
    games: usize,
    mean_diff: f64,
    median_diff: f64,
    p90_diff: f64,
    p95_diff: f64,
    exceed: Vec<JsonExceed>,
    fidelity_score: f64,
    fidelity_estimate: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    winner: Option<String>,
}

#[derive(Serialize)]
struct JsonVariant {
    name: String,
    games: usize,
    mean_diff: f64,
    median_diff: f64,
    p90_diff: f64,
    p95_diff: f64,
    exceed: Vec<JsonExceed>,
    fidelity_score: f64,
    fidelity_estimate: f64,
    victory: Vec<JsonVictory>,
}

#[derive(Serialize)]
struct JsonExceed {
    threshold: f64,
    fraction: f64,
}

#[derive(Serialize)]
struct JsonVictory {
    player: String,
    top1: f64,
    top1_lo: f64,
    top1_hi: f64,
    top3: f64,
}

#[allow(clippy::too_many_arguments)]
fn write_outputs(
    dir: &Path,
    reports: &[VariantReport],
    observed: Option<&ObservedReport>,
    base: &Tournament,
    jitter: f64,
    seed: u64,
    rounds: u32,
    runs: u64,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let names = player_names(base);

    let json = JsonReport {
        runs,
        rounds,
        seed,
        jitter,
        observed: observed.map(|o| JsonObserved {
            rounds: o.rounds,
            games: o.diff.count,
            mean_diff: o.diff.mean,
            median_diff: o.diff.median,
            p90_diff: o.diff.p90,
            p95_diff: o.diff.p95,
            exceed: o
                .diff
                .exceed
                .iter()
                .map(|(t, f)| JsonExceed {
                    threshold: *t,
                    fraction: *f,
                })
                .collect(),
            fidelity_score: o.fidelity_score,
            fidelity_estimate: o.fidelity_estimate,
            winner: o.winner.and_then(|id| names.get(&id).cloned()),
        }),
        variants: reports
            .iter()
            .map(|r| JsonVariant {
                name: r.name.clone(),
                games: r.diff.count,
                mean_diff: r.diff.mean,
                median_diff: r.diff.median,
                p90_diff: r.diff.p90,
                p95_diff: r.diff.p95,
                exceed: r
                    .diff
                    .exceed
                    .iter()
                    .map(|(t, f)| JsonExceed {
                        threshold: *t,
                        fraction: *f,
                    })
                    .collect(),
                fidelity_score: r.fidelity_score,
                fidelity_estimate: r.fidelity_estimate,
                victory: r
                    .players
                    .iter()
                    .map(|pp| JsonVictory {
                        player: names
                            .get(&pp.player)
                            .cloned()
                            .unwrap_or_else(|| pp.player.to_string()),
                        top1: pp.top1.p,
                        top1_lo: pp.top1.lo,
                        top1_hi: pp.top1.hi,
                        top3: pp.top3,
                    })
                    .collect(),
            })
            .collect(),
    };
    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )?;

    // One CSV histogram (50-point bins) of the pooled |diff| per variant (and the
    // observed distribution), from the diffs retained at aggregation — no re-sim.
    let bin = 50.0;
    let write_hist = |name: &str, pooled: &[f64]| -> std::io::Result<()> {
        let mut csv = String::from("bin_lo,bin_hi,count\n");
        let max = pooled.last().copied().unwrap_or(0.0);
        let nbins = (max / bin).ceil() as usize + 1;
        let mut counts = vec![0usize; nbins];
        for d in pooled {
            let b = (*d / bin) as usize;
            counts[b.min(nbins - 1)] += 1;
        }
        for (i, c) in counts.iter().enumerate() {
            csv.push_str(&format!(
                "{},{},{}\n",
                i as f64 * bin,
                (i + 1) as f64 * bin,
                c
            ));
        }
        std::fs::write(dir.join(format!("elo-diff-{}.csv", sanitize(name))), csv)
    };
    for r in reports {
        write_hist(&r.name, &r.pooled)?;
    }
    if let Some(o) = observed {
        write_hist("observed", &o.pooled)?;
    }
    Ok(())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("config")
        .to_string()
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
