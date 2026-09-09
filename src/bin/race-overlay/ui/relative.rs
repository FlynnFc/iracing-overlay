// Rust guideline compliant 2026-02-16

//! Renders the Relative widget, matching `design mocks/Screenshot_12.jpg`.
//!
//! Layout is a status gutter outside the card on the left, then the card
//! itself: a header (field strength, incidents), one row per nearby car, and
//! a footer (race clock, lap progress, track conditions). The gutter sits
//! outside the card so a stopwatch or PIT marker reads as an annotation on
//! the row rather than as another column competing with the driver's name.
//!
//! Every dimension is measured off that image and passed through
//! [`Metrics`]; see the parent module for why scaling works this way.

use egui::{Color32, Pos2, Rect, RichText, Stroke, Ui};
use iracing_telem::flags::TrackLocation;

use super::theme;
use super::{
    ACCENT, Metrics, PLAYER_PLATE, PLAYER_ROW_FILL, POSITION_PLATE, WIND, class_accent, elide_to_width, format_clock,
    format_irating, format_short_lap_time, hairline, icons, lap_text, margin, paint_text, parse_hex_color, readout,
    row_stripe, text_primary, text_secondary, text_tertiary, text_width, tint,
};
use crate::config::RelativeConfig;
use crate::telemetry::snapshot::{CarSnapshot, RelativeMeta, TelemetrySnapshot, WeatherSnapshot};

/// The card width the design mockup measures, and how far a config may pull
/// it either way.
///
/// Most of the design's width is the run the driver's name sits in, so
/// narrowing is really narrowing that run — the fixed columns keep their
/// measured widths and long names gain an ellipsis. The floor still fits a
/// short name beside every column; past the ceiling a 16:9 screen is all
/// panel.
pub const DESIGN_WIDTH: f32 = 690.0;
pub const WIDTH_RANGE: std::ops::RangeInclusive<f32> = 480.0..=900.0;

/// The card's horizontal padding, applied inside the tab rail.
const SIDE_PADDING: f32 = 16.0;

/// The status gutter that sits to the card's left.
const GUTTER_WIDTH: f32 = 40.0;
const GUTTER_GAP: f32 = 8.0;

/// The gutter's full run — its width plus the gap to the card — in unscaled
/// pixels. The black box's status frame widens its left limb into a band
/// across exactly this, so the gutter's markers sit *on* the border rather
/// than the border detouring around them.
#[must_use]
pub fn gutter_span() -> f32 {
    GUTTER_WIDTH + GUTTER_GAP
}

/// The configured card width, defended against a hand-edited file the same
/// way `Metrics::new` defends the scale: a NaN width is a panel that
/// silently fails to lay out, with no clue as to why.
fn card_width(config: &RelativeConfig) -> f32 {
    if config.width.is_finite() { config.width.clamp(*WIDTH_RANGE.start(), *WIDTH_RANGE.end()) } else { DESIGN_WIDTH }
}

/// The card's outer width for this config — content plus side padding.
///
/// The black box's other pages set their width to this so page one, which is
/// on screen all race, never changes size under the rail.
#[must_use]
pub fn outer_width(config: &RelativeConfig) -> f32 {
    card_width(config) + 2.0 * SIDE_PADDING
}

/// Row geometry. Rows touch, separated by a groove rather than by a gap —
/// see `ui::paint_row_groove`.
const ROW_HEIGHT: f32 = 44.0;

/// Type scale.
const NAME_SIZE: f32 = 21.0;
/// The gap to the car on this row, at the end of it.
///
/// A step above the name beside it: it is the number the whole widget exists
/// to deliver, and the one thing on a row read mid-corner.
const GAP_SIZE: f32 = 24.0;
/// The position, in the readout face — see `ui::readout`.
///
/// Sized to fill its plate rather than to sit politely inside it. The
/// position is the first thing read on a row and the plate is 44 points of
/// otherwise empty slate, so the number takes the space.
const POSITION_SIZE: f32 = 30.0;
/// The iRating and its projected change, on the badge at the end of each row.
///
/// Set in the proportional face rather than the mono one every other number in
/// this widget uses, and larger than the row's supporting text. A lap time or
/// a gap is read as a column and wants its digits in fixed columns; this is
/// read as a single value at a glance, and IBM Plex Mono at 14 was too light
/// and too small to be caught that way against a bright chip.
const BADGE_SIZE: f32 = 17.0;
const FOOTER_SIZE: f32 = 15.0;
/// The recent-pace lap time beside each driver's name.
const RECENT_LAP_SIZE: f32 = 15.0;
const HEADER_VALUE_SIZE: f32 = 21.0;

/// How far past the name's left edge a severe driver's red block reaches.
///
/// Measured from wherever the name starts, since the number column before it
/// can be switched off. The class colour used to wash across this same
/// stretch and no longer does — see [`paint_class_bar`].
const WASH_PAST_NAME: f32 = 16.0;

/// The car number's column, between the class slash and the driver's name.
///
/// Fixed width so every name starts on the same line whatever the number's
/// length: three digits and the hash, in the mono face at [`NUMBER_SIZE`],
/// with the same gap after it that the slash leaves before it.
const NUMBER_WIDTH: f32 = 44.0;
const NUMBER_SIZE: f32 = 15.0;

/// Column positions within a row, in pixels from its left edge.
///
/// The class colour, struck down the position plate's trailing edge — see
/// [`super::paint_position_plate`]. It was a wash across the row, then a bar
/// in the slot below; on the plate it belongs to the element the row starts
/// with and leaves the slot to the one thing that needs it.
const CLASS_EDGE: f32 = 4.0;
/// The slot beside the plate, which now holds only the standing danger mark
/// — see [`paint_danger_mark`]. The width still spaces the columns whether
/// or not a mark is in it, so a row with no mark has no hole.
const DANGER_BAR_SIZE: (f32, f32) = (4.0, 24.0);
/// Where the position plate ends.
const POSITION_PLATE_END: f32 = 44.0;
/// The gap either side of the bar slot: between the plate's edge and the
/// bar, and between the bar and the car number.
///
/// One value for both, with [`BAR_X`] and [`NUMBER_X`] derived from it, so the
/// bar sits evenly between its neighbours instead of hard against the plate on
/// one side and adrift from the number on the other.
const BAR_GAP: f32 = 8.0;
/// The bar slot's centre, a gap clear of the plate's edge.
const BAR_X: f32 = POSITION_PLATE_END + BAR_GAP + DANGER_BAR_SIZE.0 / 2.0;
/// Where the car number starts, and the name after it.
const NUMBER_X: f32 = BAR_X + DANGER_BAR_SIZE.0 / 2.0 + BAR_GAP;
/// The alpha of the red block behind a severely-marked driver's name.
const DANGER_WASH_ALPHA: u8 = 48;
const NAME_X: f32 = NUMBER_X + NUMBER_WIDTH;
/// The driver's flag, between the number and the name: its height, from
/// which the 4:3 file sets the width, and the gap it leaves before the name.
/// The name moves right by [`flag_slot`] while flags are on and sits at
/// [`NAME_X`] when they are off, so turning them off leaves no hole.
const FLAG_HEIGHT: f32 = 15.0;
const FLAG_GAP: f32 = 8.0;
/// The chance of rain below which the footer omits it entirely.
const PRECIP_WORTH_SHOWING: f32 = 0.10;

