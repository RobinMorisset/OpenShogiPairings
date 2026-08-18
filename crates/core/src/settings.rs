//! Tournament-wide settings.

use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU32;
use std::ops::Range;

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::cup::CupFormat;
use crate::player::Grade;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DateError {
    #[error("invalid date {0:?}: expected an ISO calendar date, YYYY-MM-DD")]
    Malformed(String),
    #[error("the tournament's last day ({last}) precedes its first ({first})")]
    Backwards { first: IsoDate, last: IsoDate },
}

/// A calendar date in ISO `YYYY-MM-DD` form — the shape the FESA rating program
/// reads out of an American Grid header (see [`crate::american_grid()`]).
///
/// Kept as a validated string rather than pulling in a date crate: nothing here
/// does date *arithmetic*, only prints the date back out. The validation is real
/// though — the layout *and* the calendar, so `2025-02-30` is rejected — and it
/// runs in `Deserialize`, so a malformed date fails at the API boundary rather
/// than surfacing inside an export a rating administrator has to read.
///
/// The derived `Ord` compares the strings, which for this fixed-width
/// zero-padded form is chronological order; [`TournamentDates`] relies on that.
///
/// `Deserialize` is hand-written (rather than `#[serde(try_from = …)]`) for the
/// same reason as [`Ratio`]'s: ts-rs' serde-compat pass emits a spurious warning
/// for anything beyond `rename`/`default`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct IsoDate(String);

impl IsoDate {
    /// Parse an ISO `YYYY-MM-DD` calendar date, rejecting anything else.
    pub fn parse(s: &str) -> Result<Self, DateError> {
        let malformed = || DateError::Malformed(s.to_string());
        // ASCII-only, so the byte offsets below are also char boundaries.
        if !s.is_ascii() || s.len() != 10 || s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
            return Err(malformed());
        }
        // Digits only: `str::parse` would otherwise accept `+1` and friends.
        let digits = |r: Range<usize>| -> Option<u32> {
            let part = &s[r];
            part.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| part.parse().ok())
                .flatten()
        };
        let (year, month, day) = (
            digits(0..4).ok_or_else(malformed)?,
            digits(5..7).ok_or_else(malformed)?,
            digits(8..10).ok_or_else(malformed)?,
        );
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return Err(malformed());
        }
        Ok(IsoDate(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Gregorian leap years included.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

impl fmt::Display for IsoDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IsoDate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        IsoDate::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// The days the tournament runs: its first and its last (equal for a one-day
/// event). Both are required together, because the American Grid header wants
/// both the range *and* the closing date, and a range whose end precedes its
/// start is rejected at construction — including when deserialized — rather than
/// printed into an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TournamentDates {
    pub first: IsoDate,
    /// Last day of play — the same as `first` for a one-day event.
    pub last: IsoDate,
}

impl TournamentDates {
    /// The dates of an event running `first..=last`.
    pub fn new(first: IsoDate, last: IsoDate) -> Result<Self, DateError> {
        if last < first {
            return Err(DateError::Backwards { first, last });
        }
        Ok(TournamentDates { first, last })
    }

    pub fn single_day(&self) -> bool {
        self.first == self.last
    }
}

impl<'de> Deserialize<'de> for TournamentDates {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            first: IsoDate,
            last: IsoDate,
        }
        let Raw { first, last } = Raw::deserialize(d)?;
        TournamentDates::new(first, last).map_err(serde::de::Error::custom)
    }
}

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

/// Nationality protection: the same idea as [`ClubProtection`], one notch weaker
/// — avoid pairing two players of the same nationality, and when the two rules
/// disagree, the club one wins (it sits directly above this one in the pairing
/// ladder, see [`crate::pairing`]). Off by default, and shaped identically to
/// club protection: an optional round window and an exempt list, both folded
/// into `On` so they cannot be set while protection is disabled.
///
/// Deliberately a separate type rather than a reuse of [`ClubProtection`]: the
/// two are configured independently, and each carries its own exempt list under
/// its own JSON name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NationalityProtection {
    #[default]
    Off,
    /// Avoid pairing players of the same nationality.
    On {
        /// `None` = every round; `Some(n)` = only rounds `1..=n`, later rounds
        /// pairing on score alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(as = "Option::<u32>")]
        rounds: Option<NonZeroU32>,
        /// Nationalities exempt from protection — the "host country" case, where
        /// most of the field shares one nationality and is expected to meet.
        /// Matched case-insensitively.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exempt_nationalities: Vec<String>,
    },
}

impl NationalityProtection {
    /// Whether protection applies to the given (1-based) round.
    pub fn active(&self, round: u32) -> bool {
        match self {
            NationalityProtection::Off => false,
            NationalityProtection::On { rounds, .. } => rounds.is_none_or(|n| round <= n.get()),
        }
    }

    /// The exempt nationalities in canonical (normalized) form; empty when off.
    fn exempt_normalized(&self) -> HashSet<String> {
        match self {
            NationalityProtection::On {
                exempt_nationalities,
                ..
            } => exempt_nationalities
                .iter()
                .map(|c| TournamentSettings::normalize_nationality(c))
                .collect(),
            NationalityProtection::Off => HashSet::new(),
        }
    }

