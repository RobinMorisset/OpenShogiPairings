//! Tournament-wide settings.

use std::collections::HashSet;
use std::num::NonZeroU32;

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

use crate::player::Grade;

/// An integer-percent multiplier (`100` = ×1.0), read as a float via
/// [`Ratio::as_f64`]. The wire form stays a bare number; wrapping it means the
/// percent → float interpretation lives in one place instead of at every read.
///
/// `Deserialize` is hand-written (rather than `#[serde(from = …)]` or a field
/// `deserialize_with`) purely so ts-rs' serde-compat pass has no attribute to
/// choke on — it emits a spurious warning for anything beyond `rename`/`default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Ratio(u32);

impl Ratio {
    pub fn from_percent(percent: u32) -> Self {
        Ratio(percent)
    }

    pub fn as_f64(self) -> f64 {
        self.0 as f64 / 100.0
    }
}

impl<'de> Deserialize<'de> for Ratio {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u32::deserialize(d).map(Ratio::from_percent)
    }
}

/// A [`Ratio`] that can never fall below ×1.0 (percent ≥ 100). Used where a value
/// below parity is nonsensical — an upward ELO revision is never *harder* than a
/// downward one, and a provisional prior is never *more* reliable than an
/// established one. The floor is clamped in the one constructor (used by
/// construction *and* deserialization), so the `.max(100)` that used to live in
/// the getters *and* in [`TournamentSettings::normalized`] is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RatioAtLeastOne(u32);

impl RatioAtLeastOne {
    pub fn from_percent(percent: u32) -> Self {
        RatioAtLeastOne(percent.max(100))
    }

    pub fn as_f64(self) -> f64 {
        self.0 as f64 / 100.0
    }
}

impl<'de> Deserialize<'de> for RatioAtLeastOne {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u32::deserialize(d).map(RatioAtLeastOne::from_percent)
    }
}

/// The K (prior width) of an unrated player, `≥ 1` — a zero-width prior would
/// divide by zero in the estimator. The floor is clamped in the constructor
/// (construction and deserialization both), retiring the `.max(1)` that lived in
/// the getter and in [`TournamentSettings::normalized`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct UnratedK(u32);

impl UnratedK {
    pub fn new(k: u32) -> Self {
        UnratedK(k.max(1))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for UnratedK {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u32::deserialize(d).map(UnratedK::new)
    }
}

/// Test-only convenience: compare a value newtype against a bare integer in its
/// stored unit (a [`Ratio`]/[`RatioAtLeastOne`] against its percent, a
/// [`UnratedK`] against its K), so the settings tests read `looseness == 100`
/// rather than unwrapping. Gated on `test`, so production stays strict.
#[cfg(test)]
impl PartialEq<u32> for Ratio {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
impl PartialEq<u32> for RatioAtLeastOne {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
impl PartialEq<u32> for UnratedK {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

/// Which player a score group sends *up* as its ascending floater when it has to
/// pair across groups. The descending floater is always the last (weakest) of the
/// upper group; this only chooses the ascending one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum FloaterStyle {
    /// Classic Swiss: the first (strongest) of the lower group floats up.
    #[default]
    Classic,
    /// Median Swiss: the median of the lower group floats up.
    Median,
}

/// Club protection: whether the pairing engine avoids pairing players from the
/// same club, and — when on — for how long and with which clubs exempt. Off by
/// default. Folding the round window and the exempt list *into* the `On` variant
/// makes "a round limit or exempt list while protection is disabled"
/// unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClubProtection {
    /// No club protection.
    #[default]
    Off,
    /// Avoid pairing club-mates.
    On {
        /// `None` = every round; `Some(n)` = only rounds `1..=n`, later rounds
        /// pairing on score alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(as = "Option::<u32>")]
        rounds: Option<NonZeroU32>,
        /// Clubs exempt from protection — the "local club" case, where many
        /// entrants share the host club and are expected to meet. Matched
        /// case-insensitively.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exempt_clubs: Vec<String>,
    },
}

impl ClubProtection {
    /// Whether protection applies to the given (1-based) round.
    pub fn active(&self, round: u32) -> bool {
        match self {
            ClubProtection::Off => false,
            ClubProtection::On { rounds, .. } => rounds.is_none_or(|n| round <= n.get()),
        }
    }

    /// The exempt clubs in canonical (normalized) form; empty when off.
    fn exempt_normalized(&self) -> HashSet<String> {
        match self {
            ClubProtection::On { exempt_clubs, .. } => exempt_clubs
                .iter()
                .map(|c| TournamentSettings::normalize_club(c))
                .collect(),
            ClubProtection::Off => HashSet::new(),
        }
    }

    /// `skip_serializing_if` helper — `Off` is the default and omitted from JSON.
    fn is_off(&self) -> bool {
        matches!(self, ClubProtection::Off)
    }
}

/// How the referee wants handicap games treated in this tournament. When handicaps
/// are enabled, [`HandicapDisplay`] controls what the pairings view shows and
/// `wiel_rule` whether the giver always counts as the winner. Folding the Wiel
/// rule *into* the `Enabled` variant makes "Wiel on while handicaps are off"
/// unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HandicapPolicy {
    /// No handicap column at all.
    None,
    /// Handicaps are played. `display` chooses whether a suggested-handicap column
    /// is shown; `wiel_rule` whether the giver always counts as the winner.
    Enabled {
        display: HandicapDisplay,
        /// The "Wiel" rule: whether a handicap game always counts as a win for the
        /// giver in the standings and for pairing, regardless of the actual result.
        /// Off by default: the actual result then counts, like any other game.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        wiel_rule: bool,
    },
}

impl Default for HandicapPolicy {
    /// Handicaps allowed (picker shown, no suggestion), Wiel off — the historical
    /// default.
    fn default() -> Self {
        HandicapPolicy::Enabled {
            display: HandicapDisplay::Allowed,
            wiel_rule: false,
        }
    }
}

impl HandicapPolicy {
    /// Whether a handicap game counts as a win for the giver regardless of result.
    fn wiel_rule(self) -> bool {
        matches!(
            self,
            HandicapPolicy::Enabled {
                wiel_rule: true,
                ..
            }
        )
    }
}

/// What the pairings view shows once handicaps are [`enabled`](HandicapPolicy::Enabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum HandicapDisplay {
    /// The handicap picker is shown, but no suggested-handicap column.
    #[default]
    Allowed,
    /// The handicap picker plus a suggested-handicap column.
    Suggested,
}

