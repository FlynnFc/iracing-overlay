// Rust guideline compliant 2026-02-16

//! Estimates the field's Strength of Field (SOF) from live iRatings.
//!
//! iRacing does not publish live SOF through the local telemetry SDK — like
//! iRating change, it's a server-side number, and the exact formula isn't
//! public. This computes a Bradley-Terry "field-equivalent rating": the
//! iRating `X` a hypothetical opponent would need so their average win
//! chance against the real field is exactly 50%, using the same pairwise
//! win-chance model as [`super::irating`]. For a field where every driver
//! has the same iRating, this comes out to exactly that iRating; for an
//! uneven field it's pulled toward the weaker end more than a plain average
//! would be, which matches what SOF is meant to convey (a few strong
//! drivers shouldn't make a field of backmarkers "look" as strong as a
//! simple mean would suggest). Treat this as an approximation, not a
//! reproduction of iRacing's real number.

use std::f32::consts::LN_2;

/// Number of bisection steps: halves the search range each time, so 40
/// steps narrows a 0..20000 range to well under 0.001 iRating — far past
/// the precision this estimate is meaningful to.
const BISECTION_STEPS: u32 = 40;

/// Estimates SOF from a field's iRatings. Returns `None` for an empty field.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "iRatings are far below f32's exact-integer range; this whole module is already an approximation"
)]
pub fn estimate_sof(iratings: &[i32]) -> Option<i32> {
    if iratings.is_empty() {
        return None;
    }
    let br1 = 1600.0 / LN_2;

    let mut low = 0.0_f32;
    let mut high = 20_000.0_f32;
    for _ in 0..BISECTION_STEPS {
        let mid = f32::midpoint(low, high);
        let mean_chance: f32 =
            iratings.iter().map(|&r| chance(mid, r.max(0) as f32, br1)).sum::<f32>() / iratings.len() as f32;
        if mean_chance > 0.5 {
            high = mid;
        } else {
            low = mid;
        }
    }
    Some((f32::midpoint(low, high)).round() as i32)
}

/// The probability a driver rated `a` beats a driver rated `b`, from the
/// same pairwise model as `telemetry::irating`.
fn chance(a: f32, b: f32, br1: f32) -> f32 {
    let ea = (-a / br1).exp();
    let eb = (-b / br1).exp();
    (1.0 - ea) * eb / ((1.0 - eb) * ea + (1.0 - ea) * eb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_field_has_no_sof() {
        assert_eq!(estimate_sof(&[]), None);
    }

    #[test]
    fn a_uniform_field_sofs_at_its_own_rating() {
        let sof = estimate_sof(&[2500, 2500, 2500, 2500]).expect("non-empty field must have a SOF");
        assert!((sof - 2500).abs() <= 2, "expected ~2500, got {sof}");
    }

    #[test]
    fn a_stronger_field_has_a_higher_sof() {
        let weak = estimate_sof(&[1500, 1500, 1500]).expect("must have a SOF");
        let strong = estimate_sof(&[3500, 3500, 3500]).expect("must have a SOF");
        assert!(strong > weak);
    }

    #[test]
    fn adding_a_weaker_driver_pulls_sof_down() {
        let before = estimate_sof(&[3000, 3000, 3000]).expect("must have a SOF");
        let after = estimate_sof(&[3000, 3000, 3000, 500]).expect("must have a SOF");
        assert!(after < before, "adding a much weaker driver should lower SOF");
    }

    #[test]
    fn a_lone_strong_driver_does_not_inflate_sof_as_much_as_a_plain_average_would() {
        // One 8000-iR driver plus four 1000-iR drivers: a plain arithmetic
        // mean would be 2400, but SOF should stay much closer to the
        // backmarkers' level, since beating one outlier doesn't make the
        // whole field competitively "worth" 2400.
        let sof = estimate_sof(&[8000, 1000, 1000, 1000, 1000]).expect("must have a SOF");
        assert!(sof < 2000, "expected SOF well under the plain average of 2400, got {sof}");
    }
}