/// The own-car `WET` tag in the footer: its height, the padding either side
/// of the word, its corner radius, and the gap it leaves before the rest of
/// the conditions run. Sized to the footer text so it reads as part of the
/// row, not a badge floating over it.
const WET_TAG_HEIGHT: f32 = 17.0;
const WET_TAG_PAD: f32 = 6.0;
const WET_TAG_ROUNDING: f32 = 4.0;
const WET_TAG_GAP: f32 = 14.0;

/// The gap below which a row shows hundredths instead of tenths — see
/// [`gap_text`].
const HUNDREDTHS_BELOW_SECS: f32 = 1.0;

/// The right-hand run's geometry, from the row's right edge inward: the
/// margin before the gap column, the gap's fixed width, the space before the
/// iRating badge, and the badge's two widths (with and without a projected
/// change on it). Named so the name column can compute where it must stop
/// before anything on the right is painted.
const ROW_RIGHT_PAD: f32 = 14.0;
const GAP_COLUMN_WIDTH: f32 = 70.0;
const TRAILING_GAP: f32 = 10.0;
/// Wider with a delta on it, and wider than it once was: the chevron beside
/// that figure is sized to be seen now, not to fit.
/// Snug: the rating is left-aligned and the change right-aligned, so the
/// badge's width *is* the gap between them, and a wide chip reads as two
/// unrelated numbers rather than one value and its trend.
const BADGE_WIDTH: f32 = 58.0;
const BADGE_WIDTH_WITH_DELTA: f32 = 82.0;
/// The clear space kept between the name column's contents and the
/// right-hand run, and after the name before its trailing marks.
const NAME_CLEARANCE: f32 = 10.0;

/// The outlined `SOF` chip in the header.
const SOF_CHIP_WIDTH: f32 = 34.0;

/// The scrolled-away-from-the-player marker beside it.
const SUBTITLE_SIZE: f32 = 12.0;

/// The slot the manufacturer's mark that follows a driver's name may occupy.
///
/// The mark is centred in this and keeps its own proportions rather than
/// filling it (see `ui::logos::draw`), so the width is really the horizontal
/// space the row gives up to it, and the height is what caps the drawn size:
/// every mark on disk is square, so a mark comes out `BRAND_HEIGHT` on a side.
/// That is why the height is the number to change to resize them, and why it
/// wants a few points of clearance under [`ROW_HEIGHT`].
const BRAND_WIDTH: f32 = 40.0;
const BRAND_HEIGHT: f32 = 38.0;

/// The up/down chevron on the iRating badge.
///
/// This is the size of the icon's *box*, not of the mark. The Lucide chevron
/// is a stroked path spanning the middle half of a 24-unit square, so the
/// visible mark is half this wide and a quarter of it tall, and its stroke
/// scales with the box too — at 11 that came out a sub-pixel hairline. Sized
/// for the mark to read, with [`CHEVRON_BOX_PADDING`] taking the slack out of
/// the spacing.
const CHEVRON_SIZE: f32 = 19.0;
/// The fraction of [`CHEVRON_SIZE`] that is empty padding on each side of the
/// glyph, from the icon's own geometry: the path runs x 6..18 of a 0..24 box.
const CHEVRON_BOX_PADDING: f32 = 0.25;
/// The gap left between the chevron's ink and the delta figure beside it.
const CHEVRON_GAP: f32 = 2.0;

/// The card's fill: the same near-black ink as the shared [`PANEL_BG`], but
/// thinner.
///
/// This panel sits low and central, over the road rather than over sky or
/// trim, so letting a little of the track through keeps it from reading as a
/// hole punched in the screen. Kept dark enough that white text still has its
/// contrast on a bright surface — premultiplied, so the channels are the ink
/// color already scaled by this alpha.
const CARD_BG: Color32 = Color32::from_rgba_premultiplied(6, 6, 7, 175);

/// Draws the Relative.
///
/// `scroll` moves the visible window through the field — negative looks
/// further ahead, positive further behind — so a driver can check a car
/// several places up without leaving the panel. Zero centres on the player,
/// which is where it returns to on its own; see `ui::blackbox`.
///
/// `rail` is the black box's tab rail, drawn across the top of the card as
/// it is on every other page; clicks on its tabs go into `clicks`.
pub fn draw(
    ui: &mut Ui,
    snapshot: Option<&TelemetrySnapshot>,
    config: &RelativeConfig,
    scroll: i32,
    rail: super::blackbox::Rail,
    clicks: &mut Vec<super::blackbox::Click>,
    options: super::RowOptions,
) {
    let metrics = Metrics::new(config.scale);
    let Some(snapshot) = snapshot else {
        placeholder(ui, metrics, "waiting for iRacing\u{2026}");
        return;
    };

    let visible = crate::telemetry::relative::window(
        snapshot.relative.len(),
        snapshot.focus_index,
        usize::from(config.ahead_count),
        usize::from(config.behind_count),
        scroll,
    );
    let rows: &[CarSnapshot] = snapshot.relative.get(visible).unwrap_or(&[]);
    // One class running means the class colors distinguish nothing, so rows
    // take a stand-in accent instead; see `ui::class_accent`.
    let class_count = snapshot.class_sections.len();

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // The gutter is allocated first but painted last: its markers have
        // to line up with rows whose positions aren't known until the card
        // beside it has been laid out.
        let (gutter_rect, _response) = ui.allocate_exact_size(metrics.vec2(GUTTER_WIDTH, 1.0), egui::Sense::hover());
        ui.add_space(metrics.px(GUTTER_GAP));

        // The rail runs the card's full width, so the card's own padding is
        // applied inside it, around the header, rows and footer, rather than
        // by the frame.
        let card = super::card_frame(metrics, CARD_BG, margin(metrics, 0.0, 0.0), super::table_card_rounding(metrics))
            .show(ui, |ui| {
                // `Frame::show` inherits the parent's layout direction, and the
                // parent here is the horizontal strip holding the gutter — so
                // without this the header, rows and footer would be laid out
                // side by side instead of stacked.
                ui.vertical(|ui| {
                    ui.set_width(metrics.px(outer_width(config)));
                    super::blackbox::draw_rail(ui, metrics, rail, clicks);
                    egui::Frame::none()
                        .inner_margin(margin(metrics, SIDE_PADDING, 14.0))
                        .show(ui, |ui| {
                            ui.set_width(metrics.px(card_width(config)));
                            draw_header(ui, metrics, &snapshot.relative_meta, snapshot.weather, scroll);
                            if rows.is_empty() {
                                placeholder(ui, metrics, "waiting for nearby cars\u{2026}");
                                return Vec::new();
                            }
                            let row_rects = rows
                                .iter()
                                .enumerate()
                                .map(|(i, car)| {
                                    let marked = car.cust_id.and_then(|id| options.danger.get(&id).copied());
                                    draw_car_row(
                                        ui,
                                        metrics,
                                        car,
                                        RowContext {
                                            odd: i % 2 == 1,
                                            class_count,
                                            first: i == 0,
                                            show_flags: options.show_flags,
                                            marked,
                                            config,
                                        },
                                    )
                                })
                                .collect();
                            draw_footer(ui, metrics, &snapshot.relative_meta, options.fuel_target);
                            row_rects
                        })
                        .inner
                })
                .inner
            });

        for (car, row_rect) in rows.iter().zip(card.inner) {
            let slot = Rect::from_min_size(
                egui::pos2(gutter_rect.left(), row_rect.top()),
                metrics.vec2(GUTTER_WIDTH, ROW_HEIGHT),
            );
            let marked = car.cust_id.and_then(|id| options.danger.get(&id).copied());
            draw_status_marker(ui, metrics, car, slot, options.show_off_tracks);
            danger_context_menu(ui, metrics, car, row_rect, marked, clicks);
        }
    });
}