    /// `skip_serializing_if` helper — `Off` is the default and omitted from JSON.
    fn is_off(&self) -> bool {
        matches!(self, NationalityProtection::Off)
    }
}

/// Canonical form of an exempt list (clubs or nationalities): each entry
/// trimmed, blanks dropped, and repeats removed case-insensitively keeping the
/// first spelling — so the stored list is independent of entry order and of how
/// each name was capitalized.
fn normalize_exempt_list(list: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    list.into_iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty() && seen.insert(c.to_lowercase()))
        .collect()
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
/// no arm to widen. See `docs/archive/elo-pairing-mode.md`.
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
#[serde(deny_unknown_fields)]
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
    /// Board wins: the games a team's players won across every board — the
    /// established second criterion after match points, separating two teams
    /// level on matches. Distinct from [`Points`](Self::Points), which counts
    /// the match rather than the boards inside it (and carries the MacMahon
    /// start besides).
    ///
    /// **Team mode only.** An individual's board wins are just their wins, so
    /// the criterion would repeat a column and could never break a tie;
    /// [`TournamentSettings::normalized`] drops it outside team mode, which is
    /// what keeps the arm for it in individual standings unreachable.
    BoardWins,
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

    /// The default ranking order for a **team** tournament: match points, then
    /// board wins, then the SOS family — established team-event practice, and
    /// what the referee gets by switching a fresh tournament to team mode.
    pub fn default_team_order() -> Vec<Tiebreak> {
        vec![
            Tiebreak::Points,
            Tiebreak::BoardWins,
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
/// keeps the Swiss-only knobs (floater style, airtight groups, club and
/// nationality protection, MacMahon) from coexisting with ELO pairing, and the
/// estimator from existing without an estimate to configure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PairingMode {
    /// Swiss / MacMahon pairing.
    Swiss {
        #[serde(default)]
        floater_style: FloaterStyle,
        /// "Airtight groups": if set, forbid pairing across MacMahon groups during
        /// rounds `1..=n`. Meaningless without thresholds — with none there is a
        /// single group — so [`TournamentSettings::normalized`] clears it there
        /// rather than storing a window that groups by nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(as = "Option::<u32>")]
        airtight_groups: Option<NonZeroU32>,
        #[serde(default, skip_serializing_if = "ClubProtection::is_off")]
        club_protection: ClubProtection,
        /// The weaker sibling of `club_protection`, configured independently.
        #[serde(default, skip_serializing_if = "NationalityProtection::is_off")]
        nationality_protection: NationalityProtection,
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
            nationality_protection: NationalityProtection::Off,
            macmahon: MacMahon::default(),
        }
    }
}

/// A referee-defined player category — an optional descriptive tag such as
/// "Women", "U18" or "U14". Categories are created in the settings (none by
/// default, no cap on how many, and freely renamed or deleted), shown as
/// checkbox columns in the Players tab, and used to filter/highlight the
/// standings and flag each category's leader. They are **purely descriptive**:
/// a category never affects pairing, MacMahon points or the ranking itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(deny_unknown_fields)]
pub struct PlayerCategory {
    /// Stable identifier, minted by the client when the category is created, so a
    /// player's membership (which references this id) survives a later rename.
    pub id: Uuid,
    /// Display name. Trimmed and guaranteed non-empty in canonical form (see
    /// [`TournamentSettings::normalized`]).
    pub name: String,
}

/// Team-tournament configuration. Present exactly when the tournament is a team
/// tournament — teams become the unit of pairing and ranking, while the games
/// stay ordinary individual boards (see `docs/archive/team-tournaments.md`).
///
/// Orthogonal to [`PairingMode`], like the cup: a team tournament is still
/// paired Swiss or MacMahon, over teams instead of players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(deny_unknown_fields)]
pub struct TeamSettings {
    /// Players per team — every team has exactly this many members, and a match
    /// is this many boards. Validated in [`TEAM_SIZES`]; 3 is the European
    /// convention.
    pub size: u32,
}

impl Default for TeamSettings {
    fn default() -> Self {
        TeamSettings { size: 3 }
    }
}

/// The team sizes a tournament may be run with. Two is the smallest thing that
/// is still a team; nine is well past any real event and keeps a match's board
/// count sane.
pub(crate) const TEAM_SIZES: std::ops::RangeInclusive<u32> = 2..=9;

/// A feature that cannot be combined with team mode (v1). Enabling team mode
/// while one of these is on — or turning one on while team mode is — is a
/// settings error naming the conflict, never a silent auto-disable.
///
/// Serialized snake_case, and each variant has a matching error code, so the
/// referee is told exactly which two settings disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum TeamModeConflict {
    /// The direct-elimination cup: a team bracket is a different format, not a
    /// variation on this one.
    Cup,
    /// Long (two-round) games: a board that spans two rounds would leave its
    /// match — and the round — half-derived.
    LongGames,
    /// ELO pairing mode: it pairs on a live per-player estimate, which has no
    /// team-level meaning.
    EloPairing,
    /// Grade-based MacMahon thresholds: a team has an average rating, not a
    /// grade, so only the ELO criterion has a team reading.
    GradeThresholds,
    /// The estimated-ELO tie-break, for the same reason as ELO pairing.
    EstEloTiebreak,
}

