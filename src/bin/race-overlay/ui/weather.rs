// Rust guideline compliant 2026-02-16

//! The rain reading every weather surface shares.
//!
//! There is no Weather panel any more: the black box's Weather page and the
//! Relative's footer are where conditions are read now. What is left here is
//! the one judgement both of them have to make the same way — whether to
//! print the rain falling now or the session's declared chance of it, and
//! what to call the intensity — so the two surfaces can never disagree about
//! the weather they are describing.

use crate::telemetry::snapshot::WeatherSnapshot;

/// Rain lighter than this is light; from here up to [`HEAVY_RAIN_FROM`] it is
/// plain rain, past that heavy. The sim's `Precipitation` is its own relative
/// 0–1 scale, so the splits are judgement calls: thirds-ish, biased so
/// "heavy" is not said cheaply.
const LIGHT_RAIN_BELOW: f32 = 0.25;
const HEAVY_RAIN_FROM: f32 = 0.60;

/// Live rain below this counts as none: it would print as a `0%` that reads
/// like a confident dry, which is worse than saying nothing.
const LIVE_RAIN_MIN: f32 = 0.005;

/// The rain slot of every weather surface, or `None` when there is nothing
/// worth a reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RainReading {
    /// A whole percentage, e.g. `42%`.
    pub value: String,
    /// `RAIN` for a declared chance; for live rain the intensity is the
    /// label, so the number never needs decoding into a severity.
    pub label: &'static str,
    /// Water falling now, as opposed to a declared chance — drawn in the
    /// water colour so the two are never mistaken for each other.
    pub live: bool,
}

/// Rain falling now, when there is enough of it to print — see
/// [`LIVE_RAIN_MIN`].
#[must_use]
pub fn live_rain(weather: &WeatherSnapshot) -> Option<f32> {
    weather.precip_now.filter(|precip| *precip >= LIVE_RAIN_MIN)
}

/// The RAIN reading shared by the black box page and the Relative's footer:
/// live rain while it falls, otherwise the session's declared chance where
/// one is worth showing.
///
/// Live wins outright rather than sharing the row: both answer "what is the
/// rain doing?", and once water is falling the declared chance is history.
#[must_use]
pub fn rain_reading(weather: &WeatherSnapshot) -> Option<RainReading> {
    if let Some(now) = live_rain(weather) {
        return Some(RainReading { value: format!("{:.0}%", now * 100.0), label: rain_intensity(now), live: true });
    }
    percent(weather.precip_chance).map(|value| RainReading { value, label: "RAIN", live: false })
}

/// A fraction as a whole percentage, or `None` where there is nothing to
/// show: unreported, or zero.
fn percent(fraction: Option<f32>) -> Option<String> {
    fraction.filter(|value| *value > 0.0).map(|value| format!("{:.0}%", value * 100.0))
}

/// The intensity word for live rain, per the scale above. Moderate rain is
/// plain `RAIN`: the middle of the scale is the unmarked case, and the
/// narrow readings row cannot afford a `MODERATE`.
fn rain_intensity(fraction: f32) -> &'static str {
    if fraction < LIGHT_RAIN_BELOW {
        "LIGHT RAIN"
    } else if fraction < HEAVY_RAIN_FROM {
        "RAIN"
    } else {
        "HEAVY RAIN"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_fraction_as_a_whole_percentage() {
        assert_eq!(percent(Some(0.10)).as_deref(), Some("10%"));
        assert_eq!(percent(Some(0.234)).as_deref(), Some("23%"));
    }

    /// A session without a declared chance, or one reporting none, shows
    /// nothing rather than a confident `0%` that would train the eye to skip
    /// the slot.
    #[test]
    fn missing_or_zero_shows_no_reading() {
        assert_eq!(percent(None), None);
        assert_eq!(percent(Some(0.0)), None);
    }

    fn weather_with(precip_now: Option<f32>, precip_chance: Option<f32>) -> WeatherSnapshot {
        WeatherSnapshot { precip_chance, precip_now, ..WeatherSnapshot::default() }
    }

    /// Once water is falling, the declared chance is history: the live
    /// figure takes the slot outright.
    #[test]
    fn live_rain_replaces_the_declared_chance() {
        let reading = rain_reading(&weather_with(Some(0.42), Some(0.23))).expect("raining");
        assert_eq!((reading.value.as_str(), reading.label, reading.live), ("42%", "RAIN", true));
    }

    #[test]
    fn the_intensity_word_follows_the_scale() {
        assert_eq!(rain_intensity(0.10), "LIGHT RAIN");
        assert_eq!(rain_intensity(0.40), "RAIN");
        assert_eq!(rain_intensity(0.75), "HEAVY RAIN");
    }

    /// A dry session shows the declared chance when there is one, and a
    /// sub-printable trace of drizzle does not count as rain.
    #[test]
    fn a_dry_session_falls_back_to_the_chance() {
        let reading = rain_reading(&weather_with(Some(0.0), Some(0.23))).expect("a chance was declared");
        assert_eq!((reading.value.as_str(), reading.label, reading.live), ("23%", "RAIN", false));
        let trace = rain_reading(&weather_with(Some(0.004), Some(0.23))).expect("a chance was declared");
        assert!(!trace.live, "a trace below half a percent is not live rain");
        assert_eq!(rain_reading(&weather_with(None, None)), None);
    }
}