/// The shape of every player's Bayesian ELO prior (see [`crate::elo`]).
///
/// `Gaussian` is the historical `N(rating, σ₀²)`: a thin-tailed penalty whose
/// restoring force *grows* with the deviation, so it pulls the estimate back to
/// the registration rating ever harder the further it drifts and caps a single
/// result's effect near the player's K. `Laplace` is a Huber-smoothed
/// asymmetric-Laplace penalty of the *same width* but with exponential — hence
/// fatter — tails: its restoring force is *constant*, so a sustained run of
/// surprising results (e.g. an under-rated improver beating stronger opponents)
/// moves the estimate much further before the prior reins it in. `Flat` is the
/// improper limit — *no* prior at all: the estimate is the maximum-likelihood
/// performance rating over the games, reproducing the FESA rating program
/// (`turnering.py`) for unrated newcomers (an all-loss player floors to 1, an
/// all-win player is rated as if they had drawn their strongest opponent).
/// `Gaussian` and `Laplace` can additionally be made asymmetric via the
/// per-category `elo_upward_looseness_*` knobs, which widen the upward arm so an
/// *upward* revision clears on less evidence than a downward one (for the Gaussian
/// this is a two-piece normal; for the Laplace, a wider upward scale); `Flat` has
/// no arm to widen. See `docs/elo-pairing-mode.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum EloPriorShape {
    /// Thin-tailed `N(rating, σ₀²)` — the default. Behaviour-neutral (exactly the
    /// historical estimator) unless a per-category upward-looseness knob is raised,
    /// which turns it into an asymmetric two-piece normal.
    #[default]
    Gaussian,
    /// Huber-smoothed Laplace: same width as the Gaussian but fatter (exponential)
    /// tails. Also honours the per-category `elo_upward_looseness_*` asymmetry
    /// knobs.
    Laplace,
    /// **Flat** (improper/uniform) prior — *no* pull toward a prior mean. The
    /// estimate becomes the maximum-likelihood *performance rating* over the
    /// player's games, reproducing the historical `turnering.py` (`performance2`)
    /// treatment of unrated players rather than any Bayesian anchor. Intended for
    /// the `elo_prior_shape_unrated` slot: a strong newcomer's rating is then read
    /// straight off the strength of the field they beat, with no regularisation
    /// toward the unrated centre. Because a flat prior leaves the all-win / all-loss
    /// likelihood unbounded, those scorelines follow `turnering.py`'s guards (see
    /// [`crate::elo::estimate_elos`]): an all-loss player floors to `1`, an all-win
    /// player is rated as if they had *drawn* their strongest opponent, and a player
    /// with no games stays at the seed. The upward-looseness knobs do not apply
    /// (there is no arm to widen). Not log-concave-strengthening, so mixing it onto
    /// well-connected fields is fine but it supplies no curvature of its own.
    Flat,
}

/// What a MacMahon threshold compares against — an ELO rating or a dan/kyu
/// grade. A tournament's thresholds can freely mix both kinds (e.g. some bands
/// drawn from ELO, others from grade), each counted independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThresholdCriterion {
    /// Met when the player's rating is at or above `value`.
    Elo { value: u32 },
    /// Met when the player's grade is at or above `grade` (see
    /// [`Grade::rank`]).
    Grade { grade: Grade },
}

impl ThresholdCriterion {
    /// Whether a player with the given rating/grade meets this threshold. A
    /// missing rating or grade never meets the corresponding kind of
    /// threshold, same as an unrated player never meeting an ELO threshold.
    fn met_by(self, rating: Option<u32>, grade: Option<Grade>) -> bool {
        match self {
            ThresholdCriterion::Elo { value } => rating.is_some_and(|r| r >= value),
            ThresholdCriterion::Grade { grade: g } => grade.is_some_and(|pg| pg >= g),
        }
    }

    /// A key that sorts ELO thresholds by value, then grade thresholds by
    /// strength — used to keep the stored order canonical and independent of
    /// entry order, without implying that an ELO and a grade threshold are
    /// otherwise comparable.
    fn sort_key(self) -> (u8, i64) {
        match self {
            ThresholdCriterion::Elo { value } => (0, value as i64),
            ThresholdCriterion::Grade { grade } => (1, grade.rank()),
        }
    }
}

/// A single MacMahon starting-points band: the criterion (ELO or grade) a
/// player must meet or exceed to count it, plus (for degressive MacMahon) the
/// round after which it stops applying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct MacMahonThreshold {
    pub criterion: ThresholdCriterion,
    /// Degressive MacMahon ("accelerated Swiss"): if `Some(n)`, this threshold
    /// stops applying after round `n` — dropped as soon as `n` rounds are
    /// complete — so the starting-point spread shrinks as the tournament
    /// converges. `None` means it's always in effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drops_after_round: Option<u32>,
}

impl MacMahonThreshold {
    /// An ELO threshold that never degresses.
    pub fn elo(value: u32) -> Self {
        MacMahonThreshold {
            criterion: ThresholdCriterion::Elo { value },
            drops_after_round: None,
        }
    }

    /// A grade threshold that never degresses.
    pub fn grade(grade: Grade) -> Self {
        MacMahonThreshold {
            criterion: ThresholdCriterion::Grade { grade },
            drops_after_round: None,
        }
    }
}

/// A tie-break metric the referee can put in the ranking order.
///
/// Every metric comes in two flavours: the `…M` variants score an opponent by
/// their **MacMahon-inclusive points** (MacMahon start + wins — the classic
/// behaviour), the `…W` variants by their **wins only**. All twelve are always
/// computed for every player ([`crate::standings::Standing`]); the referee picks
/// which ones rank the table, and in which order, via
/// [`TournamentSettings::tiebreaks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum Tiebreak {
    /// Total score: MacMahon start + wins. Normally the primary ranking key, but
    /// the referee can reorder it like any other criterion.
    Points,
    /// Sum of opponents' points (classic SOS).
    SosM,
    /// Sum of opponents' wins.
    SosW,
    /// Sum of defeated opponents' points (classic SODOS).
    SodosM,
    /// Sum of defeated opponents' wins.
    SodosW,
    /// Sum of opponents' SOSM (classic SOSOS).
    SososM,
    /// Sum of opponents' SOSW.
    SososW,
    /// SOSM dropping the single lowest-scoring opponent (Buchholz cut 1).
    SosM1,
    /// SOSM dropping the two lowest-scoring opponents (Buchholz cut 2).
    SosM2,
    /// SOSW dropping the single lowest-scoring opponent.
    SosW1,
    /// SOSW dropping the two lowest-scoring opponents.
    SosW2,
    /// Cumulative sum of the running points total after each round.
    CussM,
    /// Cumulative sum of the running win total after each round.
    CussW,
    /// Direct confrontation: among players still tied on every earlier
    /// criterion, the sum of a player's wins against the others in that tied
    /// group — but only when the group is a complete subgraph (every pair in
    /// it has played each other). Otherwise 0, and the group stays tied.
    Dc,
    /// The live Bayesian ELO estimate (experimental ELO pairing mode). Ranks
    /// higher estimate first, like the other metrics.
    EstElo,
}

impl Tiebreak {
    /// The default ranking order — the classic points → SOS → SODOS → SOSOS
    /// order that preceded this setting, so pre-existing tournaments (and new
    /// ones that never touch the setting) rank exactly as before.
    pub fn default_order() -> Vec<Tiebreak> {
        vec![
            Tiebreak::Points,
            Tiebreak::SosM,
            Tiebreak::SodosM,
            Tiebreak::SososM,
        ]
    }
}

