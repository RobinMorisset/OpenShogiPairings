//! `osp-sim` — Monte-Carlo comparison of pairing-settings variants.
//!
//! Loads a base tournament (an `.osp` save or a FESA result table), then for each
//! settings variant runs many simulated tournaments and reports how the variants
//! differ on game mismatch (are there fewer foregone-conclusion games?) and on
//! who tends to win / how faithfully the final ranking tracks true strength. See
//! `docs/simulation-cli.md` for the design.
//!
//! Public items link to the private helpers that explain them (see the note in
//! `osp_core`'s crate docs); rustdoc renders those unlinked unless it is run with
//! `--document-private-items`, so that warning is off here too.
//!
//! `unreachable_pub` and `unnameable_types` are turned on for the reason given
//! in `osp_core`'s crate docs: between them they pin down what is really part
//! of the API.
#![allow(rustdoc::private_intra_doc_links)]
#![warn(unnameable_types, unreachable_pub)]

mod fesa;
mod stats;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;
use serde::Serialize;
use uuid::Uuid;

use osp_core::sim::{
    cup_eligibility, game_elo_diffs, interest_welfare, sample_strengths, simulate_run,
    welfare_shortfall, CupConfig, OracleModel, RunOutcome, StrengthMap,
};
use osp_core::{
    cup_field_size, decode_latin1, estimate_elos, import_fesa_results, reconstruct_cup_from_final,
    CupFormat, Player, Tournament, TournamentId, TournamentSettings,
};

use stats::{diff_stats, mean, mean_ci95, paired_diff, spearman, wilson, DiffStats, Proportion};

/// `--cup-format` on the command line. A local mirror of [`CupFormat`] because
/// `clap::ValueEnum` has to be derived on the type, and osp-core has no business
/// depending on clap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CupFormatArg {
    Direct,
    Qualifier,
}

impl From<CupFormatArg> for CupFormat {
    fn from(arg: CupFormatArg) -> Self {
        match arg {
            CupFormatArg::Direct => CupFormat::Direct,
            CupFormatArg::Qualifier => CupFormat::Qualifier,
        }
    }
}

impl std::fmt::Display for CupFormatArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CupFormatArg::Direct => "direct",
            CupFormatArg::Qualifier => "qualifier",
        })
    }
}

/// Compare pairing-settings variants by simulating a tournament many times.
#[derive(Debug, Parser)]
#[command(name = "osp-sim", version)]
struct Args {
    /// Base tournament as an `.osp` save file (JSON). One base source is required.
    #[arg(long, value_name = "FILE", conflicts_with = "results")]
    base: Option<PathBuf>,

    /// Base tournament as a FESA post-tournament result table. Also supplies each
    /// player's true strength (pre-ELO + points gained, or the assigned rating for
    /// a pre-unrated player), so no separate --strength is needed.
    #[arg(long, value_name = "FILE", conflicts_with = "strength")]
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
    /// tournament number (as a string) to an ELO. The override is the *center* of
    /// that player's ground-truth strength: exact at `--jitter 0`, and the mean it
    /// scatters around at higher jitter (rather than the registration rating).
    #[arg(long, value_name = "FILE")]
    strength: Option<PathBuf>,

    /// Fill each player's FESA game count from the rating list in force on this
    /// date (YYYY-MM-DD) — pass the tournament's own start date. That is the last
    /// permanent list (FESA publishes on 1 Jan and 1 Jul) dated on or before it,
    /// fetched from fesashogi.eu and matched by name; the counts are therefore
    /// the ones the players went *into* the event with. A result table carries no
    /// game counts, so without this every player is treated as provisional; this
    /// restores the established/provisional distinction (the reliability signal)
    /// while leaving strengths untouched.
    #[arg(long, value_name = "YYYY-MM-DD", conflicts_with = "games_fesa_list")]
    games_fesa_before: Option<String>,

    /// Fill each player's FESA game count from a specific rating list (https URL or
    /// local file path), matched by name. See `--games-fesa-before`.
    #[arg(long, value_name = "URL|PATH")]
    games_fesa_list: Option<String>,

    /// Multiplier on each player's prior width: 0 pins true strength to its center
    /// (the override — e.g. the post-tournament ELO — else the registration
    /// rating), 1 samples at the raw-FESA-K prior width around that center, >1
    /// stress-tests worse-than-assumed ratings. Part of the settings-independent
    /// truth model — never affected by a variant's `elo_k_multiplier`.
    #[arg(long, default_value_t = 0.0)]
    jitter: f64,

    /// The oracle's provisional-player width multiplier: how much wider the
    /// *true-strength* prior is for a provisionally-rated player (no FESA list entry
    /// or too few games) than for an established one. Its own truth-model knob,
    /// independent of any variant's pairing `elo_provisional_multiplier`. Clamped to
    /// ≥ 1. Only matters at `--jitter > 0`.
    #[arg(long, default_value_t = 2.0)]
    oracle_provisional: f64,