/// Right-click a row to mark its driver dangerous, or clear the mark.
///
/// The one place a mark is set or read — no list, no screen: the driver is
/// marked where they are seen. A row whose car publishes no customer id can't
/// be marked (there'd be no session-stable key to persist), so its menu says
/// so rather than offering an option that wouldn't survive the session.
fn danger_context_menu(
    ui: &Ui,
    metrics: Metrics,
    car: &CarSnapshot,
    row_rect: Rect,
    marked: Option<crate::config::DangerLevel>,
    clicks: &mut Vec<super::blackbox::Click>,
) {
    use crate::config::DangerLevel;
    // A stable interact id per car so egui keeps each row's menu its own.
    let id = ui.id().with(("danger", car.car_idx));
    let response = ui.interact(row_rect, id, egui::Sense::click());
    response.context_menu(|ui| {
        ui.set_max_width(metrics.px(160.0));
        let Some(cust_id) = car.cust_id else {
            ui.label(RichText::new("No driver id to mark").size(metrics.px(FOOTER_SIZE)).color(text_secondary()));
            return;
        };
        ui.label(RichText::new(car.driver_name.as_ref()).size(metrics.px(FOOTER_SIZE)).strong());
        for level in DangerLevel::ALL {
            let picked = marked == Some(level);
            if ui.selectable_label(picked, level.label()).clicked() {
                let next = DangerLevel::toggled(marked, level);
                clicks.push(super::blackbox::Click::Danger { cust_id, level: next });
                ui.close_menu();
            }
        }
        if marked.is_some() {
            ui.separator();
            if ui.button("Clear mark").clicked() {
                clicks.push(super::blackbox::Click::Danger { cust_id, level: None });
                ui.close_menu();
            }
        }
    });
}

fn placeholder(ui: &mut Ui, metrics: Metrics, message: &str) {
    ui.label(RichText::new(message).size(metrics.px(FOOTER_SIZE)).color(text_secondary()));
}

/// The header: a boxed `SOF` label with the field strength beside it, then
/// track conditions and the incident count together on the right.
///
/// Conditions live up here rather than in the footer because they and the
/// incident count are both "state of the session" — glanced at between
/// corners — while the footer is left to the two numbers that change every
/// lap.
#[expect(clippy::cast_precision_loss, reason = "SOF is an estimate far below f32's exact-integer range")]
fn draw_header(ui: &mut Ui, metrics: Metrics, meta: &RelativeMeta, weather: WeatherSnapshot, scroll: i32) {
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(28.0)), egui::Sense::hover());
    let middle = rect.center().y;

    let chip =
        Rect::from_min_size(egui::pos2(rect.left(), middle - metrics.px(9.0)), metrics.vec2(SOF_CHIP_WIDTH, 18.0));
    ui.painter().rect_stroke(chip, metrics.px(4.0), Stroke::new(1.0_f32, Color32::from_white_alpha(45)));
    paint_text(
        ui,
        chip.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new("SOF").size(metrics.px(11.0)).strong().color(text_tertiary()),
    );

    let sof_text = meta.sof.map_or_else(|| "-".to_owned(), |sof| format!("{:.1}k", sof as f32 / 1000.0));
    let sof = RichText::new(sof_text).size(metrics.px(HEADER_VALUE_SIZE)).strong().color(text_primary());
    let sof_width = text_width(ui, sof.clone());
    paint_text(ui, egui::pos2(chip.right() + metrics.px(10.0), middle), egui::Align2::LEFT_CENTER, sof);

    // Both notices below say the same kind of thing — this panel is not
    // centred where you would assume — so they share a run of space to the
    // right of the field strength, laid out one after the other.
    let mut notice_left = chip.right() + metrics.px(20.0) + sof_width;

    // Whose race this is, whenever it is not the player's own. A Relative
    // quietly about somebody else is the one way spectator focus could
    // mislead, and the only fix is to put the name on the panel. Drawn in the
    // accent rather than in [`CAUTION`] because while spectating this is the
    // normal state of the widget for the whole session, not a warning — an
    // amber that never goes away is an amber nobody reads.
    if let Some(driver) = &meta.spectating {
        let watching =
            RichText::new(format!("\u{25C9} {driver}")).size(metrics.px(SUBTITLE_SIZE)).strong().color(ACCENT);
        let width = text_width(ui, watching.clone());
        paint_text(ui, egui::pos2(notice_left, middle), egui::Align2::LEFT_CENTER, watching);
        notice_left += width + metrics.px(12.0);
    }

    // Who is driving the player's own car when it is not the player. The same
    // slot and colour as the spectating notice: for a team-mate's stint this
    // is the normal state of the panel for hours at a time, not a warning.
    if let Some(driver) = &meta.team_mate {
        let driving =
            RichText::new(format!("\u{21C4} {driver}")).size(metrics.px(SUBTITLE_SIZE)).strong().color(ACCENT);
        let width = text_width(ui, driving.clone());
        paint_text(ui, egui::pos2(notice_left, middle), egui::Align2::LEFT_CENTER, driving);
        notice_left += width + metrics.px(12.0);
    }

    // A scrolled panel says so. Without this a driver glancing down mid-lap
    // could read someone else's battle as their own, which is the one way
    // this feature could actively mislead.
    if scroll != 0 {
        paint_text(
            ui,
            egui::pos2(notice_left, middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(format!("{scroll:+} \u{25B2}\u{25BC}"))
                .size(metrics.px(SUBTITLE_SIZE))
                .strong()
                .color(theme::caution()),
        );
    }

    let incidents_text = match meta.incident_limit {
        Some(limit) => format!("{}/{limit}", meta.incidents),
        None => meta.incidents.to_string(),
    };
    let incidents = RichText::new(incidents_text).size(metrics.px(HEADER_VALUE_SIZE)).strong().color(text_primary());
    let incidents_width = text_width(ui, incidents.clone());
    paint_text(ui, egui::pos2(rect.right(), middle), egui::Align2::RIGHT_CENTER, incidents);
    let cross = Rect::from_center_size(
        egui::pos2(rect.right() - incidents_width - metrics.px(11.0), middle),
        metrics.vec2(13.0, 13.0),
    );
    icons::cross(ui, cross, text_secondary());

    draw_conditions(ui, metrics, egui::pos2(cross.left() - metrics.px(18.0), middle), weather);
    ui.add_space(metrics.px(10.0));
}

