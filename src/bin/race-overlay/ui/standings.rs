// Rust guideline compliant 2026-02-16

//! Renders the Standings as one continuous, softly layered timing table.
//!
//! One card, three bands: the drivers, their timing (gap, fastest, last), and
//! — in endurance mode — their strategy (stint, completed stops, projected net
//! position), each band a step lighter than the one before so a row reads as
//! one unbroken line across all three. A status gutter sits outside the card.
//!
//! The bands are laid out from one shared row plan rather than by three
//! independent layouts. Row heights are fixed per row kind, so the plan can
//! be walked once per column and every column lands on the same y — which is
//! the whole reason the design can split a single logical row across several
//! bands and a gutter without them drifting apart.
//!
//! Positions are class-relative throughout: each class gets its own section
//! showing its leaders, and the player's own class additionally opens out
//! into a window around wherever the player currently sits.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use egui::{Color32, Rect, RichText, Rounding, Ui};
use iracing_telem::flags::TrackLocation;

use super::theme;
use super::{
    ACCENT, BLOCK_ROUNDING, Metrics, PLAYER_PLATE, PLAYER_ROW_FILL, POSITION_PLATE, TABLE_CARD_ROUNDING, WIND,
    card_frame, class_accent, class_color, flags, format_clock, format_irating, format_lap_time,
    format_minutes_seconds, format_session_length, icons, lap_text, logos, margin, paint_hatch, paint_text, readout,
    row_stripe, table_card_rounding, text_primary, text_secondary, text_tertiary, text_width, tint,
};
use crate::config::{EnduranceMode, StandingsConfig};
use crate::telemetry::snapshot::{
    ClassSection, EnduranceMeta, GridStatus, SessionKind, StandingsEntry, TelemetrySnapshot,
};
use crate::telemetry::standings::windowed_class_positions;

/// A class id no car can have, standing in for "the player isn't classified
/// yet", so no class matches and every one falls to its leaders-only view.
const NO_CLASS: i32 = -1;

/// How many of the player's own class's top positions to always show.
const TOP_N: i32 = 3;
/// How many drivers a class the player isn't in shows: just its leader.
/// Other classes are context, not the race the player is running, so one
/// name answers "who's winning it" without spending two more rows per class.
const OTHER_CLASS_TOP_N: i32 = 1;
/// How many positions before/after the player to show in the windowed view
/// of the player's own class: the car to beat and the car to hold off. The
/// mockup shows three each way; that made the panel the tallest thing on
/// screen, and the rows past the nearest were the ones never read.
const WINDOW_BEFORE: i32 = 1;
const WINDOW_AFTER: i32 = 1;
/// The fewest drivers the player's own class shows when it has that many.
///
/// The window alone leaves a leader looking at two rows, because their own
/// window merges into the top three. This is what a mid-field player gets —
/// the top three, then the three around them — so the panel's height holds
/// steady wherever in the class the player is running.
const MIN_CLASS_ROWS: i32 = TOP_N + 1 + WINDOW_BEFORE + WINDOW_AFTER;

/// Column widths and the gaps between them.
///
/// The driver band's width is the design's measurement and the one a config
/// may change: the timing and strategy bands are all fixed columns, while
/// the driver band ends in a run of name room that a narrower screen can
/// give up — long names gain an ellipsis instead. The floor still fits a
/// short name between the plate and the iRating pill; the ceiling is as far
/// as a long name is worth a wider panel.
pub const DESIGN_NAME_WIDTH: f32 = 440.0;
pub const NAME_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 300.0..=560.0;
const RIGHT_WIDTH: f32 = 286.0;

/// The optional tyre column tacked onto the timing band's right edge: the
/// column's width, the compound circle's radius within it, and the letter
/// inside the circle.
const TYRE_WIDTH: f32 = 40.0;
const TYRE_RADIUS: f32 = 10.0;
const TYRE_LETTER_SIZE: f32 = 11.0;

/// The timing band's width: its three fixed columns, plus the tyre column
/// when that is on.
fn timing_width(show_tyres: bool) -> f32 {
    RIGHT_WIDTH + if show_tyres { TYRE_WIDTH } else { 0.0 }
}

/// The optional race-change slot between the position plate and the class
/// bar — `▲2` for places gained since the race began — and its type size.
/// The class bar, flag and name all step right by the slot while it is on;
/// the name column's room absorbs the width.
const CHANGE_SLOT: f32 = 30.0;
const CHANGE_SIZE: f32 = 12.0;

/// The configured driver-band width, defended against a hand-edited file the
/// same way `Metrics::new` defends the scale.
fn name_width(config: &StandingsConfig) -> f32 {
    if config.name_width.is_finite() {
        config.name_width.clamp(*NAME_WIDTH_RANGE.start(), *NAME_WIDTH_RANGE.end())
    } else {
        DESIGN_NAME_WIDTH
    }
}
/// The strategy band's width, added only when endurance mode is on. Narrow,
/// with a stint bar, completed stop count and projected position delta.
const ENDURANCE_WIDTH: f32 = 170.0;
/// Height of the strategy line beneath the table in endurance mode: tall
/// enough for readout numerals, because it is the panel's verdict.
const SUMMARY_HEIGHT: f32 = 44.0;
/// The strategy line's figures and their captions.
const SUMMARY_VALUE_SIZE: f32 = 26.0;
const SUMMARY_TAG_HEIGHT: f32 = 24.0;

/// The strategy band's columns, from its left edge: the stint bar, the stop
/// count, the net delta.
const STINT_X: f32 = 12.0;
const STINT_BAR_SIZE: (f32, f32) = (56.0, 8.0);
/// The bar shortened to make room for the lap count beside it, when that is
/// switched on, and the count's type and gap.
const STINT_BAR_SHORT: f32 = 36.0;
const STINT_LAPS_GAP: f32 = 5.0;
const STINT_LAPS_SIZE: f32 = 11.0;
const STOPS_X: f32 = 80.0;
const NET_X: f32 = 128.0;
const GUTTER_GAP: f32 = 8.0;
const GUTTER_WIDTH: f32 = 32.0;

/// This card's fill: the shared [`super::PANEL_BG`] ink washed further back,
/// so the track shows through the standings more than the other widgets.
/// Standings is the tallest panel on screen, and at the shared opacity it
/// reads as a wall rather than an overlay.
const STANDINGS_BG: Color32 = Color32::from_rgba_premultiplied(17, 19, 23, 218);
/// The timing band's lifted fill, faded by the same step as [`STANDINGS_BG`]
/// so the bands keep their contrast with each other; and the strategy band's,
/// lifted the same step again.
const STANDINGS_TILE_BG: Color32 = Color32::from_rgba_premultiplied(22, 24, 29, 222);
const STANDINGS_STRATEGY_BG: Color32 = Color32::from_rgba_premultiplied(27, 29, 35, 226);

/// Fixed row heights. These are what keep the three columns in lockstep, so
/// they must not depend on a row's content.
///
/// Tighter than the mockup's 40-point rows throughout: this panel is on
/// screen for the whole race, and every point of height is a point of track
/// it covers. The type scale below came down with it.
const ROW_HEIGHT: f32 = 34.0;
const CLASS_HEADER_HEIGHT: f32 = 28.0;
const SKIP_HEIGHT: f32 = 12.0;
/// The rule drawn across a skip: how far in from the card's edges it starts,
/// and the length of a dash and the gap after it.
///
/// Dashed rather than solid, because a skip is not the end of a section — it
/// is a run of cars deliberately left out between the leaders and the window
/// around the player. A broken line says "there is more here" where a solid
/// one would say "this is where the group ends", and an empty band, which is
/// what this was, said neither: it read as spacing.
const SKIP_RULE_INSET: f32 = 12.0;
const SKIP_RULE_DASH: f32 = 5.0;
const SKIP_RULE_GAP: f32 = 5.0;
const TOP_BAR_HEIGHT: f32 = 38.0;

/// Type scale.
const NAME_SIZE: f32 = 18.0;
/// The position leads the row, with breathing room inside its compact tile.
const POSITION_SIZE: f32 = 22.0;
const TIME_SIZE: f32 = 16.0;
const CLOCK_SIZE: f32 = 18.0;
const COLUMN_LABEL_SIZE: f32 = 14.0;

/// The watched driver's name in the top bar. A step below the clock beside it:
/// it needs to be readable at a glance and read once, not compete with the
/// number that changes every second.
const SPECTATING_SIZE: f32 = 14.0;
const TAG_SIZE: f32 = 13.0;
const RATING_SIZE: f32 = 14.0;

/// The manufacturer's mark at the end of each row. Square, because every
/// mark on disk is, and capped by [`ROW_HEIGHT`] with a few points to spare.
const LOGO_SIZE: f32 = 28.0;
/// The iRating pill's half-height and width, beside the mark.
const PILL_HALF_HEIGHT: f32 = 11.0;
const PILL_WIDTH: f32 = 52.0;

/// Column positions within a driver row, in pixels from its left edge.
///
/// The class colour, struck down the position plate's trailing edge in a
/// field with more than one class — see [`super::paint_position_plate`].
const CLASS_EDGE: f32 = 4.0;
/// The slot beside the plate. Nothing is drawn in it now that the class has
/// moved onto the plate, but its width still spaces the columns, so nothing
/// moved when the bar went.
const CLASS_BAR_SIZE: (f32, f32) = (4.0, 22.0);
/// Where the position plate ends.
const POSITION_PLATE_END: f32 = 40.0;
/// The gap either side of the bar slot: between the plate's edge and the
/// slot, and between the slot and the driver's name.
///
/// One value for both, with [`BAR_X`] and [`NAME_X`] derived from it, so the
/// slot sits evenly between its neighbours instead of hard against the plate
/// on one side and adrift from the name on the other.
const BAR_GAP: f32 = 8.0;
/// The bar slot's centre, a gap clear of the plate's edge.
const BAR_X: f32 = POSITION_PLATE_END + BAR_GAP + CLASS_BAR_SIZE.0 / 2.0;
const NAME_X: f32 = BAR_X + CLASS_BAR_SIZE.0 / 2.0 + BAR_GAP;
/// The driver's flag, between the class bar and the name: its height, from
/// which the 4:3 file sets the width, and the gap it leaves before the name.
/// The name moves right by [`flag_slot`] while flags are on and sits at
/// [`NAME_X`] when they are off, so turning them off leaves no hole.
const FLAG_HEIGHT: f32 = 14.0;
const FLAG_GAP: f32 = 8.0;

