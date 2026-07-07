//! Tournament-wide settings.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Which player a score group sends *up* as its ascending floater when it has to
/// pair across groups. The descending floater is always the last (weakest) of the
/// upper group; this only chooses the ascending one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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

/// A tie-break metric the referee can put in the ranking order.
///
/// Every metric comes in two flavours: the `…M` variants score an opponent by
/// their **MacMahon-inclusive points** (MacMahon start + wins — the classic
/// behaviour), the `…W` variants by their **wins only**. All twelve are always
/// computed for every player ([`crate::standings::Standing`]); the referee picks
/// which ones rank the table, and in which order, via
/// [`TournamentSettings::tiebreaks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// The live Bayesian ELO estimate (experimental ELO pairing mode). Ranks
    /// higher estimate first, like the other metrics.
    EstElo,
}

impl Tiebreak {
    /// The default ranking order — the classic points → SOS → SODOS → SOSOS
    /// order that preceded this setting, so pre-existing tournaments (and new
    /// ones that never touch the setting) rank exactly as before.
    pub fn default_order() -> Vec<Tiebreak> {
        vec![Tiebreak::Points, Tiebreak::SosM, Tiebreak::SodosM, Tiebreak::SososM]
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

/// Configuration that isn't tied to a single player or round.
///
/// Kept as its own record so it can grow (time controls, tie-break choices, …)
/// without disturbing the rest of the tournament shape. Added as an additive,
/// defaulted field, so tournaments saved before it existed still load (with no
/// MacMahon groups).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TournamentSettings {
    /// ELO thresholds (ascending, de-duplicated) defining the MacMahon starting
    /// groups. A player's MacMahon points is the number of thresholds their
    /// rating meets or exceeds — e.g. thresholds `[1200, 1700]` give 0 points
    /// below 1200, 1 in `[1200, 1700)`, and 2 at 1700 or above. Empty means no
    /// MacMahon (everyone starts at 0). An unrated player counts as below every
    /// threshold, so they get 0.
    #[serde(default)]
    pub macmahon_thresholds: Vec<u32>,
    /// Degressive MacMahon ("accelerated Swiss"): round numbers at whose *end*
    /// one bottom MacMahon threshold is dropped, so the starting-point spread
    /// shrinks as the tournament converges. Sorted ascending; a repeated round
    /// number drops several thresholds at the end of that round. Length is capped
    /// at `macmahon_thresholds.len()` — you can't drop more groups than exist.
    #[serde(default)]
    pub macmahon_removals: Vec<u32>,
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
    /// `docs/elo-pairing-mode.md`. Off by default.
    #[serde(default)]
    pub elo_pairing_enabled: bool,
    /// The multiplier `m` on each player's FIDE K, as an integer percent
    /// (100 = ×1.0), controlling how far the ELO estimate is allowed to drift from
    /// the registration rating (bigger = faster drift). Stored as an integer so
    /// the settings stay `Eq`; read as a float via [`Self::elo_k_multiplier`].
    /// Only meaningful when `elo_pairing_enabled`; expected range ~100–400.
    #[serde(default = "default_elo_k_multiplier_percent")]
    pub elo_k_multiplier_percent: u32,
    /// Extra K multiplier applied to a **provisionally-rated** player — one who is
    /// not in the FESA list (rating typed by hand) or whose FESA `#games` is below
    /// [`crate::PROVISIONAL_GAMES_THRESHOLD`] — as an integer percent (200 = ×2.0).
    /// Widens their prior so their estimate drifts faster, since their seed rating
    /// is less trustworthy. Stacks on top of `elo_k_multiplier_percent`. Clamped to
    /// ≥ 100 so a provisional rating is never treated as *more* reliable than an
    /// established one. Only meaningful when `elo_pairing_enabled`.
    #[serde(default = "default_elo_provisional_multiplier_percent")]
    pub elo_provisional_multiplier_percent: u32,
}

impl Default for TournamentSettings {
    fn default() -> Self {
        TournamentSettings {
            macmahon_thresholds: Vec::new(),
            macmahon_removals: Vec::new(),
            club_protection_enabled: false,
            club_protection_rounds: None,
            club_protection_exempt_clubs: Vec::new(),
            floater_style: FloaterStyle::default(),
            cup_enabled: false,
            handicap_policy: HandicapPolicy::default(),
            tiebreaks: default_tiebreaks(),
            elo_pairing_enabled: false,
            elo_k_multiplier_percent: default_elo_k_multiplier_percent(),
            elo_provisional_multiplier_percent: default_elo_provisional_multiplier_percent(),
        }
    }
}

impl TournamentSettings {
    /// The MacMahon starting points for a player with the given rating, using the
    /// full configured thresholds (i.e. before any degressive removal). This is
    /// [`macmahon_points_at`](Self::macmahon_points_at) at round 0.
    pub fn macmahon_points(&self, rating: Option<u32>) -> u32 {
        self.macmahon_points_at(rating, 0)
    }

