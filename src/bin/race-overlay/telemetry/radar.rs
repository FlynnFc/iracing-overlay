// Rust guideline compliant 2026-02-16

//! Pure math for the Radar Bars widget. Kept free of telemetry and UI types
//! so it's cheap to unit test.
//!
//! iRacing's SDK doesn't expose true 2D coordinates for other cars, so the
//! widget is built from the signals it does give: the same signed
//! relative-time gaps the Relative widget shows, read off the player's own
//! lap curve (see [`super::relative::LapCurve`]), the track positions those
//! gaps come from, and `CarLeftRight` (a coarse "car on this side" signal —
//! not *which* car, nor how far).
//!
//! # Units
//!
//! Every car reaching this module arrives as a [`Contact`], carrying both a
//! relative-time gap and a separation in metres. Selection and range gating
//! work in seconds, because that is the unit the gap is natively measured in
//! and the unit that stays meaningful through a slow corner. Everything the
//! driver actually *sees* works in metres, because "can I move over half a
//! car length" is a question about metres and no driver should have to
//! convert. The caller is responsible for producing the metres — see
//! [`Contact::gap_m`] for how, and why it is not simply seconds times speed.
//!
//! This module reduces a field of contacts to the ones worth drawing; the
//! caller decides which side they belong to using `CarLeftRight`, since that
//! is the only side information the SDK gives.

use std::collections::VecDeque;

/// One other car's separation from the player, in both units the widget needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    /// Signed relative-time gap in seconds; positive means ahead.
    pub gap_secs: f32,
    /// Signed separation in metres, positive ahead, measured centre to centre.
    ///
    /// Derived from the two cars' track positions against the track's length
    /// rather than from `gap_secs * speed`, because the seconds-to-metres
    /// conversion collapses at low speed: rolling out of a chicane at 60 km/h
    /// a car 82 ms back is half a car length away, and at 250 km/h the same
    /// 82 ms is more than a car length of clear air. A radar that draws those
    /// two identically is worse than no radar.
    pub gap_m: f32,
}

/// How the two cars' bodies relate, once their lengths are accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threat {
    /// More than a car length of clear air. Present, but not a factor yet.
    Clear,
    /// Less than a car length of clear air, but not yet overlapping.
    Close,
    /// The two cars share track. There is no room on that side.
    Overlapping,
}

/// The nearest car ahead and the nearest behind, within `range_secs`.
///
/// The ahead/behind distinction is carried by which slot a contact lands in,
/// not by its sign — returned contacts keep their signed gaps untouched.
/// Returns `None` in a slot with no car close enough.
///
/// A gap of exactly zero counts as ahead, an arbitrary but harmless choice:
/// two cars whose projected lap times match to the float are level, and the
/// widget draws either slot identically at that position.
#[must_use]
pub fn nearest_ahead_behind(contacts: &[Contact], range_secs: f32) -> (Option<Contact>, Option<Contact>) {
    let nearest = |want_ahead: bool| -> Option<Contact> {
        contacts
            .iter()
            .copied()
            .filter(|c| in_range(*c, range_secs) && (c.gap_secs >= 0.0) == want_ahead)
            .min_by(|a, b| a.gap_secs.abs().total_cmp(&b.gap_secs.abs()))
    };
    (nearest(true), nearest(false))
}

/// The two nearest cars overall, nearest first, ignoring which way they are.
///
/// Used when `CarLeftRight` reports cars on both sides: the field then holds
/// at least two cars that can be hit, and the widget wants one per bar rather
/// than the same car drawn twice.
#[must_use]
pub fn two_nearest(contacts: &[Contact], range_secs: f32) -> (Option<Contact>, Option<Contact>) {
    let mut in_range: Vec<Contact> = contacts.iter().copied().filter(|c| in_range(*c, range_secs)).collect();
    in_range.sort_by(|a, b| a.gap_secs.abs().total_cmp(&b.gap_secs.abs()));
    let mut found = in_range.into_iter();
    (found.next(), found.next())
}