/// The room the top bar's car count and its icon keep on the right: the
/// icon's centre sits 34 in from the edge and the icon is 20 wide, plus a
/// little clearance. The optional runs after the clock stop here.
const TOP_BAR_COUNT_RESERVED: f32 = 52.0;

/// How long a row takes to slide into a changed slot, in seconds.
///
/// Long enough that a position change is seen to *happen* — the row travels,
/// rather than the table silently being different — and short enough that the
/// panel has settled again before the next glance. The digits roll a touch
/// slower ([`POSITION_TUMBLE_SECS`]) so the number is still turning as its row
/// lands, which is what reads as a tumble rather than a redraw.
const ROW_TUMBLE_SECS: f32 = 0.28;

/// How long the position number takes to roll to a new value, in seconds.
const POSITION_TUMBLE_SECS: f32 = 0.45;

/// Position rolls wider than this many places snap instead of turning.
///
/// The same reasoning as [`MAX_TUMBLE_ROWS`]: a pass moves a number one or
/// two places, and a wider jump is scoring catching up or the window
/// re-slicing — a blur of digits would dramatise a non-event.
const MAX_POSITION_ROLL: f32 = 5.0;

/// Slides longer than this many rows snap instead of animating.
///
/// A pass moves a row one or two slots. Anything further is the *window*
/// moving — a re-slice around the player, a change of focus, the panel
/// re-planned — and a row sailing across class banners to its new home says
/// "position change" about something that wasn't one.
const MAX_TUMBLE_ROWS: f32 = 2.5;

/// Each driver row's animated vertical offset from its planned slot, in
/// already-scaled pixels — the tumble.
///
/// Targets are measured from the column's top rather than the screen, so
/// dragging the panel moves it in one piece instead of setting every row
/// swimming. Rows the animator hasn't seen start at their slot (egui's
/// animator adopts a new id's first value instantly), so a car entering the
/// window doesn't slide in from nowhere.
fn row_offsets(ui: &Ui, metrics: Metrics, rows: &[Row<'_>]) -> HashMap<i32, f32> {
    let mut offsets = HashMap::new();
    let mut y = metrics.px(TOP_BAR_HEIGHT);
    for row in rows {
        if let Row::Driver(entry) = row {
            let id = egui::Id::new(("standings-row-tumble", entry.car_idx));
            let animated = ui.ctx().animate_value_with_time(id, y, ROW_TUMBLE_SECS);
            let offset = animated - y;
            if offset.abs() > metrics.px(ROW_HEIGHT) * MAX_TUMBLE_ROWS {
                // Snap: a zero-time animation lands the value at once.
                ui.ctx().animate_value_with_time(id, y, 0.0);
            } else if offset.abs() > 0.5 {
                offsets.insert(entry.car_idx, offset);
            }
        }
        y += metrics.px(row.height());
    }
    offsets
}

/// The two blocks of a class header: the class itself, then how many cars
/// are in it — and the gap between them, and before the first.
const CLASS_TAG_WIDTH: f32 = 64.0;
const COUNT_TAG_WIDTH: f32 = 40.0;
const CLASS_TAG_GAP: f32 = 4.0;
const CLASS_TAG_INSET: f32 = 10.0;

/// Which edge of the enclosing card a column sits against, so its bottom
/// row can round the corners the card rounds and no others.
#[derive(Debug, Clone, Copy)]
enum Side {
    Left,
    Right,
    Standalone,
}

/// Everything [`column`] needs to know about the strip it's laying out,
/// grouped so the call doesn't turn into a row of bare positional arguments.
#[derive(Debug, Clone, Copy)]
struct ColumnSpec {
    metrics: Metrics,
    width: f32,
    /// Painted behind the whole column before any row; `None` leaves the
    /// enclosing card's own fill showing through.
    fill: Option<Color32>,
    side: Side,
    /// Whether this column's last row is also the card's bottom edge.
    ///
    /// False in endurance mode, where the strategy line takes that edge and
    /// every band instead ends square against it, one tint meeting the next.
    at_card_bottom: bool,
    /// Whether touching driver rows are separated by a groove.
    ///
    /// True for the columns that make up the table itself; false for the
    /// status gutter, which sits outside the card and has no surface for a
    /// bevel to be cut into.
    grooved: bool,
}

/// How a row should paint its own background.
///
/// Row fills are square everywhere except the last row, which has to follow
/// the card's rounded bottom corners — otherwise the fill paints over them
/// and the card ends in two hard, faintly translucent square corners while
/// every other panel is smoothly rounded.
#[derive(Debug, Clone, Copy)]
struct RowStyle {
    /// Whether this row takes the alternating stripe.
    odd: bool,
    rounding: Rounding,
}

/// One entry in the shared row plan the three columns are painted from.
enum Row<'a> {
    /// A class banner: a tinted label, car count, and field strength.
    ClassHeader(&'a ClassSection),
    Driver(&'a StandingsEntry),
    /// The break between a class's leaders and the window around the player.
    Skip,
}

impl Row<'_> {
    /// This row's height. Fixed per kind, never content-dependent — see the
    /// module docs.
    fn height(&self) -> f32 {
        match self {
            Self::ClassHeader(_) => CLASS_HEADER_HEIGHT,
            Self::Driver(_) => ROW_HEIGHT,
            Self::Skip => SKIP_HEIGHT,
        }
    }
}

/// `options` carries the top-level switches a driver row reads: the off-track
/// tally in the gutter, and the flag before each name.
pub fn draw(ui: &mut Ui, snapshot: Option<&TelemetrySnapshot>, config: &StandingsConfig, options: super::RowOptions) {
    let metrics = Metrics::new(config.scale);
    let Some(snapshot) = snapshot else {
        placeholder(ui, metrics, "waiting for iRacing\u{2026}");
        return;
    };
    if snapshot.standings.is_empty() {
        placeholder(ui, metrics, "waiting for session standings\u{2026}");
        return;
    }
    // The player's own row is what opens their class out into a window around
    // them. Without it — before the first flying lap, or sitting in the pits
    // pre-session — every class still shows its leaders, which is the useful
    // half of the widget. It used to refuse to draw anything at all until the
    // sim had scored the player, which is exactly the stretch of a session
    // where you most want to see who is out there.
    let player = snapshot.standings.iter().find(|e| e.is_focus);
    // The class comes from the entry list where the player has a row, and
    // otherwise straight from the session's driver list, which knows it from
    // the moment the session loads. Relying on the row alone meant a driver
    // who had not set a lap had no class, and a class nobody is in is never
    // opened out — so a full practice session showed three names.
    //
    // A class position of zero for an unscored player is not a fallback but
    // the honest answer: they are not classified, so the window opens at the
    // top of their class, which is what you want to see while you wait.
    // One class running means the class colors distinguish nothing, so rows
    // take a stand-in accent instead; see `ui::class_accent`.
    let class_count = snapshot.class_sections.len();
    let my_class_id = player.map(|p| p.car_class_id).or(snapshot.focus_car_class_id).unwrap_or(NO_CLASS);
    let rows = plan_rows(snapshot, my_class_id, player.map_or(0, |p| p.class_position), config.show_other_classes);
    if rows.is_empty() {
        placeholder(ui, metrics, "waiting for session standings\u{2026}");
        return;
    }

    // The tumble: where each row's slot has moved since the last frame, so
    // every column below slides the same logical row together.
    let tumble = row_offsets(ui, metrics, &rows);

    let endurance = snapshot.endurance;
    // Auto offers the extra columns for endurance races. On also exposes
    // measured stint lengths and completed stops during practice; NET stays
    // unknown without a race finish to project.
    let show_endurance = match config.endurance_mode {
        EnduranceMode::On => true,
        EnduranceMode::Off => false,
        EnduranceMode::Auto => endurance.multi_stop_race && snapshot.relative_meta.session_kind.is_race(),
    };
    // The strategy line across the card's foot is not gated with the columns:
    // laps left, stops to go and the net position are worth the row in any
    // race, including with the endurance columns switched off. Practice and
    // qualifying never get a race-finish summary, even with the columns On.
    let show_summary = snapshot.relative_meta.session_kind.is_race();
    // The tyre column is opt-in twice over: the config switch, and the sim
    // actually publishing a compound for at least one car — without that the
    // column would be a strip of empty circles all session.
    let show_tyres = config.show_tyres && snapshot.standings.iter().any(|e| e.tyre.is_some());
    // The change column self-hides the same way: the figure only exists in
    // races, so practice and qualifying never spend the slot.
    let show_position_change =
        config.show_position_change && snapshot.standings.iter().any(|e| e.race_position_change.is_some());
    let meta = TopBarMeta {
        session_kind: snapshot.relative_meta.session_kind,
        car_count: snapshot.relative_meta.car_count,
        elapsed_secs: snapshot.relative_meta.race_elapsed_secs,
        countdown_secs: snapshot.relative_meta.countdown_secs(),
        session_length_secs: snapshot.relative_meta.session_length_secs,
        lap: snapshot.relative_meta.session_kind.is_race().then(|| lap_text(&snapshot.relative_meta)),
        holding: snapshot.relative_meta.clock_is_holding(),
        spectating: snapshot.relative_meta.spectating.clone(),
        team_mate: snapshot.relative_meta.team_mate.clone(),
        grid: snapshot.relative_meta.grid,
    };

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        // One card, not two. Two cards meant two drop shadows, and the right
        // one fell across the left one's edge as a dark seam down the middle
        // of what is meant to read as a single table. The timing half is
        // instead a lighter fill painted inside this one card.
        let card =
            card_frame(metrics, STANDINGS_BG, margin(metrics, 0.0, 0.0), table_card_rounding(metrics)).show(ui, |ui| {
                ui.vertical(|ui| {
                    draw_columns(
                        ui,
                        metrics,
                        &rows,
                        &tumble,
                        &meta,
                        ColumnsSpec {
                            class_count,
                            show_endurance,
                            show_summary,
                            show_tyres,
                            show_position_change,
                            player_avg_stint_laps: player.and_then(|p| p.avg_stint_laps),
                            show_stint_laps: config.show_stint_laps,
                            show_flags: options.show_flags,
                            name_width: name_width(config),
                            timing_order: timing_order(&config.column_order),
                        },
                    );
                    if show_summary {
                        let (band, _response) = ui.allocate_exact_size(
                            egui::vec2(ui.min_rect().width(), metrics.px(SUMMARY_HEIGHT)),
                            egui::Sense::hover(),
                        );
                        draw_strategy_line(ui, metrics, band, endurance);
                    }
                });
            });

        ui.add_space(metrics.px(GUTTER_GAP));
        column(
            ui,
            ColumnSpec {
                metrics,
                width: GUTTER_WIDTH,
                fill: None,
                side: Side::Standalone,
                // Outside the card entirely, so nothing here is a corner.
                at_card_bottom: true,
                grooved: false,
            },
            &rows,
            &tumble,
            |_, _, _| {},
            |ui, metrics, rect, row, style| draw_gutter_row(ui, metrics, rect, row, style, options.show_off_tracks),
        );

        // The class rule spans the whole table, so it's painted last, over
        // every band.
        paint_class_rules(ui, metrics, &rows, card.response.rect, show_endurance, show_tyres, name_width(config));
    });
}