/// The default value for [`TournamentSettings::tiebreaks`]. Kept as a free
/// function so `#[serde(default = …)]` can name it for tournaments saved before
/// the field existed.
fn default_tiebreaks() -> Vec<Tiebreak> {
    Tiebreak::default_order()
}

/// The default ELO-estimate K multiplier (×1.0). Named so `#[serde(default = …)]`
/// can fill it in for tournaments saved before the field existed.
fn default_elo_k_multiplier_percent() -> Ratio {
    Ratio::from_percent(100)
}

/// The default extra K multiplier for a provisionally-rated player (×2.0). Named
/// so `#[serde(default = …)]` can fill it in for tournaments saved before the
/// field existed.
fn default_elo_provisional_multiplier_percent() -> RatioAtLeastOne {
    RatioAtLeastOne::from_percent(200)
}

/// The default center of the unrated-player prior (ELO). Named so
/// `#[serde(default = …)]` can fill it in for tournaments saved before the field
/// existed. Matches [`crate::elo::UNRATED_PRIOR_MEAN`].
fn default_elo_unrated_prior_center() -> u32 {
    crate::elo::UNRATED_PRIOR_MEAN as u32
}

/// The default K for the unrated-player prior. Named so `#[serde(default = …)]`
/// can fill it in for tournaments saved before the field existed. Matches
/// [`crate::elo::UNRATED_PRIOR_DEFAULT_K`] (≈ the historical `σ 350`).
fn default_elo_unrated_k() -> UnratedK {
    UnratedK::new(crate::elo::UNRATED_PRIOR_DEFAULT_K as u32)
}

/// The default upward-looseness ratio `r` for the Laplace prior (×1.0 =
/// symmetric). Named so `#[serde(default = …)]` can fill it in for tournaments
/// saved before the field existed.
fn default_elo_upward_looseness_percent() -> RatioAtLeastOne {
    RatioAtLeastOne::from_percent(100)
}

/// The configuration of the live Bayesian ELO estimate. Exists only where an
/// estimate is actually maintained — inside [`PairingMode::Elo`] (it drives
/// pairing) or [`MacMahonSource::FromEstimate`] (it drives MacMahon points) — so
/// there is no way to carry estimator settings with no estimate behind them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct EloEstimator {
    /// K multiplier `m` (see [`TournamentSettings::elo_k_multiplier`]). `0` pins
    /// every rated player to their registration rating.
    #[serde(default = "default_elo_k_multiplier_percent")]
    pub k_multiplier: Ratio,
    /// Extra K multiplier for a provisionally-rated player (≥ ×1.0).
    #[serde(default = "default_elo_provisional_multiplier_percent")]
    pub provisional_multiplier: RatioAtLeastOne,
    /// Center (mean) of the unrated-player prior, on the ELO scale.
    #[serde(default = "default_elo_unrated_prior_center")]
    pub unrated_prior_center: u32,
    /// K (width) of the unrated-player prior (≥ 1).
    #[serde(default = "default_elo_unrated_k")]
    pub unrated_k: UnratedK,
    /// Prior shape for an established (reliably-rated) player.
    #[serde(default)]
    pub prior_shape_established: EloPriorShape,
    /// Prior shape for a provisionally-rated player.
    #[serde(default)]
    pub prior_shape_provisional: EloPriorShape,
    /// Prior shape for an unrated player (the one heavy-tailed category).
    #[serde(default)]
    pub prior_shape_unrated: EloPriorShape,
    /// Upward-looseness ratio `r` for an established player (≥ ×1.0).
    #[serde(default = "default_elo_upward_looseness_percent")]
    pub upward_looseness_established: RatioAtLeastOne,
    /// Upward-looseness ratio `r` for a provisionally-rated player (≥ ×1.0).
    #[serde(default = "default_elo_upward_looseness_percent")]
    pub upward_looseness_provisional: RatioAtLeastOne,
    /// Upward-looseness ratio `r` for an unrated player (≥ ×1.0).
    #[serde(default = "default_elo_upward_looseness_percent")]
    pub upward_looseness_unrated: RatioAtLeastOne,
}

impl Default for EloEstimator {
    fn default() -> Self {
        EloEstimator {
            k_multiplier: default_elo_k_multiplier_percent(),
            provisional_multiplier: default_elo_provisional_multiplier_percent(),
            unrated_prior_center: default_elo_unrated_prior_center(),
            unrated_k: default_elo_unrated_k(),
            prior_shape_established: EloPriorShape::default(),
            prior_shape_provisional: EloPriorShape::default(),
            prior_shape_unrated: EloPriorShape::default(),
            upward_looseness_established: default_elo_upward_looseness_percent(),
            upward_looseness_provisional: default_elo_upward_looseness_percent(),
            upward_looseness_unrated: default_elo_upward_looseness_percent(),
        }
    }
}

/// Where MacMahon starting points come from: the static registration rating, or
/// the live ELO estimate (which then carries its own [`EloEstimator`] config).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MacMahonSource {
    /// From each player's static registration rating (or grade).
    #[default]
    Static,
    /// From the live ELO estimate, recomputed each round. Inert unless there is
    /// at least one ELO-based threshold to compare against.
    FromEstimate { estimator: EloEstimator },
}

/// The MacMahon configuration of a Swiss tournament: the starting-point
/// thresholds and where those points are drawn from. Lives under
/// [`PairingMode::Swiss`] because MacMahon is a Swiss concept — ELO pairing has
/// none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct MacMahon {
    /// Thresholds (sorted, de-duplicated) defining the starting groups. Empty
    /// means everyone starts at 0.
    #[serde(default)]
    pub thresholds: Vec<MacMahonThreshold>,
    #[serde(default)]
    pub source: MacMahonSource,
}

/// How the tournament is paired: classic Swiss/MacMahon over static ratings, or
/// the experimental ELO mode over a live estimate. Making this a sum type is what
/// keeps the Swiss-only knobs (floater style, airtight groups, club protection,
/// MacMahon) from coexisting with ELO pairing, and the estimator from existing
/// without an estimate to configure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PairingMode {
    /// Swiss / MacMahon pairing.
    Swiss {
        #[serde(default)]
        floater_style: FloaterStyle,
        /// "Airtight groups": if set, forbid pairing across MacMahon groups during
        /// rounds `1..=n`. Meaningless without thresholds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(as = "Option::<u32>")]
        airtight_groups: Option<NonZeroU32>,
        #[serde(default, skip_serializing_if = "ClubProtection::is_off")]
        club_protection: ClubProtection,
        #[serde(default)]
        macmahon: MacMahon,
    },
    /// Experimental ELO pairing over a live estimate.
    Elo {
        #[serde(default)]
        estimator: EloEstimator,
    },
}

impl Default for PairingMode {
    fn default() -> Self {
        PairingMode::Swiss {
            floater_style: FloaterStyle::default(),
            airtight_groups: None,
            club_protection: ClubProtection::Off,
            macmahon: MacMahon::default(),
        }
    }
}

