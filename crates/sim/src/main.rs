//! `osp-sim` — Monte-Carlo comparison of pairing-settings variants.
//!
//! Loads a base tournament (an `.osp` save or an American Grid), then for each
//! settings variant runs many simulated tournaments and reports how the variants
//! differ on game mismatch (are there fewer foregone-conclusion games?) and on
//! who tends to win / how faithfully the final ranking tracks true strength. See
//! `docs/simulation-cli.md` for the design.

mod stats;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::Parser;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;
use serde::Serialize;
use uuid::Uuid;

use osp_core::sim::{simulate_run, RunOutcome, StrengthMap};
use osp_core::{Tournament, TournamentSettings};

use stats::{diff_stats, mean, spearman, wilson, DiffStats, Proportion};

/// Compare pairing-settings variants by simulating a tournament many times.
#[derive(Debug, Parser)]
#[command(name = "osp-sim", version)]
struct Args {
    /// Base tournament as an `.osp` save file (JSON). Mutually exclusive with --grid.
    #[arg(long, value_name = "FILE", conflicts_with = "grid")]
    base: Option<PathBuf>,

    /// Base tournament as an American Grid text export. Mutually exclusive with --base.
    #[arg(long, value_name = "FILE")]
    grid: Option<PathBuf>,

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

    /// Multiplier on each (non-overridden) player's prior width: 0 pins true
    /// strength to the rating, 1 samples from the estimator's own prior.
    #[arg(long, default_value_t = 0.0)]
    jitter: f64,

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
    let base = load_base(&args)?;
    let rounds = args.rounds.unwrap_or(base.rounds.len() as u32);
    if rounds == 0 {
        return Err("no rounds to simulate: pass --rounds (the base has none)".into());
    }

    let overrides = match &args.strength {
        Some(path) => load_overrides(path, &base)?,
        None => StrengthMap::new(),
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
    let mut reports = Vec::new();
    for (name, settings) in &variants {
        let outcomes =
            simulate_variant(&base, settings, &overrides, args.jitter, rounds, args.runs)
                .map_err(|e| format!("variant '{name}': {e}"))?;
        reports.push(aggregate(name.clone(), &outcomes, &args.thresholds));
    }

    print_report(&reports, &names, rounds, args.runs, args.seed, args.jitter);

    if let Some(dir) = &args.out {
        write_outputs(
            dir,
            &reports,
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

fn load_base(args: &Args) -> Result<Tournament, String> {
    match (&args.base, &args.grid) {
        (Some(path), None) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            serde_json::from_str(&text)
                .map_err(|e| format!("parsing {} as a tournament: {e}", path.display()))
        }
        (None, Some(path)) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            osp_core::import_american_grid(&text)
                .map_err(|e| format!("parsing {} as an American Grid: {e}", path.display()))
        }
        _ => Err("provide exactly one of --base or --grid".into()),
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

fn simulate_variant(
    base: &Tournament,
    settings: &TournamentSettings,
    overrides: &StrengthMap,
    jitter: f64,
    rounds: u32,
    runs: u64,
) -> Result<Vec<RunOutcome>, String> {
    (0..runs)
        .into_par_iter()
        .map(|i| {
            let mut rng = ChaCha8Rng::seed_from_u64(i);
            simulate_run(base, settings, overrides, jitter, rounds, &mut rng)
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

/// True-strength order (strongest first) for one run.
fn true_order(out: &RunOutcome) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = out.strengths.keys().copied().collect();
    ids.sort_by(|a, b| {
        out.strengths[b]
            .partial_cmp(&out.strengths[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ids
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
    names: &HashMap<Uuid, String>,
    rounds: u32,
    runs: u64,
    seed: u64,
    jitter: f64,
) {
    println!("osp-sim: {runs} runs × {rounds} rounds, seed {seed}, jitter {jitter}");
    println!("(no draws modelled; byes excluded from game stats)\n");

    // Headline table: one row per variant.
    println!(
        "{:<14} {:>7} {:>7} {:>7} {:>7}   P(|d|>T)",
        "variant", "mean|d|", "median", "p90", "p95"
    );
    for r in reports {
        let exceed: Vec<String> = r
            .diff
            .exceed
            .iter()
            .map(|(t, p)| format!("{}:{:.1}%", *t as i64, p * 100.0))
            .collect();
        println!(
            "{:<14} {:>7.0} {:>7.0} {:>7.0} {:>7.0}   {}",
            trunc(&r.name, 14),
            r.diff.mean,
            r.diff.median,
            r.diff.p90,
            r.diff.p95,
            exceed.join("  "),
        );
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

    // Victory probabilities: union of the top few players across variants.
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
        println!();
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

fn write_outputs(
    dir: &Path,
    reports: &[VariantReport],
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

    // One CSV histogram (50-point bins) of the pooled |diff| per variant, from the
    // diffs retained at aggregation time — no re-simulation.
    let bin = 50.0;
    for r in reports {
        let mut csv = String::from("bin_lo,bin_hi,count\n");
        let max = r.pooled.last().copied().unwrap_or(0.0);
        let nbins = (max / bin).ceil() as usize + 1;
        let mut counts = vec![0usize; nbins];
        for d in &r.pooled {
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
        std::fs::write(dir.join(format!("elo-diff-{}.csv", sanitize(&r.name))), csv)?;
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
