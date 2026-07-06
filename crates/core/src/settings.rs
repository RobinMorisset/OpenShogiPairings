//! Tournament-wide settings.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Configuration that isn't tied to a single player or round.
///
/// Kept as its own record so it can grow (time controls, tie-break choices, …)
/// without disturbing the rest of the tournament shape. Added as an additive,
/// defaulted field, so tournaments saved before it existed still load (with no
/// MacMahon groups).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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
        self
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
}