/// Configuration that isn't tied to a single player or round.
///
/// Kept as its own record so it can grow (time controls, tie-break choices, …)
/// without disturbing the rest of the tournament shape. Added as an additive,
/// defaulted field, so tournaments saved before it existed still load (with no
/// MacMahon groups).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TournamentSettings {
    /// How the tournament is paired: Swiss/MacMahon over static ratings (with the
    /// floater/airtight/club/MacMahon knobs) or ELO mode over a live estimate (see
    /// [`PairingMode`]). Defaults to plain Swiss.
    #[serde(default)]
    pub pairing: PairingMode,
    /// Whether this is a hybrid tournament with a direct-elimination cup among the
    /// top eligible players. Off by default.
    #[serde(default)]
    pub cup_enabled: bool,
    /// Whether the referee may flag individual boards as "long games" that last
    /// two rounds and score two points for the winner. Off by default. See
    /// `docs/two-round-boards.md`.
    #[serde(default)]
    pub long_boards_enabled: bool,
    /// How handicap games are treated (see [`HandicapPolicy`]).
    #[serde(default)]
    pub handicap_policy: HandicapPolicy,
    /// Whether a player marked **absent** for a round is awarded half a point
    /// (rather than the default zero). Off by default.
    ///
    /// This is only the *default*, applied to each absence as the round is
    /// confirmed: what a sit-out actually scores is recorded on the round
    /// ([`Sitout::value`]), where the referee can override it per player and per
    /// round. So turning this on affects rounds confirmed from then on, not ones
    /// already played.
    ///
    /// [`Sitout::value`]: crate::round::Sitout::value
    #[serde(default)]
    pub half_point_absences: bool,
    /// The criteria used to rank the standings, in order of priority (the
    /// tournament number breaks anything still level). Defaults to the classic
    /// points → SOS → SODOS → SOSOS order.
    #[serde(default = "default_tiebreaks")]
    pub tiebreaks: Vec<Tiebreak>,
}

impl Default for TournamentSettings {
    fn default() -> Self {
        TournamentSettings {
            pairing: PairingMode::default(),
            cup_enabled: false,
            long_boards_enabled: false,
            handicap_policy: HandicapPolicy::default(),
            half_point_absences: false,
            tiebreaks: default_tiebreaks(),
        }
    }
}

impl TournamentSettings {
    /// The MacMahon starting points for a player with the given rating/grade,
    /// using the full configured thresholds (i.e. before any degressive
    /// removal). This is [`macmahon_points_at`](Self::macmahon_points_at) at
    /// round 0.
    pub fn macmahon_points(&self, rating: Option<u32>, grade: Option<Grade>) -> u32 {
        self.macmahon_points_at(rating, grade, 0)
    }

    /// The threshold criteria in effect once `rounds_played` rounds are
    /// complete: those whose
    /// [`drops_after_round`](MacMahonThreshold::drops_after_round) is `None`
    /// or still ahead of `rounds_played` (a threshold dropping "after round N"
    /// applies as soon as round N is complete, i.e. for pairing round N+1 and
    /// standings after N).
    pub fn effective_macmahon_thresholds(&self, rounds_played: u32) -> Vec<ThresholdCriterion> {
        self.macmahon_thresholds()
            .iter()
            .filter(|t| t.drops_after_round.is_none_or(|r| rounds_played < r))
            .map(|t| t.criterion)
            .collect()
    }

    /// The configured MacMahon thresholds (empty under ELO pairing, which has no
    /// MacMahon).
    pub fn macmahon_thresholds(&self) -> &[MacMahonThreshold] {
        match &self.pairing {
            PairingMode::Swiss { macmahon, .. } => &macmahon.thresholds,
            PairingMode::Elo { .. } => &[],
        }
    }

    /// Which player each score group sends up as its ascending floater (Swiss
    /// only; a default under ELO pairing, which doesn't float).
    pub fn floater_style(&self) -> FloaterStyle {
        match &self.pairing {
            PairingMode::Swiss { floater_style, .. } => *floater_style,
            PairingMode::Elo { .. } => FloaterStyle::default(),
        }
    }

    /// The live-estimate config, if an estimate is maintained — ELO pairing, or
    /// estimate-based MacMahon. `None` in plain Swiss, where the estimator can't
    /// exist.
    fn estimator(&self) -> Option<&EloEstimator> {
        match &self.pairing {
            PairingMode::Elo { estimator } => Some(estimator),
            PairingMode::Swiss { macmahon, .. } => match &macmahon.source {
                MacMahonSource::FromEstimate { estimator } => Some(estimator),
                MacMahonSource::Static => None,
            },
        }
    }

    /// The MacMahon starting points for a player once `rounds_played` rounds
    /// are complete: the number of *effective* thresholds the player's rating
    /// or grade meets or exceeds. A player missing the value a threshold needs
    /// never meets it (see [`ThresholdCriterion::met_by`]).
    pub fn macmahon_points_at(
        &self,
        rating: Option<u32>,
        grade: Option<Grade>,
        rounds_played: u32,
    ) -> u32 {
        self.effective_macmahon_thresholds(rounds_played)
            .into_iter()
            .filter(|c| c.met_by(rating, grade))
            .count() as u32
    }

    /// The canonical form of a club name for comparison: trimmed and lower-cased,
    /// so "Paris" and " paris " count as the same club.
    pub fn normalize_club(club: &str) -> String {
        club.trim().to_lowercase()
    }

    /// Whether club protection applies to the given (1-based) round: enabled and,
    /// if a round limit is set, within it.
    pub fn club_protection_active(&self, round: u32) -> bool {
        match &self.pairing {
            PairingMode::Swiss {
                club_protection, ..
            } => club_protection.active(round),
            PairingMode::Elo { .. } => false,
        }
    }

    /// Whether the "Wiel" rule is in effect: a handicap game always counts as a
    /// win for the giver. Always `false` unless handicaps are enabled.
    pub fn handicap_wiel_rule(&self) -> bool {
        self.handicap_policy.wiel_rule()
    }

    /// Whether "airtight groups" applies to the given (1-based) round: within
    /// the configured window, if any.
    pub fn airtight_groups_active(&self, round: u32) -> bool {
        match &self.pairing {
            PairingMode::Swiss {
                airtight_groups, ..
            } => airtight_groups.is_some_and(|n| round <= n.get()),
            PairingMode::Elo { .. } => false,
        }
    }

    /// The exempt clubs in canonical (normalized) form, for membership tests.
    pub fn exempt_clubs_normalized(&self) -> HashSet<String> {
        match &self.pairing {
            PairingMode::Swiss {
                club_protection, ..
            } => club_protection.exempt_normalized(),
            PairingMode::Elo { .. } => HashSet::new(),
        }
    }

