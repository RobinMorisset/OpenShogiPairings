//! Suggested handicap, per FFS Arbitre Annexe 7 ("Système de calcul des
//! handicaps"): ratings are first compressed through a piecewise-linear
//! transform (stronger players compress more), then the absolute difference of
//! the transformed ratings is bucketed into a handicap.

use crate::round::Handicap;

/// The rating transform: identity below 1360, damped by 0.8x between 1360 and
/// 1560, damped by 2/3x above 1560.
fn elo_prime(elo: f64) -> f64 {
    if elo < 1360.0 {
        elo
    } else if elo <= 1560.0 {
        1360.0 + 0.8 * (elo - 1360.0)
    } else {
        1560.0 + (2.0 / 3.0) * (elo - 1560.0)
    }
}

/// `None` when either player is unrated (the transform needs a numeric rating)
/// or the adjusted gap is small enough to call it an even game (≤45). The nine
/// non-equal bins map 1:1 onto [`Handicap::ALL`].
pub(crate) fn suggested_handicap(rating1: Option<u32>, rating2: Option<u32>) -> Option<Handicap> {
    let diff = (elo_prime(rating1? as f64) - elo_prime(rating2? as f64))
        .abs()
        .round() as u32;
    Some(match diff {
        0..=45 => return None,
        46..=135 => Handicap::Sente,
        136..=225 => Handicap::Lance,
        226..=315 => Handicap::Bishop,
        316..=405 => Handicap::Rook,
        406..=495 => Handicap::RookLance,
        496..=675 => Handicap::TwoPiece,
        676..=765 => Handicap::FourPiece,
        766..=855 => Handicap::FivePiece,
        _ => Handicap::SixPiece,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrated_player_has_no_suggestion() {
        assert_eq!(suggested_handicap(None, Some(1500)), None);
        assert_eq!(suggested_handicap(Some(1500), None), None);
        assert_eq!(suggested_handicap(None, None), None);
    }

    #[test]
    fn symmetric_in_the_two_ratings() {
        assert_eq!(
            suggested_handicap(Some(1200), Some(1800)),
            suggested_handicap(Some(1800), Some(1200)),
        );
    }

    #[test]
    fn below_1360_is_untransformed_so_bins_land_on_raw_difference() {
        // Both below 1360: elo' = elo, so a 45-point gap is even and a 46-point
        // gap is the smallest handicap.
        assert_eq!(suggested_handicap(Some(1000), Some(1045)), None);
        assert_eq!(
            suggested_handicap(Some(1000), Some(1046)),
            Some(Handicap::Sente)
        );
    }

    #[test]
    fn bin_boundaries_above_1560_use_the_two_thirds_damping() {
        // elo'(2000) = 1560 + 2/3*(2000-1560) ≈ 1853.33.
        // elo'(1560) = 1360 + 0.8*(1560-1360) = 1520 (the middle band's formula
        // applies exactly at 1560; the transform has a small jump there, per spec).
        // diff ≈ 333.33 -> rook.
        assert_eq!(
            suggested_handicap(Some(2000), Some(1560)),
            Some(Handicap::Rook)
        );
    }

    #[test]
    fn six_piece_is_open_ended_at_the_top() {
        assert_eq!(
            suggested_handicap(Some(3000), Some(500)),
            Some(Handicap::SixPiece)
        );
    }

    #[test]
    fn boundary_between_lance_and_bishop_below_1360() {
        assert_eq!(
            suggested_handicap(Some(1000), Some(1225)),
            Some(Handicap::Lance)
        );
        assert_eq!(
            suggested_handicap(Some(1000), Some(1226)),
            Some(Handicap::Bishop)
        );
    }
}