    /// The oracle's center (mean) for an **unrated** player's true strength (ELO).
    /// Its own truth-model knob, the counterpart of a variant's
    /// `elo_unrated_prior_center`. Default 600 (matches the settings default).
    #[arg(long, default_value_t = 600.0)]
    oracle_unrated_center: f64,

    /// The oracle's **K** setting the width of an unrated player's true-strength
    /// prior: σ = √(K·s), the same law a rated player's K obeys. Its own truth-model
    /// knob, the counterpart of a variant's `elo_unrated_k`. Default 705 (≈ σ 350,
    /// matching the settings default). Only matters at `--jitter > 0`.
    #[arg(long, default_value_t = 705.0)]
    oracle_unrated_k: f64,

    /// Run the hybrid direct-elimination cup with this bracket size (8/16/32/64).
    /// Requires --cup-nations.
    #[arg(long, value_name = "N", requires = "cup_nations")]
    cup_size: Option<u32>,

    /// How the bracket is filled: `direct` (the top --cup-size eligible players
    /// are the bracket, from round 1 — the French / European Championship) or
    /// `qualifier` (the top half is pre-qualified and plays the open in round 1
    /// while the next --cup-size play a qualification round, and the bracket runs
    /// from round 2 — the German Championship). The qualifier format needs half as
    /// many eligible players again, and one more round.
    /// Rejected alongside --cup-final: a roster reconstructed backward from the
    /// finalists is a plain bracket, so a format given there could only be ignored.
    #[arg(
        long,
        value_name = "FORMAT",
        default_value = "direct",
        requires = "cup_size",
        conflicts_with = "cup_final"
    )]
    cup_format: CupFormatArg,

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

    /// Reconstruct the cup roster from its two finalists — the gold and silver
    /// medalists' names — instead of a nationality filter. The exact bracket is read
    /// backward from the final (`Cup`-agnostic), so it is immune to the stale seeding
    /// ratings, declined entries and borderline nationalities that defeat a
    /// top-N-by-rating guess (the WOSC / European Championship case). Give both names;
    /// size is derived. Mutually exclusive with --cup-size/--cup-nations.
    #[arg(
        long,
        value_name = "NAME",
        num_args = 2,
        conflicts_with_all = ["cup_size", "cup_nations"]
    )]
    cup_final: Vec<String>,

    /// One or more |ELO diff| thresholds T for reporting P(|diff| > T).
    #[arg(long = "threshold", value_name = "T", default_values_t = [400.0_f64])]
    thresholds: Vec<f64>,

    /// Optional output directory for report.json and per-variant CSV histograms.
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,

    /// Optional CSV of per-run metrics (one row per variant × run): the run's mean
    /// |ELO diff|, fraction of games over the first `--threshold`, standings
    /// fidelity, and Open winner. Because every variant shares a run's seed (common
    /// random numbers), rows with the same `run` are paired — enabling paired
    /// significance tests across variants.
    #[arg(long, value_name = "FILE")]
    dump_runs: Option<PathBuf>,

    /// Optional CSV of the sampled ground-truth ("source of truth") strength of
    /// every player in every run (variant,run,player,strength), for inspecting the
    /// strength model — e.g. that `--jitter` spreads each player around their
    /// post-tournament ELO. Large: one row per variant × run × player.
    #[arg(long, value_name = "FILE")]
    dump_strengths: Option<PathBuf>,
}