    /// Return these settings in canonical form: thresholds sorted ascending by
    /// value and de-duplicated (keeping the first entry for a repeated value,
    /// and treating a `drops_after_round` of 0 as "never drops" since it can't
    /// take effect before round 1 anyway), and exempt clubs trimmed,
    /// emptied-dropped and de-duplicated case-insensitively. Independent of the
    /// order fields were entered, so pairing/standings are reproducible from the
    /// stored settings.
    pub fn normalized(mut self) -> Self {
        // The Swiss-only knobs live under `pairing`; ELO pairing has nothing to
        // canonicalize here.
        if let PairingMode::Swiss {
            club_protection,
            macmahon,
            ..
        } = &mut self.pairing
        {
            macmahon.thresholds =
                Self::normalize_thresholds(std::mem::take(&mut macmahon.thresholds));

            // Exempt clubs: keep the first spelling of each, trimmed and non-empty.
            if let ClubProtection::On { exempt_clubs, .. } = club_protection {
                let mut seen = HashSet::new();
                *exempt_clubs = std::mem::take(exempt_clubs)
                    .into_iter()
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty() && seen.insert(c.to_lowercase()))
                    .collect();
            }
        }

        // Tie-breaks: drop duplicates keeping the first occurrence (so the order
        // is meaningful and each metric appears at most once as a column).
        let mut seen_tb = HashSet::new();
        self.tiebreaks.retain(|&tb| seen_tb.insert(tb));
        // The estimated-ELO tie-break is meaningless unless a live estimate is
        // actually maintained — without one it just sits at each player's
        // registration rating — so it is not a valid ranking criterion there.
        if !self.elo_estimate_live() {
            self.tiebreaks.retain(|&tb| tb != Tiebreak::EstElo);
        }

        // The ELO value invariants (`elo_k_multiplier` may be 0 to pin rated
        // players; the provisional/looseness ratios stay ≥ ×1.0; the unrated K
        // stays ≥ 1; an airtight window of 0 means off) are now enforced by the
        // field types themselves — see [`Ratio`], [`RatioAtLeastOne`] and the
        // `NonZeroU32` fields — so there is nothing left to clamp here.

        self
    }

    /// Whether a live ELO estimate needs to be maintained for **pairing**
    /// purposes — i.e. the ELO pairing mode. Used to gate the pairing model's ELO
    /// context (edge weights, bye ranks). Note this is *not* the only place a live
    /// estimate is maintained: [`Self::macmahon_from_estimate_active`] maintains
    /// one for scoring even in plain Swiss.
    pub fn elo_estimate_needed(&self) -> bool {
        matches!(self.pairing, PairingMode::Elo { .. })
    }

    /// Whether MacMahon starting points are actually drawn from the live ELO
    /// estimate: the source is [`MacMahonSource::FromEstimate`] *and* there is at
    /// least one ELO-based threshold for the estimate to be compared against (with
    /// only grade thresholds, or none, the estimate would change nothing, so the
    /// (non-trivial) estimate computation is skipped).
    pub fn macmahon_from_estimate_active(&self) -> bool {
        match &self.pairing {
            PairingMode::Swiss { macmahon, .. } => {
                matches!(macmahon.source, MacMahonSource::FromEstimate { .. })
                    && macmahon
                        .thresholds
                        .iter()
                        .any(|t| matches!(t.criterion, ThresholdCriterion::Elo { .. }))
            }
            PairingMode::Elo { .. } => false,
        }
    }

    /// Whether a live ELO estimate is maintained *at all* — for pairing (ELO mode)
    /// or for scoring (estimate-based MacMahon). This is the single gate for
    /// whether the estimated-ELO value is computed, shown as a standings column,
    /// and usable as a ranking tie-break; the frontend mirrors it as
    /// `eloEstimateLive`. Keep those in sync — splitting this rule is what let the
    /// estimate column diverge between the two modes.
    pub fn elo_estimate_live(&self) -> bool {
        self.elo_estimate_needed() || self.macmahon_from_estimate_active()
    }

    /// The ELO-estimate K multiplier `m` as a float. (The `map_or` default is
    /// unreachable — these getters are only read while an estimate is maintained,
    /// so [`Self::estimator`] is `Some`.)
    pub fn elo_k_multiplier(&self) -> f64 {
        self.estimator().map_or(1.0, |e| e.k_multiplier.as_f64())
    }

    /// The extra K multiplier for a provisionally-rated player, as a float.
    pub fn elo_provisional_multiplier(&self) -> f64 {
        self.estimator()
            .map_or(2.0, |e| e.provisional_multiplier.as_f64())
    }

    /// The center (mean) of the unrated-player prior, as a float.
    pub fn elo_unrated_prior_center(&self) -> f64 {
        self.estimator()
            .map_or(600.0, |e| e.unrated_prior_center as f64)
    }

    /// The K for the unrated-player prior, as a float (`≥ 1` by construction — a
    /// zero-width prior would divide by zero in the solver and freeze the estimate).
    pub fn elo_unrated_k(&self) -> f64 {
        self.estimator().map_or(705.0, |e| e.unrated_k.get() as f64)
    }

    /// The prior shape for an established player.
    pub fn elo_prior_shape_established(&self) -> EloPriorShape {
        self.estimator()
            .map_or_else(EloPriorShape::default, |e| e.prior_shape_established)
    }

    /// The prior shape for a provisionally-rated player.
    pub fn elo_prior_shape_provisional(&self) -> EloPriorShape {
        self.estimator()
            .map_or_else(EloPriorShape::default, |e| e.prior_shape_provisional)
    }

    /// The prior shape for an unrated player.
    pub fn elo_prior_shape_unrated(&self) -> EloPriorShape {
        self.estimator()
            .map_or_else(EloPriorShape::default, |e| e.prior_shape_unrated)
    }

    /// The [`EloPriorShape::Laplace`] upward-looseness ratio `r` for an
    /// **established** player, as a float (`≥ 1.0` by construction — an upward
    /// revision is never harder than a downward one). `1.0` is symmetric.
    pub fn elo_upward_looseness_established(&self) -> f64 {
        self.estimator()
            .map_or(1.0, |e| e.upward_looseness_established.as_f64())
    }

    /// The Laplace upward-looseness ratio `r` for a **provisionally-rated** player,
    /// as a float (`≥ 1.0` by construction).
    pub fn elo_upward_looseness_provisional(&self) -> f64 {
        self.estimator()
            .map_or(1.0, |e| e.upward_looseness_provisional.as_f64())
    }

    /// The Laplace upward-looseness ratio `r` for an **unrated** player, as a
    /// float (`≥ 1.0` by construction).
    pub fn elo_upward_looseness_unrated(&self) -> f64 {
        self.estimator()
            .map_or(1.0, |e| e.upward_looseness_unrated.as_f64())
    }

    /// Sort thresholds (ELO ones by value, then grade ones by strength) and
    /// drop duplicate criteria (keeping the first entry) — the canonical form
    /// kept in the settings, independent of the order they were entered. A
    /// `drops_after_round` of 0 is normalized to `None`, since it can't take
    /// effect before round 1 anyway.
    pub fn normalize_thresholds(mut thresholds: Vec<MacMahonThreshold>) -> Vec<MacMahonThreshold> {
        for t in &mut thresholds {
            if t.drops_after_round == Some(0) {
                t.drops_after_round = None;
            }
        }
        thresholds.sort_by_key(|t| t.criterion.sort_key());
        thresholds.dedup_by_key(|t| t.criterion);
        thresholds
    }
}