/// Pairs two detected cars with the sides they most likely belong to.
///
/// `CarLeftRight` says a car is on each side but never which car, so the
/// assignment is inferred from continuity: whichever pairing leaves each side
/// closest to the gap it was already showing wins. A car that has been beside
/// the player for a second keeps its bar as the second car arrives, rather
/// than the two swapping sides on the tick the signal changes.
///
/// With no history on either side the pairing is arbitrary but stable — the
/// nearer car goes left — because there is genuinely nothing to go on. Takes
/// `cars` nearest-first, as [`two_nearest`] returns them, and the sides' last
/// shown signed gaps in seconds.
#[must_use]
pub fn assign_sides(
    cars: (Option<Contact>, Option<Contact>),
    last_left_secs: Option<f32>,
    last_right_secs: Option<f32>,
) -> (Option<Contact>, Option<Contact>) {
    let (first, second) = cars;
    // An empty slot on either end costs nothing, so a side with no history —
    // or a tick with only one car in range — never forces a swap on its own.
    let cost = |car: Option<Contact>, last: Option<f32>| -> f32 {
        match (car, last) {
            (Some(car), Some(last)) => (car.gap_secs - last).abs(),
            _ => 0.0,
        }
    };
    let straight = cost(first, last_left_secs) + cost(second, last_right_secs);
    let swapped = cost(second, last_left_secs) + cost(first, last_right_secs);
    if swapped < straight { (second, first) } else { (first, second) }
}

/// Metres of clear air between two cars, negative once they overlap.
///
/// `separation_m` is centre to centre, so two cars of `car_length_m` exactly
/// that far apart are bumper to bumper and read zero.
#[must_use]
pub fn clear_gap_metres(separation_m: f32, car_length_m: f32) -> f32 {
    separation_m.abs() - car_length_m
}

/// Classifies a clear gap into the three states the widget colours by.
///
/// The boundary between [`Threat::Clear`] and [`Threat::Close`] is one car
/// length of clear air: the point past which a car can no longer tuck fully
/// in front of or behind the player, so the space stops being available.
#[must_use]
pub fn threat(clear_gap_m: f32, car_length_m: f32) -> Threat {
    if clear_gap_m < 0.0 {
        Threat::Overlapping
    } else if clear_gap_m < car_length_m {
        Threat::Close
    } else {
        Threat::Clear
    }
}

/// How far back the closing-rate window reaches, in seconds.
///
/// Long enough that a steady rate is measured over real movement rather than
/// telemetry jitter, short enough that the widget's tail still reacts within a
/// corner. At 0.6 s a car closing at 2 m/s trails about a quarter of a car
/// length — clearly visible without dominating the marker it belongs to.
pub const CLOSING_WINDOW_SECS: f64 = 0.6;

/// Above this closing rate a sample is read as a different car arriving in the
/// slot rather than as motion, in m/s.
///
/// The slot holds "the nearest car ahead on this side", which is a role, not a
/// car: when the car filling it changes, the separation jumps. 40 m/s is far
/// beyond any real closing rate between two cars racing each other and far
/// below the jump a substitution produces at radar range.
const SUBSTITUTION_RATE_MPS: f32 = 40.0;

/// A short window of one radar slot's separations, for measuring closing rate.
///
/// Held per side and per direction across ticks. Kept here rather than beside
/// the telemetry loop because it is arithmetic over a ring buffer and nothing
/// else, and because the guard against the slot changing car mid-window is the
/// part most worth testing.
#[derive(Debug, Default)]
pub struct SeparationHistory {
    /// `(session time in seconds, separation magnitude in metres)`, oldest first.
    samples: VecDeque<(f64, f32)>,
}

impl SeparationHistory {
    /// Records one tick's separation, dropping samples past the window.
    ///
    /// A jump implying more than [`SUBSTITUTION_RATE_MPS`] discards the window
    /// first: the rate that would be measured across it describes a different
    /// car, not this one's movement.
    #[expect(clippy::cast_possible_truncation, reason = "a gap between two telemetry ticks is a fraction of a second")]
    pub fn push(&mut self, time_secs: f64, separation_m: f32) {
        if let Some(&(last_time, last_m)) = self.samples.back() {
            let dt = time_secs - last_time;
            if dt <= 0.0 || (separation_m - last_m).abs() > SUBSTITUTION_RATE_MPS * dt as f32 {
                self.samples.clear();
            }
        }
        self.samples.push_back((time_secs, separation_m.abs()));
        // Keep exactly one sample at or behind the window's trailing edge, so
        // the measured interval spans the full window instead of shrinking to
        // whatever happens to have arrived inside it.
        while self.samples.len() > 2 && self.samples.get(1).is_some_and(|&(t, _)| time_secs - t >= CLOSING_WINDOW_SECS)
        {
            self.samples.pop_front();
        }
    }