impl TeamModeConflict {
    /// The stable machine code for this conflict, shared with the API's error
    /// codes so the message can be translated.
    pub fn code(self) -> &'static str {
        match self {
            TeamModeConflict::Cup => "team_mode_rejects_cup",
            TeamModeConflict::LongGames => "team_mode_rejects_long_games",
            TeamModeConflict::EloPairing => "team_mode_rejects_elo_pairing",
            TeamModeConflict::GradeThresholds => "team_mode_rejects_grade_thresholds",
            TeamModeConflict::EstEloTiebreak => "team_mode_rejects_est_elo_tiebreak",
        }
    }

    /// English description, the fallback shown when a client has no translation.
    pub fn describe(self) -> &'static str {
        match self {
            TeamModeConflict::Cup => "the direct-elimination cup",
            TeamModeConflict::LongGames => "long (two-round) games",
            TeamModeConflict::EloPairing => "ELO pairing mode",
            TeamModeConflict::GradeThresholds => "grade-based MacMahon thresholds",
            TeamModeConflict::EstEloTiebreak => "the estimated-ELO tie-break",
        }
    }
}

/// Configuration that isn't tied to a single player or round.
///
/// Kept as its own record so it can grow (time controls, tie-break choices, …)
/// without disturbing the rest of the tournament shape. Added as an additive,
/// defaulted field, so tournaments saved before it existed still load (with no
/// MacMahon groups).
///
/// `deny_unknown_fields`: an unrecognised key is a hard error, not silently
/// dropped. A stale or misplaced key (e.g. a top-level `macmahon_thresholds` from
/// before MacMahon moved under `pairing`) would otherwise parse into the default
/// and quietly disable the setting the caller asked for — the exact failure that
/// made every `mm-grades` config a no-op copy of plain Swiss. Backward compat is
/// unaffected (old saves have *fewer* fields, filled by `#[serde(default)]`); the
/// cost is forward compat — a settings field added in a newer build makes that
/// save unreadable by an older one, which `format_version` is there to gate. The
/// nested `#[serde(tag = "kind")]` enums (`PairingMode`, `MacMahonSource`, …) can't
/// carry this attribute (serde forbids it on internally-tagged enums), so a typo
/// *inside* one of those objects is still tolerated; the top-level guard here is
/// what catches the whole-schema drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(deny_unknown_fields)]
pub struct TournamentSettings {
    /// The town the tournament is held in, as the American Grid header names it
    /// (`[13. Kurpfalz New Year's Open, Ludwigshafen, Germany, …]`). Trimmed,
    /// with a blank entry normalized to `None` — which simply leaves it out of
    /// that line. Descriptive only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// The country the tournament is held in, alongside [`Self::city`] in the
    /// American Grid header. Trimmed, blank normalized to `None`. Descriptive
    /// only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// The days the tournament runs (see [`TournamentDates`]). Purely
    /// descriptive: the only thing that reads it is the American Grid header,
    /// which the FESA rating program wants stamped with the event's dates. `None`
    /// (the default) leaves them out of the header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dates: Option<TournamentDates>,
    /// The time control, as free text for the header to print verbatim — e.g.
    /// `30min + 30sec`, which is the form the FESA guide shows, but a tournament
    /// with sudden death, an increment or a per-round change needs to say so in
    /// its own words. Trimmed, with a blank entry normalized to `None`.
    /// Descriptive only, like [`Self::dates`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_control: Option<String>,
    /// How the tournament is paired: Swiss/MacMahon over static ratings (with the
    /// floater/airtight/club/MacMahon knobs) or ELO mode over a live estimate (see
    /// [`PairingMode`]). Defaults to plain Swiss.
    #[serde(default)]
    pub pairing: PairingMode,
    /// Whether this is a hybrid tournament with a direct-elimination cup among the
    /// top eligible players. Off by default.
    #[serde(default)]
    pub cup_enabled: bool,
    /// How that cup fills its bracket: straight from the top eligible players, or
    /// with a qualification round feeding half the bracket (see [`CupFormat`]).
    /// Only consulted when `cup_enabled`. Defaults to [`CupFormat::Direct`].
    #[serde(default)]
    pub cup_format: CupFormat,
    /// Whether the referee may flag individual boards as "long games" that last
    /// two rounds and score two points for the winner. Off by default. See
    /// `docs/reference/two-round-boards.md`.
    #[serde(default)]
    pub long_boards_enabled: bool,
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
    /// The point of it is that missing a round — commonly a whole day of a
    /// weekend event — should not drop a player so far down the standings that
    /// the rest of their tournament is spent against the bottom of the field. It
    /// therefore applies to a player registered *late* just as much as to one
    /// registered from the start and marked absent: both were simply not at the
    /// board. See [`Tournament::add_player`](crate::Tournament::add_player).
    ///
    /// [`Sitout::value`]: crate::round::Sitout::value
    #[serde(default)]
    pub half_point_absences: bool,
    /// The criteria used to rank the standings, in order of priority (the
    /// tournament number breaks anything still level). Defaults to the classic
    /// points → SOS → SODOS → SOSOS order.
    #[serde(default = "default_tiebreaks")]
    pub tiebreaks: Vec<Tiebreak>,
    /// Referee-defined player categories (see [`PlayerCategory`]). Empty by
    /// default. Descriptive only — used for the Players-tab checkbox columns and
    /// the standings filter/leader marks, never for pairing or scoring.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<PlayerCategory>,
    /// Present exactly when this is a **team tournament** (see [`TeamSettings`]).
    /// Absent — the default — is an ordinary individual tournament.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub teams: Option<TeamSettings>,
}