/// Test-only builders that keep settings construction readable now that the
/// pairing knobs live inside [`PairingMode`]. Each is a no-op in the mode it
/// doesn't apply to (e.g. `with_thresholds` on ELO settings), matching how the
/// real code ignores them.
#[cfg(test)]
impl TournamentSettings {
    /// Swiss settings with the given static MacMahon thresholds.
    pub(crate) fn with_thresholds(mut self, thresholds: Vec<MacMahonThreshold>) -> Self {
        if let PairingMode::Swiss { macmahon, .. } = &mut self.pairing {
            macmahon.thresholds = thresholds;
        }
        self
    }

    /// Draw MacMahon points from the live estimate (Swiss only).
    pub(crate) fn with_macmahon_from_estimate(mut self) -> Self {
        if let PairingMode::Swiss { macmahon, .. } = &mut self.pairing {
            macmahon.source = MacMahonSource::FromEstimate {
                estimator: EloEstimator::default(),
            };
        }
        self
    }

    /// ELO pairing mode with the default estimator.
    pub(crate) fn elo_pairing() -> Self {
        TournamentSettings {
            pairing: PairingMode::Elo {
                estimator: EloEstimator::default(),
            },
            ..Default::default()
        }
    }

    /// Tweak the estimator config in whichever mode carries one (ELO pairing or
    /// estimate-based MacMahon); a no-op in plain Swiss.
    pub(crate) fn map_estimator(mut self, f: impl FnOnce(&mut EloEstimator)) -> Self {
        match &mut self.pairing {
            PairingMode::Elo { estimator } => f(estimator),
            PairingMode::Swiss {
                macmahon:
                    MacMahon {
                        source: MacMahonSource::FromEstimate { estimator },
                        ..
                    },
                ..
            } => f(estimator),
            PairingMode::Swiss { .. } => {}
        }
        self
    }

    /// Set the Swiss floater style.
    pub(crate) fn with_floater(mut self, style: FloaterStyle) -> Self {
        if let PairingMode::Swiss { floater_style, .. } = &mut self.pairing {
            *floater_style = style;
        }
        self
    }

    /// Set the airtight-groups window (Swiss).
    pub(crate) fn with_airtight(mut self, rounds: Option<NonZeroU32>) -> Self {
        if let PairingMode::Swiss {
            airtight_groups, ..
        } = &mut self.pairing
        {
            *airtight_groups = rounds;
        }
        self
    }