/// Everything about the panel — as opposed to the car — one row is drawn
/// with, grouped so the call doesn't turn into a row of bare booleans.
#[derive(Clone, Copy)]
struct RowContext<'a> {
    odd: bool,
    class_count: usize,
    first: bool,
    show_flags: bool,
    /// This driver's standing danger mark, drawn into the row itself — see
    /// [`paint_danger_mark`].
    marked: Option<crate::config::DangerLevel>,
    /// The column switches, and the width the name column's room falls out of.
    config: &'a RelativeConfig,
}

/// Draws one car's row and returns the rectangle it occupied, so the caller
/// can align a gutter marker to it.
fn draw_car_row(ui: &mut Ui, metrics: Metrics, car: &CarSnapshot, row: RowContext<'_>) -> Rect {
    let RowContext { odd, class_count, first, show_flags, marked, config } = row;
    let in_pits = matches!(car.track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits);
    let flag_slot = if show_flags { flag_slot() } else { 0.0 };
    // The name column starts where the number would, when the number is off:
    // the column collapses rather than leaving a hole.
    let name_col = if config.show_car_number { NAME_X } else { NUMBER_X };
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(ROW_HEIGHT)), egui::Sense::hover());

    // A car in the pits is greyed out whole — class wash, slash, name, badge
    // and gap — so the eye slides straight past it. It is still worth a row,
    // because knowing a rival is stopped is the point, but it isn't racing
    // the player for the next thirty seconds and shouldn't read as if it is.
    let class = class_accent(&car.car_class_color, class_count);
    let accent = if in_pits && !car.is_focus { text_tertiary() } else { class };
    let plate_width = metrics.px(POSITION_PLATE_END);
    // Square, because touching rows cannot be rounded without notching against
    // each other; the card's own corners still shape the run as a whole.
    if car.is_focus {
        ui.painter().rect_filled(rect, egui::Rounding::ZERO, PLAYER_ROW_FILL);
    } else {
        if let Some(stripe) = row_stripe(odd) {
            ui.painter().rect_filled(rect, egui::Rounding::ZERO, stripe);
        }
        // A severely-marked driver keeps a block of red behind their name —
        // the one row state loud enough to deserve the whole stretch. Flat,
        // not a gradient: a colour that fades out has no edge, and the edge
        // is what the eye catches. It starts where the plate ends rather
        // than at the row's edge, which is what used to leave a rim of
        // colour down the card's rounded corner.
        if marked == Some(crate::config::DangerLevel::Severe) {
            let block = Rect::from_min_max(
                egui::pos2(rect.left() + plate_width, rect.top()),
                egui::pos2(rect.left() + metrics.px(name_col + flag_slot + WASH_PAST_NAME), rect.max.y),
            );
            ui.painter().rect_filled(block, egui::Rounding::ZERO, tint(super::ALERT, DANGER_WASH_ALPHA));
        }
    }
    // Over the row's own fill, under everything that follows. Near-white on
    // the player's own row, with the number in near-black on top: "you"
    // reads from the plate alone, without spending a colour on it.
    //
    // No shadow behind it any more. A soft falloff off the plate's trailing
    // edge is a smear across the one part of the row that has to stay
    // readable, and it bought nothing but the suggestion of depth.
    let plate_fill = if car.is_focus { PLAYER_PLATE } else { POSITION_PLATE };
    super::paint_position_plate(
        ui,
        rect,
        plate_width,
        egui::Rounding::ZERO,
        plate_fill,
        Some((metrics.px(CLASS_EDGE), accent)),
    );

    let middle = rect.center().y;

    // The position, in the readout face, centred on its plate.
    paint_text(
        ui,
        egui::pos2(rect.left() + plate_width / 2.0, middle),
        egui::Align2::CENTER_CENTER,
        readout(position_text(car), metrics.px(POSITION_SIZE)).color(position_color(car, in_pits)),
    );
    // The standing danger mark, in the bar slot beside the plate — part of
    // the row's own furniture, read in the same glance as the position and
    // name, rather than one more icon out in the gutter. The class colour
    // still reads from the row's wash.
    if let Some(level) = marked {
        paint_danger_mark(ui, metrics, rect, middle, level);
    }

    // The car number, in the column before the name: it is what the spotter
    // says and what the sim paints on the car ahead, so it is the thing that
    // ties a row to the car in the mirror.
    if config.show_car_number && !car.car_number.is_empty() {
        paint_text(
            ui,
            egui::pos2(rect.left() + metrics.px(NUMBER_X), middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(format!("#{}", car.car_number))
                .monospace()
                .size(metrics.px(NUMBER_SIZE))
                .color(if in_pits && !car.is_focus { text_tertiary() } else { text_secondary() }),
        );
    }

    // The driver's flag, in the column before the name. A driver with none
    // leaves the slot empty rather than pulling their name left, so names
    // stay in one column down the card.
    if show_flags {
        let flag = Rect::from_min_size(
            egui::pos2(rect.left() + metrics.px(name_col), middle - metrics.px(FLAG_HEIGHT / 2.0)),
            metrics.vec2(FLAG_HEIGHT * super::flags::ASPECT, FLAG_HEIGHT),
        );
        super::flags::draw(ui, flag, car.flair_id, in_pits && !car.is_focus);
    }

    let name_x = rect.left() + metrics.px(name_col + flag_slot);
    draw_name_column(ui, metrics, car, rect, name_x, in_pits, config);
    draw_row_trailing(ui, metrics, car, rect, in_pits, config.show_irating);
    // On the row's own top edge rather than its bottom, so the run of rows
    // ends cleanly against the card instead of on a divider with nothing
    // under it. The first row has the header's rule above it already.
    if !first {
        super::paint_row_groove(ui, rect.top(), rect.left(), rect.right());
    }
    rect
}

/// How far the name moves right to make room for a flag: the flag's width
/// at 4:3 plus the gap after it, in unscaled pixels.
fn flag_slot() -> f32 {
    FLAG_HEIGHT * super::flags::ASPECT + FLAG_GAP
}