impl Default for TournamentSettings {
    fn default() -> Self {
        TournamentSettings {
            city: None,
            country: None,
            dates: None,
            time_control: None,
            pairing: PairingMode::default(),
            cup_enabled: false,
            cup_format: CupFormat::default(),
            long_boards_enabled: false,
            handicap_policy: HandicapPolicy::default(),
            half_point_absences: false,
            tiebreaks: default_tiebreaks(),
            categories: Vec::new(),
            teams: None,
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

    /// The canonical form of a nationality for comparison: trimmed and
    /// lower-cased, so "FR", "fr" and " Fr " count as the same nationality.
    /// (A player's own nationality is stored upper-cased, but an exempt entry is
    /// typed by hand, so both sides are folded here rather than assumed.)
    pub fn normalize_nationality(nationality: &str) -> String {
        nationality.trim().to_lowercase()
    }

    /// The ids of every currently-defined category, for pruning a player's stale
    /// memberships once a category is deleted (see
    /// [`Tournament::update_settings`](crate::Tournament::update_settings)).
    pub fn category_ids(&self) -> HashSet<Uuid> {
        self.categories.iter().map(|c| c.id).collect()
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

    /// Whether nationality protection applies to the given (1-based) round:
    /// enabled and, if a round limit is set, within it.
    pub fn nationality_protection_active(&self, round: u32) -> bool {
        match &self.pairing {
            PairingMode::Swiss {
                nationality_protection,
                ..
            } => nationality_protection.active(round),
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

    pub fn team_mode(&self) -> bool {
        self.teams.is_some()
    }

    /// Players per team, or 1 outside team mode — the number of boards one
    /// pairing expands to, which is exactly how the round builder reads it.
    pub fn team_size(&self) -> u32 {
        self.teams.map_or(1, |t| t.size)
    }

    /// The first feature these settings enable that team mode cannot support, if
    /// any — the check `update_settings` runs so a conflicting combination is
    /// rejected naming the culprit, rather than one side being silently disabled.
    ///
    /// Returns `None` outside team mode: every one of these is fine on its own.
    pub fn team_mode_conflict(&self) -> Option<TeamModeConflict> {
        if !self.team_mode() {
            return None;
        }
        if self.cup_enabled {
            return Some(TeamModeConflict::Cup);
        }
        if self.long_boards_enabled {
            return Some(TeamModeConflict::LongGames);
        }
        match &self.pairing {
            PairingMode::Elo { .. } => return Some(TeamModeConflict::EloPairing),
            PairingMode::Swiss { macmahon, .. } => {
                // A team has an average rating, not a grade, so only the ELO
                // criterion has a team-level reading.
                if macmahon
                    .thresholds
                    .iter()
                    .any(|t| matches!(t.criterion, ThresholdCriterion::Grade { .. }))
                {
                    return Some(TeamModeConflict::GradeThresholds);
                }
            }
        }
        if self.tiebreaks.contains(&Tiebreak::EstElo) {
            return Some(TeamModeConflict::EstEloTiebreak);
        }
        None
    }

    /// Whether MacMahon starting points are actually in use — the condition that
    /// makes a pairing rating *required* for every team member, since an unrated
    /// member would otherwise contribute nothing to the team average the
    /// thresholds are applied to.
    pub fn macmahon_in_use(&self) -> bool {
        match &self.pairing {
            PairingMode::Swiss { macmahon, .. } => !macmahon.thresholds.is_empty(),
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

    /// The exempt nationalities in canonical (normalized) form, for membership
    /// tests.
    pub fn exempt_nationalities_normalized(&self) -> HashSet<String> {
        match &self.pairing {
            PairingMode::Swiss {
                nationality_protection,
                ..
            } => nationality_protection.exempt_normalized(),
            PairingMode::Elo { .. } => HashSet::new(),
        }
    }

    /// Return these settings in canonical form: thresholds sorted ascending by
    /// value and de-duplicated (keeping the first entry for a repeated value,
    /// and treating a `drops_after_round` of 0 as "never drops" since it can't
    /// take effect before round 1 anyway), the airtight-groups window dropped
    /// when no threshold is left for it to group by, and exempt clubs /
    /// nationalities trimmed, emptied-dropped and de-duplicated
    /// case-insensitively.
    /// Independent of the order fields were entered, so pairing/standings are
    /// reproducible from the stored settings.
    pub fn normalized(mut self) -> Self {
        // The Swiss-only knobs live under `pairing`; ELO pairing has nothing to
        // canonicalize here.
        if let PairingMode::Swiss {
            club_protection,
            nationality_protection,
            macmahon,
            airtight_groups,
            ..
        } = &mut self.pairing
        {
            macmahon.thresholds =
                Self::normalize_thresholds(std::mem::take(&mut macmahon.thresholds));

            // Airtight groups forbid pairing across MacMahon groups, and with no
            // threshold there is a single group: the window is dropped rather
            // than left set-but-inert, so the stored settings never claim a rule
            // that isn't running. This also drops it in the corner case where
            // manual point adjustments alone would have split players into
            // groups — a deliberate simplification: the setting is presented as
            // hanging off the thresholds.
            if macmahon.thresholds.is_empty() {
                *airtight_groups = None;
            }

            // The two exempt lists: keep the first spelling of each, trimmed and
            // non-empty.
            if let ClubProtection::On { exempt_clubs, .. } = club_protection {
                *exempt_clubs = normalize_exempt_list(std::mem::take(exempt_clubs));
            }
            if let NationalityProtection::On {
                exempt_nationalities,
                ..
            } = nationality_protection
            {
                *exempt_nationalities = normalize_exempt_list(std::mem::take(exempt_nationalities));
            }
        }

        // Tie-breaks: drop duplicates keeping the first occurrence (so the order
        // is meaningful and each metric appears at most once as a column).
        let mut seen_tb = HashSet::new();
        self.tiebreaks.retain(|&tb| seen_tb.insert(tb));
        // The estimated-ELO tie-break only ranks in ELO pairing mode, and only
        // while rated players are actually estimated — see `est_elo_ranks`.
        if !self.est_elo_ranks() {
            self.tiebreaks.retain(|&tb| tb != Tiebreak::EstElo);
        }
        // Board wins only exists as a criterion *because* a team match is not
        // the boards inside it: it separates two teams level on match points.
        // An individual has no such distinction — their board wins are simply
        // their wins, which `points` already carries — so outside team mode the
        // criterion is a column that repeats one beside it and can never break
        // a tie. Dropped here rather than merely hidden, so leaving team mode
        // takes it with it instead of leaving it lying in the save file.
        if self.teams.is_none() {
            self.tiebreaks.retain(|&tb| tb != Tiebreak::BoardWins);
        }

        // The free-text header fields: trimmed, and a blank (or whitespace-only)
        // entry means "not set" rather than a stray comma or an empty
        // `[Time control: ]` line in the export.
        for field in [&mut self.city, &mut self.country, &mut self.time_control] {
            if field.as_ref().is_some_and(|s| s.trim().is_empty()) {
                *field = None;
            } else if let Some(s) = field {
                *s = s.trim().to_string();
            }
        }

        // Categories: trim each name, drop blank-named ones, and keep the first
        // of any repeated id (client-minted, so a collision would be a client
        // bug). Entry order is otherwise preserved — it drives the column order.
        let mut seen_cat = HashSet::new();
        self.categories.retain_mut(|c| {
            c.name = c.name.trim().to_string();
            !c.name.is_empty() && seen_cat.insert(c.id)
        });

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
    /// whether the estimated-ELO value is computed and shown as a standings
    /// column; the frontend mirrors it as `eloEstimateLive`. Keep those in sync —
    /// splitting this rule is what let the estimate column diverge between the two
    /// modes. Whether it also *ranks* is the narrower [`Self::est_elo_ranks`].
    pub fn elo_estimate_live(&self) -> bool {
        self.elo_estimate_needed() || self.macmahon_from_estimate_active()
    }

    /// Whether the live estimate **pins** rated players to their registration
    /// rating — the "apply estimates to unrated players only" mode (K multiplier
    /// 0, see [`crate::estimate_elos`]). Their estimate is then not an estimate at
    /// all but a copy of the Rating column, so the standings leave their
    /// `estimated_elo` empty rather than repeating it, and the estimate stops
    /// being a ranking criterion (see [`Self::est_elo_ranks`]).
    pub fn elo_estimate_rated_pinned(&self) -> bool {
        self.elo_estimate_live() && self.elo_k_multiplier() == 0.0
    }

    /// Whether the estimated ELO is a meaningful **ranking** criterion — which is
    /// also exactly when the Results tab offers it as a tie-break column. Two
    /// conditions:
    ///
    /// - **ELO pairing mode only.** There the estimate *is* the quantity the
    ///   tournament runs on, so ranking by it is the point. Under estimate-based
    ///   MacMahon it is instead an *input* to the MacMahon points; it surfaces in
    ///   those points' tooltip rather than as a column, so that a referee never has
    ///   to wonder whether this is the decorative "estimated rating" other pairing
    ///   software shows next to a ranking it has no effect on.
    /// - **Rated players actually estimated.** With them pinned (K × 0), ranking by
    ///   the estimate would sort them by their registration rating, and against
    ///   unrated players holding a genuinely estimated number.
    ///
    /// The frontend mirrors this as `estEloRanks`; keep them in sync.
    pub fn est_elo_ranks(&self) -> bool {
        self.elo_estimate_needed() && !self.elo_estimate_rated_pinned()
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

    pub fn elo_prior_shape_established(&self) -> EloPriorShape {
        self.estimator()
            .map_or_else(EloPriorShape::default, |e| e.prior_shape_established)
    }

    pub fn elo_prior_shape_provisional(&self) -> EloPriorShape {
        self.estimator()
            .map_or_else(EloPriorShape::default, |e| e.prior_shape_provisional)
    }

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

    /// Set nationality protection (Swiss).
    pub(crate) fn with_nationality(mut self, protection: NationalityProtection) -> Self {
        if let PairingMode::Swiss {
            nationality_protection,
            ..
        } = &mut self.pairing
        {
            *nationality_protection = protection;
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
    fn normalized_drops_the_airtight_window_without_thresholds() {
        // Nothing to group by: no threshold means one MacMahon group, so the
        // window is cleared rather than stored as a rule that never separates
        // anyone.
        let window = |s: &TournamentSettings| match &s.pairing {
            PairingMode::Swiss {
                airtight_groups, ..
            } => *airtight_groups,
            PairingMode::Elo { .. } => None,
        };

        let bare = TournamentSettings::default()
            .with_airtight(NonZeroU32::new(2))
            .normalized();
        assert_eq!(window(&bare), None);
        assert!(!bare.airtight_groups_active(1));

        // With a threshold it survives untouched.
        let with_threshold = TournamentSettings::default()
            .with_thresholds(vec![mmt(1500)])
            .with_airtight(NonZeroU32::new(2))
            .normalized();
        assert_eq!(window(&with_threshold), NonZeroU32::new(2));
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
    fn nationality_protection_active_respects_toggle_and_round_window() {
        let off = TournamentSettings::default();
        assert!(!off.nationality_protection_active(1)); // disabled by default

        let all = TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: None,
            exempt_nationalities: Vec::new(),
        });
        assert!(all.nationality_protection_active(1));
        assert!(all.nationality_protection_active(99)); // None = every round

        let limited = TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: NonZeroU32::new(2),
            exempt_nationalities: Vec::new(),
        });
        assert!(limited.nationality_protection_active(1));
        assert!(limited.nationality_protection_active(2));
        assert!(!limited.nationality_protection_active(3)); // past the window
    }

    #[test]
    fn nationality_protection_is_independent_of_club_protection() {
        // The two are separate knobs: turning one on leaves the other off, and
        // each keeps its own exempt list.
        let s = TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: None,
            exempt_nationalities: vec!["JP".into()],
        });
        assert!(s.nationality_protection_active(1));
        assert!(!s.club_protection_active(1));
        assert!(s.exempt_clubs_normalized().is_empty());
        assert!(s.exempt_nationalities_normalized().contains("jp"));
    }

    #[test]
    fn normalized_trims_and_dedups_exempt_nationalities_case_insensitively() {
        let s = TournamentSettings::default()
            .with_nationality(NationalityProtection::On {
                rounds: None,
                exempt_nationalities: vec![
                    "  JP  ".into(),
                    "jp".into(),  // duplicate of JP (case/space)
                    "   ".into(), // empty after trim
                    "FR".into(),
                ],
            })
            .normalized();
        let PairingMode::Swiss {
            nationality_protection:
                NationalityProtection::On {
                    exempt_nationalities,
                    ..
                },
            ..
        } = &s.pairing
        else {
            panic!("still on");
        };
        assert_eq!(exempt_nationalities, &["JP", "FR"]);
        assert!(s.exempt_nationalities_normalized().contains("jp")); // matched lower-cased
    }

    #[test]
    fn nationality_protection_is_omitted_from_json_when_off() {
        // Off is the default and skipped, so an existing settings payload that
        // never heard of the knob round-trips unchanged.
        let json = serde_json::to_string(&TournamentSettings::default()).unwrap();
        assert!(
            !json.contains("nationality_protection"),
            "the default should not serialize the knob: {json}"
        );
        let s: TournamentSettings =
            serde_json::from_str(r#"{"pairing":{"kind":"swiss","macmahon":{}}}"#).unwrap();
        assert!(!s.nationality_protection_active(1));
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
    fn normalizing_drops_board_wins_tiebreak_outside_team_mode() {
        // Individual (the default): board wins would just repeat the player's
        // own wins, so it goes — and goes from the *settings*, so that leaving
        // team mode removes it rather than leaving it in the save file.
        let individual = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::BoardWins, Tiebreak::SosM],
            ..Default::default()
        }
        .normalized();
        assert_eq!(individual.tiebreaks, vec![Tiebreak::Points, Tiebreak::SosM]);

        // Team mode: kept, in place — it is the established second criterion.
        let teamed = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::BoardWins, Tiebreak::SosM],
            teams: Some(TeamSettings::default()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(
            teamed.tiebreaks,
            vec![Tiebreak::Points, Tiebreak::BoardWins, Tiebreak::SosM]
        );

        // And the team default order survives its own normalization, which is
        // what a fresh team tournament is handed.
        let defaults = TournamentSettings {
            tiebreaks: Tiebreak::default_team_order(),
            teams: Some(TeamSettings::default()),
            ..Default::default()
        }
        .normalized();
        assert!(defaults.tiebreaks.contains(&Tiebreak::BoardWins));
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
        // (Estimate-based MacMahon maintains a live estimate too, but does *not*
        // make it a ranking criterion — see
        // `estimate_based_macmahon_does_not_make_est_elo_a_tiebreak`.)
    }

    #[test]
    fn normalizing_drops_est_elo_tiebreak_when_only_unrated_are_estimated() {
        // ELO pairing, but "apply estimates to unrated players only" (K × 0): every
        // rated player is pinned to their registration rating, so ranking by the
        // "estimate" would just be ranking by that rating — not a valid criterion.
        let pinned = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM],
            ..TournamentSettings::elo_pairing()
                .map_estimator(|e| e.k_multiplier = Ratio::from_percent(0))
        };
        assert!(pinned.elo_estimate_live(), "the estimate is still computed");
        assert!(pinned.elo_estimate_rated_pinned());
        assert!(!pinned.est_elo_ranks());
        assert_eq!(
            pinned.normalized().tiebreaks,
            vec![Tiebreak::Points, Tiebreak::SosM]
        );

        // Same mode with rated players estimated (the default ×1.0): it survives.
        let estimated = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo, Tiebreak::SosM],
            ..TournamentSettings::elo_pairing()
        };
        assert!(estimated.est_elo_ranks());
        assert_eq!(estimated.normalized().tiebreaks.len(), 3);
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
    fn estimate_based_macmahon_does_not_make_est_elo_a_tiebreak() {
        // Plain Swiss pairing with MacMahon drawn from the estimate: a live
        // estimate is maintained (the standings still carry it, for the MacMahon
        // points' tooltip), but there the estimate is an *input* to those points,
        // not a ranking quantity of its own — so it is not a valid tie-break.
        let s = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo],
            ..TournamentSettings::default()
                .with_thresholds(vec![mmt(1500)])
                .with_macmahon_from_estimate()
        };
        assert!(s.elo_estimate_live(), "the estimate is still computed");
        assert!(!s.est_elo_ranks());
        assert_eq!(s.normalized().tiebreaks, vec![Tiebreak::Points]);
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

    #[test]
    fn unknown_fields_are_rejected_not_silently_dropped() {
        // The pre-refactor flat schema (MacMahon at the top level) must be a hard
        // error, not parse into a default that silently disables MacMahon — the bug
        // that made every mm-grades config a no-op copy of plain Swiss.
        let flat =
            r#"{ "macmahon_thresholds": [ { "criterion": { "kind": "elo", "value": 1486 } } ] }"#;
        let err = serde_json::from_str::<TournamentSettings>(flat).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown field `macmahon_thresholds`"),
            "expected an unknown-field error, got: {err}"
        );

        // The guard reaches the nested config structs too.
        let bad_estimator = r#"{ "pairing": { "kind": "swiss", "macmahon": {
            "thresholds": [], "source": { "kind": "from_estimate",
            "estimator": { "elo_k_multiplier_percent": 0 } } } } }"#;
        let err = serde_json::from_str::<TournamentSettings>(bad_estimator).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown field `elo_k_multiplier_percent`"),
            "expected an unknown-field error on the estimator, got: {err}"
        );

        // A correct nested config still parses and populates MacMahon.
        let good = r#"{ "pairing": { "kind": "swiss", "macmahon": {
            "thresholds": [ { "criterion": { "kind": "elo", "value": 1486 } } ],
            "source": { "kind": "static" } } } }"#;
        let s: TournamentSettings = serde_json::from_str(good).unwrap();
        assert_eq!(s.macmahon_thresholds().len(), 1);
    }

    // --- Team mode --------------------------------------------------------

    fn team_settings() -> TournamentSettings {
        TournamentSettings {
            teams: Some(TeamSettings::default()),
            ..TournamentSettings::default()
        }
    }

    /// Each unsupported feature is reported as its own conflict, so the message
    /// can name the two settings that disagree rather than a generic refusal.
    #[test]
    fn team_mode_names_the_feature_it_conflicts_with() {
        let cases: Vec<(TournamentSettings, TeamModeConflict)> = vec![
            (
                TournamentSettings {
                    cup_enabled: true,
                    ..team_settings()
                },
                TeamModeConflict::Cup,
            ),
            (
                TournamentSettings {
                    long_boards_enabled: true,
                    ..team_settings()
                },
                TeamModeConflict::LongGames,
            ),
            (
                TournamentSettings {
                    pairing: PairingMode::Elo {
                        estimator: EloEstimator::default(),
                    },
                    ..team_settings()
                },
                TeamModeConflict::EloPairing,
            ),
            (
                team_settings().with_thresholds(vec![MacMahonThreshold {
                    criterion: ThresholdCriterion::Grade {
                        grade: Grade::dan(1),
                    },
                    drops_after_round: None,
                }]),
                TeamModeConflict::GradeThresholds,
            ),
            (
                TournamentSettings {
                    tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo],
                    ..team_settings()
                },
                TeamModeConflict::EstEloTiebreak,
            ),
        ];
        for (settings, expected) in cases {
            assert_eq!(settings.team_mode_conflict(), Some(expected));
        }
    }

    /// The very same features are fine on their own — the conflict is with team
    /// mode, not with the feature.
    #[test]
    fn no_conflict_outside_team_mode_or_without_the_feature() {
        let cup = TournamentSettings {
            cup_enabled: true,
            long_boards_enabled: true,
            ..TournamentSettings::default()
        };
        assert_eq!(cup.team_mode_conflict(), None);
        assert_eq!(team_settings().team_mode_conflict(), None);
        // An ELO MacMahon threshold reads perfectly well against a team average.
        assert_eq!(
            team_settings()
                .with_thresholds(vec![mmt(1500)])
                .team_mode_conflict(),
            None
        );
    }

    #[test]
    fn team_size_is_one_outside_team_mode_and_the_configured_size_within() {
        assert!(!TournamentSettings::default().team_mode());
        assert_eq!(TournamentSettings::default().team_size(), 1);
        assert!(team_settings().team_mode());
        assert_eq!(team_settings().team_size(), 3);
    }

    /// The `teams` key is absent from an individual tournament's JSON, so team
    /// mode is off by construction for anything that never mentions it.
    #[test]
    fn team_settings_round_trip_and_stay_absent_when_off() {
        let json = serde_json::to_string(&TournamentSettings::default()).unwrap();
        assert!(!json.contains("teams"), "{json}");
        let s = team_settings();
        let round_tripped: TournamentSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(round_tripped.teams, Some(TeamSettings { size: 3 }));
    }

    #[test]
    fn iso_date_accepts_real_calendar_dates_only() {
        assert_eq!(IsoDate::parse("2026-07-04").unwrap().as_str(), "2026-07-04");
        assert_eq!(IsoDate::parse("2024-02-29").unwrap().as_str(), "2024-02-29"); // leap year
        for bad in [
            "",
            "2026-7-4",      // not zero-padded
            "04/07/2026",    // not ISO
            "2026-07-04 ",   // stray space
            "2026-13-01",    // month out of range
            "2026-00-10",    // month 0
            "2026-02-30",    // no such day
            "2025-02-29",    // 2025 isn't a leap year
            "1900-02-29",    // nor is 1900 (century, not a multiple of 400)
            "+026-07-04",    // `parse::<u32>` would accept the sign
            "2026-07-04T12", // longer than a date
        ] {
            assert_eq!(
                IsoDate::parse(bad),
                Err(DateError::Malformed(bad.to_string())),
                "should have been rejected: {bad:?}"
            );
        }
        // 2000 is a leap year (multiple of 400).
        assert!(IsoDate::parse("2000-02-29").is_ok());
    }

    #[test]
    fn iso_dates_order_chronologically() {
        assert!(IsoDate::parse("2026-01-31").unwrap() < IsoDate::parse("2026-02-01").unwrap());
        assert!(IsoDate::parse("2025-12-31").unwrap() < IsoDate::parse("2026-01-01").unwrap());
    }

    #[test]
    fn tournament_dates_reject_a_backwards_range() {
        let first = IsoDate::parse("2026-07-05").unwrap();
        let last = IsoDate::parse("2026-07-04").unwrap();
        assert_eq!(
            TournamentDates::new(first.clone(), last.clone()),
            Err(DateError::Backwards { first, last })
        );
        // A one-day event (first == last) is fine, and knows it.
        let day = IsoDate::parse("2026-07-04").unwrap();
        let dates = TournamentDates::new(day.clone(), day).unwrap();
        assert!(dates.single_day());
    }

    #[test]
    fn dates_and_time_control_round_trip_and_fail_loudly() {
        let json = r#"{ "city": "Ludwigshafen", "country": "Germany",
                        "dates": { "first": "2026-07-04", "last": "2026-07-05" },
                        "time_control": "30min + 30sec" }"#;
        let s: TournamentSettings = serde_json::from_str(json).unwrap();
        let dates = s.dates.clone().unwrap();
        assert_eq!(dates.first.as_str(), "2026-07-04");
        assert_eq!(dates.last.as_str(), "2026-07-05");
        assert!(!dates.single_day());
        assert_eq!(s.city.as_deref(), Some("Ludwigshafen"));
        assert_eq!(s.country.as_deref(), Some("Germany"));
        assert_eq!(s.time_control.as_deref(), Some("30min + 30sec"));
        // Round-trips through JSON unchanged.
        let back: TournamentSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);

        // All of them are optional, and absent by default.
        let none: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(none.city, None);
        assert_eq!(none.country, None);
        assert_eq!(none.dates, None);
        assert_eq!(none.time_control, None);

        // A malformed date, a half-filled range and a backwards one are all hard
        // errors rather than a silently dropped (or half-applied) header.
        for (bad, expected) in [
            (
                r#"{ "dates": { "first": "04/07/2026", "last": "2026-07-05" } }"#,
                "invalid date",
            ),
            (r#"{ "dates": { "first": "2026-07-04" } }"#, "missing field"),
            (
                r#"{ "dates": { "first": "2026-07-05", "last": "2026-07-04" } }"#,
                "precedes its first",
            ),
        ] {
            let err = serde_json::from_str::<TournamentSettings>(bad).unwrap_err();
            assert!(
                err.to_string().contains(expected),
                "expected {expected:?} in the error, got: {err}"
            );
        }
    }

    #[test]
    fn normalized_trims_the_header_text_and_drops_blank_entries() {
        let trimmed = TournamentSettings {
            city: Some(" Ludwigshafen ".into()),
            country: Some("Germany\t".into()),
            time_control: Some("  40min + 30sec  ".into()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(trimmed.city.as_deref(), Some("Ludwigshafen"));
        assert_eq!(trimmed.country.as_deref(), Some("Germany"));
        assert_eq!(trimmed.time_control.as_deref(), Some("40min + 30sec"));

        let blank = TournamentSettings {
            city: Some("".into()),
            country: Some(" ".into()),
            time_control: Some("   ".into()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(blank.city, None);
        assert_eq!(blank.country, None);
        assert_eq!(blank.time_control, None);
    }
}