    /// The thresholds in effect once `rounds_played` rounds are complete: the
    /// bottom `k` are dropped, where `k` is the number of removals scheduled at
    /// or before that round (a removal "at the end of round N" applies as soon as
    /// round N is complete, i.e. for pairing round N+1 and standings after N).
    pub fn effective_macmahon_thresholds(&self, rounds_played: u32) -> &[u32] {
        let dropped = self
            .macmahon_removals
            .iter()
            .filter(|&&r| r <= rounds_played)
            .count()
            .min(self.macmahon_thresholds.len());
        &self.macmahon_thresholds[dropped..]
    }

    /// The MacMahon starting points for a player once `rounds_played` rounds are
    /// complete: the number of *effective* thresholds the rating meets or
    /// exceeds. An unrated player counts as below every threshold, so gets 0.
    pub fn macmahon_points_at(&self, rating: Option<u32>, rounds_played: u32) -> u32 {
        match rating {
            Some(r) => self
                .effective_macmahon_thresholds(rounds_played)
                .iter()
                .filter(|&&t| r >= t)
                .count() as u32,
            None => 0,
        }
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

    /// The exempt clubs in canonical (normalized) form, for membership tests.
    pub fn exempt_clubs_normalized(&self) -> HashSet<String> {
        self.club_protection_exempt_clubs
            .iter()
            .map(|c| Self::normalize_club(c))
            .collect()
    }

    /// Return these settings in canonical form: thresholds sorted ascending and
    /// de-duplicated, removals sorted ascending with zeros dropped and the list
    /// truncated to the threshold count (can't drop more groups than exist), and
    /// exempt clubs trimmed, emptied-dropped and de-duplicated case-insensitively.
    /// Independent of the order fields were entered, so pairing/standings are
    /// reproducible from the stored settings.
    pub fn normalized(mut self) -> Self {
        self.macmahon_thresholds = Self::normalize_thresholds(self.macmahon_thresholds);
        self.macmahon_removals.retain(|&r| r >= 1);
        self.macmahon_removals.sort_unstable();
        // Keeping the earliest removals matches "these fire soonest"; the excess
        // couldn't take effect anyway once every threshold is gone.
        self.macmahon_removals.truncate(self.macmahon_thresholds.len());

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

        // A zero K multiplier would give a degenerate (zero-width) prior, freezing
        // every estimate at its registration rating and dividing by zero in the
        // solver — clamp it up to at least 1%.
        self.elo_k_multiplier_percent = self.elo_k_multiplier_percent.max(1);
        // A provisional rating should never be treated as more reliable than an
        // established one, so the extra multiplier is at least ×1.
        self.elo_provisional_multiplier_percent = self.elo_provisional_multiplier_percent.max(100);

        self
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

    /// Sort thresholds ascending and drop duplicates — the canonical form kept in
    /// the settings, independent of the order they were entered.
    pub fn normalize_thresholds(mut thresholds: Vec<u32>) -> Vec<u32> {
        thresholds.sort_unstable();
        thresholds.dedup();
        thresholds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macmahon_points_count_thresholds_met() {
        let s = TournamentSettings {
            macmahon_thresholds: vec![1200, 1700],
            ..Default::default()
        };
        assert_eq!(s.macmahon_points(Some(1000)), 0);
        assert_eq!(s.macmahon_points(Some(1200)), 1); // inclusive lower bound
        assert_eq!(s.macmahon_points(Some(1699)), 1);
        assert_eq!(s.macmahon_points(Some(1700)), 2);
        assert_eq!(s.macmahon_points(Some(2500)), 2);
        assert_eq!(s.macmahon_points(None), 0); // unrated → below every threshold
    }

    #[test]
    fn no_thresholds_means_zero_points() {
        let s = TournamentSettings::default();
        assert_eq!(s.macmahon_points(Some(9000)), 0);
        assert_eq!(s.macmahon_points(None), 0);
    }

    #[test]
    fn normalize_sorts_and_dedups() {
        assert_eq!(
            TournamentSettings::normalize_thresholds(vec![1700, 1200, 1200, 1500]),
            vec![1200, 1500, 1700]
        );
    }

    #[test]
    fn degressive_drops_bottom_thresholds_at_the_scheduled_round() {
        // Three groups; drop two at the end of round 2, one more at the end of 4.
        let s = TournamentSettings {
            macmahon_thresholds: vec![1200, 1500, 1800],
            macmahon_removals: vec![2, 2, 4],
            ..Default::default()
        };
        // Rounds 1–2: full thresholds.
        assert_eq!(s.effective_macmahon_thresholds(0), &[1200, 1500, 1800]);
        assert_eq!(s.effective_macmahon_thresholds(1), &[1200, 1500, 1800]);
        // After round 2, two bottom thresholds are gone.
        assert_eq!(s.effective_macmahon_thresholds(2), &[1800]);
        assert_eq!(s.effective_macmahon_thresholds(3), &[1800]);
        // After round 4, the last one goes too.
        assert_eq!(s.effective_macmahon_thresholds(4), &[] as &[u32]);

        // Points reflect the shrinking spread: a 2000 player has 3 pts up front,
        // 1 pt from round 3, 0 from round 5.
        assert_eq!(s.macmahon_points_at(Some(2000), 1), 3);
        assert_eq!(s.macmahon_points_at(Some(2000), 2), 1);
        assert_eq!(s.macmahon_points_at(Some(2000), 4), 0);
    }

    #[test]
    fn normalized_sorts_and_caps_removals_to_threshold_count() {
        let s = TournamentSettings {
            macmahon_thresholds: vec![1500, 1200, 1200], // → [1200, 1500]
            macmahon_removals: vec![4, 0, 1, 2],         // 0 dropped, capped to 2
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.macmahon_thresholds, vec![1200, 1500]);
        assert_eq!(s.macmahon_removals, vec![1, 2]); // earliest two kept
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
        assert_eq!(TournamentSettings::default().handicap_policy, HandicapPolicy::Allowed);
        assert_eq!(serde_json::to_string(&HandicapPolicy::Suggested).unwrap(), "\"suggested\"");
        // Omitted in the payload → the default (Allowed).
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.handicap_policy, HandicapPolicy::Allowed);
    }

    #[test]
    fn tiebreaks_default_to_the_classic_order_and_dedup_in_place() {
        // Missing from an old save → the classic points → SOS → SODOS → SOSOS order.
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            s.tiebreaks,
            vec![Tiebreak::Points, Tiebreak::SosM, Tiebreak::SodosM, Tiebreak::SososM]
        );
        // Snake-case codes on the wire.
        assert_eq!(serde_json::to_string(&Tiebreak::Points).unwrap(), "\"points\"");
        assert_eq!(serde_json::to_string(&Tiebreak::CussW).unwrap(), "\"cuss_w\"");

        // Normalizing drops duplicates, keeping the first occurrence (so order is
        // meaningful and each column appears once).
        let n = TournamentSettings {
            tiebreaks: vec![Tiebreak::SosW, Tiebreak::SosM, Tiebreak::SosW, Tiebreak::CussM],
            ..Default::default()
        }
        .normalized();
        assert_eq!(n.tiebreaks, vec![Tiebreak::SosW, Tiebreak::SosM, Tiebreak::CussM]);
    }

    #[test]
    fn floater_style_defaults_to_classic_and_round_trips_snake_case() {
        assert_eq!(TournamentSettings::default().floater_style, FloaterStyle::Classic);
        assert_eq!(serde_json::to_string(&FloaterStyle::Median).unwrap(), "\"median\"");
        // Omitted in the payload → the default (Classic).
        let s: TournamentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.floater_style, FloaterStyle::Classic);
    }
}