/// The name column: the driver's name, elided to the room the row has left,
/// then the marks that trail it while they fit.
///
/// The manufacturer's mark and recent pace trail the name rather than
/// sitting in fixed columns, so they read as belonging to that driver rather
/// than as two more things to scan across. Either is dropped for this row
/// alone when a long name on a narrow card leaves it no room — short of the
/// limit by half the name's clearance, so a mark that just fits still stands
/// clear of the badge rather than flush against it: the name is the column,
/// these are its trim.
fn draw_name_column(
    ui: &mut Ui,
    metrics: Metrics,
    car: &CarSnapshot,
    rect: Rect,
    name_x: f32,
    in_pits: bool,
    config: &RelativeConfig,
) {
    let middle = rect.center().y;
    let limit = trailing_limit(metrics, car, rect, config.show_irating);
    let name_room = (limit - metrics.px(NAME_CLEARANCE) - name_x).max(0.0);
    let ink = row_text_color(car, in_pits);
    let name = elide_to_width(ui, &car.driver_name, name_room, |text| {
        RichText::new(text).size(metrics.px(NAME_SIZE)).strong().color(ink)
    });
    let name_width = text_width(ui, name.clone());
    paint_text(ui, egui::pos2(name_x, middle), egui::Align2::LEFT_CENTER, name);

    let trim_limit = limit - metrics.px(NAME_CLEARANCE / 2.0);
    let mut trailing = name_x + name_width + metrics.px(NAME_CLEARANCE);
    if config.show_brand && !car.car_screen_name.is_empty() {
        // A wide style gets a 2:1 slot; the lap time simply moves right.
        let brand_width = if super::logos::wide_slots() { BRAND_HEIGHT * 2.0 } else { BRAND_WIDTH };
        if trailing + metrics.px(brand_width) <= trim_limit {
            let mark = Rect::from_center_size(
                egui::pos2(trailing + metrics.px(brand_width / 2.0), middle),
                metrics.vec2(brand_width, BRAND_HEIGHT),
            );
            super::logos::draw(ui, mark, &car.car_screen_name, metrics.px(RECENT_LAP_SIZE), in_pits);
            trailing = mark.right() + metrics.px(8.0);
        }
    }

    if config.show_recent_lap
        && let Some(recent) = car.best_recent_lap_secs
    {
        let recent = RichText::new(format_short_lap_time(recent))
            .monospace()
            .size(metrics.px(RECENT_LAP_SIZE))
            .color(if in_pits { text_tertiary() } else { text_secondary() });
        if trailing + text_width(ui, recent.clone()) <= trim_limit {
            paint_text(ui, egui::pos2(trailing, middle), egui::Align2::LEFT_CENTER, recent);
        }
    }
}

/// Where this row's right-hand run begins, in absolute x.
///
/// The name and its trailing marks must stop short of this on a narrowed
/// card, so it is computed before any of them are painted — from the same
/// measurements `draw_row_trailing` and the badge lay out with.
fn trailing_limit(metrics: Metrics, car: &CarSnapshot, rect: Rect, show_irating: bool) -> f32 {
    let mut limit = rect.right() - metrics.px(ROW_RIGHT_PAD + GAP_COLUMN_WIDTH);
    if show_irating {
        let badge_width = if car.irating_change_estimate.is_some() { BADGE_WIDTH_WITH_DELTA } else { BADGE_WIDTH };
        limit -= metrics.px(TRAILING_GAP + badge_width);
    }
    limit
}

/// The right-hand run of a row: license badge, iRating badge, and the gap.
///
/// Laid out right to left from the row's right edge so the gap number always
/// lands in the same column regardless of how wide the badges turn out.
fn draw_row_trailing(ui: &mut Ui, metrics: Metrics, car: &CarSnapshot, rect: Rect, in_pits: bool, show_irating: bool) {
    let middle = rect.center().y;
    let mut right = rect.right() - metrics.px(ROW_RIGHT_PAD);

    // Gap, in its own fixed-width column. A car off the player's lap is
    // marked by coloring the number and the name — the two things already
    // being read — rather than by a wash behind the number: on a real grid
    // most rows are off your lap at some point, and a block of color behind
    // most of the panel stops meaning anything.
    let gap_right = right;
    paint_text(
        ui,
        egui::pos2(gap_right, middle),
        egui::Align2::RIGHT_CENTER,
        RichText::new(gap_text(car)).size(metrics.px(GAP_SIZE)).strong().color(gap_color(car, in_pits)),
    );
    right -= metrics.px(GAP_COLUMN_WIDTH + TRAILING_GAP);

    if show_irating {
        draw_irating_badge(ui, metrics, car, egui::pos2(right, middle), in_pits);
    }
}

/// The iRating badge: a white chip with black text, bordered in the driver's
/// license-class color.
///
/// The license class rides on this badge's border rather than getting a
/// badge of its own — the safety-rating number was the least-used value in
/// the row, and folding its color in here buys back the width while keeping
/// the class readable at a glance.
fn draw_irating_badge(ui: &Ui, metrics: Metrics, car: &CarSnapshot, right_center: Pos2, dimmed: bool) {
    let rating = format_irating(car.irating);
    let delta = car.irating_change_estimate.map(|change| {
        (format!("{:.0}", change.abs()), change >= 0.0, if change >= 0.0 { theme::signal() } else { theme::alert() })
    });

    // See [`BADGE_WIDTH`] for why the two widths differ. Named, because the
    // name column has to know where this chip will start before it paints.
    let width = metrics.px(if delta.is_some() { BADGE_WIDTH_WITH_DELTA } else { BADGE_WIDTH });
    let height = metrics.px(28.0);
    let rect = Rect::from_min_size(
        egui::pos2(right_center.x - width, right_center.y - height / 2.0),
        egui::vec2(width, height),
    );
    let rounding = metrics.px(5.0);
    let fill = if dimmed { Color32::from_white_alpha(120) } else { Color32::WHITE };
    ui.painter().rect_filled(rect, rounding, fill);
    ui.painter().rect_stroke(
        rect,
        rounding,
        Stroke::new(metrics.px(2.0), tint(parse_hex_color(&car.license_color), if dimmed { 130 } else { 255 })),
    );

    // Black on white, so the delta's red/green has to darken to stay legible
    // against a light chip rather than the dark row it used to sit on.
    let value_color = Color32::BLACK;
    match delta {
        Some((delta_text, is_gain, delta_color)) => {
            paint_text(
                ui,
                egui::pos2(rect.left() + metrics.px(6.0), rect.center().y),
                egui::Align2::LEFT_CENTER,
                RichText::new(rating).size(metrics.px(BADGE_SIZE)).strong().color(value_color),
            );
            let delta = RichText::new(delta_text).size(metrics.px(BADGE_SIZE)).strong().color(on_white(delta_color));
            let delta_width = text_width(ui, delta.clone());
            let delta_left = rect.right() - metrics.px(6.0) - delta_width;
            paint_text(
                ui,
                egui::pos2(rect.right() - metrics.px(6.0), rect.center().y),
                egui::Align2::RIGHT_CENTER,
                delta,
            );

            // A proper chevron rather than Unicode's arithmetic caret, which
            // is what the embedded text face offers and reads as a typo.
            //
            // Placed by where its *ink* falls, not by its box. The Lucide
            // glyph is a stroked path across the middle half of a square
            // viewBox, so a quarter of the box is empty on each side; sizing
            // and spacing it as though the box were the mark drew a hairline
            // adrift in its own padding.
            let size = metrics.px(CHEVRON_SIZE);
            let ink_padding = size * CHEVRON_BOX_PADDING;
            let chevron = Rect::from_center_size(
                egui::pos2(delta_left - metrics.px(CHEVRON_GAP) + ink_padding - size / 2.0, rect.center().y),
                egui::vec2(size, size),
            );
            let name = if is_gain { "chevron-up" } else { "chevron-down" };
            icons::svg(ui, chevron, name, on_white(delta_color));
        }
        None => paint_text(
            ui,
            rect.center(),
            egui::Align2::CENTER_CENTER,
            RichText::new(rating).size(metrics.px(BADGE_SIZE)).strong().color(value_color),
        ),
    }
}

