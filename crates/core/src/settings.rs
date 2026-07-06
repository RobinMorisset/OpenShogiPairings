//! Tournament-wide settings.

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
}

impl TournamentSettings {
    /// The MacMahon starting points for a player with the given rating: the
    /// number of thresholds the rating meets or exceeds.
    pub fn macmahon_points(&self, rating: Option<u32>) -> u32 {
        match rating {
            Some(r) => self.macmahon_thresholds.iter().filter(|&&t| r >= t).count() as u32,
            None => 0,
        }
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
}
