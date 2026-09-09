// Rust guideline compliant 2026-02-16

//! Pure weather math: wind direction relative to the car's own heading, so
//! the UI can draw an arrow that turns with the car instead of a fixed
//! compass bearing that isn't very useful mid-corner. Kept free of
//! telemetry types so it's cheap to unit test.

use std::f32::consts::{PI, TAU};

/// Wind direction relative to the car's nose, in radians: 0 means the wind
/// is blowing toward the front of the car, increasing clockwise (`WindDir`
/// and `Yaw` are both given in the same world-frame radians, so this is
/// just their difference, normalized). Returns `None` if either input is
/// unavailable.
#[must_use]
pub fn wind_direction_relative_to_car(wind_dir_rad: Option<f32>, car_yaw_rad: Option<f32>) -> Option<f32> {
    let (wind, yaw) = (wind_dir_rad?, car_yaw_rad?);
    Some(normalize_angle(wind - yaw))
}

/// Normalizes an angle in radians to `(-\u{3c0}, \u{3c0}]`.
fn normalize_angle(rad: f32) -> f32 {
    let mut angle = rad % TAU;
    if angle > PI {
        angle -= TAU;
    } else if angle <= -PI {
        angle += TAU;
    }
    angle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_headings_give_a_relative_angle_of_zero() {
        let rel = wind_direction_relative_to_car(Some(1.0), Some(1.0)).expect("both present");
        assert!(rel.abs() < f32::EPSILON);
    }

    #[test]
    fn a_quarter_turn_gives_a_right_angle() {
        let rel = wind_direction_relative_to_car(Some(PI / 2.0), Some(0.0)).expect("both present");
        assert!((rel - PI / 2.0).abs() < 1e-5);
    }

    #[test]
    fn wraps_into_the_shortest_signed_range() {
        // Wind angle just past a full turn ahead of the car's heading
        // should normalize to a small angle, not a value near 2\u{3c0}.
        let rel = wind_direction_relative_to_car(Some(0.01), Some(TAU - 0.01)).expect("both present");
        assert!((rel - 0.02).abs() < 1e-4, "expected ~0.02, got {rel}");
    }

    #[test]
    fn missing_either_input_returns_none() {
        assert_eq!(wind_direction_relative_to_car(None, Some(0.0)), None);
        assert_eq!(wind_direction_relative_to_car(Some(0.0), None), None);
    }
}