    /// Forgets everything recorded, for a slot that has gone empty.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Metres per second the gap is closing; negative while it opens.
    ///
    /// `None` until two samples separated in time exist, which reads as "not
    /// moving relative to you" at the call site — the safe default, since a
    /// missing tail claims nothing.
    #[must_use]
    #[expect(clippy::cast_possible_truncation, reason = "the window is CLOSING_WINDOW_SECS wide, well inside f32")]
    pub fn closing_mps(&self) -> Option<f32> {
        let (&(old_time, old_m), &(new_time, new_m)) = self.samples.front().zip(self.samples.back())?;
        let dt = new_time - old_time;
        if dt <= 0.0 {
            return None;
        }
        Some((old_m - new_m) / dt as f32)
    }
}

/// Whether a contact is close enough in time to register at all.
///
/// Guards non-finite gaps too: `CarIdxEstTime` reads as a sentinel for cars
/// that aren't on track, which can surface as a non-finite gap once
/// differenced, and such a value would otherwise sort as the nearest car and
/// pin a phantom marker to the bar.
fn in_range(contact: Contact, range_secs: f32) -> bool {
    contact.gap_secs.is_finite() && contact.gap_m.is_finite() && contact.gap_secs.abs() <= range_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A contact at `secs`, with metres filled in at a plausible racing speed.
    fn at(secs: f32) -> Contact {
        Contact { gap_secs: secs, gap_m: secs * 55.0 }
    }

    #[test]
    fn picks_the_closest_car_on_each_side() {
        let gaps = [at(0.8), at(-0.2), at(0.3), at(-0.5)];
        let (ahead, behind) = nearest_ahead_behind(&gaps, 1.0);
        assert_eq!(ahead, Some(at(0.3)));
        assert_eq!(behind, Some(at(-0.2)));
    }

    #[test]
    fn keeps_gaps_signed_so_a_slot_knows_which_way_it_faces() {
        let (_ahead, behind) = nearest_ahead_behind(&[at(-0.4)], 1.0);
        assert_eq!(behind.map(|c| c.gap_secs), Some(-0.4), "behind gaps must keep their sign");
    }

    #[test]
    fn ignores_cars_beyond_the_range() {
        let (ahead, behind) = nearest_ahead_behind(&[at(2.5), at(-3.0)], 1.0);
        assert_eq!(ahead, None);
        assert_eq!(behind, None);
    }

    #[test]
    fn a_side_with_a_car_only_ahead_leaves_behind_empty() {
        let (ahead, behind) = nearest_ahead_behind(&[at(0.25)], 1.0);
        assert_eq!(ahead, Some(at(0.25)));
        assert_eq!(behind, None);
    }

    #[test]
    fn an_empty_field_yields_nothing() {
        assert_eq!(nearest_ahead_behind(&[], 1.0), (None, None));
    }

    #[test]
    fn skips_non_finite_gaps() {
        let gaps = [at(f32::NAN), at(f32::INFINITY), at(0.4)];
        let (ahead, behind) = nearest_ahead_behind(&gaps, 1.0);
        assert_eq!(ahead, Some(at(0.4)));
        assert_eq!(behind, None);
    }

    /// A car whose seconds are fine but whose metres came out non-finite —
    /// a zero track length, say — must not be drawn at all rather than drawn
    /// at an undefined position.
    #[test]
    fn skips_contacts_with_no_usable_metres() {
        let broken = Contact { gap_secs: 0.1, gap_m: f32::NAN };
        assert_eq!(nearest_ahead_behind(&[broken, at(0.4)], 1.0).0, Some(at(0.4)));
    }

    #[test]
    fn two_nearest_takes_the_closest_pair_either_side_of_the_player() {
        let gaps = [at(0.8), at(-0.2), at(0.3), at(-0.5)];
        assert_eq!(two_nearest(&gaps, 1.0), (Some(at(-0.2)), Some(at(0.3))));
    }

    #[test]
    fn two_nearest_can_return_two_cars_the_same_side_of_the_player() {
        // Both ahead: three-wide into a corner with nobody behind is a real
        // shape, and pretending one of them is behind would be a lie.
        let gaps = [at(0.2), at(0.3)];
        assert_eq!(two_nearest(&gaps, 1.0), (Some(at(0.2)), Some(at(0.3))));
    }

    #[test]
    fn two_nearest_reports_what_little_it_has() {
        assert_eq!(two_nearest(&[at(0.2)], 1.0), (Some(at(0.2)), None));
        assert_eq!(two_nearest(&[], 1.0), (None, None));
    }

    #[test]
    fn sides_keep_the_car_they_were_already_showing() {
        // Left has been showing a car 0.30s ahead; right one 0.25s behind.
        // The pair arrives nearest-first, which is the wrong order for those
        // sides, so it must be swapped rather than taken as given.
        let cars = (Some(at(-0.24)), Some(at(0.31)));
        let (left, right) = assign_sides(cars, Some(0.30), Some(-0.25));
        assert_eq!(left, Some(at(0.31)));
        assert_eq!(right, Some(at(-0.24)));
    }

    #[test]
    fn sides_with_no_history_take_the_pair_as_given() {
        let cars = (Some(at(0.1)), Some(at(-0.2)));
        assert_eq!(assign_sides(cars, None, None), cars);
    }

    /// One side having history must not drag the other's car across, only
    /// settle which of the two belongs to it.
    #[test]
    fn one_remembered_side_still_places_both_cars() {
        let cars = (Some(at(0.4)), Some(at(-0.1)));
        let (left, right) = assign_sides(cars, Some(-0.12), None);
        assert_eq!(left, Some(at(-0.1)));
        assert_eq!(right, Some(at(0.4)));
    }

    #[test]
    fn clear_air_runs_out_exactly_a_car_length_apart() {
        assert!((clear_gap_metres(4.7, 4.7) - 0.0).abs() < 1e-4);
        assert!((clear_gap_metres(-9.4, 4.7) - 4.7).abs() < 1e-4, "behind is as clear as ahead");
        assert!(clear_gap_metres(2.0, 4.7) < 0.0, "closer than a car length is an overlap");
    }

    #[test]
    fn threat_boundaries_land_where_the_driver_expects() {
        assert_eq!(threat(-0.1, 4.7), Threat::Overlapping);
        assert_eq!(threat(0.0, 4.7), Threat::Close, "bumper to bumper is not yet an overlap");
        assert_eq!(threat(4.6, 4.7), Threat::Close);
        assert_eq!(threat(4.7, 4.7), Threat::Clear, "a full car length of air is room to move");
    }

    #[test]
    fn closing_rate_is_positive_while_the_gap_shrinks() {
        let mut history = SeparationHistory::default();
        history.push(0.0, 10.0);
        history.push(0.5, 9.0);
        let rate = history.closing_mps().expect("two samples must yield a rate");
        assert!((rate - 2.0).abs() < 1e-4, "closing 1 m in 0.5 s is 2 m/s, got {rate}");
    }

    #[test]
    fn closing_rate_is_negative_while_the_gap_opens() {
        let mut history = SeparationHistory::default();
        history.push(0.0, 5.0);
        history.push(1.0, 7.0);
        assert!(history.closing_mps().is_some_and(|rate| (rate + 2.0).abs() < 1e-4));
    }

    #[test]
    fn closing_rate_needs_two_samples_in_time() {
        let mut history = SeparationHistory::default();
        assert_eq!(history.closing_mps(), None);
        history.push(1.0, 4.0);
        assert_eq!(history.closing_mps(), None, "one sample describes no movement");
    }

    /// The measured interval must not shrink as ticks arrive: a slot sampled
    /// at 60 Hz would otherwise differentiate over 16 ms of jitter.
    #[test]
    #[expect(clippy::cast_possible_truncation, reason = "two seconds of simulated ticks, well inside f32")]
    fn the_window_keeps_a_sample_behind_its_trailing_edge() {
        let mut history = SeparationHistory::default();
        for tick in 0..120 {
            let t = f64::from(tick) / 60.0;
            history.push(t, 10.0 - t as f32);
        }
        let rate = history.closing_mps().expect("a full window must yield a rate");
        assert!((rate - 1.0).abs() < 0.05, "a steady 1 m/s must read as 1 m/s, got {rate}");
    }

    /// A different car dropping into the slot jumps the separation. Measuring
    /// across that jump would report a closing rate of tens of m/s and draw a
    /// tail the length of the bar.
    #[test]
    fn a_car_swapping_into_the_slot_does_not_read_as_closing() {
        let mut history = SeparationHistory::default();
        history.push(0.0, 12.0);
        history.push(0.1, 11.8);
        history.push(0.2, 2.0);
        assert_eq!(history.closing_mps(), None, "the window must restart, not measure the jump");
    }

    #[test]
    fn a_cleared_slot_forgets_its_history() {
        let mut history = SeparationHistory::default();
        history.push(0.0, 10.0);
        history.push(0.5, 9.0);
        history.clear();
        assert_eq!(history.closing_mps(), None);
    }
}
