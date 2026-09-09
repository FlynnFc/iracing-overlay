# Rain in the weather surfaces

Live rain the overlay can act on — is it raining, how hard, how wet is the
track, are wets allowed, which way is it going — on the three surfaces that
show weather: the standalone Weather card, the black box Weather page, and
the relative footer.

## What the sim actually publishes

Audited 2026-09-05 against Flynn's own dumps (`vars_dump.txt`, root and
`target/release/`, build 2026.07.17.02) and the SDK docs.

### Live vars — verified present in both dumps

| Var | Type | Meaning |
| --- | --- | --- |
| `Precipitation` | f32 0–1 | Rain falling *now* at the start/finish line |
| `WeatherDeclaredWet` | bool | "The steward says rain tires can be used" |
| `RelativeHumidity` | f32 0–1 | At the start/finish line |

### Live var — verified against other overlays (2026-09-05)

`TrackWetness`, an enum 0–7: Unknown, Dry, MostlyDry, VeryLightlyWet,
LightlyWet, ModeratelyWet, VeryWet, ExtremelyWet. Not in either of our own
dumps (it wasn't in the candidate list), but cross-verified instead of
live-dumped, per Flynn: irdashies reads the live `TrackWetness` variable
in production and ships a `WETNESS_LEVELS` map with exactly this 1–7
wording and 0 as an empty/unknown state
(`src/frontend/components/Weather/WeatherTrackWetness/WeatherTrackWetness.tsx`),
and RaceLab ships a wetness module on the same variable. The UI still
treats it as optional and hides its rows when absent, so a session
without it costs nothing.

### Session YAML

- `WeekendOptions.ChanceOfRain` ("23 %") — a **static session setting**,
  already parsed into `precip_chance`. Crucially, Flynn's own
  `session_info_dump.yaml` (a Realistic-weather session) **has no
  `ChanceOfRain` key at all** — so in exactly the sessions where rain
  happens, the one rain number we currently show is absent.
- `WeekendInfo.TrackPrecipitation` ("0 %") — current precip via YAML;
  redundant with the live var, ignored.

### Not available to anyone: the forecast

"Chance of rain in 15 minutes" cannot be built. iRacing confirmed
(SimHub forum, 2024; nothing in release notes through 2026 changes it)
that the forecast graph in the in-sim weather tab is **not published via
the SDK or the /data API**. Every third-party overlay (RaceLab,
irdashies, benofficial2's SimHub pack) shows only *current* conditions —
wetness, precipitation, wind — plus at most the static session chance.
None shows a forecast, because none can.

The honest substitutes, all derivable locally:

1. **Trend** — sample `Precipitation` over time; "building" / "easing" is
   a real statement about the next few minutes without pretending to be a
   forecast.
2. **The session chance**, when the YAML declares one.
3. **`WeatherDeclaredWet`** — the actionable threshold: wets are legal.

## The current bug (relative footer)

`draw_conditions` shows only `precip_chance` (the static YAML setting,
gated at >10%). Live `Precipitation` is never read into the snapshot at
all — it exists only in the diagnostic candidate list. So in a
Realistic-weather session it can be *raining on screen* while the footer
shows nothing. Flynn observed exactly this.

## Design

### Snapshot (`WeatherSnapshot`)

New fields, all degrading to "absent" when the var is missing:

- `precip_now: Option<f32>` — live `Precipitation`; `None` when the var
  is absent, `Some(0.0)` is a real "not raining".
- `declared_wet: bool` — false when absent.
- `track_wetness: Option<TrackWetness>` — a new enum mirroring the SDK's;
  `Unknown`(0) maps to `None` like absence does.

`precip_chance` stays as-is: it is the *forecast-ish* number, distinct
from rain *now*.

### One vocabulary, everywhere

A shared helper turns `precip_now` into a phrase, so every surface says
the same thing:

| `precip_now` | word |
| --- | --- |
| = 0 | (nothing — it is not raining) |
| < 0.25 | light rain |
| < 0.60 | moderate rain |
| ≥ 0.60 | heavy rain |

The units are the sim's own relative scale, so the buckets are judgement
calls; thirds-ish splits need no tuning and can be nudged later. Track
wetness renders the enum as plain words ("lightly wet", "very wet").

### Weather card + black box Weather page

Same content on both, laid out per surface:

- **RAIN tile**: when `precip_now > 0`, the live percentage with the
  intensity word; the chance number is demoted, not shown alongside.
  When dry, today's behaviour: the chance as "RAIN n%" when the YAML
  declares one (>0), else no tile.
- **TRACK wetness**: a words-not-numbers line ("track lightly wet"),
  shown whenever `track_wetness` is `Some` and not Dry. Dry track shows
  nothing — same "no permanent zero" rule the card already follows.
- **WET TYRES OK**: shown only while `declared_wet` is true. This is the
  single most actionable bit in the whole system, so it gets colour
  (the existing `WIND` blue family, straight-edged per [[no-slanted-ui]]).
- **Trend** (phase 2): a small "building" / "easing" word beside the RAIN
  tile once a ~5-minute `Precipitation` history exists; silent below a
  change threshold, so it never flickers on noise.

### Relative footer

- Raining (`precip_now > 0`): show the live percentage (same blue),
  replacing the chance — the footer keeps exactly one rain number.
- Dry: unchanged — chance when >10%, else nothing.
- `declared_wet` while dry (rain gone, wets still legal): footer stays
  as-is; the declaration lives on the weather surfaces.

### Own car on wets (relative footer)

Are *we* on wet tyres — own-car state, always on when true, regardless
of what the rest of the field is doing (the per-row tag for the field is
[tyre-compound.md](tyre-compound.md), and stays gated on disagreement).

**Data.** `PlayerTireCompound` (i32, verified present in both dumps,
value 0 in the dry session) indexes into `DriverInfo.DriverTires` in the
session YAML, which names each index — Flynn's own dump carries
`TireIndex: 0 → "Hard"`, `TireIndex: 1 → "Wet"`. So the player's
compound is known **by name**, no index-guessing:

- On wets ⇔ the named compound contains "wet", case-insensitive.
- `PlayerTireCompound` −1 or absent → unknown → no tag.
- `DriverTires` missing or index out of range → fall back to
  tyre-compound.md's rule: index 1 or 3 counts as wet only while the
  track is wet (`track_wetness` above dry, or `precip_now` > 1%);
  otherwise no tag. Quiet failure, never a wrong "WET".

**Snapshot.** `WeatherSnapshot.on_wet_tyres: bool` — resolved in the
telemetry layer so the UI never sees the index. (It sits with weather
because every consumer of it is a weather surface.)

**Where it shows.** In `draw_conditions`, rightmost slot of the footer's
right-to-left run — a straight-edged `WET` block, `WIND` cyan fill, dark
text, the same tag geometry tyre-compound.md specifies, so the two
features render identically when both land. Shown only while true; on
dry tyres the footer is unchanged. Also echoed on the Weather card and
black box Weather page beside the WET TYRES OK badge (phase 2), since
"wets are legal" and "we are on wets" answer the same decision.

### Edge cases

- Var absent (older build, weird session): field is `None`/false, every
  row hides, layout closes up — the existing readings-row pattern.
- `Precipitation` present but 0 in a wet-declared session: WET TYRES OK
  still shows; rain rows don't. (Drying track after a shower.)
- `ChanceOfRain` absent *and* raining: RAIN tile shows live rain — the
  case that is broken today.
- Demo mode: `demo.rs` gets a rainy weather state (moderate rain, lightly
  wet, declared wet) so screenshots can show the full card.

## Phases

1. **Read the vars + fix the relative.** Snapshot fields, `TrackWetness`
   in the `--dump-vars` candidate list, live-rain tile on card/page,
   live rain % in the relative footer, and the own-car `WET` tag (its
   data — `PlayerTireCompound`, `DriverTires` — is already verified).
   Ships the bug fix and the core "is it raining, how hard, what are we
   on".
2. **Wetness + declaration.** TRACK wetness row and WET TYRES OK badge.
   (`TrackWetness` cross-verified against irdashies' production use rather
   than a live dump — see the data audit above.)
3. **Trend.** Precipitation history in the session state, "building" /
   "easing" on the weather surfaces.