/// The knobs `draw_columns` needs beyond the row plan itself.
#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each mirrors an independent config switch; a state machine would misdescribe them"
)]
struct ColumnsSpec {
    class_count: usize,
    show_endurance: bool,
    /// Whether the strategy line runs across the card's foot, which is what
    /// takes the bottom corners off every band — see [`draw`].
    show_summary: bool,
    /// Whether the timing band carries the tyre-compound column; already
    /// gated on the sim publishing compounds — see [`draw`].
    show_tyres: bool,
    /// Whether each row shows its race-change marker; already gated on the
    /// figure existing, which it only does in races — see [`draw`].
    show_position_change: bool,
    /// The player's own average stint, which a row with no completed stint
    /// of its own is measured against — hatched, to say it is a guess.
    player_avg_stint_laps: Option<i32>,
    /// Whether the stint's lap count is printed beside its bar.
    show_stint_laps: bool,
    /// Whether each driver's flag is drawn before their name.
    show_flags: bool,
    /// The driver band's width, already clamped — see [`name_width`].
    name_width: f32,
    timing_order: [crate::config::StandingsColumn; 3],
}

/// Lays the table's columns side by side from one shared row plan.
///
/// Split out of [`draw`] only for length; it is one unit of work, and the
/// three calls have to stay together because they walk the same plan and
/// depend on the same `at_card_bottom`.
fn draw_columns(
    ui: &mut Ui,
    metrics: Metrics,
    rows: &[Row<'_>],
    tumble: &HashMap<i32, f32>,
    meta: &TopBarMeta,
    spec: ColumnsSpec,
) {
    let ColumnsSpec {
        class_count,
        show_endurance,
        show_summary,
        show_tyres,
        show_position_change,
        player_avg_stint_laps,
        show_stint_laps,
        show_flags,
        name_width,
        timing_order,
    } = spec;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // With the strategy line across the foot of the card, no band reaches
        // the card's bottom edge any more.
        let at_card_bottom = !show_summary;
        column(
            ui,
            ColumnSpec { metrics, width: name_width, fill: None, side: Side::Left, at_card_bottom, grooved: true },
            rows,
            tumble,
            |ui, metrics, rect| draw_left_top_bar(ui, metrics, rect, meta),
            |ui, metrics, rect, row, style| {
                draw_left_row(ui, metrics, rect, row, style, class_count, show_flags, show_position_change);
            },
        );
        let timing_side = if show_endurance { Side::Standalone } else { Side::Right };
        column(
            ui,
            ColumnSpec {
                metrics,
                width: timing_width(show_tyres),
                fill: Some(STANDINGS_TILE_BG),
                side: timing_side,
                at_card_bottom,
                grooved: true,
            },
            rows,
            tumble,
            |ui, metrics, rect| draw_right_top_bar(ui, metrics, rect, show_tyres, timing_order),
            |ui, metrics, rect, row, style| draw_right_row(ui, metrics, rect, row, style, show_tyres, timing_order),
        );
        if show_endurance {
            column(
                ui,
                ColumnSpec {
                    metrics,
                    width: ENDURANCE_WIDTH,
                    fill: Some(STANDINGS_STRATEGY_BG),
                    side: Side::Right,
                    at_card_bottom,
                    grooved: true,
                },
                rows,
                tumble,
                draw_strategy_top_bar,
                |ui, metrics, rect, row, style| {
                    draw_strategy_row(ui, metrics, rect, row, style, player_avg_stint_laps, show_stint_laps);
                },
            );
        }
    });
}

fn placeholder(ui: &mut Ui, metrics: Metrics, message: &str) {
    card_frame(metrics, STANDINGS_BG, margin(metrics, 16.0, 14.0), table_card_rounding(metrics)).show(ui, |ui| {
        ui.label(RichText::new(message).size(metrics.px(COLUMN_LABEL_SIZE)).color(text_secondary()));
    });
}

/// Builds the row plan: the player's own class opened out into a window
/// around the player, and — when `show_other_classes` — every other class's
/// leader under its own banner.
///
/// With the other classes off and no class yet known for the player, every
/// class is "other" and hiding them all would leave the panel empty; the
/// leaders-of-every-class view is shown instead, which is what the panel
/// showed before the player was classified anyway.
fn plan_rows<'a>(
    snapshot: &'a TelemetrySnapshot,
    my_class_id: i32,
    my_class_position: i32,
    show_other_classes: bool,
) -> Vec<Row<'a>> {
    let mut by_class: HashMap<i32, Vec<&'a StandingsEntry>> = HashMap::new();
    for entry in &snapshot.standings {
        by_class.entry(entry.car_class_id).or_default().push(entry);
    }
    let only_my_class = !show_other_classes && my_class_id != NO_CLASS;

    let mut rows = Vec::new();
    for section in &snapshot.class_sections {
        if only_my_class && section.car_class_id != my_class_id {
            continue;
        }
        let Some(entries) = by_class.get(&section.car_class_id) else {
            continue;
        };
        let by_position: HashMap<i32, &'a StandingsEntry> = entries.iter().map(|e| (e.class_position, *e)).collect();
        rows.push(Row::ClassHeader(section));

        // Field size comes from the highest class position present, not from
        // how many entries happen to have arrived: iRacing publishes
        // `ResultsPositions` incrementally, and counting rows would shrink
        // the window — hiding the player — whenever the middle of a class is
        // still missing.
        let field_size = entries.iter().map(|e| e.class_position).max().unwrap_or(0);
        if section.car_class_id == my_class_id {
            for slot in windowed_class_positions(
                field_size,
                my_class_position,
                TOP_N,
                WINDOW_BEFORE,
                WINDOW_AFTER,
                MIN_CLASS_ROWS,
            ) {
                match slot {
                    Some(position) => rows.extend(by_position.get(&position).map(|e| Row::Driver(e))),
                    None => rows.push(Row::Skip),
                }
            }
        } else {
            // Another class only needs enough to say who's winning it.
            for position in 1..=OTHER_CLASS_TOP_N.min(field_size) {
                rows.extend(by_position.get(&position).map(|e| Row::Driver(e)));
            }
        }
    }
    rows
}