    /// Set club protection (Swiss).
    pub(crate) fn with_club(mut self, protection: ClubProtection) -> Self {
        if let PairingMode::Swiss {
            club_protection, ..
        } = &mut self.pairing
        {
            *club_protection = protection;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A threshold with no degressive round, for terser test setup.
    fn mmt(value: u32) -> MacMahonThreshold {
        MacMahonThreshold::elo(value)
    }

    /// A threshold that drops after the given round.
    fn mmt_drops(value: u32, round: u32) -> MacMahonThreshold {
        MacMahonThreshold {
            criterion: ThresholdCriterion::Elo { value },
            drops_after_round: Some(round),
        }
    }

    /// The ELO criterion for a given value, for terser assertions.
    fn elo(value: u32) -> ThresholdCriterion {
        ThresholdCriterion::Elo { value }
    }

    #[test]
    fn macmahon_points_count_thresholds_met() {
        let s = TournamentSettings::default().with_thresholds(vec![mmt(1200), mmt(1700)]);
        assert_eq!(s.macmahon_points(Some(1000), None), 0);
        assert_eq!(s.macmahon_points(Some(1200), None), 1); // inclusive lower bound
        assert_eq!(s.macmahon_points(Some(1699), None), 1);
        assert_eq!(s.macmahon_points(Some(1700), None), 2);
        assert_eq!(s.macmahon_points(Some(2500), None), 2);
        assert_eq!(s.macmahon_points(None, None), 0); // unrated → below every threshold
    }

    #[test]
    fn grade_thresholds_count_independently_of_elo_thresholds() {
        // Mixed thresholds: an ELO band and a grade band, each counted on its
        // own axis — a strong-graded but low-rated player (or vice versa) can
        // meet one without the other.
        let s = TournamentSettings::default().with_thresholds(vec![
            MacMahonThreshold::elo(1500),
            MacMahonThreshold::grade(Grade::dan(1)),
        ]);
        // Meets neither.
        assert_eq!(s.macmahon_points(Some(1000), Some(Grade::kyu(5))), 0);
        // Meets only the grade threshold.
        assert_eq!(s.macmahon_points(Some(1000), Some(Grade::dan(3))), 1);
        // Meets only the ELO threshold.
        assert_eq!(s.macmahon_points(Some(2000), Some(Grade::kyu(5))), 1);
        // Meets both.
        assert_eq!(s.macmahon_points(Some(2000), Some(Grade::dan(3))), 2);
        // No grade at all never meets the grade threshold, same as unrated
        // never meeting an ELO one.
        assert_eq!(s.macmahon_points(Some(2000), None), 1);
    }

    #[test]
    fn no_thresholds_means_zero_points() {
        let s = TournamentSettings::default();
        assert_eq!(s.macmahon_points(Some(9000), None), 0);
        assert_eq!(s.macmahon_points(None, None), 0);
    }

    #[test]
    fn normalize_sorts_and_dedups() {
        assert_eq!(
            TournamentSettings::normalize_thresholds(vec![
                mmt(1700),
                mmt(1200),
                mmt(1200),
                mmt(1500)
            ]),
            vec![mmt(1200), mmt(1500), mmt(1700)]
        );
    }

    #[test]
    fn normalize_sorts_grade_thresholds_after_elo_ones_by_strength() {
        let grade_1d = MacMahonThreshold::grade(Grade::dan(1));
        let grade_5d = MacMahonThreshold::grade(Grade::dan(5));
        let grade_5k = MacMahonThreshold::grade(Grade::kyu(5));
        let elo_1500 = mmt(1500);
        assert_eq!(
            TournamentSettings::normalize_thresholds(vec![grade_5d, elo_1500, grade_1d, grade_5k,]),
            vec![elo_1500, grade_5k, grade_1d, grade_5d]
        );
    }

    #[test]
    fn degressive_drops_thresholds_at_their_own_scheduled_round() {
        // Two bottom groups drop at the end of round 2, the top never drops.
        let s = TournamentSettings::default().with_thresholds(vec![
            mmt_drops(1200, 2),
            mmt_drops(1500, 2),
            mmt(1800),
        ]);
        // Rounds 1–2: full thresholds.
        assert_eq!(
            s.effective_macmahon_thresholds(0),
            vec![elo(1200), elo(1500), elo(1800)]
        );
        assert_eq!(
            s.effective_macmahon_thresholds(1),
            vec![elo(1200), elo(1500), elo(1800)]
        );
        // After round 2, the two scheduled thresholds are gone; the top stays.
        assert_eq!(s.effective_macmahon_thresholds(2), vec![elo(1800)]);
        assert_eq!(s.effective_macmahon_thresholds(3), vec![elo(1800)]);

        // Points reflect the shrinking spread: a 2000 player has 3 pts up front,
        // 1 pt from round 3 onward.
        assert_eq!(s.macmahon_points_at(Some(2000), None, 1), 3);
        assert_eq!(s.macmahon_points_at(Some(2000), None, 2), 1);
        assert_eq!(s.macmahon_points_at(Some(2000), None, 4), 1);
    }

    #[test]
    fn normalized_sorts_by_value_and_zeroes_a_zero_drop_round() {
        let s = TournamentSettings::default()
            .with_thresholds(vec![
                mmt_drops(1500, 3),
                mmt(1200),
                mmt_drops(1200, 0), // duplicate value dropped, first kept
                mmt_drops(1800, 0), // a drop round of 0 can't fire, normalized away
            ])
            .normalized();
        assert_eq!(
            s.macmahon_thresholds(),
            &[mmt(1200), mmt_drops(1500, 3), mmt(1800)]
        );
    }

    #[test]
    fn club_protection_active_respects_toggle_and_round_window() {
        let off = TournamentSettings::default();
        assert!(!off.club_protection_active(1)); // disabled by default

        let all = TournamentSettings::default().with_club(ClubProtection::On {
            rounds: None,
            exempt_clubs: Vec::new(),
        });
        assert!(all.club_protection_active(1));
        assert!(all.club_protection_active(99)); // None = every round

        let limited = TournamentSettings::default().with_club(ClubProtection::On {
            rounds: NonZeroU32::new(2),
            exempt_clubs: Vec::new(),
        });
        assert!(limited.club_protection_active(1));
        assert!(limited.club_protection_active(2));
        assert!(!limited.club_protection_active(3)); // past the window
    }

    #[test]
    fn airtight_groups_active_respects_its_round_window() {
        let off = TournamentSettings::default();
        assert!(!off.airtight_groups_active(1)); // disabled by default (no window)

        let s = TournamentSettings::default().with_airtight(NonZeroU32::new(2));
        assert!(s.airtight_groups_active(1));
        assert!(s.airtight_groups_active(2));
        assert!(!s.airtight_groups_active(3)); // past the window
    }

    #[test]
    fn a_zero_airtight_groups_window_is_off_and_unrepresentable() {
        // `0 rounds` can't apply to any round, so it means "off" — now enforced by
        // the type: `NonZeroU32::new(0)` is `None`, i.e. the window is absent.
        assert_eq!(NonZeroU32::new(0), None);
        // A JSON `0` is rejected outright rather than silently kept (the frontend
        // sends `null`, never `0`, for an off window).
        assert!(serde_json::from_str::<TournamentSettings>(
            r#"{"pairing":{"kind":"swiss","airtight_groups":0}}"#
        )
        .is_err());
    }

    #[test]
    fn normalized_trims_and_dedups_exempt_clubs_case_insensitively() {
        let s = TournamentSettings::default()
            .with_club(ClubProtection::On {
                rounds: None,
                exempt_clubs: vec![
                    "  Paris  ".into(),
                    "paris".into(), // duplicate of Paris (case/space)
                    "   ".into(),   // empty after trim
                    "Lyon".into(),
                ],
            })
            .normalized();
        // First spelling kept, trimmed; the case-variant dup and the blank dropped.
        let PairingMode::Swiss {
            club_protection: ClubProtection::On { exempt_clubs, .. },
            ..
        } = &s.pairing
        else {
            panic!("still on");
        };
        assert_eq!(exempt_clubs, &["Paris", "Lyon"]);
        assert!(s.exempt_clubs_normalized().contains("paris")); // matched lower-cased
    }

    #[test]
    fn handicap_policy_defaults_to_allowed_and_round_trips_tagged() {
        assert_eq!(
            TournamentSettings::default().handicap_policy,
            HandicapPolicy::Enabled {
                display: HandicapDisplay::Allowed,
                wiel_rule: false,
            }
        );
        // The Enabled variant is internally tagged; Wiel off is omitted.
        assert_eq!(
            serde_json::to_string(&HandicapPolicy::Enabled {
                display: HandicapDisplay::Suggested,
                wiel_rule: false,
            })
            .unwrap(),
            r#"{"kind":"enabled","display":"suggested"}"#
        );
        // Omitted in the payload → the default (Allowed, Wiel off).
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            s.handicap_policy,
            HandicapPolicy::Enabled {
                display: HandicapDisplay::Allowed,
                wiel_rule: false,
            }
        );
    }

    #[test]
    fn wiel_rule_cannot_be_on_without_handicaps_and_defaults_off() {
        assert!(!TournamentSettings::default().handicap_wiel_rule());
        // `None` (handicaps off) reports no Wiel — the accessor can't say otherwise.
        let off = TournamentSettings {
            handicap_policy: HandicapPolicy::None,
            ..Default::default()
        };
        assert!(!off.handicap_wiel_rule());
        // Wiel is only reachable via the Enabled variant.
        let wiel = TournamentSettings {
            handicap_policy: HandicapPolicy::Enabled {
                display: HandicapDisplay::Allowed,
                wiel_rule: true,
            },
            ..Default::default()
        };
        assert!(wiel.handicap_wiel_rule());
    }

    #[test]
    fn half_point_absences_defaults_to_off() {
        assert!(!TournamentSettings::default().half_point_absences);
        // Omitted in the payload (an old save) → still off.
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert!(!s.half_point_absences);
        // Round-trips on the wire.
        let on: TournamentSettings =
            serde_json::from_str(r#"{"half_point_absences":true}"#).unwrap();
        assert!(on.half_point_absences);
    }

    #[test]
    fn tiebreaks_default_to_the_classic_order_and_dedup_in_place() {
        // Missing from an old save → the classic points → SOS → SODOS → SOSOS order.
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            s.tiebreaks,
            vec![
                Tiebreak::Points,
                Tiebreak::SosM,
                Tiebreak::SodosM,
                Tiebreak::SososM
            ]
        );
        // Snake-case codes on the wire.
        assert_eq!(
            serde_json::to_string(&Tiebreak::Points).unwrap(),
            "\"points\""
        );
        assert_eq!(
            serde_json::to_string(&Tiebreak::CussW).unwrap(),
            "\"cuss_w\""
        );

        // Normalizing drops duplicates, keeping the first occurrence (so order is
        // meaningful and each column appears once).
        let n = TournamentSettings {
            tiebreaks: vec![
                Tiebreak::SosW,
                Tiebreak::SosM,
                Tiebreak::SosW,
                Tiebreak::CussM,
            ],
            ..Default::default()
        }
        .normalized();
        assert_eq!(
            n.tiebreaks,
            vec![Tiebreak::SosW, Tiebreak::SosM, Tiebreak::CussM]
        );
    }

    #[test]
    fn normalizing_drops_est_elo_tiebreak_when_elo_mode_off() {
        // Off (the default): the estimated-ELO tie-break is dropped.
        let off = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM],
            ..Default::default()
        }
        .normalized();
        assert_eq!(off.tiebreaks, vec![Tiebreak::Points, Tiebreak::SosM]);