/// Darkens an accent so it stays readable on a white chip.
///
/// `SIGNAL` and `ALERT` are tuned for a near-black card; placed on white they
/// wash out, so each channel is pulled two-thirds of the way toward black.
#[expect(clippy::cast_possible_truncation, reason = "the result is at most 255 * 2 / 5 = 102, well inside u8")]
fn on_white(color: Color32) -> Color32 {
    let darken = |c: u8| (u16::from(c) * 2 / 5) as u8;
    Color32::from_rgb(darken(color.r()), darken(color.g()), darken(color.b()))
}

/// Paints this row's gutter marker, if it has one.
///
/// Rows with nothing to report get nothing drawn: the gutter is outside the
/// card, so an empty slot costs no alignment and a placeholder box would
/// only add noise beside the panel.
fn draw_status_marker(ui: &Ui, metrics: Metrics, car: &CarSnapshot, slot: Rect, show_off_tracks: bool) {
    let rect = Rect::from_center_size(slot.center(), metrics.vec2(30.0, 30.0));
    let rounding = metrics.px(6.0);

    // A flag outranks everything else the slot could say: the fastest car in
    // the race with a drive-through to serve is, for the next lap, a car with
    // a drive-through to serve.
    if let Some(penalty) = car.penalty {
        super::paint_penalty_marker(ui, metrics, rect, penalty);
    } else if car.is_fastest_overall {
        ui.painter().rect_filled(rect, rounding, theme::fastest());
        icons::stopwatch(ui, rect.shrink(metrics.px(6.0)), Color32::WHITE);
    } else if matches!(car.track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits) {
        ui.painter().rect_filled(rect, rounding, Color32::WHITE);
        paint_text(
            ui,
            rect.center(),
            egui::Align2::CENTER_CENTER,
            RichText::new("PIT").monospace().size(metrics.px(12.0)).strong().color(Color32::BLACK),
        );
    } else {
        let count = if show_off_tracks { car.off_tracks } else { 0 };
        super::paint_off_track_marker(ui, metrics, rect, car.track_location == TrackLocation::OffTrack, count);
    }
}

/// Paints the standing danger mark: a straight-edged bar in the slot beside
/// the position plate. Amber at the class-bar height for caution; amber the
/// row's full height for warning; the alert red full height for severe,
/// whose row also takes a red wash — see [`DANGER_WASH_ALPHA`]. The level
/// reads from how much of the row the mark claims, not from a glyph.
fn paint_danger_mark(ui: &Ui, metrics: Metrics, rect: Rect, middle: f32, level: crate::config::DangerLevel) {
    use crate::config::DangerLevel;
    let (colour, height) = match level {
        DangerLevel::Caution => (super::CAUTION, metrics.px(DANGER_BAR_SIZE.1)),
        DangerLevel::Warning => (super::CAUTION, rect.height()),
        DangerLevel::Severe => (super::ALERT, rect.height()),
    };
    let bar = Rect::from_center_size(
        egui::pos2(rect.left() + metrics.px(BAR_X), middle),
        egui::vec2(metrics.px(DANGER_BAR_SIZE.0), height),
    );
    ui.painter().rect_filled(bar, egui::Rounding::ZERO, colour);
}

fn position_text(car: &CarSnapshot) -> String {
    if car.position > 0 { car.position.to_string() } else { "-".to_owned() }
}

/// Ink on the player's own near-white plate, quiet in the pits, primary
/// otherwise.
fn position_color(car: &CarSnapshot, in_pits: bool) -> Color32 {
    if car.is_focus {
        Color32::from_black_alpha(230)
    } else if in_pits {
        text_tertiary()
    } else {
        text_primary()
    }
}

/// The gap magnitude, unsigned: a row's position above or below the player's
/// already says which side of them that car is on, so a sign would only
/// repeat it. The mockup shows bare numbers for the same reason.
fn gap_text(car: &CarSnapshot) -> String {
    let gap = if car.is_focus { 0.0 } else { car.gap_to_player_secs.abs() };
    // Hundredths only while a car is genuinely alongside, where a tenth is
    // too coarse to tell a pass from a stalemate. Past a second the extra
    // digit is noise, and it would widen the column for every row.
    if gap < HUNDREDTHS_BELOW_SECS { format!("{gap:.2}") } else { format!("{gap:.1}") }
}

/// Grey in the pits — which outranks everything else, since a stopped car's
/// gap is about to be meaningless anyway — then white on the player's own
/// row, the lap-status color for a car off their lap, and primary otherwise.
fn gap_color(car: &CarSnapshot, in_pits: bool) -> Color32 {
    if in_pits && !car.is_focus {
        text_tertiary()
    } else if car.is_focus {
        Color32::WHITE
    } else {
        lap_status_color(car).unwrap_or_else(text_primary)
    }
}

/// `ALERT` (red) if this car is a lap up on the player — it's catching up
/// to lap them — or `LAPPED` (blue) if the player has lapped this car;
/// `None` on the same lap, where no special color applies. Matches the
/// red/blue lap-status convention from
/// <https://github.com/tariknz/irdashies>'s Relative widget.
fn lap_status_color(car: &CarSnapshot) -> Option<Color32> {
    match car.lap_diff {
        d if d > 0 => Some(theme::alert()),
        d if d < 0 => Some(theme::lapped()),
        _ => None,
    }
}

fn row_text_color(car: &CarSnapshot, in_pits: bool) -> Color32 {
    if car.is_focus {
        Color32::WHITE
    } else if in_pits {
        text_tertiary()
    } else {
        lap_status_color(car).unwrap_or_else(text_primary)
    }
}