/// The knobs describing the whole invocation, resolved once from [`Args`] and
/// shared by the simulation loop and both report writers. Grouped because they
/// travel together and are all plain scalars: passed one by one, `seed` and
/// `runs` (both `u64`) transpose silently.
#[derive(Clone, Copy)]
struct RunSpec {
    /// Rounds per simulated tournament, defaulted from the base's round count.
    rounds: u32,
    /// Simulated tournaments per variant.
    runs: u64,
    /// Master seed; run i uses `seed + i`.
    seed: u64,
    /// The oracle's prior-width multiplier, kept here for the report headers
    /// (the simulation itself reads it from the [`OracleModel`]).
    jitter: f64,
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Find the unique base player whose name matches `name` — a full name in any
/// order (e.g. "Jean Fortin"), accent-folded and word-based, so a player's last-
/// and first-name words must all appear in it. Errors unless exactly one matches.
fn find_player_by_name(base: &Tournament, name: &str) -> Result<TournamentId, String> {
    let target: HashSet<String> = fesa::fold_name(name)
        .split_whitespace()
        .map(String::from)
        .collect();
    let matches: Vec<TournamentId> = base
        .players
        .iter()
        .filter(|p| {
            let last = fesa::fold_name(&p.last_name);
            let first = fesa::fold_name(&p.first_name);
            last.split_whitespace().all(|w| target.contains(w))
                && first.split_whitespace().all(|w| target.contains(w))
        })
        .filter_map(|p| p.tournament_id)
        .collect();
    match matches.as_slice() {
        [id] => Ok(*id),
        [] => Err(format!("no player matches cup finalist {name:?}")),
        many => Err(format!(
            "{} players match cup finalist {name:?}",
            many.len()
        )),
    }
}

fn run(args: Args) -> Result<(), String> {
    let (mut base, results_strengths) = load_base(&args)?;
    let rounds = args.rounds.unwrap_or(base.rounds.len() as u32);
    if rounds == 0 {
        return Err("no rounds to simulate: pass --rounds (the base has none)".into());
    }

    // Enrich the (game-count-less) result-table players with FESA game counts, so
    // the established/provisional reliability signal is real rather than "everyone
    // provisional". Strengths are untouched.
    if let Some(source) = &args.games_fesa_list {
        let games = fesa::games_from_list(source, &base)?;
        let matched = games.len();
        apply_games(&mut base, &games);
        eprintln!(
            "fesa game counts from {source} — matched {matched}/{} players",
            base.players.len()
        );
    } else if let Some(date) = &args.games_fesa_before {
        let (games, url) = fesa::games_before(date, &base)?;
        let matched = games.len();
        apply_games(&mut base, &games);
        eprintln!(
            "fesa game counts from the list in force on {date} ({url}) — matched \
             {matched}/{} players",
            base.players.len()
        );
    }

    let overrides = if let Some(strengths) = results_strengths {
        eprintln!(
            "true strength from the results table — {}/{} players (elo + points won)",
            strengths.len(),
            base.players.len()
        );
        strengths
    } else if let Some(path) = &args.strength {
        load_overrides(path, &base)?
    } else {
        StrengthMap::new()
    };

    // Cup format: compute the eligible set from the base's real rounds, once, and
    // apply it across all variants — either reconstructed exactly from the two
    // finalists, or by nationality + attendance.
    let cup = if !args.cup_final.is_empty() {
        let gold = find_player_by_name(&base, &args.cup_final[0])?;
        let silver = find_player_by_name(&base, &args.cup_final[1])?;
        let (size, roster) =
            reconstruct_cup_from_final(&base.rounds, gold, silver).ok_or_else(|| {
                format!(
                    "could not reconstruct a power-of-two cup bracket from finalists {:?} / {:?} \
                 (did they reach the final?)",
                    args.cup_final[0], args.cup_final[1]
                )
            })?;
        eprintln!(
            "cup: size {size} reconstructed from finalists {:?} / {:?}",
            args.cup_final[0], args.cup_final[1]
        );
        // A roster read backward off the final is a plain bracket by construction.
        let eligible = roster
            .into_iter()
            .filter_map(|t| {
                base.players
                    .iter()
                    .find(|p| p.tournament_id == Some(t))
                    .map(|p| p.id)
            })
            .collect();
        Some(CupConfig {
            eligible,
            size,
            format: CupFormat::Direct,
        })
    } else if let Some(size) = args.cup_size {
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
        let format = args.cup_format.into();
        // A cup player must have played every cup round: log2(size) of bracket,
        // plus the qualification round when the format has one.
        let cup_rounds =
            size.trailing_zeros() as usize + usize::from(matches!(format, CupFormat::Qualifier));
        let eligible = cup_eligibility(&base, &nations, cup_rounds);
        // The qualifier format takes half as many players again as the bracket
        // holds. Check here rather than letting `simulate_run` fail per run, so a
        // mis-sized cup is one clear message instead of `--runs` copies of it.
        let needed = cup_field_size(size, format) as usize;
        if eligible.len() < needed {
            return Err(format!(
                "a {size}-player {} cup needs {needed} eligible players, but only {} of the \
                 {} nationalities were present through round {cup_rounds}",
                args.cup_format,
                eligible.len(),
                nations.len(),
            ));
        }
        eprintln!(
            "cup: size {size} ({}), {} eligible ({} nationalities, present through round \
             {cup_rounds})",
            args.cup_format,
            eligible.len(),
            nations.len(),
        );
        Some(CupConfig {
            eligible,
            size,
            format,
        })
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

    // The settings-independent truth model shared by every variant of this run.
    let oracle = OracleModel {
        jitter: args.jitter,
        provisional: args.oracle_provisional,
        unrated_center: args.oracle_unrated_center,
        unrated_k: args.oracle_unrated_k,
    };
    let spec = RunSpec {
        rounds,
        runs: args.runs,
        seed: args.seed,
        jitter: args.jitter,
    };

    let names = player_names(&base);
    let observed = observed_report(&base, &overrides, &oracle, &args.thresholds);
    let mut reports = Vec::new();
    let mut dump = args
        .dump_runs
        .as_ref()
        .map(|_| String::from("variant,run,mean_diff,frac_exceed,fidelity,hit,interest,winner\n"));
    let mut strengths_dump = args
        .dump_strengths
        .as_ref()
        .map(|_| String::from("variant,run,player,strength\n"));
    for (name, settings) in &variants {
        let outcomes = simulate_variant(&base, settings, &overrides, &oracle, cup.as_ref(), spec)
            .map_err(|e| format!("variant '{name}': {e}"))?;
        if let Some(buf) = &mut dump {
            append_run_rows(buf, name, &outcomes, &args.thresholds, &names);
        }
        if let Some(buf) = &mut strengths_dump {
            append_strength_rows(buf, name, &outcomes, &names);
        }
        reports.push(aggregate(name.clone(), &outcomes, &args.thresholds));
    }
    if let (Some(path), Some(buf)) = (&args.dump_runs, &dump) {
        std::fs::write(path, buf).map_err(|e| format!("writing {}: {e}", path.display()))?;
        eprintln!("wrote per-run metrics to {}", path.display());
    }
    if let (Some(path), Some(buf)) = (&args.dump_strengths, &strengths_dump) {
        std::fs::write(path, buf).map_err(|e| format!("writing {}: {e}", path.display()))?;
        eprintln!("wrote ground-truth strengths to {}", path.display());
    }

    print_report(&reports, observed.as_ref(), &names, spec);

    if let Some(dir) = &args.out {
        write_outputs(dir, &reports, observed.as_ref(), &base, spec)
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
    match (&args.base, &args.results) {
        (Some(path), None) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            let t = serde_json::from_str(&text)
                .map_err(|e| format!("parsing {} as a tournament: {e}", path.display()))?;
            Ok((t, None))
        }
        (None, Some(path)) => {
            // FESA result tables are Latin-1, like the rating lists.
            let bytes =
                std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
            let (t, strengths) = import_fesa_results(&decode_latin1(&bytes))
                .map_err(|e| format!("parsing {} as a FESA result table: {e}", path.display()))?;
            Ok((t, Some(strengths)))
        }
        _ => Err("provide exactly one of --base or --results".into()),
    }
}

fn load_settings(path: &Path) -> Result<TournamentSettings, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parsing {} as settings: {e}", path.display()))
}

/// Write the matched FESA game counts onto the base players (unmatched keep their
/// `None`, i.e. stay provisional).
fn apply_games(base: &mut Tournament, games: &HashMap<Uuid, u32>) {
    for p in &mut base.players {
        if let Some(&g) = games.get(&p.id) {
            p.fesa_games = Some(g);
        }
    }
}

/// Parse `{ "<tournament_number>": <elo>, ... }` and resolve to player ids.
fn load_overrides(path: &Path, base: &Tournament) -> Result<StrengthMap, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let by_number: HashMap<String, f64> = serde_json::from_str(&text)
        .map_err(|e| format!("parsing {} as a strength map: {e}", path.display()))?;
    let id_of: HashMap<TournamentId, Uuid> = base
        .players
        .iter()
        .filter_map(|p| p.tournament_id.map(|n| (n, p.id)))
        .collect();
    let mut map = StrengthMap::new();
    for (key, elo) in by_number {
        let number: u32 = key
            .parse()
            .map_err(|_| format!("strength key '{key}' is not a tournament number"))?;
        match id_of.get(&TournamentId(number)) {
            Some(_) => {
                // Strengths are keyed by tournament number, which is the JSON key.
                map.insert(TournamentId(number), elo);
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
    oracle: &OracleModel,
    cup: Option<&CupConfig>,
    spec: RunSpec,
) -> Result<Vec<RunOutcome>, String> {
    // `jitter` is the oracle's, and reaches the model through `oracle`.
    let RunSpec {
        rounds, runs, seed, ..
    } = spec;
    (0..runs)
        .into_par_iter()
        .map(|i| {
            // Run i uses `seed + i`, so `--seed` actually changes the simulated
            // worlds (not just the printed label), while every variant still sees
            // the *same* seed per run — common random numbers, for a fair A/B.
            let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_add(i));
            simulate_run(base, settings, overrides, oracle, rounds, cup, &mut rng)
                .map_err(|e| e.to_string())
        })
        .collect()
}

// --- aggregation -----------------------------------------------------------

/// Aggregated metrics for one variant.
struct VariantReport {
    name: String,
    diff: DiffStats,
    /// Pooled absolute ELO diffs across all runs, sorted — retained for the CSV
    /// histogram so outputs need no re-simulation.
    pooled: Vec<f64>,
    /// Mean Spearman(final standings, true-strength order) over runs.
    fidelity_score: f64,
    /// Mean Spearman(ELO-estimate order, true-strength order) over runs.
    fidelity_estimate: f64,
    /// Fraction of runs whose winner is the truly-strongest player (top-1 fidelity):
    /// mean over runs of `1[final_order[0] == true_order[0]]`.
    hit_rate: f64,
    /// Per-player top-1 probability (with CI) and top-3 rate for the Open (overall
    /// standings), sorted best first.
    players: Vec<PlayerProb>,
    /// Per-player probability (with CI) of taking the direct-elimination cup, over
    /// all runs, sorted best first. Empty when no cup was configured.
    cup_champions: Vec<(TournamentId, Proportion)>,
    /// Per-run mean |ELO diff| (one value per run) — the run-level replications the
    /// CI and paired comparisons are computed from.
    per_run_mean_diff: Vec<f64>,
    /// Per-run fraction of games exceeding the first `--threshold`.
    per_run_exceed: Vec<f64>,
    /// Per-run standings fidelity (Spearman vs true strength).
    per_run_fidelity: Vec<f64>,
    /// Per-run top-1 hit (1.0 if the winner is the truly-strongest player, else 0.0).
    per_run_hit: Vec<f64>,
    /// Mean game-interest metric (game-weighted Sen welfare) over runs.
    interest: f64,
    /// Per-run game-interest metric.
    per_run_interest: Vec<f64>,
    /// Each player's pooled game interest across runs: (mean interest, mean games
    /// per tournament).
    interest_by_player: HashMap<TournamentId, (f64, f64)>,
    /// The (up to) 5 players whose dull games most lowered `interest`, worst first.
    worst_interest: Vec<TournamentId>,
}

struct PlayerProb {
    player: TournamentId,
    top1: Proportion,
    top3: f64,
}

/// Strength order (strongest first) for a strength map. Ties broken by
/// tournament number: without a total order, equal strengths would keep the
/// `HashMap`'s iteration order, which varies per process *and* per rayon worker
/// thread — the fidelity/hit metrics would then differ from run to run.
fn strength_order(strengths: &StrengthMap) -> Vec<TournamentId> {
    let mut ids: Vec<TournamentId> = strengths.keys().copied().collect();
    ids.sort_by(|a, b| {
        strengths[b]
            .partial_cmp(&strengths[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });
    ids
}

/// True-strength order (strongest first) for one run.
fn true_order(out: &RunOutcome) -> Vec<TournamentId> {
    strength_order(&out.strengths)
}

/// Top-1 hit for one run: 1.0 if the standings winner is the truly-strongest
/// player, else 0.0 (0.0 too if either order is empty).
fn top1_hit(final_order: &[TournamentId], true_ord: &[TournamentId]) -> f64 {
    match (final_order.first(), true_ord.first()) {
        (Some(w), Some(b)) if w == b => 1.0,
        _ => 0.0,
    }
}

/// Players ordered by descending ELO estimate, ties broken by tournament number.
fn estimate_order(players: &[Player], estimate: &HashMap<Uuid, f64>) -> Vec<TournamentId> {
    let mut ids: Vec<&Player> = players.iter().collect();
    ids.sort_by(|a, b| {
        let ea = estimate.get(&a.id).copied().unwrap_or(f64::MIN);
        let eb = estimate.get(&b.id).copied().unwrap_or(f64::MIN);
        eb.partial_cmp(&ea)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tournament_id.cmp(&b.tournament_id))
    });
    ids.into_iter().filter_map(|p| p.tournament_id).collect()
}

/// Append one CSV row per run: the run's mean |diff|, fraction of games over the
/// first threshold, standings fidelity, and Open winner. Winners are written as
/// the report's player label so paired analysis reads the same names as the table.
fn append_run_rows(
    buf: &mut String,
    variant: &str,
    outcomes: &[RunOutcome],
    thresholds: &[f64],
    names: &HashMap<TournamentId, String>,
) {
    let first_t = thresholds.first().copied().unwrap_or(400.0);
    for (i, o) in outcomes.iter().enumerate() {
        let mean_diff = mean(&o.game_diffs);
        let frac_exceed = if o.game_diffs.is_empty() {
            0.0
        } else {
            o.game_diffs.iter().filter(|d| **d > first_t).count() as f64 / o.game_diffs.len() as f64
        };
        let true_ord = true_order(o);
        let fidelity = spearman(&o.final_order, &true_ord);
        let hit = top1_hit(&o.final_order, &true_ord);
        let winner = o
            .final_order
            .first()
            .and_then(|id| names.get(id))
            .map(String::as_str)
            .unwrap_or("?");
        buf.push_str(&format!(
            "{variant},{i},{mean_diff:.4},{frac_exceed:.4},{fidelity:.4},{hit:.4},{:.4},\"{winner}\"\n",
            o.interest
        ));
    }
}

/// Append one CSV row per player per run with that run's sampled ground-truth
/// strength — the "source of truth" the outcomes were generated from.
fn append_strength_rows(
    buf: &mut String,
    variant: &str,
    outcomes: &[RunOutcome],
    names: &HashMap<TournamentId, String>,
) {
    for (i, o) in outcomes.iter().enumerate() {
        // Sorted by tournament number: HashMap iteration order varies per process
        // and per thread, and the dump should be reproducible.
        let mut rows: Vec<(TournamentId, f64)> =
            o.strengths.iter().map(|(&id, &s)| (id, s)).collect();
        rows.sort_by_key(|&(id, _)| id);
        for (id, strength) in rows {
            let player = names.get(&id).map(String::as_str).unwrap_or("?");
            buf.push_str(&format!("{variant},{i},\"{player}\",{strength:.2}\n"));
        }
    }
}

fn aggregate(name: String, outcomes: &[RunOutcome], thresholds: &[f64]) -> VariantReport {
    // Deterministic tie-break: a player's tournament number (never their random id).
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
    let hits: Vec<f64> = outcomes
        .iter()
        .map(|o| top1_hit(&o.final_order, &true_order(o)))
        .collect();

    // Per-run replications for the inferential comparison: each run is one
    // independent draw, so CIs and paired Δ are computed over these, not over the
    // pooled per-game diffs (which are correlated within a run).
    let first_t = thresholds.first().copied().unwrap_or(400.0);
    let per_run_mean_diff: Vec<f64> = outcomes.iter().map(|o| mean(&o.game_diffs)).collect();
    let per_run_exceed: Vec<f64> = outcomes
        .iter()
        .map(|o| {
            if o.game_diffs.is_empty() {
                0.0
            } else {
                o.game_diffs.iter().filter(|d| **d > first_t).count() as f64
                    / o.game_diffs.len() as f64
            }
        })
        .collect();

    // Victory counts.
    let mut top1: HashMap<TournamentId, usize> = HashMap::new();
    let mut top3: HashMap<TournamentId, usize> = HashMap::new();
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
            .then_with(|| a.player.cmp(&b.player))
    });

    // Cup champions: count each player's titles across the runs that produced one.
    // The probability is over *all* runs (a run with no champion — no cup, or a
    // double no-show final — simply isn't a win for anyone), so the column reads as
    // "P(this player wins the cup)".
    let mut cup_wins: HashMap<TournamentId, usize> = HashMap::new();
    for o in outcomes {
        if let Some(c) = o.cup_champion {
            *cup_wins.entry(c).or_default() += 1;
        }
    }
    let mut cup_champions: Vec<(TournamentId, Proportion)> = cup_wins
        .into_iter()
        .map(|(id, n)| (id, wilson(n, runs)))
        .collect();
    cup_champions.sort_by(|a, b| {
        b.1.p
            .partial_cmp(&a.1.p)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let per_run_interest: Vec<f64> = outcomes.iter().map(|o| o.interest).collect();

    // Pool each player's game interest across all runs (sum of entropies / games),
    // then rank players by their contribution to the welfare shortfall — the ones
    // whose consistently dull games most lowered `interest`.
    let mut pool: HashMap<TournamentId, (f64, u64)> = HashMap::new();
    for o in outcomes {
        for &(id, sum_h, games) in &o.player_interest {
            let e = pool.entry(id).or_insert((0.0, 0));
            e.0 += sum_h;
            e.1 += games as u64;
        }
    }
    // Sorted: HashMap order varies per process/thread, and it feeds both the
    // float-summation order and the tie order inside the welfare computation.
    let mut ids: Vec<TournamentId> = pool.keys().copied().collect();
    ids.sort_unstable();
    let values: Vec<f64> = ids
        .iter()
        .map(|id| pool[id].0 / pool[id].1 as f64)
        .collect();
    let weights: Vec<f64> = ids.iter().map(|id| pool[id].1 as f64).collect();
    let shortfall = welfare_shortfall(&values, &weights);
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by(|&a, &b| {
        shortfall[b]
            .partial_cmp(&shortfall[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ids[a].cmp(&ids[b]))
    });
    let runs_f = runs.max(1) as f64;
    let interest_by_player: HashMap<TournamentId, (f64, f64)> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, (values[i], pool[&id].1 as f64 / runs_f)))
        .collect();
    let worst_interest: Vec<TournamentId> = order.iter().take(5).map(|&i| ids[i]).collect();

    VariantReport {
        name,
        diff,
        pooled,
        fidelity_score: mean(&score_rhos),
        fidelity_estimate: mean(&est_rhos),
        hit_rate: mean(&hits),
        players,
        cup_champions,
        per_run_mean_diff,
        per_run_exceed,
        per_run_fidelity: score_rhos,
        per_run_hit: hits,
        interest: mean(&per_run_interest),
        per_run_interest,
        interest_by_player,
        worst_interest,
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
    hit_rate: f64,
    interest: f64,
    /// Finishing rank (0-based) of each player in the real standings.
    rank_of: HashMap<TournamentId, usize>,
    winner: Option<TournamentId>,
}

/// Build the observed report from the base's real results, or `None` if the base
/// has no played games (e.g. a synthetic, never-played base).
fn observed_report(
    base: &Tournament,
    overrides: &StrengthMap,
    oracle: &OracleModel,
    thresholds: &[f64],
) -> Option<ObservedReport> {
    let played = base
        .rounds
        .iter()
        .flat_map(|r| &r.boards)
        .any(|b| b.outcome.winner().is_some());
    if !played {
        return None;
    }

    // Nominal strengths: force jitter 0 (override else registration rating), so the
    // width knobs and the rng are unused here — but keep the rest of the oracle so
    // an unrated player still sits at the oracle's unrated center.
    let mut rng = ChaCha8Rng::seed_from_u64(0);
    let nominal = OracleModel {
        jitter: 0.0,
        ..*oracle
    };
    let strengths = sample_strengths(&base.players, overrides, &nominal, &mut rng);

    let mut pooled = game_elo_diffs(base, &strengths);
    let diff = diff_stats(&pooled, thresholds);
    pooled.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let tid_of: HashMap<Uuid, TournamentId> = base
        .players
        .iter()
        .filter_map(|p| p.tournament_id.map(|t| (p.id, t)))
        .collect();
    let final_order: Vec<TournamentId> = base
        .standings()
        .into_iter()
        .filter_map(|s| tid_of.get(&s.player_id).copied())
        .collect();
    let true_ord = strength_order(&strengths);
    let estimate = estimate_elos(&base.players, &base.settings, &base.rounds);
    let est_ord = estimate_order(&base.players, &estimate);

    Some(ObservedReport {
        rounds: base.rounds.len() as u32,
        diff,
        pooled,
        fidelity_score: spearman(&final_order, &true_ord),
        fidelity_estimate: spearman(&est_ord, &true_ord),
        hit_rate: top1_hit(&final_order, &true_ord),
        interest: interest_welfare(base, &strengths),
        rank_of: final_order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect(),
        winner: final_order.first().copied(),
    })
}

// --- output ----------------------------------------------------------------

fn player_names(base: &Tournament) -> HashMap<TournamentId, String> {
    base.players
        .iter()
        .filter_map(|p| {
            let t = p.tournament_id?;
            Some((t, format!("#{t} {}", p.last_name)))
        })
        .collect()
}

fn print_report(
    reports: &[VariantReport],
    observed: Option<&ObservedReport>,
    names: &HashMap<TournamentId, String>,
    spec: RunSpec,
) {
    let RunSpec {
        rounds,
        runs,
        seed,
        jitter,
    } = spec;
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
        "\n{:<14} {:>16} {:>16} {:>10} {:>12}",
        "variant", "fidelity(score)", "fidelity(est)", "hit(top1)", "interest(W)"
    );
    for r in reports {
        println!(
            "{:<14} {:>16.3} {:>16.3} {:>10.3} {:>12.3}",
            trunc(&r.name, 14),
            r.fidelity_score,
            r.fidelity_estimate,
            r.hit_rate,
            r.interest,
        );
    }
    if let Some(o) = observed {
        println!(
            "{:<14} {:>16.3} {:>16.3} {:>10.3} {:>12.3}",
            "observed*", o.fidelity_score, o.fidelity_estimate, o.hit_rate, o.interest,
        );
    }

    // Inferential comparison, so the point estimates above are not read as exact.
    // Column 1 is each metric's value ±95% CI (from run-to-run variation); the
    // later columns are the paired Δ against the first variant — every variant
    // shares a run's seed (common random numbers), so the difference is taken per
    // run, which cancels shared variation.
    if let Some((base, rest)) = reports.split_first() {
        let star = |z: f64| {
            let a = z.abs();
            if a > 3.290_5 {
                "***"
            } else if a > 2.575_8 {
                "**"
            } else if a > 1.96 {
                "*"
            } else {
                "ns"
            }
        };
        println!(
            "\nstatistical comparison — col 1: value ±95% CI; others: paired Δ vs '{}' ±95% CI",
            trunc(&base.name, 20)
        );
        println!(
            "  (*** p<.001  ** p<.01  * p<.05  ns=not significant; Δ uses common random numbers)"
        );
        print!("{:<16}", "metric");
        print!("  {:>20}", trunc(&base.name, 20));
        for r in rest {
            print!("  {:>22}", trunc(&r.name, 22));
        }
        println!();

        let first_t = base
            .diff
            .exceed
            .first()
            .map(|(t, _)| *t as i64)
            .unwrap_or(400);
        // (label, per-run accessor, display scale, decimals, base-unit, Δ-unit)
        type Acc = fn(&VariantReport) -> &Vec<f64>;
        let rows: [(String, Acc, f64, usize, &str, &str); 5] = [
            (
                "mean|d|".to_string(),
                |r| &r.per_run_mean_diff,
                1.0,
                1,
                "",
                "",
            ),
            (
                format!("P(|d|>{first_t})"),
                |r| &r.per_run_exceed,
                100.0,
                1,
                "%",
                "pp",
            ),
            (
                "fidelity(score)".to_string(),
                |r| &r.per_run_fidelity,
                1.0,
                3,
                "",
                "",
            ),
            ("hit(top1)".to_string(), |r| &r.per_run_hit, 1.0, 3, "", ""),
            (
                "interest(W)".to_string(),
                |r| &r.per_run_interest,
                1.0,
                3,
                "",
                "",
            ),
        ];
        for (label, acc, scale, dec, base_unit, delta_unit) in rows {
            print!("{:<16}", trunc(&label, 16));
            let c = mean_ci95(acc(base));
            print!(
                "  {:>20}",
                format!(
                    "{:.*}{} ±{:.*}",
                    dec,
                    c.mean * scale,
                    base_unit,
                    dec,
                    c.ci * scale
                )
            );
            for r in rest {
                match paired_diff(acc(base), acc(r)) {
                    Some(d) => print!(
                        "  {:>22}",
                        format!(
                            "{:+.*}{} ±{:.*} {}",
                            dec,
                            d.delta * scale,
                            delta_unit,
                            dec,
                            d.ci * scale,
                            star(d.z)
                        )
                    ),
                    None => print!("  {:>22}", "-"),
                }
            }
            println!();
        }
    }

    // Open winner: rank-1 in the overall standings. Union of the top few players
    // across variants, with an extra column for each player's real finishing rank.
    let mut shown: Vec<TournamentId> = Vec::new();
    for r in reports {
        for pp in r.players.iter().take(6) {
            if !shown.contains(&pp.player) {
                shown.push(pp.player);
            }
        }
    }
    println!(
        "\nOpen winner probability (top-1 in final standings, 95% CI) — showing {} players",
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

    // Cup champion: winner of the direct-elimination bracket, when a cup was
    // configured. The bracket is rating-seeded and independent of the pairing
    // settings being compared, so it plays out identically across variants —
    // report it once (from the first variant), not as a per-variant comparison.
    if let Some(base) = reports.first() {
        if !base.cup_champions.is_empty() {
            let shown = base.cup_champions.len().min(8);
            println!(
                "\ncup champion probability (top-1, 95% CI) — settings-independent, showing top {shown}"
            );
            for (id, p) in base.cup_champions.iter().take(shown) {
                println!(
                    "{:<20}  {:>6.1}% [{:>4.1},{:>4.1}]",
                    trunc(names.get(id).map(String::as_str).unwrap_or("?"), 20),
                    p.p * 100.0,
                    p.lo * 100.0,
                    p.hi * 100.0
                );
            }
        }
    }

    // Least-interesting players: the union of each variant's 5 biggest interest
    // shortfalls, with each player's pooled mean game interest per variant (`*`
    // marks that variant's own worst-5). `g` is the player's mean games per
    // tournament — a low count (absences) is why the metric discounts them.
    let mut dull_shown: Vec<TournamentId> = Vec::new();
    for r in reports {
        for id in &r.worst_interest {
            if !dull_shown.contains(id) {
                dull_shown.push(*id);
            }
        }
    }
    if !dull_shown.is_empty() {
        println!("\nleast-interesting players (pooled mean interest I; * = variant's 5 biggest shortfalls)");
        print!("{:<22}", "player");
        for r in reports {
            print!("  {:>16}", trunc(&r.name, 16));
        }
        println!();
        for id in &dull_shown {
            let games = reports
                .iter()
                .find_map(|r| r.interest_by_player.get(id))
                .map(|&(_, g)| g)
                .unwrap_or(0.0);
            let label = format!(
                "{} ({:.0}g)",
                trunc(names.get(id).map(String::as_str).unwrap_or("?"), 15),
                games
            );
            print!("{label:<22}");
            for r in reports {
                match r.interest_by_player.get(id) {
                    Some(&(i, _)) => {
                        let mark = if r.worst_interest.contains(id) {
                            "*"
                        } else {
                            " "
                        };
                        print!("  {:>15.3}{mark}", i);
                    }
                    None => print!("  {:>16}", "-"),
                }
            }
            println!();
        }
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
    /// Cup champion probabilities — reported once, not per variant: the bracket is
    /// rating-seeded and independent of the pairing settings, so it is identical
    /// across variants. Empty when no cup was configured.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cup_champions: Vec<JsonCupChampion>,
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
    hit_rate: f64,
    interest: f64,
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
    hit_rate: f64,
    interest: f64,
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

#[derive(Serialize)]
struct JsonCupChampion {
    player: String,
    prob: f64,
    prob_lo: f64,
    prob_hi: f64,
}

fn write_outputs(
    dir: &Path,
    reports: &[VariantReport],
    observed: Option<&ObservedReport>,
    base: &Tournament,
    spec: RunSpec,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let names = player_names(base);
    let RunSpec {
        rounds,
        runs,
        seed,
        jitter,
    } = spec;

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
            hit_rate: o.hit_rate,
            interest: o.interest,
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
                hit_rate: r.hit_rate,
                interest: r.interest,
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
        // Settings-independent, so taken once from the first variant.
        cup_champions: reports
            .first()
            .map(|r| {
                r.cup_champions
                    .iter()
                    .map(|(id, p)| JsonCupChampion {
                        player: names.get(id).cloned().unwrap_or_else(|| id.to_string()),
                        prob: p.p,
                        prob_lo: p.lo,
                        prob_hi: p.hi,
                    })
                    .collect()
            })
            .unwrap_or_default(),
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