        // On: it is kept, in place.
        let on = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM],
            ..TournamentSettings::elo_pairing()
        }
        .normalized();
        assert_eq!(
            on.tiebreaks,
            vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM]
        );
        // (Estimate-based MacMahon also keeps a live estimate; that path is covered
        // by `estimate_based_macmahon_keeps_the_est_elo_tiebreak_valid`.)
    }

    #[test]
    fn macmahon_from_estimate_active_needs_the_toggle_and_an_elo_threshold() {
        // Toggle off: never active, regardless of thresholds.
        let off = TournamentSettings::default().with_thresholds(vec![mmt(1500)]);
        assert!(!off.macmahon_from_estimate_active());

        // Toggle on but no ELO threshold (grade only): inert, so not active.
        let grade_only = TournamentSettings::default()
            .with_thresholds(vec![MacMahonThreshold::grade(Grade::dan(1))])
            .with_macmahon_from_estimate();
        assert!(!grade_only.macmahon_from_estimate_active());

        // Toggle on with no thresholds at all: also inert.
        let none = TournamentSettings::default().with_macmahon_from_estimate();
        assert!(!none.macmahon_from_estimate_active());

        // Toggle on with an ELO threshold (even alongside a grade one): active.
        let on = TournamentSettings::default()
            .with_thresholds(vec![mmt(1500), MacMahonThreshold::grade(Grade::dan(1))])
            .with_macmahon_from_estimate();
        assert!(on.macmahon_from_estimate_active());
    }

    #[test]
    fn estimate_based_macmahon_keeps_the_est_elo_tiebreak_valid() {
        // Plain Swiss pairing, but MacMahon is drawn from the estimate: a live
        // estimate is maintained, so the estimated-ELO tie-break survives.
        let s = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo],
            ..TournamentSettings::default()
                .with_thresholds(vec![mmt(1500)])
                .with_macmahon_from_estimate()
        }
        .normalized();
        assert_eq!(s.tiebreaks, vec![Tiebreak::Points, Tiebreak::EstElo]);

        // But with only a grade threshold the estimate isn't used, so it's dropped.
        let grade_only = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo],
            ..TournamentSettings::default()
                .with_thresholds(vec![MacMahonThreshold::grade(Grade::dan(1))])
                .with_macmahon_from_estimate()
        }
        .normalized();
        assert_eq!(grade_only.tiebreaks, vec![Tiebreak::Points]);
    }

    #[test]
    fn unrated_prior_defaults_reproduce_the_historical_prior() {
        // Default settings maintain no estimate; the getters fall back to the
        // historical prior, and the estimator's own defaults match.
        let s = TournamentSettings::default();
        assert!((s.elo_unrated_prior_center() - 600.0).abs() < 1e-9);
        // √(705·s) ≈ 350, the historical unrated std.
        let std = (s.elo_unrated_k() * crate::elo::S).sqrt();
        assert!((std - 350.0).abs() < 1.0, "unrated std ~350, got {std}");
        let est = EloEstimator::default();
        assert_eq!(est.unrated_prior_center, 600);
        assert_eq!(est.unrated_k, 705);
    }

    #[test]
    fn unrated_k_is_clamped_to_at_least_one_by_its_type() {
        // A zero-width prior would divide by zero in the estimator; the type floors
        // it at construction and on deserialize, so `normalized` no longer needs to.
        assert_eq!(UnratedK::new(0), 1);
        // Deserialization clamps too (the floor lives in the type, not the nesting).
        assert_eq!(serde_json::from_str::<UnratedK>("0").unwrap(), 1);
        // And the settings getter never yields a zero-width prior.
        let s = TournamentSettings::elo_pairing().map_estimator(|e| e.unrated_k = UnratedK::new(0));
        assert!(s.elo_unrated_k() >= 1.0);
    }

    #[test]
    fn prior_shape_defaults_to_gaussian_and_is_behaviour_neutral() {
        // Default settings: the shape getters report Gaussian, looseness symmetric.
        let s = TournamentSettings::default();
        assert_eq!(s.elo_prior_shape_established(), EloPriorShape::Gaussian);
        assert_eq!(s.elo_prior_shape_provisional(), EloPriorShape::Gaussian);
        assert_eq!(s.elo_prior_shape_unrated(), EloPriorShape::Gaussian);
        assert!((s.elo_upward_looseness_established() - 1.0).abs() < 1e-9);
        assert!((s.elo_upward_looseness_provisional() - 1.0).abs() < 1e-9);
        assert!((s.elo_upward_looseness_unrated() - 1.0).abs() < 1e-9);
        // The estimator's own defaults match.
        let est = EloEstimator::default();
        assert_eq!(est.prior_shape_unrated, EloPriorShape::Gaussian);
        assert_eq!(est.upward_looseness_unrated, 100);
        // A per-category Laplace shape round-trips through JSON (in ELO mode).
        let laplace = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.prior_shape_unrated = EloPriorShape::Laplace);
        let json = serde_json::to_string(&laplace).unwrap();
        let back: TournamentSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.elo_prior_shape_unrated(), EloPriorShape::Laplace);
        assert_eq!(back.elo_prior_shape_established(), EloPriorShape::Gaussian);
    }

    #[test]
    fn upward_looseness_is_clamped_to_at_least_symmetric_by_its_type() {
        // An upward revision is never harder than a downward one, so each
        // category's r ≥ 1 — floored by [`RatioAtLeastOne`] at construction and on
        // deserialize, so `normalized` no longer clamps.
        assert_eq!(RatioAtLeastOne::from_percent(50), 100);
        assert_eq!(RatioAtLeastOne::from_percent(0), 100);
        // Deserialization clamps too.
        assert_eq!(serde_json::from_str::<RatioAtLeastOne>("50").unwrap(), 100);
        // And through the settings getter (in a mode that carries an estimator).
        let s = TournamentSettings::elo_pairing().map_estimator(|e| {
            e.upward_looseness_established = RatioAtLeastOne::from_percent(50);
            e.upward_looseness_unrated = RatioAtLeastOne::from_percent(0);
        });
        assert!(s.elo_upward_looseness_established() >= 1.0);
        assert!(s.elo_upward_looseness_unrated() >= 1.0);
    }

    #[test]
    fn elo_estimate_needed_is_true_for_elo_pairing() {
        assert!(!TournamentSettings::default().elo_estimate_needed());
        assert!(TournamentSettings::elo_pairing().elo_estimate_needed());
    }

    #[test]
    fn floater_style_defaults_to_classic_and_round_trips_snake_case() {
        assert_eq!(
            TournamentSettings::default().floater_style(),
            FloaterStyle::Classic
        );
        assert_eq!(
            serde_json::to_string(&FloaterStyle::Median).unwrap(),
            "\"median\""
        );
        // Omitted in the payload → the default (Classic).
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.floater_style(), FloaterStyle::Classic);
    }
}
