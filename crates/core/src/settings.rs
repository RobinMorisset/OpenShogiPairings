//! Tournament-wide settings.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::player::Grade;

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

/// How the referee wants handicap games treated in this tournament, controlling
/// both what the pairings view shows and whether a suggested handicap is
/// computed for display. The suggestion never affects pairing itself and is
/// never auto-filled — the referee always picks the handicap by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum HandicapPolicy {
    /// No handicap column at all.
    None,
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
/// moves the estimate much further before the prior reins it in. **Either** shape
/// can additionally be made asymmetric via the per-category
/// `elo_upward_looseness_*` knobs, which widen the upward arm so an *upward*
/// revision clears on less evidence than a downward one (for the Gaussian this is
/// a two-piece normal; for the Laplace, a wider upward scale). See
/// `docs/elo-pairing-mode.md`.
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

/// The default ELO-estimate K multiplier, as an integer percent (100 = ×1.0).
/// Named so `#[serde(default = …)]` can fill it in for tournaments saved before
/// the field existed.
fn default_elo_k_multiplier_percent() -> u32 {
    100
}

/// The default extra K multiplier for a provisionally-rated player, as an integer
/// percent (200 = ×2.0). Named so `#[serde(default = …)]` can fill it in for
/// tournaments saved before the field existed.
fn default_elo_provisional_multiplier_percent() -> u32 {
    200
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
fn default_elo_unrated_k() -> u32 {
    crate::elo::UNRATED_PRIOR_DEFAULT_K as u32
}

/// The default upward-looseness ratio `r` for the Laplace prior, as an integer
/// percent (100 = ×1.0 = symmetric). Named so `#[serde(default = …)]` can fill
/// it in for tournaments saved before the field existed.
fn default_elo_upward_looseness_percent() -> u32 {
    100
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
    /// Thresholds (sorted, de-duplicated) defining the MacMahon starting
    /// groups, each an ELO rating or a dan/kyu grade (see
    /// [`ThresholdCriterion`]) — a tournament can freely mix both kinds. A
    /// player's MacMahon points is the number of thresholds they meet or
    /// exceed — e.g. ELO thresholds `[1200, 1700]` give 0 points below 1200, 1
    /// in `[1200, 1700)`, and 2 at 1700 or above. Empty means no MacMahon
    /// (everyone starts at 0). A player missing the value a threshold needs
    /// (no rating for an ELO threshold, no grade for a grade one) never meets
    /// it. Each threshold can carry its own degressive round (see
    /// [`MacMahonThreshold::drops_after_round`]).
    #[serde(default)]
    pub macmahon_thresholds: Vec<MacMahonThreshold>,
    /// "Airtight groups": if `Some(n)`, an extra pairing rule — just below
    /// no-rematch, above the score-gap rule — forbids pairing players with a
    /// different number of MacMahon points during rounds `1..=n` (penalty grows
    /// with the square of the gap, like the other gap rules). `None` (the
    /// default) disables it. Meaningless without MacMahon thresholds, since
    /// every player has 0 MacMahon points otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airtight_groups_rounds: Option<u32>,
    /// Whether the pairing engine avoids pairing players from the same club
    /// ("club protection"). Off by default — enable it per tournament.
    #[serde(default)]
    pub club_protection_enabled: bool,
    /// If `Some(n)`, club protection applies only to rounds `1..=n`; later rounds
    /// pair on score alone. `None` (the default) means every round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub club_protection_rounds: Option<u32>,
    /// Clubs exempt from protection — the "local club" case, where many entrants
    /// share the host club and are expected to meet. Matched case-insensitively.
    #[serde(default)]
    pub club_protection_exempt_clubs: Vec<String>,
    /// Which player each score group sends up as its ascending floater (classic
    /// vs median Swiss). The descending floater is always the group's weakest.
    #[serde(default)]
    pub floater_style: FloaterStyle,
    /// Whether this is a hybrid tournament with a direct-elimination cup among the
    /// top eligible players. Off by default. When on, registration gains an
    /// eligibility column and finalization asks for the cup size.
    #[serde(default)]
    pub cup_enabled: bool,
    /// How handicap games are treated: hidden, allowed, or suggested (see
    /// [`HandicapPolicy`]).
    #[serde(default)]
    pub handicap_policy: HandicapPolicy,
    /// The "Wiel" rule: whether a handicap game always counts as a win for the
    /// giver in the standings and for pairing, regardless of the actual result.
    /// Off by default: handicap games then score like any other game (the
    /// actual result counts). Enable it per tournament to have the giver always
    /// count as the winner.
    #[serde(default)]
    pub handicap_wiel_rule: bool,
    /// The criteria used to rank the standings, in order of priority (the
    /// tournament number breaks anything still level). Points is one of these and
    /// can be reordered like any other. Only these columns are shown on the
    /// Results tab. Defaults to the classic points → SOS → SODOS → SOSOS order.
    #[serde(default = "default_tiebreaks")]
    pub tiebreaks: Vec<Tiebreak>,
    /// Experimental ELO-based (non-Swiss) pairing mode. When on, MacMahon, the
    /// Swiss-specific rules (score gap, float repeat, floater selection, fold) and
    /// club protection are all disabled; pairing instead minimizes the squared
    /// difference of a live Bayesian ELO estimate. See
    /// `docs/elo-pairing-mode.md`. Off by default. Mutually exclusive with
    /// [`Self::mixed_elo_pairing_enabled`] (this one wins if both are set; see
    /// [`Self::normalized`]).
    #[serde(default)]
    pub elo_pairing_enabled: bool,
    /// Mixed mode: keeps MacMahon and the Swiss score-group rules (score gap,
    /// float repeat, club protection, airtight groups) but replaces *only* the
    /// fold and floater-selection rules with the squared-ELO-gap rule, so
    /// within-group (and cross-group) ordering follows the live estimate instead
    /// of a static registration rating. Unlike [`Self::elo_pairing_enabled`], this
    /// stays fully compatible with MacMahon points. Off by default. Mutually
    /// exclusive with `elo_pairing_enabled` (see [`Self::normalized`]).
    #[serde(default)]
    pub mixed_elo_pairing_enabled: bool,
    /// Award MacMahon starting points from the **live ELO estimate** rather than
    /// the static registration rating: each ELO-based threshold is compared
    /// against the same Bayesian estimate that drives the ELO pairing modes,
    /// recomputed each round, so a player's MacMahon points can rise or fall as
    /// their estimated strength moves (grade-based thresholds are unaffected —
    /// they still read the player's grade). Independent of the pairing mode:
    /// this can be combined with plain Swiss or mixed-ELO pairing. Inert unless
    /// there is at least one ELO threshold to compare against (see
    /// [`Self::macmahon_from_estimate_active`]); the UI greys it out until then.
    /// Off by default.
    #[serde(default)]
    pub macmahon_from_estimated_elo: bool,
    /// The multiplier `m` on each player's FESA K, as an integer percent
    /// (100 = ×1.0), controlling how far the ELO estimate is allowed to drift from
    /// the registration rating (bigger = faster drift). Stored as an integer so
    /// the settings stay `Eq`; read as a float via [`Self::elo_k_multiplier`].
    /// Only meaningful when [`Self::elo_estimate_needed`]; expected range ~100–400.
    #[serde(default = "default_elo_k_multiplier_percent")]
    pub elo_k_multiplier_percent: u32,
    /// Extra K multiplier applied to a **provisionally-rated** player — one who is
    /// not in the FESA list (rating typed by hand) or whose FESA `#games` is below
    /// [`crate::PROVISIONAL_GAMES_THRESHOLD`] — as an integer percent (200 = ×2.0).
    /// Widens their prior so their estimate drifts faster, since their seed rating
    /// is less trustworthy. Stacks on top of `elo_k_multiplier_percent`. Clamped to
    /// ≥ 100 so a provisional rating is never treated as *more* reliable than an
    /// established one. Only meaningful when [`Self::elo_estimate_needed`].
    #[serde(default = "default_elo_provisional_multiplier_percent")]
    pub elo_provisional_multiplier_percent: u32,
    /// The center (mean) of the Bayesian prior for an **unrated** player, on the
    /// ELO scale. Where their estimate sits before any game pulls it. Default
    /// `600` (the midpoint of the assumed `[1, 1200]` unrated range). Only
    /// meaningful when [`Self::elo_estimate_needed`] or
    /// [`Self::macmahon_from_estimate_active`].
    #[serde(default = "default_elo_unrated_prior_center")]
    pub elo_unrated_prior_center: u32,
    /// The **K** setting the width of an unrated player's prior: its standard
    /// deviation is `√(K · s)`, the same law a rated player's K obeys, so this
    /// reads on the same familiar scale (a rated player's K is ~16–40; an unrated
    /// one is far wider). Bigger = a looser prior that lets results move the
    /// estimate faster. Default `705` (≈ the historical `σ 350`). Stored as an
    /// integer so the settings stay `Eq`; read via [`Self::elo_unrated_k`], which
    /// clamps it ≥ 1 (a zero-width prior would be degenerate). Only meaningful
    /// when [`Self::elo_estimate_needed`] or [`Self::macmahon_from_estimate_active`].
    #[serde(default = "default_elo_unrated_k")]
    pub elo_unrated_k: u32,
    /// The shape of every player's ELO prior — thin-tailed Gaussian (default,
    /// behaviour-neutral) or the fatter-tailed, optionally asymmetric Laplace.
    /// See [`EloPriorShape`]. Only meaningful when [`Self::elo_estimate_needed`]
    /// or [`Self::macmahon_from_estimate_active`].
    #[serde(default)]
    pub elo_prior_shape: EloPriorShape,
    /// How much *looser* an upward revision is than a downward one for an
    /// **established** (reliably-rated) player, as an integer percent
    /// (100 = ×1.0 = symmetric). Widens that player's upward arm in **either**
    /// prior shape (the Gaussian's `σ_up = r·σ₀`, or the Laplace's
    /// `b_up = r·b_down`): `r > 1` lets a win revise the estimate up on less
    /// evidence than a loss revises it down. A reliable rating is the one we trust
    /// most, so this usually stays at `100` (symmetric) — asymmetry is most useful
    /// for the less certain players below. Read via
    /// [`Self::elo_upward_looseness_established`], which clamps it ≥ 1.0 (an upward
    /// revision is never *harder* than a downward one). Only meaningful when a live
    /// estimate is maintained.
    #[serde(default = "default_elo_upward_looseness_percent")]
    pub elo_upward_looseness_established_percent: u32,
    /// The upward-looseness ratio `r` for a **provisionally-rated** player (not in
    /// the FESA list, or with fewer than [`crate::PROVISIONAL_GAMES_THRESHOLD`]
    /// games), as an integer percent (100 = ×1.0 = symmetric). Same meaning as
    /// [`Self::elo_upward_looseness_established_percent`] but for the less-trusted
    /// provisional prior, where a modest upward tilt is often warranted. Read via
    /// [`Self::elo_upward_looseness_provisional`] (clamps ≥ 1.0).
    #[serde(default = "default_elo_upward_looseness_percent")]
    pub elo_upward_looseness_provisional_percent: u32,
    /// The upward-looseness ratio `r` for an **unrated** player, as an integer
    /// percent (100 = ×1.0 = symmetric). Same meaning as
    /// [`Self::elo_upward_looseness_established_percent`] but for the wide unrated
    /// prior — the case where an upward tilt helps most, since a newcomer beating
    /// the field is far more likely genuinely strong than a fluke. Read via
    /// [`Self::elo_upward_looseness_unrated`] (clamps ≥ 1.0).
    #[serde(default = "default_elo_upward_looseness_percent")]
    pub elo_upward_looseness_unrated_percent: u32,
}

impl Default for TournamentSettings {
    fn default() -> Self {
        TournamentSettings {
            macmahon_thresholds: Vec::new(),
            airtight_groups_rounds: None,
            club_protection_enabled: false,
            club_protection_rounds: None,
            club_protection_exempt_clubs: Vec::new(),
            floater_style: FloaterStyle::default(),
            cup_enabled: false,
            handicap_policy: HandicapPolicy::default(),
            handicap_wiel_rule: false,
            tiebreaks: default_tiebreaks(),
            elo_pairing_enabled: false,
            mixed_elo_pairing_enabled: false,
            macmahon_from_estimated_elo: false,
            elo_k_multiplier_percent: default_elo_k_multiplier_percent(),
            elo_provisional_multiplier_percent: default_elo_provisional_multiplier_percent(),
            elo_unrated_prior_center: default_elo_unrated_prior_center(),
            elo_unrated_k: default_elo_unrated_k(),
            elo_prior_shape: EloPriorShape::default(),
            elo_upward_looseness_established_percent: default_elo_upward_looseness_percent(),
            elo_upward_looseness_provisional_percent: default_elo_upward_looseness_percent(),
            elo_upward_looseness_unrated_percent: default_elo_upward_looseness_percent(),
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
        self.macmahon_thresholds
            .iter()
            .filter(|t| t.drops_after_round.is_none_or(|r| rounds_played < r))
            .map(|t| t.criterion)
            .collect()
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
        self.club_protection_enabled && self.club_protection_rounds.is_none_or(|n| round <= n)
    }

    /// Whether "airtight groups" applies to the given (1-based) round: within
    /// the configured window, if any.
    pub fn airtight_groups_active(&self, round: u32) -> bool {
        self.airtight_groups_rounds.is_some_and(|n| round <= n)
    }

    /// The exempt clubs in canonical (normalized) form, for membership tests.
    pub fn exempt_clubs_normalized(&self) -> HashSet<String> {
        self.club_protection_exempt_clubs
            .iter()
            .map(|c| Self::normalize_club(c))
            .collect()
    }

    /// Return these settings in canonical form: thresholds sorted ascending by
    /// value and de-duplicated (keeping the first entry for a repeated value,
    /// and treating a `drops_after_round` of 0 as "never drops" since it can't
    /// take effect before round 1 anyway), and exempt clubs trimmed,
    /// emptied-dropped and de-duplicated case-insensitively. Independent of the
    /// order fields were entered, so pairing/standings are reproducible from the
    /// stored settings.
    pub fn normalized(mut self) -> Self {
        self.macmahon_thresholds = Self::normalize_thresholds(self.macmahon_thresholds);
        // A round count of 0 can't apply to any round, so it's the same as off.
        if self.airtight_groups_rounds == Some(0) {
            self.airtight_groups_rounds = None;
        }

        // The two ELO modes are mutually exclusive; pure ELO wins if both are
        // somehow set (e.g. a stale client payload).
        if self.elo_pairing_enabled {
            self.mixed_elo_pairing_enabled = false;
        }

        // Exempt clubs: keep the first spelling of each, trimmed and non-empty.
        let mut seen = HashSet::new();
        self.club_protection_exempt_clubs = self
            .club_protection_exempt_clubs
            .into_iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty() && seen.insert(c.to_lowercase()))
            .collect();

        // Tie-breaks: drop duplicates keeping the first occurrence (so the order
        // is meaningful and each metric appears at most once as a column).
        let mut seen_tb = HashSet::new();
        self.tiebreaks.retain(|&tb| seen_tb.insert(tb));
        // The estimated-ELO tie-break is meaningless unless a live estimate is
        // actually maintained — outside both ELO pairing modes *and* estimate-
        // based MacMahon it just sits at each player's registration rating — so
        // it is not a valid ranking criterion there.
        if !self.elo_estimate_needed() && !self.macmahon_from_estimate_active() {
            self.tiebreaks.retain(|&tb| tb != Tiebreak::EstElo);
        }

        // A zero K multiplier would give a degenerate (zero-width) prior, freezing
        // every estimate at its registration rating and dividing by zero in the
        // solver — clamp it up to at least 1%.
        self.elo_k_multiplier_percent = self.elo_k_multiplier_percent.max(1);
        // A provisional rating should never be treated as more reliable than an
        // established one, so the extra multiplier is at least ×1.
        self.elo_provisional_multiplier_percent = self.elo_provisional_multiplier_percent.max(100);
        // A zero-width unrated prior would divide by zero in the solver — clamp K ≥ 1.
        self.elo_unrated_k = self.elo_unrated_k.max(1);
        // An upward revision is never *harder* than a downward one, so r ≥ 1, for
        // each player category.
        self.elo_upward_looseness_established_percent =
            self.elo_upward_looseness_established_percent.max(100);
        self.elo_upward_looseness_provisional_percent =
            self.elo_upward_looseness_provisional_percent.max(100);
        self.elo_upward_looseness_unrated_percent =
            self.elo_upward_looseness_unrated_percent.max(100);

        self
    }

    /// Whether a live ELO estimate needs to be maintained for **pairing**
    /// purposes — either ELO mode. Used to gate the pairing model's ELO context
    /// (edge weights, bye ranks). Note this is *not* the only place a live
    /// estimate is maintained: [`Self::macmahon_from_estimate_active`] maintains
    /// one for scoring even in plain Swiss.
    pub fn elo_estimate_needed(&self) -> bool {
        self.elo_pairing_enabled || self.mixed_elo_pairing_enabled
    }

    /// Whether MacMahon starting points are actually drawn from the live ELO
    /// estimate: the [`Self::macmahon_from_estimated_elo`] toggle is on *and*
    /// there is at least one ELO-based threshold for the estimate to be compared
    /// against (with only grade thresholds, or none, the estimate would change
    /// nothing, so the (non-trivial) estimate computation is skipped).
    pub fn macmahon_from_estimate_active(&self) -> bool {
        self.macmahon_from_estimated_elo
            && self
                .macmahon_thresholds
                .iter()
                .any(|t| matches!(t.criterion, ThresholdCriterion::Elo { .. }))
    }

    /// The ELO-estimate K multiplier `m` as a float (percent / 100).
    pub fn elo_k_multiplier(&self) -> f64 {
        self.elo_k_multiplier_percent as f64 / 100.0
    }

    /// The extra K multiplier for a provisionally-rated player, as a float
    /// (percent / 100).
    pub fn elo_provisional_multiplier(&self) -> f64 {
        self.elo_provisional_multiplier_percent as f64 / 100.0
    }

    /// The center (mean) of the unrated-player prior, as a float.
    pub fn elo_unrated_prior_center(&self) -> f64 {
        self.elo_unrated_prior_center as f64
    }

    /// The K for the unrated-player prior, as a float, clamped ≥ 1 (a zero-width
    /// prior would divide by zero in the solver and freeze the estimate).
    pub fn elo_unrated_k(&self) -> f64 {
        self.elo_unrated_k.max(1) as f64
    }

    /// The [`EloPriorShape::Laplace`] upward-looseness ratio `r` for an
    /// **established** player, as a float, clamped ≥ 1.0 (an upward revision is
    /// never harder than a downward one). `1.0` is symmetric.
    pub fn elo_upward_looseness_established(&self) -> f64 {
        self.elo_upward_looseness_established_percent.max(100) as f64 / 100.0
    }

    /// The Laplace upward-looseness ratio `r` for a **provisionally-rated** player,
    /// as a float, clamped ≥ 1.0.
    pub fn elo_upward_looseness_provisional(&self) -> f64 {
        self.elo_upward_looseness_provisional_percent.max(100) as f64 / 100.0
    }

    /// The Laplace upward-looseness ratio `r` for an **unrated** player, as a
    /// float, clamped ≥ 1.0.
    pub fn elo_upward_looseness_unrated(&self) -> f64 {
        self.elo_upward_looseness_unrated_percent.max(100) as f64 / 100.0
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
        let s = TournamentSettings {
            macmahon_thresholds: vec![mmt(1200), mmt(1700)],
            ..Default::default()
        };
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
        let s = TournamentSettings {
            macmahon_thresholds: vec![
                MacMahonThreshold::elo(1500),
                MacMahonThreshold::grade(Grade::dan(1)),
            ],
            ..Default::default()
        };
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
        let s = TournamentSettings {
            macmahon_thresholds: vec![mmt_drops(1200, 2), mmt_drops(1500, 2), mmt(1800)],
            ..Default::default()
        };
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
        let s = TournamentSettings {
            macmahon_thresholds: vec![
                mmt_drops(1500, 3),
                mmt(1200),
                mmt_drops(1200, 0), // duplicate value dropped, first kept
                mmt_drops(1800, 0), // a drop round of 0 can't fire, normalized away
            ],
            ..Default::default()
        }
        .normalized();
        assert_eq!(
            s.macmahon_thresholds,
            vec![mmt(1200), mmt_drops(1500, 3), mmt(1800)]
        );
    }

    #[test]
    fn club_protection_active_respects_toggle_and_round_window() {
        let off = TournamentSettings::default();
        assert!(!off.club_protection_active(1)); // disabled by default

        let all = TournamentSettings {
            club_protection_enabled: true,
            ..Default::default()
        };
        assert!(all.club_protection_active(1));
        assert!(all.club_protection_active(99)); // None = every round

        let limited = TournamentSettings {
            club_protection_enabled: true,
            club_protection_rounds: Some(2),
            ..Default::default()
        };
        assert!(limited.club_protection_active(1));
        assert!(limited.club_protection_active(2));
        assert!(!limited.club_protection_active(3)); // past the window
    }

    #[test]
    fn airtight_groups_active_respects_its_round_window() {
        let off = TournamentSettings::default();
        assert!(!off.airtight_groups_active(1)); // disabled by default (no window)

        let s = TournamentSettings {
            airtight_groups_rounds: Some(2),
            ..Default::default()
        };
        assert!(s.airtight_groups_active(1));
        assert!(s.airtight_groups_active(2));
        assert!(!s.airtight_groups_active(3)); // past the window
    }

    #[test]
    fn normalized_zeroes_a_zero_airtight_groups_window() {
        let s = TournamentSettings {
            airtight_groups_rounds: Some(0),
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.airtight_groups_rounds, None);
    }

    #[test]
    fn normalized_trims_and_dedups_exempt_clubs_case_insensitively() {
        let s = TournamentSettings {
            club_protection_enabled: true,
            club_protection_exempt_clubs: vec![
                "  Paris  ".into(),
                "paris".into(), // duplicate of Paris (case/space)
                "   ".into(),   // empty after trim
                "Lyon".into(),
            ],
            ..Default::default()
        }
        .normalized();
        // First spelling kept, trimmed; the case-variant dup and the blank dropped.
        assert_eq!(s.club_protection_exempt_clubs, vec!["Paris", "Lyon"]);
        assert!(s.exempt_clubs_normalized().contains("paris")); // matched lower-cased
    }

    #[test]
    fn handicap_policy_defaults_to_allowed_and_round_trips_snake_case() {
        assert_eq!(
            TournamentSettings::default().handicap_policy,
            HandicapPolicy::Allowed
        );
        assert_eq!(
            serde_json::to_string(&HandicapPolicy::Suggested).unwrap(),
            "\"suggested\""
        );
        // Omitted in the payload → the default (Allowed).
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.handicap_policy, HandicapPolicy::Allowed);
    }

    #[test]
    fn handicap_wiel_rule_defaults_to_off() {
        assert!(!TournamentSettings::default().handicap_wiel_rule);
        // Omitted in the payload (an old save) → still off.
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert!(!s.handicap_wiel_rule);
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
            elo_pairing_enabled: false,
            ..Default::default()
        }
        .normalized();
        assert_eq!(off.tiebreaks, vec![Tiebreak::Points, Tiebreak::SosM]);

        // On: it is kept, in place.
        let on = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM],
            elo_pairing_enabled: true,
            ..Default::default()
        }
        .normalized();
        assert_eq!(
            on.tiebreaks,
            vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM]
        );

        // Mixed ELO mode also keeps a live estimate, so it counts too.
        let mixed = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM],
            mixed_elo_pairing_enabled: true,
            ..Default::default()
        }
        .normalized();
        assert_eq!(
            mixed.tiebreaks,
            vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM]
        );
    }

    #[test]
    fn the_two_elo_modes_are_mutually_exclusive() {
        // Pure ELO wins if a stale payload somehow sets both.
        let s = TournamentSettings {
            elo_pairing_enabled: true,
            mixed_elo_pairing_enabled: true,
            ..Default::default()
        }
        .normalized();
        assert!(s.elo_pairing_enabled);
        assert!(!s.mixed_elo_pairing_enabled);

        // Mixed alone is left untouched.
        let s = TournamentSettings {
            mixed_elo_pairing_enabled: true,
            ..Default::default()
        }
        .normalized();
        assert!(!s.elo_pairing_enabled);
        assert!(s.mixed_elo_pairing_enabled);
    }

    #[test]
    fn macmahon_from_estimate_active_needs_the_toggle_and_an_elo_threshold() {
        // Toggle off: never active, regardless of thresholds.
        let off = TournamentSettings {
            macmahon_thresholds: vec![mmt(1500)],
            ..Default::default()
        };
        assert!(!off.macmahon_from_estimate_active());

        // Toggle on but no ELO threshold (grade only): inert, so not active.
        let grade_only = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::grade(Grade::dan(1))],
            macmahon_from_estimated_elo: true,
            ..Default::default()
        };
        assert!(!grade_only.macmahon_from_estimate_active());

        // Toggle on with no thresholds at all: also inert.
        let none = TournamentSettings {
            macmahon_from_estimated_elo: true,
            ..Default::default()
        };
        assert!(!none.macmahon_from_estimate_active());

        // Toggle on with an ELO threshold (even alongside a grade one): active.
        let on = TournamentSettings {
            macmahon_thresholds: vec![mmt(1500), MacMahonThreshold::grade(Grade::dan(1))],
            macmahon_from_estimated_elo: true,
            ..Default::default()
        };
        assert!(on.macmahon_from_estimate_active());
    }

    #[test]
    fn estimate_based_macmahon_keeps_the_est_elo_tiebreak_valid() {
        // Plain Swiss pairing, but MacMahon is drawn from the estimate: a live
        // estimate is maintained, so the estimated-ELO tie-break survives.
        let s = TournamentSettings {
            macmahon_thresholds: vec![mmt(1500)],
            macmahon_from_estimated_elo: true,
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo],
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.tiebreaks, vec![Tiebreak::Points, Tiebreak::EstElo]);

        // But with only a grade threshold the estimate isn't used, so it's dropped.
        let grade_only = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::grade(Grade::dan(1))],
            macmahon_from_estimated_elo: true,
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo],
            ..Default::default()
        }
        .normalized();
        assert_eq!(grade_only.tiebreaks, vec![Tiebreak::Points]);
    }

    #[test]
    fn unrated_prior_defaults_reproduce_the_historical_prior() {
        let s = TournamentSettings::default();
        assert_eq!(s.elo_unrated_prior_center, 600);
        assert_eq!(s.elo_unrated_k, 705);
        assert!((s.elo_unrated_prior_center() - 600.0).abs() < 1e-9);
        // √(705·s) ≈ 350, the historical unrated std.
        let std = (s.elo_unrated_k() * crate::elo::S).sqrt();
        assert!((std - 350.0).abs() < 1.0, "unrated std ~350, got {std}");
        // Omitted from an old save → the defaults.
        let loaded: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.elo_unrated_prior_center, 600);
        assert_eq!(loaded.elo_unrated_k, 705);
    }

    #[test]
    fn normalized_clamps_unrated_k_to_at_least_one() {
        let s = TournamentSettings {
            elo_unrated_k: 0,
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.elo_unrated_k, 1);
        // And the accessor never yields a zero-width prior either.
        let raw = TournamentSettings {
            elo_unrated_k: 0,
            ..Default::default()
        };
        assert!(raw.elo_unrated_k() >= 1.0);
    }

    #[test]
    fn prior_shape_defaults_to_gaussian_and_is_behaviour_neutral() {
        let s = TournamentSettings::default();
        assert_eq!(s.elo_prior_shape, EloPriorShape::Gaussian);
        assert_eq!(s.elo_upward_looseness_established_percent, 100);
        assert_eq!(s.elo_upward_looseness_provisional_percent, 100);
        assert_eq!(s.elo_upward_looseness_unrated_percent, 100);
        assert!((s.elo_upward_looseness_established() - 1.0).abs() < 1e-9);
        assert!((s.elo_upward_looseness_provisional() - 1.0).abs() < 1e-9);
        assert!((s.elo_upward_looseness_unrated() - 1.0).abs() < 1e-9);
        // Omitted from an old save → the (Gaussian, symmetric) defaults.
        let loaded: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.elo_prior_shape, EloPriorShape::Gaussian);
        assert_eq!(loaded.elo_upward_looseness_unrated_percent, 100);
        // A Laplace shape round-trips through JSON.
        let laplace = TournamentSettings {
            elo_prior_shape: EloPriorShape::Laplace,
            ..Default::default()
        };
        let json = serde_json::to_string(&laplace).unwrap();
        let back: TournamentSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.elo_prior_shape, EloPriorShape::Laplace);
    }

    #[test]
    fn normalized_clamps_upward_looseness_to_at_least_symmetric() {
        // An upward revision is never harder than a downward one, so each
        // category's r ≥ 1.
        let s = TournamentSettings {
            elo_upward_looseness_established_percent: 50,
            elo_upward_looseness_provisional_percent: 80,
            elo_upward_looseness_unrated_percent: 0,
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.elo_upward_looseness_established_percent, 100);
        assert_eq!(s.elo_upward_looseness_provisional_percent, 100);
        assert_eq!(s.elo_upward_looseness_unrated_percent, 100);
        let raw = TournamentSettings {
            elo_upward_looseness_unrated_percent: 0,
            ..Default::default()
        };
        assert!(raw.elo_upward_looseness_unrated() >= 1.0);
    }

    #[test]
    fn elo_estimate_needed_is_true_for_either_elo_mode() {
        assert!(!TournamentSettings::default().elo_estimate_needed());
        assert!(TournamentSettings {
            elo_pairing_enabled: true,
            ..Default::default()
        }
        .elo_estimate_needed());
        assert!(TournamentSettings {
            mixed_elo_pairing_enabled: true,
            ..Default::default()
        }
        .elo_estimate_needed());
    }

    #[test]
    fn floater_style_defaults_to_classic_and_round_trips_snake_case() {
        assert_eq!(
            TournamentSettings::default().floater_style,
            FloaterStyle::Classic
        );
        assert_eq!(
            serde_json::to_string(&FloaterStyle::Median).unwrap(),
            "\"median\""
        );
        // Omitted in the payload → the default (Classic).
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.floater_style, FloaterStyle::Classic);
    }
}