/// The footer: race clock on the left, lap progress hard right.
fn draw_footer(ui: &mut Ui, metrics: Metrics, meta: &RelativeMeta, fuel_target: Option<super::FuelTargetReadout>) {
    ui.add_space(metrics.px(6.0));
    let (rule, _response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rule, 0.0, hairline());
    ui.add_space(metrics.px(10.0));

    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(22.0)), egui::Sense::hover());
    let middle = rect.center().y;

    // The grid takes the clock's place while the field is forming up: the
    // clock is holding at the race length and has nothing to say yet, and
    // how long there is to grid, and whether everyone is out, is what a
    // driver sitting on it wants.
    match meta.grid {
        Some(grid) => paint_label_value(ui, metrics, egui::pos2(rect.left(), middle), "Grid", &super::grid_text(grid)),
        None => paint_label_value(
            ui,
            metrics,
            egui::pos2(rect.left(), middle),
            meta.session_kind.label(),
            &race_clock_text(meta),
        ),
    }
    paint_label_value_right(ui, metrics, egui::pos2(rect.right(), middle), "Lap", &lap_text(meta));
    // The shared fuel target rides in the centre of the footer, between the
    // clock and the lap: a number the crew chief set for the driver to hold.
    if let Some(target) = fuel_target {
        draw_fuel_target(ui, metrics, egui::pos2(rect.center().x, middle), target);
    }
}

/// The shared fuel target: `TGT 2.55` with the current burn beside it,
/// coloured green when at or under target and amber when over, and the lap the
/// tank makes at that burn. Centred on `at`.
fn draw_fuel_target(ui: &Ui, metrics: Metrics, at: egui::Pos2, target: super::FuelTargetReadout) {
    use std::fmt::Write as _;

    let on_target = target.current_lpl.is_none_or(|current| current <= target.target_lpl + FUEL_TARGET_SLACK);
    let value_color = if on_target { super::SIGNAL } else { super::CAUTION };

    let mut text = format!("TGT {:.2}", target.target_lpl);
    if let Some(current) = target.current_lpl {
        // Writing to a `String` cannot fail; the result is discarded by the
        // same convention `format!` itself relies on.
        let _ = write!(text, "  \u{00B7} {current:.2}");
    }
    if let Some(lap) = target.hit_lap {
        let _ = write!(text, "  \u{00B7} L{lap}");
    }
    paint_text(
        ui,
        at,
        egui::Align2::CENTER_CENTER,
        RichText::new(text).monospace().size(metrics.px(FOOTER_SIZE)).color(value_color),
    );
}

/// Litres per lap of slack before the burn reads as "over target" — a hair,
/// so sensor jitter around the number doesn't flicker the colour.
const FUEL_TARGET_SLACK: f32 = 0.02;

/// Track conditions, laid out right to left from `right`: the own-car `WET`
/// tag while the player is on wet tyres, track temperature, and one rain
/// number — the live rain while it falls, else the chance of rain when it's
/// high enough to be worth planning around.
///
/// Below [`PRECIP_WORTH_SHOWING`] the chance is noise — a dry session still
/// reports a percent or two — so it's left out entirely rather than shown as
/// a permanent near-zero that trains the eye to skip the one place a real
/// forecast would appear. Live rain has no such gate: once water is falling,
/// any printable amount of it is the figure that matters.
fn draw_conditions(ui: &Ui, metrics: Metrics, right: Pos2, weather: WeatherSnapshot) {
    let mut right = right;
    if weather.on_wet_tyres {
        right.x = draw_wet_tag(ui, metrics, right) - metrics.px(WET_TAG_GAP);
    }
    let temp = RichText::new(format!("{:.0}\u{b0}C", weather.track_temp_c))
        .size(metrics.px(FOOTER_SIZE))
        .strong()
        .color(text_secondary());
    let temp_width = text_width(ui, temp.clone());
    paint_text(ui, right, egui::Align2::RIGHT_CENTER, temp);

    let road =
        Rect::from_center_size(egui::pos2(right.x - temp_width - metrics.px(17.0), right.y), metrics.vec2(16.0, 16.0));
    icons::road(ui, road, text_tertiary());

    let live = super::weather::live_rain(&weather);
    let Some(rain) = live.or_else(|| weather.precip_chance.filter(|c| *c > PRECIP_WORTH_SHOWING)) else {
        return;
    };
    let mut edge = road.left() - metrics.px(14.0);
    let precip = RichText::new(format!("{:.0}%", rain * 100.0)).size(metrics.px(FOOTER_SIZE)).strong().color(WIND);
    let precip_width = text_width(ui, precip.clone());
    paint_text(ui, egui::pos2(edge, right.y), egui::Align2::RIGHT_CENTER, precip);
    edge -= precip_width + metrics.px(10.0);

    let cloud = Rect::from_center_size(egui::pos2(edge - metrics.px(10.0), right.y), metrics.vec2(20.0, 16.0));
    icons::fog(ui, cloud, text_tertiary());
}

/// The straight-edged block saying the player's own car is on wet tyres —
/// filled with the panel's water colour so it reads with the rain figure it
/// sits beside. Own-car state, so it shows whenever true, not only while the
/// field disagrees about compounds. Returns the tag's left edge.
fn draw_wet_tag(ui: &Ui, metrics: Metrics, right: Pos2) -> f32 {
    let word = RichText::new("WET").size(metrics.px(FOOTER_SIZE)).strong().color(Color32::from_black_alpha(230));
    let width = text_width(ui, word.clone()) + metrics.px(WET_TAG_PAD * 2.0);
    let rect = Rect::from_min_size(
        egui::pos2(right.x - width, right.y - metrics.px(WET_TAG_HEIGHT / 2.0)),
        egui::vec2(width, metrics.px(WET_TAG_HEIGHT)),
    );
    ui.painter().rect_filled(rect, metrics.px(WET_TAG_ROUNDING), WIND);
    paint_text(ui, rect.center(), egui::Align2::CENTER_CENTER, word);
    rect.left()
}

/// Paints a grey label followed by a brighter value, ending at `right`.
fn paint_label_value_right(ui: &Ui, metrics: Metrics, right: Pos2, label: &str, value: &str) {
    let value_text = RichText::new(value).size(metrics.px(FOOTER_SIZE)).strong().color(text_secondary());
    let value_width = text_width(ui, value_text.clone());
    paint_text(ui, right, egui::Align2::RIGHT_CENTER, value_text);
    paint_text(
        ui,
        egui::pos2(right.x - value_width - metrics.px(8.0), right.y),
        egui::Align2::RIGHT_CENTER,
        RichText::new(label).size(metrics.px(FOOTER_SIZE)).color(text_tertiary()),
    );
}

/// Paints a grey label followed by a brighter value.
fn paint_label_value(ui: &Ui, metrics: Metrics, pos: Pos2, label: &str, value: &str) {
    let label_text = RichText::new(label).size(metrics.px(FOOTER_SIZE)).color(text_tertiary());
    let label_width = text_width(ui, label_text.clone());
    paint_text(ui, pos, egui::Align2::LEFT_CENTER, label_text);
    paint_text(
        ui,
        egui::pos2(pos.x + label_width + metrics.px(8.0), pos.y),
        egui::Align2::LEFT_CENTER,
        RichText::new(value).size(metrics.px(FOOTER_SIZE)).strong().color(text_secondary()),
    );
}