/// Lays out one column: a top bar, then every row at its planned height.
///
/// `top_bar` and `row` paint into rectangles this function computes, so all
/// three columns walk the same plan and land on identical y positions.
fn column(
    ui: &mut Ui,
    spec: ColumnSpec,
    rows: &[Row<'_>],
    offsets: &HashMap<i32, f32>,
    top_bar: impl FnOnce(&Ui, Metrics, Rect),
    mut row: impl FnMut(&mut Ui, Metrics, Rect, &Row<'_>, RowStyle),
) {
    let ColumnSpec { metrics, width, fill, side, at_card_bottom, grooved } = spec;
    let height = metrics.px(TOP_BAR_HEIGHT) + rows.iter().map(|r| metrics.px(r.height())).sum::<f32>();
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(metrics.px(width), height), egui::Sense::hover());

    // The foot of this column: the card's own corner where the column reaches
    // it, and square where it stops short — which is every band once the
    // strategy line takes the card's bottom edge for itself.
    let foot = || -> Rounding {
        let card_r = metrics.px(TABLE_CARD_ROUNDING);
        match (at_card_bottom, side) {
            (true, Side::Left) => Rounding { sw: card_r, ..Rounding::ZERO },
            (true, Side::Right) => Rounding { se: card_r, ..Rounding::ZERO },
            (true, Side::Standalone) => Rounding { sw: card_r, se: card_r, ..Rounding::ZERO },
            (false, _) => Rounding::ZERO,
        }
    };

    // A column's own fill goes down before anything else, rounded on its
    // outer side so it follows the enclosing card's corners.
    // Under Instrument the bands are hollowed out and separated by their own
    // outline instead: the card's border is already doing the separating, and
    // a filled surface inside a bordered one reads as a box in a box.
    let fill = fill.map(theme::tile_fill).filter(|fill| *fill != egui::Color32::TRANSPARENT);
    if let Some(fill) = fill {
        let top = match side {
            Side::Right if at_card_bottom => {
                Rounding { ne: metrics.px(TABLE_CARD_ROUNDING), se: metrics.px(TABLE_CARD_ROUNDING), ..Rounding::ZERO }
            }
            Side::Right => Rounding { ne: metrics.px(TABLE_CARD_ROUNDING), ..Rounding::ZERO },
            Side::Left | Side::Standalone => Rounding::ZERO,
        };
        let bottom = foot();
        ui.painter().rect_filled(rect, Rounding { sw: bottom.sw, se: bottom.se, ..top }, fill);
    }
    let bottom_rounding = |is_last: bool| -> Rounding { if is_last { foot() } else { Rounding::ZERO } };

    let bar = Rect::from_min_size(rect.min, egui::vec2(rect.width(), metrics.px(TOP_BAR_HEIGHT)));
    top_bar(ui, metrics, bar);

    let mut y = bar.bottom();
    // A tumbling row paints inside the row area only, so a slide never runs
    // it across the top bar or out of the card's foot mid-flight.
    let saved_clip = ui.clip_rect();
    let row_area = Rect::from_min_max(
        egui::pos2(saved_clip.min.x, bar.bottom().max(saved_clip.min.y)),
        egui::pos2(saved_clip.max.x, rect.bottom().min(saved_clip.max.y)),
    );
    // Striping counts driver rows only, so a class banner in the middle of
    // the table doesn't flip the rhythm of the rows below it.
    let mut driver_ordinal = 0_usize;
    for (index, entry) in rows.iter().enumerate() {
        let row_rect =
            Rect::from_min_size(egui::pos2(rect.left(), y), egui::vec2(rect.width(), metrics.px(entry.height())));
        let odd = driver_ordinal % 2 == 1;
        if matches!(entry, Row::Driver(_)) {
            driver_ordinal += 1;
        }
        let style = RowStyle { odd, rounding: bottom_rounding(index + 1 == rows.len()) };
        // The tumble: a driver row mid-slide paints at its animated offset,
        // while the y walk — and the grooves below — stay on the slot grid.
        let offset = match entry {
            Row::Driver(driver) => offsets.get(&driver.car_idx).copied().unwrap_or(0.0),
            Row::ClassHeader(_) | Row::Skip => 0.0,
        };
        if offset == 0.0 {
            row(ui, metrics, row_rect, entry, style);
        } else {
            ui.set_clip_rect(row_area);
            row(ui, metrics, row_rect.translate(egui::vec2(0.0, offset)), entry, style);
            ui.set_clip_rect(saved_clip);
        }
        // After the row, so its own fill doesn't paint over the divider. Only
        // between two drivers: a class banner is already a break, and putting
        // a bevel under one would read as a second, weaker one.
        let follows_driver = index > 0 && matches!(rows[index - 1], Row::Driver(_));
        if grooved && follows_driver && matches!(entry, Row::Driver(_)) {
            super::paint_row_groove(ui, row_rect.top(), row_rect.left(), row_rect.right());
        }
        // Drawn here rather than in each card's own row function so all three
        // cards break on the same line — see [`SKIP_RULE_INSET`].
        if matches!(entry, Row::Skip) {
            super::dashed_line_h(
                ui,
                row_rect.center().y,
                row_rect.left() + metrics.px(SKIP_RULE_INSET),
                row_rect.right() - metrics.px(SKIP_RULE_INSET),
                metrics.px(SKIP_RULE_DASH),
                metrics.px(SKIP_RULE_GAP),
                egui::Stroke::new(metrics.px(1.0), super::hairline()),
            );
        }
        y = row_rect.bottom();
    }
}

/// The left card's top bar: session type, race clock, and total car count.
fn draw_left_top_bar(ui: &Ui, metrics: Metrics, rect: Rect, meta: &TopBarMeta) {
    let inner = rect.shrink2(metrics.vec2(14.0, 0.0));
    let middle = inner.center().y;

    let letter = meta.session_letter();
    let badge = Rect::from_center_size(egui::pos2(inner.left() + metrics.px(11.0), middle), metrics.vec2(22.0, 22.0));
    if theme::is_instrument() {
        icons::circled_text(ui, badge, &letter, text_secondary(), metrics.px(11.0));
    } else {
        ui.painter().rect_filled(badge, metrics.px(6.0), Color32::from_white_alpha(10));
        paint_text(
            ui,
            badge.center(),
            egui::Align2::CENTER_CENTER,
            RichText::new(letter).size(metrics.px(11.0)).strong().color(text_secondary()),
        );
    }

    // A held clock reads quieter than a running one, so a driver can tell at
    // a glance whether the race has started without reading the number twice.
    let clock = RichText::new(meta.clock_text())
        .monospace()
        .size(metrics.px(CLOCK_SIZE))
        .strong()
        .color(if meta.holding { text_secondary() } else { text_primary() });
    let clock_width = text_width(ui, clock.clone());
    let clock_left = badge.right() + metrics.px(10.0);
    paint_text(ui, egui::pos2(clock_left, middle), egui::Align2::LEFT_CENTER, clock);
    let mut notice_left = clock_left + clock_width + metrics.px(8.0);

    // Everything after the clock is optional and painted most-important
    // last, so each run first has to fit before the car count on the right:
    // a narrowed band drops the session length and lap rather than running
    // them under the count.
    let boundary = inner.right() - metrics.px(TOP_BAR_COUNT_RESERVED);
    let mut fit = |ui: &Ui, text: RichText| -> bool {
        let width = text_width(ui, text.clone());
        if notice_left + width > boundary {
            return false;
        }
        paint_text(ui, egui::pos2(notice_left, middle), egui::Align2::LEFT_CENTER, text);
        notice_left += width + metrics.px(12.0);
        true
    };

    // The session's length after the clock, and the lap after that — both
    // already on the Relative's footer, and absent from the panel that most
    // needs them. Quiet: the clock is what changes.
    if let Some(length) = meta.session_length_secs {
        fit(
            ui,
            RichText::new(format!("/ {}", format_session_length(length)))
                .monospace()
                .size(metrics.px(CLOCK_SIZE))
                .color(text_tertiary()),
        );
    }
    if let Some(lap) = &meta.lap {
        fit(ui, RichText::new(format!("L{lap}")).monospace().size(metrics.px(SPECTATING_SIZE)).color(text_secondary()));
    }

    // Both notices below sit after those, in the run of space before the car
    // count, which is empty in every session — one after the other when both
    // apply.

    // The grid, while the field is forming up: how many are out, and how long
    // there is. The held clock beside it is the race length, which is still
    // worth a glance; this is what changes.
    if let Some(grid) = meta.grid {
        fit(
            ui,
            RichText::new(format!("GRID {}", super::grid_text(grid)))
                .size(metrics.px(SPECTATING_SIZE))
                .strong()
                .color(theme::caution()),
        );
    }

    // Whose race this is, whenever it is not the player's own; see
    // `TopBarMeta::spectating`.
    if let Some(driver) = &meta.spectating {
        fit(ui, RichText::new(format!("\u{25C9} {driver}")).size(metrics.px(SPECTATING_SIZE)).strong().color(ACCENT));
    }
    // Who is driving the player's own car when it is not the player; see
    // `RelativeMeta::team_mate`. Never shown alongside the spectating notice,
    // since a car cannot be both somebody else's and the player's own.
    if let Some(driver) = &meta.team_mate {
        fit(ui, RichText::new(format!("\u{21C4} {driver}")).size(metrics.px(SPECTATING_SIZE)).strong().color(ACCENT));
    }

    paint_text(
        ui,
        egui::pos2(inner.right(), middle),
        egui::Align2::RIGHT_CENTER,
        RichText::new(meta.car_count.to_string()).size(metrics.px(CLOCK_SIZE)).strong().color(text_primary()),
    );
    let car_icon =
        Rect::from_center_size(egui::pos2(inner.right() - metrics.px(34.0), middle), metrics.vec2(20.0, 20.0));
    if !icons::svg(ui, car_icon, "car", text_secondary()) {
        // The painter-drawn car stands in when the asset folder is missing.
        icons::car(ui, car_icon, text_secondary());
    }
}

/// The right card's top bar: the timing column headings.
fn draw_right_top_bar(
    ui: &Ui,
    metrics: Metrics,
    rect: Rect,
    show_tyres: bool,
    order: [crate::config::StandingsColumn; 3],
) {
    let middle = rect.center().y;
    for (label, x) in timing_columns(metrics, rect, order) {
        paint_text(
            ui,
            egui::pos2(x, middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(label).size(metrics.px(COLUMN_LABEL_SIZE)).color(text_secondary()),
        );
    }
    if show_tyres {
        paint_text(
            ui,
            egui::pos2(rect.right() - metrics.px(TYRE_WIDTH / 2.0), middle),
            egui::Align2::CENTER_CENTER,
            RichText::new("Tyre").size(metrics.px(COLUMN_LABEL_SIZE)).color(text_secondary()),
        );
    }
}

/// The timing card's three column headings and their left edges.
fn timing_order(configured: &[crate::config::StandingsColumn]) -> [crate::config::StandingsColumn; 3] {
    let mut result = Vec::new();
    for column in configured.iter().copied().chain(crate::config::StandingsColumn::ALL) {
        if !result.contains(&column) {
            result.push(column);
        }
    }
    [result[0], result[1], result[2]]
}
fn timing_columns(
    metrics: Metrics,
    rect: Rect,
    order: [crate::config::StandingsColumn; 3],
) -> [(&'static str, f32); 3] {
    use crate::config::StandingsColumn as C;
    let mut result = [("Gap", 0.0), ("Fastest", 0.0), ("Last", 0.0)];
    let mut x = rect.left() + metrics.px(12.0);
    for column in order {
        let (index, width) = match column {
            C::Gap => (0, 56.0),
            C::Fastest => (1, 102.0),
            C::Last => (2, 102.0),
        };
        result[index].1 = x;
        x += metrics.px(width);
    }
    result
}

/// One row of the left card: class banner, driver, or a skip break.
#[expect(clippy::too_many_arguments, reason = "called from one place; a struct would only rename the list")]
fn draw_left_row(
    ui: &mut Ui,
    metrics: Metrics,
    rect: Rect,
    row: &Row<'_>,
    style: RowStyle,
    class_count: usize,
    show_flags: bool,
    show_position_change: bool,
) {
    match row {
        Row::ClassHeader(section) => draw_class_header(ui, metrics, rect, section, class_count),
        Row::Driver(entry) => {
            draw_driver_row(ui, metrics, rect, entry, style, class_count, show_flags, show_position_change);
        }
        Row::Skip => {}
    }
}

/// A class banner: a tinted class label, quiet car count, and field strength.
fn draw_class_header(ui: &Ui, metrics: Metrics, rect: Rect, section: &ClassSection, class_count: usize) {
    // One class running means the tag has nothing to tell apart, so it is
    // paper rather than a colour that would only be decoration.
    let color =
        if class_count > 1 { class_accent(&section.color, class_count) } else { Color32::from_white_alpha(200) };
    let inner = rect.shrink2(metrics.vec2(0.0, 4.0));
    let middle = inner.center().y;
    let rounding = metrics.px(BLOCK_ROUNDING);

    // Class color stays local to its label, leaving timing data to carry the
    // contrast in the table below.
    let name = if section.short_name.is_empty() { "?" } else { &*section.short_name };
    let name_rect = Rect::from_min_max(
        egui::pos2(inner.left() + metrics.px(CLASS_TAG_INSET), inner.top()),
        egui::pos2(inner.left() + metrics.px(CLASS_TAG_INSET + CLASS_TAG_WIDTH), inner.bottom()),
    );
    ui.painter().rect_filled(name_rect, rounding, tint(color, 24));
    paint_text(
        ui,
        name_rect.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new(name).size(metrics.px(TAG_SIZE)).strong().color(color),
    );

    // The count shares the class label's baseline without another badge.
    let count_rect = Rect::from_min_max(
        egui::pos2(name_rect.right() + metrics.px(CLASS_TAG_GAP), inner.top()),
        egui::pos2(name_rect.right() + metrics.px(CLASS_TAG_GAP + COUNT_TAG_WIDTH), inner.bottom()),
    );
    // The count is supporting text, with no second badge.
    paint_text(
        ui,
        count_rect.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new(section.car_count.to_string()).size(metrics.px(TAG_SIZE)).color(text_secondary()),
    );

    if let Some(sof) = section.sof {
        paint_text(
            ui,
            egui::pos2(rect.right() - metrics.px(14.0), middle),
            egui::Align2::RIGHT_CENTER,
            RichText::new(format!("SOF {sof}")).size(metrics.px(TAG_SIZE)).color(text_secondary()),
        );
    }
}

/// One driver's row in the left card.
#[expect(clippy::too_many_arguments, reason = "called from one place; a struct would only rename the list")]
fn draw_driver_row(
    ui: &mut Ui,
    metrics: Metrics,
    rect: Rect,
    entry: &StandingsEntry,
    style: RowStyle,
    class_count: usize,
    show_flags: bool,
    show_position_change: bool,
) {
    let dimmed = is_dimmed(entry);
    let color = class_accent(&entry.car_class_color, class_count);
    let plate_width = metrics.px(POSITION_PLATE_END);
    let flag_slot = if show_flags { flag_slot() } else { 0.0 };
    // The race-change slot pushes everything between the plate and the name
    // right by its width; the name's room absorbs it.
    let change_slot = if show_position_change { CHANGE_SLOT } else { 0.0 };

    if entry.is_focus {
        ui.painter().rect_filled(rect, style.rounding, PLAYER_ROW_FILL);
    } else if let Some(stripe) = row_stripe(style.odd) {
        ui.painter().rect_filled(rect, style.rounding, stripe);
    }
    // Over the row's own fill, under everything that follows. Near-white on
    // the player's own row, with the number in near-black on top, and no
    // shadow off its trailing edge — see the same call in `ui::relative` for
    // why both.
    // The class rides the plate's trailing edge, tucked against it, rather
    // than standing as a bar in the column beside — where it collided with
    // the position-change tick and read as one more floating mark. Only with
    // a second class to tell this one from: a single-class field has nothing
    // to say here.
    let plate_fill = if entry.is_focus { PLAYER_PLATE } else { POSITION_PLATE };
    let edge = (class_count > 1).then(|| (metrics.px(CLASS_EDGE), color));
    super::paint_position_plate(ui, rect, plate_width, style.rounding, plate_fill, edge);

    let middle = rect.center().y;
    let text_color = row_text_color(entry, dimmed);

    // The position, in the readout face: the one thing on a row read at a
    // glance rather than studied, set like a pit board's slot.
    let position_color = if entry.is_focus {
        Color32::from_black_alpha(230)
    } else if dimmed {
        text_tertiary()
    } else {
        text_primary()
    };
    draw_position_tumbler(ui, metrics, rect, entry, plate_width, position_color);

    // The race-change marker, in its slot beside the plate: how the race has
    // treated this car so far. A dash for level, so the column reads as a
    // column rather than as scattered arrows.
    if show_position_change && let Some(delta) = entry.race_position_change {
        let (text, ink) = match delta {
            d if d > 0 => (format!("\u{25B2}{d}"), theme::signal()),
            d if d < 0 => (format!("\u{25BC}{}", -d), theme::alert()),
            _ => ("\u{2014}".to_owned(), text_tertiary()),
        };
        paint_text(
            ui,
            egui::pos2(rect.left() + metrics.px(POSITION_PLATE_END + CLASS_EDGE + CHANGE_SLOT / 2.0), middle),
            egui::Align2::CENTER_CENTER,
            RichText::new(text).monospace().size(metrics.px(CHANGE_SIZE)).strong().color(ink),
        );
    }

    // The driver's flag, in the column before the name. A driver with none
    // leaves the slot empty rather than pulling their name left, so names
    // stay in one column down the card.
    if show_flags {
        let flag = Rect::from_min_size(
            egui::pos2(rect.left() + metrics.px(NAME_X + change_slot), middle - metrics.px(FLAG_HEIGHT / 2.0)),
            metrics.vec2(FLAG_HEIGHT * flags::ASPECT, FLAG_HEIGHT),
        );
        flags::draw(ui, flag, entry.flair_id, dimmed);
    }

    // Manufacturer mark, hard against the right edge, with the iRating pill
    // to its left — both placed before the name is painted, because on a
    // narrowed band the pill's left edge is where the name must stop.
    // Wide styles (wordmarks, horizontal badges) get a 2:1 slot that grows
    // leftward, so the mark's right edge stays where the mockup has it and
    // the pill moves with it — see `logos::wide_slots`.
    let logo_width = if logos::wide_slots() { LOGO_SIZE * 2.0 } else { LOGO_SIZE };
    let logo_right = rect.right() - metrics.px(24.0 - LOGO_SIZE / 2.0);
    let logo = Rect::from_min_max(
        egui::pos2(logo_right - metrics.px(logo_width), middle - metrics.px(LOGO_SIZE / 2.0)),
        egui::pos2(logo_right, middle + metrics.px(LOGO_SIZE / 2.0)),
    );
    logos::draw(ui, logo, &entry.car_screen_name, metrics.px(12.0), dimmed);

    let pill = Rect::from_min_max(
        egui::pos2(logo.left() - metrics.px(PILL_WIDTH + 10.0), middle - metrics.px(PILL_HALF_HEIGHT)),
        egui::pos2(logo.left() - metrics.px(10.0), middle + metrics.px(PILL_HALF_HEIGHT)),
    );

    let name_x = rect.left() + metrics.px(NAME_X + change_slot + flag_slot);
    let name_room = (pill.left() - metrics.px(10.0) - name_x).max(0.0);
    let name = super::elide_to_width(ui, &entry.driver_name, name_room, |text| {
        let label = RichText::new(text).size(metrics.px(NAME_SIZE)).color(text_color);
        if entry.is_focus { label.strong() } else { label }
    });
    paint_text(ui, egui::pos2(name_x, middle), egui::Align2::LEFT_CENTER, name);
    ui.painter().rect_filled(pill, metrics.px(6.0), Color32::from_white_alpha(if dimmed { 3 } else { 7 }));
    paint_text(
        ui,
        pill.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new(format_irating(entry.irating)).monospace().size(metrics.px(RATING_SIZE)).color(if dimmed {
            text_tertiary()
        } else {
            text_secondary()
        }),
    );

    // Keep strategy detail available without adding another visible column.
    stint_tooltip(ui, rect, entry);
}

/// How far the name moves right to make room for a flag: the flag's width
/// at 4:3 plus the gap after it, in unscaled pixels.
fn flag_slot() -> f32 {
    FLAG_HEIGHT * flags::ASPECT + FLAG_GAP
}

/// The position number in its plate, rolled like an odometer when it changes.
///
/// The roll passes through every intermediate value — a three-place gain
/// reads 8, 7, 6, 5 — clipped to the plate so a passing digit never leaks
/// into the row, and tinted toward gain-green or loss-red while turning so
/// the direction reads before the number has settled. Jumps wider than
/// [`MAX_POSITION_ROLL`] snap; see that constant.
fn draw_position_tumbler(
    ui: &mut Ui,
    metrics: Metrics,
    rect: Rect,
    entry: &StandingsEntry,
    plate_width: f32,
    color: Color32,
) {
    let centre = egui::pos2(rect.left() + plate_width / 2.0, rect.center().y);
    #[expect(clippy::cast_precision_loss, reason = "positions are far inside f32's exact-integer range")]
    let target = entry.class_position as f32;
    let id = egui::Id::new(("standings-position", entry.car_idx));
    let mut rolled = ui.ctx().animate_value_with_time(id, target, POSITION_TUMBLE_SECS);
    if (rolled - target).abs() > MAX_POSITION_ROLL {
        // Snap: a zero-time animation lands the value at once.
        rolled = ui.ctx().animate_value_with_time(id, target, 0.0);
    }
    // Settled — or near enough that a sub-pixel slide would only blur the
    // glyph — so the number is set plainly, in the row's own colour.
    if (rolled - target).abs() < 0.01 {
        paint_text(
            ui,
            centre,
            egui::Align2::CENTER_CENTER,
            readout(entry.class_position.to_string(), metrics.px(POSITION_SIZE)).color(color),
        );
        return;
    }

    // Mid-roll: the whole values either side of the animated one slide
    // through the plate, one leaving as the next arrives. Rolling *down*
    // through the numbers moves the digits downward, the way a gained place
    // turns an odometer.
    let gaining = target < rolled;
    let ink = mix(color, if gaining { theme::signal() } else { theme::alert() }, (rolled - target).abs().min(1.0));
    let step = metrics.px(POSITION_SIZE * 1.15);
    let saved_clip = ui.clip_rect();
    let plate = Rect::from_min_max(rect.min, egui::pos2(rect.left() + plate_width, rect.bottom()));
    ui.set_clip_rect(plate.intersect(saved_clip));
    #[expect(clippy::cast_possible_truncation, reason = "rolled sits between two small positions")]
    let lower = rolled.floor() as i32;
    for value in [lower, lower + 1] {
        #[expect(clippy::cast_precision_loss, reason = "positions are far inside f32's exact-integer range")]
        let offset = (value as f32 - rolled) * step;
        paint_text(
            ui,
            egui::pos2(centre.x, centre.y + offset),
            egui::Align2::CENTER_CENTER,
            readout(value.to_string(), metrics.px(POSITION_SIZE)).color(ink),
        );
    }
    ui.set_clip_rect(saved_clip);
}

/// A straight per-channel mix of two colours, `t` of the way from `a` to
/// `b`. Both are premultiplied, so mixing channel by channel is sound.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| -> u8 {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a lerp of u8s stays in 0..=255")]
        let mixed = (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        mixed
    };
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// One row of the timing card.
fn draw_right_row(
    ui: &mut Ui,
    metrics: Metrics,
    rect: Rect,
    row: &Row<'_>,
    style: RowStyle,
    show_tyres: bool,
    order: [crate::config::StandingsColumn; 3],
) {
    let Row::Driver(entry) = row else { return };
    let dimmed = is_dimmed(entry);
    let middle = rect.center().y;

    if entry.is_focus {
        ui.painter().rect_filled(rect, style.rounding, PLAYER_ROW_FILL);
    } else if let Some(stripe) = row_stripe(style.odd) {
        ui.painter().rect_filled(rect, style.rounding, stripe);
    }

    let columns = timing_columns(metrics, rect, order);
    let text_color = if entry.is_focus {
        Color32::WHITE
    } else if dimmed {
        text_tertiary()
    } else {
        text_primary()
    };

    // A car on the hook has no meaningful gap, so the column shows how long
    // it has been gone instead — ticking up, in the caution colour its
    // gutter chip carries.
    if let Some(tow) = entry.tow_secs {
        paint_text(
            ui,
            egui::pos2(columns[0].1, middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(format_minutes_seconds(tow))
                .monospace()
                .size(metrics.px(TIME_SIZE))
                .strong()
                .color(theme::caution()),
        );
    } else {
        paint_text(
            ui,
            egui::pos2(columns[0].1, middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(gap_text(entry)).monospace().size(metrics.px(TIME_SIZE)).color(text_color),
        );
    }

    // The fastest lap receives a soft inset tint. Its semantic accent is
    // repeated by the stopwatch in the gutter.
    let lap_color = if entry.is_focus || dimmed { text_color } else { text_secondary() };
    let fastest_text = format_lap_time(entry.best_lap_secs);
    if entry.is_class_fastest {
        let cell = Rect::from_min_max(
            egui::pos2(columns[1].1 - metrics.px(8.0), rect.top() + metrics.px(3.0)),
            egui::pos2(columns[1].1 + metrics.px(98.0), rect.bottom() - metrics.px(3.0)),
        );
        ui.painter().rect_filled(cell, metrics.px(6.0), theme::fastest_cell());
    }
    paint_text(
        ui,
        egui::pos2(columns[1].1, middle),
        egui::Align2::LEFT_CENTER,
        RichText::new(fastest_text).monospace().size(metrics.px(TIME_SIZE)).color(if entry.is_class_fastest {
            theme::fastest_text()
        } else {
            lap_color
        }),
    );

    paint_text(
        ui,
        egui::pos2(columns[2].1, middle),
        egui::Align2::LEFT_CENTER,
        RichText::new(format_lap_time(entry.last_lap_secs)).monospace().size(metrics.px(TIME_SIZE)).color(lap_color),
    );

    // The compound circle: the letter ringed in the panel's water blue for a
    // wet, faint white for a dry — so a field split across compounds reads
    // as a pattern of rings before any letter is. A car the sim reports no
    // compound for leaves the slot empty rather than guessing.
    if show_tyres && let Some(tyre) = entry.tyre {
        let centre = egui::pos2(rect.right() - metrics.px(TYRE_WIDTH / 2.0), middle);
        let ring = if tyre.wet { WIND } else { Color32::from_white_alpha(70) };
        let ring = if dimmed { tint(ring, 110) } else { ring };
        ui.painter().circle_stroke(centre, metrics.px(TYRE_RADIUS), egui::Stroke::new(metrics.px(1.5), ring));
        paint_text(
            ui,
            centre,
            egui::Align2::CENTER_CENTER,
            RichText::new(tyre.letter.to_string()).size(metrics.px(TYRE_LETTER_SIZE)).strong().color(text_color),
        );
    }
}

/// The strategy band's column headings and their left edges.
fn strategy_columns(metrics: Metrics, rect: Rect) -> [(&'static str, f32); 3] {
    let left = rect.left();
    [("Stint", left + metrics.px(STINT_X)), ("Stops", left + metrics.px(STOPS_X)), ("Net", left + metrics.px(NET_X))]
}

fn draw_strategy_top_bar(ui: &Ui, metrics: Metrics, rect: Rect) {
    let middle = rect.center().y;
    for (label, x) in strategy_columns(metrics, rect) {
        paint_text(
            ui,
            egui::pos2(x, middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(label).size(metrics.px(COLUMN_LABEL_SIZE)).color(text_secondary()),
        );
    }
}

/// One row of the strategy band: how far into its stint this car is, how
/// many stops it has completed, and where its strategy is projected to leave it.
///
/// Three columns that each vary row to row. The old strip printed laps,
/// stops and measured pit time as figures, and in a steady race every row
/// read the same — a column of identical values has told the driver nothing
/// per row. A bar, a pattern of dots and a signed delta are read as shapes.
fn draw_strategy_row(
    ui: &mut Ui,
    metrics: Metrics,
    rect: Rect,
    row: &Row<'_>,
    style: RowStyle,
    player_avg_stint_laps: Option<i32>,
    show_stint_laps: bool,
) {
    let Row::Driver(entry) = row else { return };
    let dimmed = is_dimmed(entry);
    let middle = rect.center().y;

    if entry.is_focus {
        ui.painter().rect_filled(rect, style.rounding, PLAYER_ROW_FILL);
    } else if let Some(stripe) = row_stripe(style.odd) {
        ui.painter().rect_filled(rect, style.rounding, stripe);
    }

    let columns = strategy_columns(metrics, rect);
    let ink = if entry.is_focus {
        Color32::WHITE
    } else if dimmed {
        text_tertiary()
    } else {
        text_secondary()
    };

    // The stint bar: laps since this car's last stop over its own average
    // stint, so a bar near full is a car about to pit. Measured against the
    // player's average — hatched, to say so — while it has no stint of its
    // own to judge by; absent when nobody has one yet.
    let bar_width = if show_stint_laps { STINT_BAR_SHORT } else { STINT_BAR_SIZE.0 };
    let bar = Rect::from_min_size(
        egui::pos2(columns[0].1, middle - metrics.px(STINT_BAR_SIZE.1 / 2.0)),
        metrics.vec2(bar_width, STINT_BAR_SIZE.1),
    );
    let trough = if entry.is_focus { Color32::from_black_alpha(60) } else { Color32::from_white_alpha(30) };
    ui.painter().rect_filled(bar, metrics.px(2.0), trough);
    let (measured, against) = match (entry.avg_stint_laps, player_avg_stint_laps) {
        (Some(own), _) => (true, Some(own)),
        (None, Some(player)) => (false, Some(player)),
        (None, None) => (false, None),
    };
    if let Some(average) = against.filter(|laps| *laps > 0) {
        let fraction = stint_fraction(entry.current_stint_laps, average);
        let fill = Rect::from_min_max(bar.min, egui::pos2(bar.left() + bar.width() * fraction, bar.bottom()));
        if measured {
            ui.painter().rect_filled(fill, metrics.px(2.0), ink);
        } else {
            paint_hatch(ui, metrics, fill, ink);
        }
    }
    // The lap count beside the bar, where the option asks for the figure as
    // well as the length.
    if show_stint_laps {
        paint_text(
            ui,
            egui::pos2(bar.right() + metrics.px(STINT_LAPS_GAP), middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(entry.current_stint_laps.to_string())
                .monospace()
                .size(metrics.px(STINT_LAPS_SIZE))
                .color(ink),
        );
    }

    // A completed service is one stop. Remaining-stop forecasts belong in
    // the race summary and must never replace this observed count.
    paint_text(
        ui,
        egui::pos2(columns[1].1, middle),
        egui::Align2::LEFT_CENTER,
        RichText::new(entry.pit_stops.max(0).to_string()).monospace().size(metrics.px(TIME_SIZE)).strong().color(ink),
    );

    // The projected finish as a delta from where the car is now: "does the
    // strategy gain or lose me places?", not "what number will I be?".
    let (net, net_color) = match entry.projected_class_position.map(|p| entry.class_position - p) {
        Some(delta) if delta > 0 => (format!("\u{25B2}{delta}"), theme::signal()),
        Some(delta) if delta < 0 => (format!("\u{25BC}{}", -delta), theme::alert()),
        Some(_) | None => ("\u{2014}".to_owned(), ink),
    };
    paint_text(
        ui,
        egui::pos2(columns[2].1, middle),
        egui::Align2::LEFT_CENTER,
        RichText::new(net).monospace().size(metrics.px(TIME_SIZE)).strong().color(net_color),
    );
}

/// How far through an average stint of `average` laps a car `laps` in is,
/// clamped to the bar.
fn stint_fraction(laps: i32, average: i32) -> f32 {
    if average <= 0 {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss, reason = "lap counts are far inside f32's exact-integer range")]
    let fraction = laps as f32 / average as f32;
    fraction.clamp(0.0, 1.0)
}

/// The strategy line beneath the table in endurance mode: laps left, stops
/// still owed, the projected finish — and, the figure the whole mode exists
/// for, whether a rival is on a longer strategy.
///
/// The largest type on the panel, because this is its verdict; the old band
/// set it in the panel's smallest.
fn draw_strategy_line(ui: &Ui, metrics: Metrics, rect: Rect, meta: EnduranceMeta) {
    let card = metrics.px(TABLE_CARD_ROUNDING);
    ui.painter().rect_filled(rect, Rounding { sw: card, se: card, ..Rounding::ZERO }, Color32::from_black_alpha(90));
    let middle = rect.center().y;
    let mut x = rect.left() + metrics.px(14.0);

    // A figure in the readout face, with its caption after it on the same
    // line: the number is what is read, the caption what it is.
    let figure = |ui: &Ui, x: &mut f32, value: String, caption: &str| {
        let value_text = readout(value, metrics.px(SUMMARY_VALUE_SIZE)).color(text_primary());
        let value_width = text_width(ui, value_text.clone());
        paint_text(ui, egui::pos2(*x, middle), egui::Align2::LEFT_CENTER, value_text);
        *x += value_width + metrics.px(6.0);

        let caption_text = RichText::new(caption).size(metrics.px(TAG_SIZE)).strong().color(text_tertiary());
        let caption_width = text_width(ui, caption_text.clone());
        paint_text(ui, egui::pos2(*x, middle + metrics.px(1.0)), egui::Align2::LEFT_CENTER, caption_text);
        *x += caption_width + metrics.px(24.0);
    };
    let dash = || "\u{2014}".to_owned();

    figure(ui, &mut x, meta.laps_remaining.map_or_else(dash, |n| n.to_string()), "LAPS LEFT");
    let stops = meta.stops_remaining;
    figure(
        ui,
        &mut x,
        stops.map_or_else(dash, |n| n.to_string()),
        if stops == Some(1) { "STOP TO GO" } else { "STOPS TO GO" },
    );
    figure(ui, &mut x, meta.projected_class_position.map_or_else(dash, |p| format!("P{p}")), "NET");

    // A rival on fewer stops is the one thing worth shouting about, so it is
    // a tag rather than a caption: alert-red when it is so, signal-green
    // when the strategies are level. Multi-stop races only — now that the
    // line runs in every race, a sprint where nobody stops has no strategy
    // to call level.
    if meta.multi_stop_race
        && let (Some(mine), Some(best)) = (meta.stops_remaining, meta.best_stops_in_class)
    {
        let (text, fill) = if best < mine {
            (format!("RIVAL ON {} FEWER", mine - best), theme::alert())
        } else {
            ("STRATEGY LEVEL".to_owned(), theme::signal())
        };
        let label = RichText::new(text).size(metrics.px(TAG_SIZE)).strong().color(Color32::from_black_alpha(230));
        let width = text_width(ui, label.clone()) + metrics.px(24.0);
        let tag = Rect::from_min_max(
            egui::pos2(rect.right() - metrics.px(14.0) - width, middle - metrics.px(SUMMARY_TAG_HEIGHT / 2.0)),
            egui::pos2(rect.right() - metrics.px(14.0), middle + metrics.px(SUMMARY_TAG_HEIGHT / 2.0)),
        );
        ui.painter().rect_filled(tag, metrics.px(BLOCK_ROUNDING), fill);
        paint_text(ui, tag.center(), egui::Align2::CENTER_CENTER, label);
    }
}

/// One slot of the status gutter, outside both cards.
///
/// Rows with nothing to report get nothing drawn: an empty placeholder would
/// only add noise beside the panel, and the gutter is outside the cards so a
/// blank slot costs no alignment.
fn draw_gutter_row(ui: &mut Ui, metrics: Metrics, rect: Rect, row: &Row<'_>, _style: RowStyle, show_off_tracks: bool) {
    let Row::Driver(entry) = row else { return };
    let chip = Rect::from_center_size(rect.center(), metrics.vec2(26.0, 26.0));
    let rounding = metrics.px(6.0);

    // A flag first — see the same order in `ui::relative`.
    if let Some(penalty) = entry.penalty {
        super::paint_penalty_marker(ui, metrics, chip, penalty);
    } else if entry.tow_secs.is_some() {
        // A car on the hook is the row's whole story; even its fastest-lap
        // chip can wait until it is back on the road.
        ui.painter().rect_filled(chip, rounding, theme::caution());
        paint_text(
            ui,
            chip.center(),
            egui::Align2::CENTER_CENTER,
            RichText::new("TOW").monospace().size(metrics.px(9.0)).strong().color(Color32::BLACK),
        );
    } else if matches!(entry.track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits) {
        // Ahead of the fastest-lap chip, for the same reason the tow is: where
        // a car is right now is the transient fact worth the slot, and a
        // class-leading lap it will still hold in a minute used to hide the
        // one stop the panel exists to report.
        ui.painter().rect_filled(chip, rounding, Color32::WHITE);
        paint_text(
            ui,
            chip.center(),
            egui::Align2::CENTER_CENTER,
            RichText::new("PIT").monospace().size(metrics.px(11.0)).strong().color(Color32::BLACK),
        );
    } else if entry.is_class_fastest {
        ui.painter().rect_filled(chip, rounding, theme::fastest());
        icons::stopwatch(ui, chip.shrink(metrics.px(6.0)), Color32::WHITE);
    } else {
        let count = if show_off_tracks { entry.off_tracks } else { 0 };
        super::paint_off_track_marker(ui, metrics, chip, entry.track_location == TrackLocation::OffTrack, count);
    }
}

/// A subdued class hairline connects the label to timing and strategy data.
fn paint_class_rules(
    ui: &Ui,
    metrics: Metrics,
    rows: &[Row<'_>],
    left_rect: Rect,
    show_endurance: bool,
    show_tyres: bool,
    name_width: f32,
) {
    let full_width =
        metrics.px(name_width + timing_width(show_tyres) + if show_endurance { ENDURANCE_WIDTH } else { 0.0 });
    let mut y = left_rect.top() + metrics.px(TOP_BAR_HEIGHT);
    for row in rows {
        let height = metrics.px(row.height());
        if let Row::ClassHeader(section) = row {
            let rule = Rect::from_min_size(
                egui::pos2(left_rect.left() + metrics.px(10.0), y + height - metrics.px(1.0)),
                egui::vec2(full_width - metrics.px(20.0), metrics.px(1.0)),
            );
            ui.painter().rect_filled(rule, 0.0, tint(class_color(&section.color), 65));
        }
        y += height;
    }
}

/// Whether a row is drawn muted: in the pits, or laps down on its class.
fn is_dimmed(entry: &StandingsEntry) -> bool {
    entry.laps_down > 0 || matches!(entry.track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits)
}

fn row_text_color(entry: &StandingsEntry, dimmed: bool) -> Color32 {
    if entry.is_focus {
        Color32::WHITE
    } else if dimmed {
        text_tertiary()
    } else {
        text_primary()
    }
}

/// The gap column: a dash for a class leader, laps for a lapped car, seconds
/// otherwise. A time gap to a car laps down says nothing useful, which is
/// why the lap count replaces it rather than sitting beside it.
fn gap_text(entry: &StandingsEntry) -> String {
    if entry.laps_down > 0 {
        format!("{}L", entry.laps_down)
    } else if entry.class_position <= 1 {
        "-".to_owned()
    } else {
        format!("{:.1}", entry.gap_to_leader_secs)
    }
}

/// Attaches the stint detail to a row as a hover tooltip.
fn stint_tooltip(ui: &mut Ui, rect: Rect, entry: &StandingsEntry) {
    let response = ui.interact(rect, ui.id().with(("stint", entry.car_idx)), egui::Sense::hover());
    if !response.hovered() {
        return;
    }
    let mut text = format!(
        "current stint: {}L / {} \u{b7} {} pit stop(s)",
        entry.current_stint_laps,
        format_minutes_seconds(entry.current_stint_secs),
        entry.pit_stops
    );
    if let Some(avg_laps) = entry.avg_stint_laps {
        let _ = write!(text, " \u{b7} ~{avg_laps}L avg stint");
    }
    if let Some(avg_secs) = entry.avg_stint_secs {
        let _ = write!(text, " (~{})", format_minutes_seconds(avg_secs));
    }
    if let Some(last_pit) = entry.last_pit_secs {
        let _ = write!(text, " \u{b7} last stop {last_pit:.1}s");
    }
    response.on_hover_text(text);
}

/// Session-wide values the left card's top bar shows.
#[derive(Debug, Clone, Default)]
struct TopBarMeta {
    session_kind: SessionKind,
    car_count: i32,
    elapsed_secs: f64,
    /// Time left, counting down — see `RelativeMeta::countdown_secs`, which
    /// this is taken straight from so the two widgets can never disagree
    /// about how much session is left.
    countdown_secs: Option<f64>,
    /// The session's length, shown after the clock; `None` for a session
    /// with no time limit.
    session_length_secs: Option<f64>,
    /// The lap counter, in a race; see `ui::lap_text`.
    lap: Option<String>,
    /// Whether that figure is holding at the session length rather than
    /// running, which is the case on the grid and behind the pace car.
    holding: bool,
    /// Who is being watched, when this widget is following a camera car rather
    /// than the player's own — see `RelativeMeta::spectating`. Standings says
    /// so as well as Relative because its window opens around the focus car
    /// too, and either widget can be on screen without the other.
    ///
    /// `Arc<str>`, like its source, so building this once a frame is a refcount
    /// bump rather than a copy of the name.
    spectating: Option<Arc<str>>,
    /// Who is driving the player's own car when it is not the player — see
    /// `RelativeMeta::team_mate`.
    team_mate: Option<Arc<str>>,
    /// The grid, while a race's field is still forming up — see
    /// `RelativeMeta::grid`.
    grid: Option<GridStatus>,
}

impl TopBarMeta {
    /// The session's initial, shown in the compact session badge.
    fn session_letter(&self) -> String {
        self.session_kind.letter().to_owned()
    }

    /// The session clock, counting down.
    fn clock_text(&self) -> String {
        match self.countdown_secs {
            Some(remain) => format_clock(remain),
            None => format_clock(self.elapsed_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(class_position: i32, gap: f32, laps_down: i32) -> StandingsEntry {
        StandingsEntry {
            position: class_position,
            class_position,
            car_idx: 0,
            driver_name: Arc::from("Test Driver"),
            car_screen_name: Arc::from("Audi R8 LMS GT3"),
            irating: 2500,
            flair_id: 0,
            car_class_id: 0,
            car_class_short_name: Arc::from("GT3"),
            car_class_color: Arc::from("0xE81E5B"),
            best_lap_secs: 98.0,
            last_lap_secs: 99.0,
            gap_to_leader_secs: gap,
            pit_stops: 0,
            current_stint_laps: 3,
            current_stint_secs: 300.0,
            avg_stint_laps: None,
            avg_stint_secs: None,
            track_location: TrackLocation::OnTrack,
            is_class_fastest: false,
            laps_down,
            last_pit_secs: None,
            avg_pit_secs: None,
            stops_remaining: None,
            projected_class_position: None,
            is_focus: false,
            off_tracks: 0,
            penalty: None,
            tow_secs: None,
            tyre: None,
            race_position_change: None,
        }
    }

    #[test]
    fn a_completed_stop_increments_the_row_while_the_remaining_forecast_falls() {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx);
        let mut rendered = Vec::new();
        for (completed, remaining) in [(0, 2), (1, 1)] {
            let mut car = entry(1, 0.0, 0);
            car.pit_stops = completed;
            // Keep the per-car forecast deliberately populated: the row must
            // still take its value from the observed completed-stop count.
            car.stops_remaining = Some(remaining);
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_strategy_row(
                        ui,
                        Metrics::new(1.0),
                        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(ENDURANCE_WIDTH, ROW_HEIGHT)),
                        &Row::Driver(&car),
                        RowStyle { odd: false, rounding: Rounding::ZERO },
                        Some(7),
                        false,
                    );
                    draw_strategy_line(
                        ui,
                        Metrics::new(1.0),
                        Rect::from_min_size(egui::pos2(0.0, 50.0), egui::vec2(600.0, SUMMARY_HEIGHT)),
                        EnduranceMeta { stops_remaining: Some(remaining), ..EnduranceMeta::default() },
                    );
                });
            });
            let numbers: Vec<(f32, i32)> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => text.galley.text().parse().ok().map(|value| (text.pos.y, value)),
                    _ => None,
                })
                .collect();
            let row = numbers.iter().find(|(y, _)| *y < 50.0).map(|(_, value)| *value);
            let summary = numbers.iter().find(|(y, _)| *y >= 50.0).map(|(_, value)| *value);
            rendered.push((row, summary));
        }
        assert_eq!(rendered, [(Some(0), Some(2)), (Some(1), Some(1))]);
    }

    #[test]
    fn the_stops_column_renders_completed_stops_even_when_nine_more_are_projected() {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx);
        for completed in [0, 1, 2, 10] {
            let mut car = entry(1, 0.0, 0);
            car.pit_stops = completed;
            car.stops_remaining = Some(9);
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_strategy_row(
                        ui,
                        Metrics::new(1.0),
                        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(ENDURANCE_WIDTH, ROW_HEIGHT)),
                        &Row::Driver(&car),
                        RowStyle { odd: false, rounding: Rounding::ZERO },
                        Some(7),
                        false,
                    );
                });
            });
            let text: Vec<&str> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text()),
                    _ => None,
                })
                .collect();
            assert!(text.contains(&completed.to_string().as_str()), "completed {completed}: {text:?}");
            assert!(!text.contains(&"9"), "the forecast belongs in the summary, not this column");
        }
    }

    #[test]
    fn only_races_show_the_remaining_stop_summary_even_with_endurance_columns_forced_on() {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx);
        let config = StandingsConfig { endurance_mode: EnduranceMode::On, show_tyres: false, ..Default::default() };
        let danger = std::collections::BTreeMap::new();
        let options = super::super::RowOptions { show_off_tracks: false, show_flags: false, danger: &danger, fuel_target: None };
        for kind in [SessionKind::Race, SessionKind::Practice, SessionKind::Qualifying, SessionKind::Warmup, SessionKind::Unknown] {
            let mut snapshot = crate::demo::snapshot();
            snapshot.relative_meta.session_kind = kind;
            // Deliberately retain a stale race forecast: the session kind
            // still has to prevent it being presented as a practice target.
            snapshot.endurance.stops_remaining = Some(9);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1200.0))),
                ..Default::default()
            };
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| draw(ui, Some(&snapshot), &config, options));
            });
            let text: Vec<&str> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            }).collect();
            assert_eq!(text.contains(&"STOPS TO GO"), kind.is_race(), "{kind:?}");
            assert!(text.contains(&"Stops"), "measured stops remain available in {kind:?}");
            if kind.is_race() {
                assert!(text.contains(&"9"), "the race summary retains its remaining-stop estimate");
            }
        }
    }

    #[test]
    fn the_class_leader_has_no_gap_to_show() {
        assert_eq!(gap_text(&entry(1, 0.0, 0)), "-");
    }

    /// A hand-edited band width outside the slider's range — or not a number
    /// at all — must not be laid out as written; see [`name_width`].
    #[test]
    fn a_hand_edited_name_width_is_clamped_before_layout() {
        let mut config = crate::config::StandingsConfig::default();
        assert!((name_width(&config) - DESIGN_NAME_WIDTH).abs() < f32::EPSILON);
        config.name_width = 5000.0;
        assert!((name_width(&config) - *NAME_WIDTH_RANGE.end()).abs() < f32::EPSILON);
        config.name_width = f32::NAN;
        assert!((name_width(&config) - DESIGN_NAME_WIDTH).abs() < f32::EPSILON);
    }

    #[test]
    fn a_gap_on_the_lead_lap_reads_in_seconds() {
        assert_eq!(gap_text(&entry(2, 0.9, 0)), "0.9");
        assert_eq!(gap_text(&entry(9, 32.0, 0)), "32.0");
    }

    /// A car eleven laps down has no meaningful time gap, so the lap count
    /// takes the column instead — the mockup's `11L`.
    #[test]
    fn a_lapped_car_reads_in_laps() {
        assert_eq!(gap_text(&entry(15, 400.0, 11)), "11L");
    }

    #[test]
    fn lapped_and_pitted_rows_are_dimmed() {
        assert!(is_dimmed(&entry(15, 0.0, 11)));
        let mut pitted = entry(11, 44.1, 0);
        pitted.track_location = TrackLocation::InPitStall;
        assert!(is_dimmed(&pitted));
        assert!(!is_dimmed(&entry(2, 0.9, 0)));
    }

    #[test]
    fn the_session_badge_shows_the_first_letter() {
        let meta = TopBarMeta { session_kind: SessionKind::Race, ..TopBarMeta::default() };
        assert_eq!(meta.session_letter(), "R");
        let qualify = TopBarMeta { session_kind: SessionKind::Qualifying, ..TopBarMeta::default() };
        assert_eq!(qualify.session_letter(), "Q");
    }

    /// Before the session-info YAML has been read there's no session type;
    /// the badge must still render rather than vanish.
    #[test]
    fn an_unknown_session_type_still_renders_a_badge() {
        assert_eq!(TopBarMeta::default().session_letter(), "?");
    }

    #[test]
    fn the_top_bar_clock_counts_down() {
        let meta = TopBarMeta { countdown_secs: Some(24.0 * 60.0 + 52.0), ..TopBarMeta::default() };
        assert_eq!(meta.clock_text(), "00:24:52");
    }
}
