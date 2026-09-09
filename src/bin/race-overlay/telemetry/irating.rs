// Rust guideline compliant 2026-02-16

//! Estimates iRating change from a race result.
//!
//! iRacing does not publish live iRating change through the local telemetry
//! SDK — it's calculated server-side after the session ends. This is a
//! from-scratch port of the publicly documented, but unofficial and
//! reverse-engineered, pairwise-duel algorithm from
//! [Turbo87/irating-rs](https://github.com/Turbo87/irating-rs), adapted to
//! this crate's types. Treat the result as a rough estimate of "if the
//! session ended right now with everyone in their current order," not
//! iRacing's actual number — the real formula isn't public.

use std::f32::consts::LN_2;

/// One driver's result, as input to [`estimate_changes`].
///
/// `PartialEq` so a caller can tell whether the field has actually moved since
/// the last estimate; see `session::IratingCache` for why that is worth
/// knowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaceResult {
    /// 1-based finishing (or current live) position.
    pub finish_rank: u32,
    pub start_irating: u32,
    /// Whether this driver started (took the green flag). Non-starters are
    /// still part of the field for expected-score purposes but redistribute
    /// their share of the change among the starters.
    pub started: bool,
}

/// Estimates each driver's iRating change for the field in `results`.
///
/// Returns one change (in iRating points) per input, in the same order.
/// `results` should include every driver in the session; `finish_rank`
/// determines placement, not array position.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "iRatings and field sizes are far below f32's exact-integer range; this whole module is already a rough estimate"
)]
pub fn estimate_changes(results: &[RaceResult]) -> Vec<f32> {
    // iRacing's documented "iRating is like 1600 pairwise Elo duels" design;
    // this constant converts a rating difference into a duel-win chance.
    let br1 = 1600.0 / LN_2;

    let num_registrations = results.len() as f32;
    let num_starters = results.iter().filter(|r| r.started).count();
    let num_non_starters = results.len() - num_starters;

    // For each driver, the sum of their win-chance against every other
    // driver (including themselves, hence the `- 0.5` correction below)
    // gives their expected finishing score.
    let expected_scores: Vec<f32> = results
        .iter()
        .map(|a| {
            let sum: f32 = results.iter().map(|b| chance(a.start_irating as f32, b.start_irating as f32, br1)).sum();
            sum - 0.5
        })
        .collect();

    // A small correction so the mid-field doesn't systematically gain or
    // lose relative to a field of this size.
    let fudge_factors: Vec<f32> = results
        .iter()
        .map(|r| {
            if r.started {
                let x = num_registrations - num_non_starters as f32 / 2.0;
                (x / 2.0 - r.finish_rank as f32) / 100.0
            } else {
                0.0
            }
        })
        .collect();

    let starter_changes: Vec<Option<f32>> = results
        .iter()
        .zip(&expected_scores)
        .zip(&fudge_factors)
        .map(|((r, expected), fudge)| {
            r.started
                .then(|| (num_registrations - r.finish_rank as f32 - expected - fudge) * 200.0 / num_starters as f32)
        })
        .collect();

    if num_non_starters == 0 {
        return starter_changes.into_iter().map(Option::unwrap_or_default).collect();
    }

    // Non-starters "give back" their expected score, split proportionally
    // among themselves, funded by the starters' combined change — keeps the
    // whole field's changes summing to roughly zero.
    let sum_starter_changes: f32 = starter_changes.iter().filter_map(|c| *c).sum();
    let sum_non_starter_expected: f32 =
        results.iter().zip(&expected_scores).filter(|(r, _)| !r.started).map(|(_, e)| *e).sum();

    results
        .iter()
        .zip(&expected_scores)
        .zip(&starter_changes)
        .map(|((_, expected), starter_change)| {
            if let Some(change) = starter_change {
                *change
            } else if sum_non_starter_expected.abs() > f32::EPSILON {
                -sum_starter_changes / num_non_starters as f32 * expected
                    / (sum_non_starter_expected / num_non_starters as f32)
            } else {
                0.0
            }
        })
        .collect()
}

/// The probability driver `a` finishes ahead of driver `b`, from their
/// iRatings and the `br1` scaling constant.
fn chance(a: f32, b: f32, br1: f32) -> f32 {
    let ea = (-a / br1).exp();
    let eb = (-b / br1).exp();
    (1.0 - ea) * eb / ((1.0 - eb) * ea + (1.0 - ea) * eb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(finish_rank: u32, start_irating: u32) -> RaceResult {
        RaceResult { finish_rank, start_irating, started: true }
    }

    #[test]
    fn a_full_field_sums_to_a_constant_plus_one() {
        // For a field where every entry starts (no non-starters), the
        // (registrations - rank - expected_score) term is exactly
        // zero-sum, but the fudge factor's own sum works out to
        // -num_starters/200, so the total change across the whole field is
        // always +1.0 regardless of field size or iRating spread — this is
        // a property of the formula itself (iRacing's system has a known
        // slight positive drift for full fields), not something specific
        // to this field's makeup.
        let results = vec![result(1, 3000), result(2, 2500), result(3, 2000), result(4, 1500)];
        let changes = estimate_changes(&results);
        let sum: f32 = changes.iter().sum();
        assert!((sum - 1.0).abs() < 0.01, "expected the field to sum to ~1.0, got {sum}");
    }

    #[test]
    fn beating_a_higher_irating_opponent_gains_more_than_beating_a_lower_one() {
        // Two identical fields except who the low-iRating driver beat.
        let underdog_beats_favorite = vec![result(1, 1500), result(2, 3000)];
        let favorite_beats_underdog = vec![result(1, 3000), result(2, 1500)];

        let upset_gain = estimate_changes(&underdog_beats_favorite)[0];
        let expected_gain = estimate_changes(&favorite_beats_underdog)[0];

        assert!(upset_gain > expected_gain, "an upset win should gain more than a favorite's expected win");
    }

    #[test]
    fn finishing_last_in_an_equal_field_loses_irating() {
        let results = vec![result(1, 2000), result(2, 2000), result(3, 2000), result(4, 2000)];
        let changes = estimate_changes(&results);
        assert!(changes[3] < 0.0, "last place in an evenly-matched field should lose iRating, got {}", changes[3]);
        assert!(changes[0] > 0.0, "first place in an evenly-matched field should gain iRating, got {}", changes[0]);
    }

    #[test]
    fn non_starters_do_not_panic_and_still_produce_a_result_per_entry() {
        let results = vec![
            RaceResult { finish_rank: 1, start_irating: 2500, started: true },
            result(2, 2000),
            RaceResult { finish_rank: 0, start_irating: 1800, started: false },
        ];
        let changes = estimate_changes(&results);
        assert_eq!(changes.len(), 3);
    }
}