/// The session clock, counting down — see [`RelativeMeta::countdown_secs`].
///
/// A countdown rather than the elapsed-over-total it replaced, because the
/// question a driver asks of a session clock is how much of it is left, and
/// elapsed-over-total made them subtract one from the other to find out.
fn race_clock_text(meta: &RelativeMeta) -> String {
    match meta.countdown_secs() {
        Some(remain) => format_clock(remain),
        // Nothing to count down to, so the only honest reading is how long
        // this has been going on.
        None => format_clock(meta.race_elapsed_secs),
    }
}

#[cfg(test)]
mod tests {
    use crate::telemetry::snapshot::SessionKind;

    use super::*;
    use std::sync::Arc;

    fn car(gap: f32, is_focus: bool) -> CarSnapshot {
        CarSnapshot {
            car_idx: 0,
            cust_id: None,
            position: 3,
            track_location: TrackLocation::OnTrack,
            gap_to_player_secs: gap,
            driver_name: Arc::from("Test Driver"),
            car_number: Arc::from("42"),
            car_screen_name: Arc::from(""),
            irating: 2500,
            flair_id: 0,
            license_color: Arc::from("0x00C702"),
            car_class_color: Arc::from("0xE81E5B"),
            is_fastest_overall: false,
            irating_change_estimate: None,
            is_focus,
            off_tracks: 0,
            lap_diff: 0,
            best_recent_lap_secs: None,
            penalty: None,
        }
    }

    /// The mockup shows bare magnitudes: a car 1.8s behind reads `1.8`, not
    /// `-1.8`, because the row's own position already says which side of the
    /// player it's on.
    #[test]
    fn gaps_render_unsigned() {
        assert_eq!(gap_text(&car(-1.84, false)), "1.8");
        assert_eq!(gap_text(&car(7.4, false)), "7.4");
    }

    /// Close enough to matter gets hundredths; anything further out doesn't.
    #[test]
    fn only_cars_within_a_second_get_hundredths() {
        assert_eq!(gap_text(&car(0.36, false)), "0.36");
        assert_eq!(gap_text(&car(-0.07, false)), "0.07");
        assert_eq!(gap_text(&car(0.999, false)), "1.00", "rounding up to a second keeps the row's width");
        assert_eq!(gap_text(&car(1.0, false)), "1.0");
    }

    #[test]
    fn the_player_row_reads_zero() {
        assert_eq!(gap_text(&car(0.0, true)), "0.00");
    }

    /// A hand-edited width outside the slider's range — or not a number at
    /// all — must not be laid out as written; see [`card_width`].
    #[test]
    fn a_hand_edited_width_is_clamped_before_layout() {
        let mut config = RelativeConfig::default();
        assert!((card_width(&config) - DESIGN_WIDTH).abs() < f32::EPSILON);
        config.width = 5000.0;
        assert!((card_width(&config) - *WIDTH_RANGE.end()).abs() < f32::EPSILON);
        config.width = 10.0;
        assert!((card_width(&config) - *WIDTH_RANGE.start()).abs() < f32::EPSILON);
        config.width = f32::NAN;
        assert!((card_width(&config) - DESIGN_WIDTH).abs() < f32::EPSILON);
    }

    #[test]
    fn the_race_clock_counts_down_once_the_racing_starts() {
        let meta = RelativeMeta {
            session_kind: SessionKind::Race,
            racing_under_way: true,
            race_remain_secs: Some(24.0 * 60.0 + 59.0),
            session_length_secs: Some(45.0 * 60.0),
            ..RelativeMeta::default()
        };
        assert_eq!(race_clock_text(&meta), "00:24:59");
    }

    /// On the grid and behind the pace car the clock holds at the session's
    /// full length: none of that is the race, and a driver glancing down on
    /// the formation lap should not find minutes already gone.
    #[test]
    fn a_race_clock_holds_at_the_full_length_until_the_green() {
        let gridded = RelativeMeta {
            session_kind: SessionKind::Race,
            racing_under_way: false,
            // The sim may already be counting this down; it is not the race.
            race_remain_secs: Some(44.0 * 60.0),
            session_length_secs: Some(45.0 * 60.0),
            ..RelativeMeta::default()
        };
        assert_eq!(race_clock_text(&gridded), "00:45:00");
        assert!(gridded.clock_is_holding());
    }

    /// Practice and qualifying have no green flag to wait for, so their clock
    /// simply runs.
    #[test]
    fn practice_and_qualifying_count_down_from_the_start() {
        for kind in [SessionKind::Practice, SessionKind::Qualifying] {
            let meta = RelativeMeta {
                session_kind: kind,
                racing_under_way: false,
                race_remain_secs: Some(12.0 * 60.0 + 30.0),
                session_length_secs: Some(30.0 * 60.0),
                ..RelativeMeta::default()
            };
            assert_eq!(race_clock_text(&meta), "00:12:30", "{kind:?} has no green to wait for");
            assert!(!meta.clock_is_holding());
        }
    }

    /// A scheduled lap count is a fact and shows as one; a projection from the
    /// clock is marked as the estimate it is.
    #[test]
    fn the_lap_counter_prefers_the_scheduled_count_to_the_projection() {
        let lap_limited = RelativeMeta {
            current_lap: 12,
            session_laps: Some(40),
            predicted_total_laps: Some(38),
            ..RelativeMeta::default()
        };
        assert_eq!(lap_text(&lap_limited), "12/40");
        let timed = RelativeMeta { current_lap: 12, predicted_total_laps: Some(25), ..RelativeMeta::default() };
        assert_eq!(lap_text(&timed), "12/~25");
        let untimed = RelativeMeta { current_lap: 12, ..RelativeMeta::default() };
        assert_eq!(lap_text(&untimed), "12");
    }

    #[test]
    fn the_grid_reads_as_a_count_and_a_countdown() {
        use crate::telemetry::snapshot::GridStatus;
        let timed = GridStatus { cars_gridded: 12, car_count: 24, countdown_secs: Some(91.2) };
        assert_eq!(super::super::grid_text(timed), "12/24 \u{00B7} 1:32");
        let lap_limited = GridStatus { cars_gridded: 24, car_count: 24, countdown_secs: None };
        assert_eq!(super::super::grid_text(lap_limited), "24/24");
    }

    /// A practice session has no fixed length, so there's no total to show
    /// and the footer must not print a `/` with nothing after it.
    #[test]
    fn an_untimed_session_shows_only_the_elapsed_clock() {
        let meta = RelativeMeta {
            race_elapsed_secs: 61.0,
            race_remain_secs: None,
            session_length_secs: None,
            racing_under_way: true,
            ..RelativeMeta::default()
        };
        assert_eq!(race_clock_text(&meta), "00:01:01");
    }
}
