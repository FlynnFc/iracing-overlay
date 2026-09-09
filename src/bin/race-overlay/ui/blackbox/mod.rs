// Rust guideline compliant 2026-02-16

//! The black box: one panel, several pages, driven entirely from the wheel.
//!
//! Every page sits in the same chassis: a tab rail of the pages on offer, in
//! the order the wheel walks them, with the current one lit; a body as tall
//! as its content; and a footer saying what the page's controls apply to. The
//! rail is what answers "where will the next press land?" — see
//! [`draw_rail`]. Page one is the Relative widget, with the same rail across
//! its top.
//!
//! Every control the wheel can reach, the mouse can too: a click lands the
//! cursor on the control and then does what the bind would have done,
//! through the same [`BlackBox::apply`], so the two can never disagree — see
//! [`Click`]. The overlay stops being click-through while the pointer is
//! over a panel, which is what makes the panels draggable, so nothing new is
//! needed for a click to arrive.
//!
//! The pages themselves follow `plans/standings-and-blackbox-redesign.md`.
//!
//! Two pages in the mockups are not here, because this car publishes no
//! telemetry behind them: **Pit-stop Adjustments** (front and rear ARB, rear
//! wing — seventeen candidate variable names probed, none present) and the
//! ten-slot **Weather forecast**, which exists in neither telemetry nor the
//! session YAML. The Weather page shows current conditions instead, and is
//! honest about being current rather than forecast.
//!
//! Nothing here renders local intent: see [`crate::telemetry::pit`].

pub mod pages;

use std::time::{Duration, Instant};

use egui::{Color32, Rect, RichText, Stroke, Ui};

use super::{
    ALERT, CAUTION, Metrics, PANEL_BG, PLAYER_ROW, card_frame, card_rounding, margin, paint_text, row_stripe,
    text_primary, text_secondary, text_tertiary, text_width,
};
use crate::config::RelativeConfig;
use crate::input::Action;
use crate::telemetry::pit::{PitRequest, PitService};
use crate::telemetry::snapshot::CourseFlag;
use crate::telemetry::snapshot::{Seat, TelemetrySnapshot};
use crate::ui::theme;

/// The shortest gap Auto Fuel will leave between two pit commands.
///
/// A hard ceiling on a path that is already quiet — commands are only sent in
/// the pit lane at all. It exists because every pit command is a
/// `SendNotifyMessage` to `HWND_BROADCAST`: a window message delivered to
/// *every* top-level window on the machine, not just iRacing's. The overlay
/// repaints continuously, so anything the sim declines to act on — service
/// already under way, driver out of the car — would otherwise be rebroadcast
/// at frame rate, which floods the whole desktop's message queues rather than
/// merely this app's. Nothing about a fuel load needs answering inside two
/// seconds, and the first command after a quiet spell is never delayed — only
/// a follow-up is.
const AUTO_FUEL_MIN_INTERVAL: Duration = Duration::from_secs(2);

/// How much the tank must gain, in litres, for Auto Fuel to call a stop
/// served on the fuel level alone.
///
/// A backstop for the checkbox signal below. Large enough that sloshing and
/// the sim's own rounding can't trip it, small enough that any real splash
/// does.
const FUEL_SERVED_LITRES: f32 = 1.0;

/// Panel geometry.
///
/// One width for every page: the Relative's (now configurable — see
/// `ui::relative::outer_width`), including its padding, because page one is
/// on screen all race and must not change size under the rail.
const ROW_HEIGHT: f32 = 27.0;

/// The tab rail across the top of every page — see [`draw_rail`].
const RAIL_HEIGHT: f32 = 26.0;
const RAIL_PAD: f32 = 6.0;
const RAIL_TAB_GAP: f32 = 2.0;
const RAIL_TAB_HEIGHT: f32 = 20.0;
const RAIL_LABEL_SIZE: f32 = 11.0;

/// The footer under a page's body, saying what its controls apply to.
const FOOTER_HEIGHT: f32 = 22.0;
const FOOTER_SIZE: f32 = 12.0;

/// Type scale.
const ROW_SIZE: f32 = 14.0;

/// Where a row's label ends and its value begins, as a fraction of the card.
/// Labels are right-aligned to this line and values start just past it, which
/// is what gives the mockups their strong centre gutter.
const LABEL_FRACTION: f32 = 0.47;

/// The blue of a ticked checkbox.
const CHECK: Color32 = Color32::from_rgb(0x2E, 0x7C, 0xE8);

/// The arrows at each end of a control plate.
const CHEVRON_SIZE: f32 = 15.0;
/// How far each arrow sits inside the plate's own ends.
const CHEVRON_INSET: f32 = 13.0;

/// The corner grid: the four wheels where a car's wheels sit, the body drawn
/// faintly between them, and the whole-set control on it — see
/// [`draw_corners`].
const GRID_HEIGHT: f32 = 340.0;
/// One wheel, seen from above. Tall, because it now carries what came off
/// the car at the last stop under what goes on at the next.
const TYRE_SIZE: (f32, f32) = (156.0, 150.0);
/// Where each column of wheels is centred, in from the grid's edges.
const TYRE_COLUMN_INSET: f32 = 112.0;
/// The two axles, in pixels down from the grid's top.
const FRONT_AXLE_Y: f32 = 90.0;
const REAR_AXLE_Y: f32 = 250.0;
const TYRE_ROUNDING: f32 = 16.0;
/// The chassis under the wheels: two axles and the spine between them.
///
/// This was a rounded outline around the whole middle of the grid, which is
/// the shape of a car seen from above and, unfortunately, also the shape of
/// a border drawn around the whole-set control sitting inside it. Everyone
/// read it as the second thing. An outline encloses; a chassis connects, so
/// this is drawn as fills that run *into* the wheels and stop.
const AXLE_THICKNESS: f32 = 7.0;
const SPINE_WIDTH: f32 = 24.0;
/// How much of the panel's own light the chassis carries.
///
/// Very low, and lower than it looks like it should be: this is spread
/// across a spine and two axles rather than drawn as a hairline, and the
/// same alpha that reads as a quiet rule at one pixel wide reads as a solid
/// grey casting at twenty-four. The old outline used 45 and shouted at 2px;
/// 11 still drew a grey H across the middle of the page. Measured off a
/// render rather than guessed: at 11 the axles came out rgb(72) against a
/// rgb(51) panel, a fifth of the way to white for something meant to be
/// barely there.
const CHASSIS_ALPHA: u8 = 5;
/// The spine's corner radius. Small, so it reads as a structural member;
/// rounded to its own half-width it came out a pill with a button on it.
const SPINE_ROUNDING: f32 = 6.0;
/// The cursor on the whole-set plate: an olive bar along its foot when the
/// plate is amber, since a ring round it read as a border.
const ALL_FOUR_CURSOR_BAR: f32 = 4.0;
/// Grooves down the tread, out by the sidewalls where the pressure isn't.
///
/// Two rather than the three a tyre really has: a third runs down the middle,
/// which is exactly where the number is, and a groove through the one thing
/// the panel exists to show is a texture that costs more than it gives.
const TREAD_GROOVES: [f32; 2] = [0.18, 0.82];

/// Where each part of a wheel sits inside it, from its top edge.
///
/// The pressure to set, then what came off the car: the tread as three bars,
/// their temperatures, the wear as one fill, and the hot pressure. No corner
/// label: which wheel this is, is where it is.
const TYRE_VALUE_Y: f32 = 28.0;
const TYRE_UNIT_Y: f32 = 52.0;
const TYRE_LABEL_SIZE: f32 = 13.0;
const TYRE_VALUE_SIZE: f32 = 34.0;
const TYRE_UNIT_SIZE: f32 = 13.0;
const TYRE_BARS_TOP: f32 = 64.0;
const TYRE_BARS_HEIGHT: f32 = 36.0;
const TYRE_BAR_WIDTH: f32 = 22.0;
const TYRE_TEMP_Y: f32 = 112.0;
const TYRE_TEMP_SIZE: f32 = 16.0;
const TYRE_WEAR_Y: f32 = 126.0;
const TYRE_WEAR_HEIGHT: f32 = 5.0;
const TYRE_HOT_Y: f32 = 139.0;
const TYRE_HOT_SIZE: f32 = 11.0;
const TYRE_PAD_X: f32 = 14.0;
/// How far off its tyre's own mean a reading has to be before it is worth a
/// colour. Eight degrees is a tyre with one edge doing the work; less is
/// inside the noise of a carcass reading.
const TYRE_TEMP_FAR_C: f32 = 8.0;
/// Tread remaining, as a fraction, below which the wear fill goes red.
const TYRE_WEAR_WORN: f32 = 0.75;

/// The pit-window strip: lap numbers, traffic bars, and the fuel each asks.
const WINDOW_STRIP_HEIGHT: f32 = 128.0;
const WINDOW_LABEL_HEIGHT: f32 = 22.0;
/// Stops to the end, counting the one being placed — the row that decides it.
const WINDOW_STOPS_HEIGHT: f32 = 26.0;
const WINDOW_STOPS_SIZE: f32 = 19.0;
const WINDOW_SAVE_HEIGHT: f32 = 24.0;
const WINDOW_BAR_WIDTH: f32 = 26.0;
const WINDOW_BAR_ROUNDING: f32 = 4.0;

/// The recommendation: a lap on a position plate at pit-board size, the
/// largest type anywhere in the overlay, because it is the one thing on this
/// page a driver has to be able to read without looking.
const VERDICT_HEIGHT: f32 = 72.0;
const VERDICT_SIZE: f32 = 44.0;
const VERDICT_PLATE_HEIGHT: f32 = 52.0;
const VERDICT_PLATE_PAD: f32 = 14.0;
/// The reasons beside it, two lines at most.
const VERDICT_SUB_HEIGHT: f32 = 20.0;
const VERDICT_SUB_SIZE: f32 = 14.0;
/// The strip's lap numbers, and the plate the recommended one sits on.
const WINDOW_LAP_SIZE: f32 = 20.0;
const WINDOW_PICK_PAD: f32 = 8.0;
/// The workings, quiet, on one line.
const WORKINGS_HEIGHT: f32 = 32.0;
const WORKINGS_SIZE: f32 = 13.0;

/// The Fuel page, band by band down the card — see [`draw_fuel`].
const FUEL_SIDE_MARGIN: f32 = 20.0;
const FUEL_TOP_PAD: f32 = 10.0;
/// The line over the tank: the `ADD` label, and the `FINISH` flag's tab.
const FUEL_HEAD_HEIGHT: f32 = 26.0;
/// The tank itself, the lanes either side of it for the stepper's chevrons,
/// and the figures written inside its segments.
const TANK_HEIGHT: f32 = 56.0;
const TANK_ROUNDING: f32 = 8.0;
const TANK_CHEVRON_LANE: f32 = 26.0;
const TANK_VALUE_SIZE: f32 = 30.0;
const TANK_VALUE_PAD: f32 = 16.0;
/// The finish flag: a tab over the tank at the litres the race needs, with
/// a stem down through the tank.
const FINISH_TAB_HEIGHT: f32 = 16.0;
const FINISH_TAB_SIZE: f32 = 10.0;
const FINISH_TAB_PAD: f32 = 7.0;
const FINISH_STEM: f32 = 2.0;
/// The legend under the tank, and the three readouts under that.
const FUEL_LEGEND_HEIGHT: f32 = 24.0;
const FUEL_LEGEND_SWATCH: (f32, f32) = (14.0, 10.0);
const FUEL_FIGURES_HEIGHT: f32 = 62.0;
const FUEL_FIGURE_SIZE: f32 = 34.0;
const FUEL_FIGURE_GAP: f32 = 40.0;

/// The arm strip: fuel, tearoff, fast repair and Auto Fuel, lit or not.
const ARM_TILES: usize = 4;
const ARM_STRIP_HEIGHT: f32 = 56.0;
const ARM_TILE_GAP: f32 = 10.0;
const ARM_TILE_ROUNDING: f32 = 6.0;
const ARM_TILE_TEXT_SIZE: f32 = 14.0;
/// The note under a tile's label: the fast-repair count, Auto Fuel's margin.
const ARM_TILE_NOTE_SIZE: f32 = 11.0;
const ARM_TILE_NOTE_Y: f32 = 11.0;
const ARM_TILE_NOTE_LIFT: f32 = 7.0;

/// The tile strip: one tile per reading the car or the session publishes.
const TILE_STRIP_HEIGHT: f32 = 120.0;
const TILE_STRIP_PAD: f32 = 12.0;
const TILE_GAP: f32 = 10.0;
const TILE_ROUNDING: f32 = 12.0;
const TILE_PAD_X: f32 = 14.0;
const TILE_VALUE_Y: f32 = 34.0;
const TILE_VALUE_SIZE: f32 = 34.0;
const TILE_LABEL_Y: f32 = 18.0;
const TILE_LABEL_SIZE: f32 = 13.0;
/// The split bar on a tile that is one (brake bias), and the wind arrow.
const TILE_SPLIT_Y: f32 = 58.0;
const TILE_SPLIT_HEIGHT: f32 = 8.0;
const TILE_ARROW_SIZE: f32 = 28.0;

/// The whole-set control between the rear wheels.
const ALL_FOUR_SIZE: (f32, f32) = (150.0, 40.0);
const ALL_FOUR_TEXT_SIZE: f32 = 16.0;

/// A control plate's corner radius, on its leading edge only.
const PLATE_ROUNDING: f32 = 6.0;
/// How far a control plate stops short of the row's right edge, and how far it
/// is inset from the row's top and bottom.
const PLATE_RIGHT_INSET: f32 = 16.0;
const PLATE_INSET_Y: f32 = 3.0;

// A timed automatic recentre was tried and removed. From the driver's seat a
// panel that snaps back on its own — seconds after the last click, with no
// input to explain it — reads as the widget malfunctioning, not as a helpful
// default. Scrolling now stays exactly where it is put; the marker in the
// header says the view is off-centre, and press-in returns it.

/// Which page is showing.
///
/// The Relative comes first because it is what the panel shows by default and
/// what a driver spends a race looking at. The two pages a stop is armed from
/// follow it, in the order a stop is actually set up — fuel, then tyres — and
/// Strategy sits after them: it is the page you go to once, to decide what the
/// stop should be, rather than one you page through mid-lap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Relative,
    Fuel,
    Tires,
    Strategy,
    InCarAdjustments,
    Weather,
}

impl Page {
    pub const ALL: [Self; 6] =
        [Self::Relative, Self::Fuel, Self::Tires, Self::Strategy, Self::InCarAdjustments, Self::Weather];

    /// Where this page sits in [`Page::ALL`], which is what [`PageSet`] indexes
    /// by.
    fn ordinal(self) -> usize {
        Self::ALL.iter().position(|page| *page == self).unwrap_or(0)
    }

    /// The page's name on the tab rail.
    fn tab_label(self) -> &'static str {
        match self {
            Self::Relative => "RELATIVE",
            Self::Strategy => "PIT WINDOW",
            Self::Fuel => "FUEL",
            Self::Tires => "TIRES",
            Self::InCarAdjustments => "IN-CAR",
            Self::Weather => "WEATHER",
        }
    }

    /// Whether this page has anything honest to show from `seat`; see
    /// [`pages_for`].
    fn available_from(self, seat: &Seat, synced: Option<&crate::sync::store::SyncedCar>) -> bool {
        let spectating = matches!(seat, Seat::Spectating(_));
        match self {
            // The Relative follows whichever car is being watched, and the
            // weather is the session's.
            Self::Relative | Self::Weather => true,
            // The stop plan is about the player's own race, whoever is
            // driving it — and, while spectating, the team car's, once sync is
            // feeding its fuel. The traffic and pace it also reads come from
            // the followed car, which is present in every seat.
            Self::Strategy => !spectating || synced.is_some(),
            // The fuel level and the armed service are shared with the crew;
            // the tyres and the in-car controls are the driver's alone. While
            // spectating the local tank is empty, so each page only unlocks
            // when team sync is feeding the driver's real data — the Fuel page
            // whenever a car is synced, the Tyres page only once a stop has
            // put life on the wire. See `plans/team-sync.md` and
            // [`crate::sync::store::SyncedCar`].
            Self::Fuel => {
                matches!(seat, Seat::Driving | Seat::OutOfCar | Seat::TeamMate(_)) || (spectating && synced.is_some())
            }
            Self::Tires => {
                matches!(seat, Seat::Driving | Seat::OutOfCar)
                    || (spectating && synced.is_some_and(|car| car.tyres.is_some()))
            }
            Self::InCarAdjustments => matches!(seat, Seat::Driving | Seat::OutOfCar),
        }
    }

    /// The page `--demo-page` names, or `None` for a name that is not a page.
    ///
    /// Demo mode exists to hold a widget up against its mockup, and until this
    /// it could only ever show page one — every other page is reached by a
    /// wheel button, which is exactly what demo mode has no way to press.
    #[must_use]
    pub fn from_arg(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|page| page.arg_name() == name.to_ascii_lowercase())
    }

    /// How `--demo-page` spells this page.
    fn arg_name(self) -> &'static str {
        match self {
            Self::Relative => "relative",
            Self::Strategy => "strategy",
            Self::Fuel => "fuel",
            Self::Tires => "tires",
            Self::InCarAdjustments => "in-car",
            Self::Weather => "weather",
        }
    }

    /// The footer's note: what this page's controls apply to, or how far to
    /// trust its figures. `None` for a page with nothing to say.
    fn footer(self) -> Option<&'static str> {
        match self {
            // The Tires page's footer is built where its compound is known —
            // see [`draw`].
            Self::Fuel | Self::Tires => Some("Applies at the next stop"),
            // Said plainly, because iRacing's own page is a forecast and this
            // one cannot be: no forecast is published to read.
            Self::Weather => Some("Current conditions"),
            Self::InCarAdjustments => Some("Read-only"),
            // The pit window says how far to trust itself in its own workings
            // line, in its own terms.
            Self::Strategy | Self::Relative => None,
        }
    }
}

/// Which pages this session actually offers, as a set over [`Page::ALL`].
///
/// A set rather than a `Vec`, so working out what to page to costs no
/// allocation on a path that runs every frame.
///
/// Pages come and go with the session: Strategy appears once a race is seen to
/// need more than one stop, and the panel must not move underneath the driver
/// when it does. That is why the black box holds a page *identity* rather than
/// an index into a list whose length changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSet(u16);

impl PageSet {
    /// Whether `page` is on offer.
    #[must_use]
    pub fn contains(self, page: Page) -> bool {
        self.0 & (1 << page.ordinal()) != 0
    }

    /// Every page on offer, in [`Page::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Page> {
        Page::ALL.into_iter().filter(move |page| self.contains(*page))
    }

    /// The page `step` places along from `from`, wrapping at both ends.
    ///
    /// Falls back to the first page on offer when `from` is not itself on
    /// offer, which is how a driver parked on a page that has just gone away
    /// ends up somewhere sensible rather than nowhere.
    #[must_use]
    pub fn stepped(self, from: Page, step: i32) -> Page {
        let pages: Vec<Page> = self.iter().collect();
        let Some(first) = pages.first().copied() else { return Page::Relative };
        let Some(here) = pages.iter().position(|page| *page == from) else { return first };
        let len = i32::try_from(pages.len()).unwrap_or(1);
        let index = (i32::try_from(here).unwrap_or(0) + step).rem_euclid(len.max(1));
        pages.get(usize::try_from(index).unwrap_or(0)).copied().unwrap_or(first)
    }
}

/// Which pages the current session offers.
///
/// Every page, always, with one exception below. Strategy used to come and go
/// with the Standings widget's `endurance_mode` — hidden outside a race, and in
/// any race not yet seen to need two stops. That made the page a thing a driver
/// had to hunt for at the moment they needed it most, and a page whose presence
/// is itself a judgement is one nobody learns the position of. It is honest
/// about having no plan yet (see [`Verdict::Waiting`]), which is a better
/// answer than not being there; and in a sprint every fuel target resolves to
/// the same number of laps, so it reads sensibly there too.
///
/// The exception: pages read from player-only telemetry variables are
/// withdrawn from any seat those variables do not describe — see [`Seat`]
/// and [`Page::available_from`]. Spectating leaves only the Relative, which
/// follows the camera, and the Weather, which is the session's and belongs to
/// nobody: watching somebody else does not fill the fuel page with that car's
/// load, it leaves it reading a tank that is empty because nobody is in it,
/// and zeros that look like measurements are worse than a page that isn't
/// there. A team-mate's stint is gentler: the race is still the player's, so
/// the stop plan stays and the fuel stays as a readout, and only the pages
/// about the seat itself — tyres, in-car — step aside until the player is
/// back in it.
#[must_use]
pub fn pages_for(snapshot: Option<&TelemetrySnapshot>, synced: Option<&crate::sync::store::SyncedCar>) -> PageSet {
    let driving = Seat::Driving;
    let seat = snapshot.map_or(&driving, |s| &s.seat);
    let mut offered = 0_u16;
    for page in Page::ALL {
        if page.available_from(seat, synced) {
            offered |= 1 << page.ordinal();
        }
    }
    PageSet(offered)
}

/// What a row does when the cursor is on it and a value is stepped or toggled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Litres of fuel to add.
    Fuel,
    /// One wheel: a press arms or clears it, a turn sets its cold pressure.
    Tyre(crate::telemetry::pit::Corner),
    /// Tick or untick all four corners in one press.
    AllTyres,
    Tearoff,
    FastRepair,
    /// Ours, not the sim's: keep the fuel load topped to the finish. A press
    /// toggles it; a turn, while it is on, changes the laps of fuel it leaves
    /// in hand.
    AutoFuel,
    /// Ours too: a manual BOX BOX reminder that lights the status border. A
    /// press toggles it; it arms nothing on the sim. See [`BlackBox::box_called`].
    BoxBox,
    /// The shared fuel-per-lap target a spec sets for the driver to chase — a
    /// turn steps it, a press clears it. Applied by the app to team sync, not
    /// to the sim. See `plans/strategy-spec-mode.md`.
    FuelTarget,
    /// The standing tyre directive a spec sets — a press cycles the policy,
    /// a turn adjusts the wear threshold while on the wear policy. Applied by
    /// the app to team sync; the driver's overlay decides each stop against
    /// it. See `plans/strategy-spec-mode.md`.
    TyrePolicy,
}

/// The spec-side team-sync controls the Strategy page offers.
///
/// Both default to hidden; the app fills them in only while spectating, which
/// is the seat these calls are made from.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncControls {
    /// The fuel-target stepper's current value; `None` hides the control.
    pub fuel_target: Option<f32>,
    /// The standing tyre directive as the control shows it; `None` hides the
    /// control entirely.
    pub tyre_policy: Option<TyreDirective>,
}

/// What the Strategy page's tyre-policy control currently reads.
///
/// A named state rather than a nested `Option`: "no directive stands" and
/// "the control is not on offer" are different things, and only one of them
/// is a value the page can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TyreDirective {
    /// No standing directive — each stop is the driver's own call.
    DriversCall,
    /// The directive the team is running to.
    Set(crate::sync::protocol::TyrePolicy),
}

/// One line of a page.
#[derive(Debug, Clone)]
pub struct Row {
    pub label: String,
    pub kind: RowKind,
}

/// How a row presents itself, and whether it can be selected at all.
#[derive(Debug, Clone)]
pub enum RowKind {
    /// A value with no control — a reading, or a heading.
    Static { value: String },
    /// A checkbox.
    Toggle { checked: bool, control: Control },
    /// A number with `<` `>` arrows.
    Stepper { value: String, control: Control },
    /// One wheel of the car: whether it is being changed, and to what
    /// pressure. Both on one control, because a press and a turn are already
    /// separate actions — landing on a corner and pressing arms it, turning
    /// sets its pressure. Splitting them cost two cursor stops per corner,
    /// eight on the way into the pit lane.
    Corner { armed: bool, pressure_kpa: i16, control: Control },
}

impl RowKind {
    /// Whether the cursor can land here. Readings can't be changed, so
    /// stopping on one would be a dead end the driver has to click past.
    fn is_selectable(&self) -> bool {
        matches!(self, Self::Toggle { .. } | Self::Stepper { .. } | Self::Corner { .. })
    }

    #[must_use]
    pub fn control(&self) -> Option<Control> {
        match self {
            Self::Toggle { control, .. } | Self::Stepper { control, .. } | Self::Corner { control, .. } => {
                Some(*control)
            }
            Self::Static { .. } => None,
        }
    }
}

/// A page's controls in cursor order, and the shape they are laid out in.
///
/// The two are deliberately separate. An encoder has to walk *something* in a
/// fixed order however the page looks, so every page keeps a flat list of
/// controls and [`BlackBox::apply`] is the same code for all of them. What
/// changes per page is where those controls are drawn, and what furniture
/// surrounds them — because a page of settings and a page about the four
/// corners of a car are not the same instrument, and drawing both as a column
/// of `label: value` was what made every page read as a form.
#[derive(Debug, Clone)]
pub struct PageLayout {
    /// What the cursor walks and what a press acts on, in order.
    pub controls: Vec<Row>,
    pub shape: Shape,
}

impl PageLayout {
    /// Whether there is nothing on this page to draw at all.
    ///
    /// Not the same as having no controls, which is what this used to test.
    /// A page whose every figure is a reading — the pit window — has no
    /// controls by design and is not thereby empty; testing the control list
    /// left it showing "waiting for iRacing" forever, with a full projection
    /// sitting behind the placeholder.
    #[must_use]
    pub fn is_bare(&self) -> bool {
        match &self.shape {
            Shape::Rows | Shape::Corners { .. } | Shape::Fuel { .. } => self.controls.is_empty(),
            // Always worth drawing: with nothing measured it says so itself,
            // in the terms of the page rather than in the terms of the app.
            // Read-only pages have no controls by definition, and are not
            // thereby empty.
            Shape::PitWindow { .. } => false,
            Shape::Tiles(tiles) => tiles.is_empty(),
        }
    }
}

/// How a page arranges its controls.
#[derive(Debug, Clone)]
pub enum Shape {
    /// A column of rows with a cursor running down it — what a page is until
    /// it has earned something better.
    Rows,
    /// The four wheels in the car's own geometry, with the whole-set controls
    /// between the axles where the car's body would be.
    ///
    /// `controls` runs `[all four, LF, RF, LR, RR]`, so the cursor takes the
    /// set-wide control first and then reads across the axles.
    Corners {
        /// Which compound the next stop will fit; see
        /// [`PitService::pending_tyre_compound`].
        compound: Option<i32>,
        /// What came off each wheel at the last stop, in `Corner::ALL` order;
        /// `None` for a corner with no stop behind it yet.
        readouts: Box<[Option<TyreReadout>; 4]>,
        /// Which of those readings the three bars on each wheel show.
        bars: crate::config::TyreBars,
    },
    /// The tank as a tank, with everything this stop will do to it lit beneath.
    ///
    /// `controls` runs `[add, fuel, tearoff, fast repair, auto]` — the load
    /// first because it is the number the page exists to set, then the four
    /// things a stop either does or doesn't.
    /// A run of value-over-label tiles and nothing else, for a page that is
    /// entirely readings — see [`draw_tiles`].
    Tiles(Vec<Tile>),
    /// A window of laps the player could box on, one column each — see
    /// [`crate::telemetry::pit_window`].
    ///
    /// `controls` is empty: nothing here is set, it is all read. The page
    /// recommends and the driver boxes.
    PitWindow {
        window: Box<crate::telemetry::pit_window::PitWindow>,
        /// What to call each car, by `CarIdx`.
        ///
        /// The projection works in indexes, which are an implementation
        /// detail of the SDK — car index 0 is not car number 0, and printing
        /// one as the other names a car that isn't there. Resolved here, where
        /// the driver list is, rather than in the pure module.
        labels: std::collections::HashMap<i32, String>,
        /// Litres ahead of (positive) or behind (negative) the rate the
        /// recommended lap asks for, on the lap being driven now. `None` when
        /// no rate is being asked for, or when either half of the measurement
        /// is missing.
        this_lap_litres: Option<f32>,
        /// "save to X.XX L/lap to skip a stop", when a reachable saving would
        /// remove one — the Spa call surfaced automatically. `None` when the
        /// plan has no stop to skip, no reachable saving does it, or the
        /// inputs can't support an honest plan. See
        /// `telemetry::race_plan::burn_to_skip_a_stop`.
        skip_hint: Option<String>,
    },
    Fuel {
        gauge: FuelGauge,
        /// How many fast repairs are left, under the tile that arms one. Page
        /// furniture rather than a property of the toggle: it answers "can I
        /// still take one", which is a question about the session and not
        /// about the switch.
        fast_repairs: String,
        /// Auto Fuel's margin, in laps, while it is on; shown on its plate,
        /// where a turn of the rotary changes it.
        margin_laps: Option<f32>,
    },
}

/// One wheel's worth of what came off the car at the last stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TyreReadout {
    /// Carcass temperature across the tread, as the sim orders it: inner,
    /// middle, outer for a left-hand wheel and the reverse for a right.
    pub temps_c: [f32; 3],
    /// Tread remaining at each of those points, as a fraction.
    pub wear: [f32; 3],
    /// Hot pressure as the tyre came off, kPa.
    pub pressure_kpa: f32,
}

impl TyreReadout {
    /// This tyre's own mean temperature, which its three readings are judged
    /// against.
    ///
    /// Against itself rather than against a target, deliberately. A target
    /// band is a property of a car and a compound and a track temperature, and
    /// none of those is published — but *which edge of this tyre is hotter
    /// than the rest of it* needs none of them, and is the thing that actually
    /// points at camber and pressure.
    fn mean_temp_c(self) -> f32 {
        self.temps_c.iter().sum::<f32>() / 3.0
    }
}

/// A reading with a name under it, for a page with no controls at all.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub value: String,
    pub label: String,
    /// Where this reading points, in radians clockwise from the car's nose.
    ///
    /// Only wind has one. A bearing written as a number has to be turned back
    /// into a direction in the driver's head, every time; drawn in the car's
    /// own frame it is already the answer — the arrow points the way the wind
    /// is pushing, so a crosswind at the braking zone is a shape and not a
    /// sum.
    pub heading_rad: Option<f32>,
    /// A reading that is a split between two ends, as the fraction at the
    /// first: brake bias, front over rear. Drawn as a bar with a notch at the
    /// middle, because a split is a length and not a number.
    pub split: Option<f32>,
}

impl Tile {
    /// A plain reading.
    #[must_use]
    pub fn new(value: String, label: &str) -> Self {
        Self { value, label: label.to_owned(), heading_rad: None, split: None }
    }

    /// A reading that points somewhere — see [`Tile::heading_rad`].
    #[must_use]
    pub fn pointing(value: String, label: &str, heading_rad: Option<f32>) -> Self {
        Self { value, label: label.to_owned(), heading_rad, split: None }
    }

    /// A reading that is a split — see [`Tile::split`].
    #[must_use]
    pub fn split(value: String, label: &str, fraction: f32) -> Self {
        Self { value, label: label.to_owned(), heading_rad: None, split: Some(fraction) }
    }
}

/// What the tank holds, and what this stop will do to it.
///
/// Whether you are putting in enough is a comparison between two numbers, and
/// a comparison between two numbers is arithmetic done on the way into the pit
/// lane. Drawn as a tank it is the position of two edges instead.
#[derive(Debug, Clone, Copy)]
pub struct FuelGauge {
    /// Litres in the tank now.
    pub in_tank_litres: f32,
    /// Litres this stop is set to add.
    pub adding_litres: f32,
    /// Tank size. `None` while it can't be derived, which leaves the bar with
    /// no scale and so nothing honest to draw.
    pub capacity_litres: Option<f32>,
    /// Litres needed to reach the end at this car's measured consumption, for
    /// the mark on the bar. `None` until a lap has been driven under green.
    pub to_finish_litres: Option<f32>,
    /// How many laps the tank covers now, as it stands.
    pub laps_covered: Option<f32>,
    /// How many it covers once this stop has been served, while the stop adds
    /// anything; `None` with nothing to add, when it would only repeat the
    /// figure above.
    pub laps_after_stop: Option<f32>,
    /// How many laps of the race are left.
    pub laps_remaining: Option<i32>,
}

/// How far Auto Fuel has got with this visit to the pit lane.
///
/// Without this the rig gets armed twice. Auto Fuel arms the latched load
/// exactly while the car is on pit road, and iRacing reports a served stop by
/// unticking the fuel box — which reads identically to "the load is right but
/// nothing is set to go in", the case that arming exists for. So the moment
/// the crew finished, the same load was armed again, and a car still in its
/// box was fuelled a second time. That is a double fuel load: the tank
/// brimmed, or a stop's worth of time lost taking on fuel nobody asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PitLane {
    /// On track, or in the lane with the stop still to come.
    Pending,
    /// The rig has been and gone on this visit. Nothing more is asked for
    /// until the car is back on track, where the load is worked out afresh.
    Served,
}

/// A click on one of the black box's controls.
///
/// Collected while the panel is drawn and applied afterwards, beside the
/// wheel's actions, by [`BlackBox::click`]: a page for a tab on the rail, or
/// an action on one of the page's controls by its place in the cursor order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// A tab on the rail.
    Page(Page),
    /// A control, by its index in [`PageLayout::controls`], and what the
    /// click asks of it — a toggle on a plate, a step on a chevron.
    Control { index: usize, action: Action },
    /// A danger mark set or cleared from a Relative row's right-click menu —
    /// `None` clears it. Applied to the config by the app, not the black box.
    /// See `plans/danger-drivers.md`.
    Danger { cust_id: u32, level: Option<crate::config::DangerLevel> },
}

/// The panel's own state: which page, where the cursor is, and how far the
/// Relative is scrolled.
#[derive(Debug)]
pub struct BlackBox {
    /// Held as an identity, not an index — see [`PageSet`].
    page: Page,
    cursor: usize,
    /// Rows away from the player on the Relative page; negative looks ahead.
    scroll: i32,
    /// Litres Auto Fuel wants **on board** at the finish, worked out while the
    /// car was still on track — see [`BlackBox::auto_fuel_request`].
    ///
    /// A target, not a load to add: what has to go in depends on the tank at
    /// the moment the rig runs, which is a lap and a pit lane later.
    latched_target_litres: Option<f32>,
    /// That target turned into litres-to-add against the most recent tank
    /// reading, purely so the Fuel page has something to print. Never what is
    /// sent — see [`BlackBox::auto_fuel_request`].
    display_litres: Option<i16>,
    /// When Auto Fuel last handed out a request, so the next one cannot
    /// follow sooner than [`AUTO_FUEL_MIN_INTERVAL`].
    last_auto_fuel_at: Option<Instant>,
    /// See [`PitLane`].
    pit_lane: PitLane,
    /// Whether the fuel box has been seen ticked on this visit to the lane,
    /// which is what tells a served stop from one not yet set up.
    fuel_armed_in_lane: bool,
    /// Litres in the tank when this visit to the lane began, so a tank that
    /// has plainly been filled counts as served even if the checkbox doesn't
    /// say so.
    fuel_on_entry_litres: Option<f32>,
    /// Whether this visit to the lane has already had its Auto Fuel working
    /// printed — see [`BlackBox::report_auto_fuel`]. One line a stop, not one
    /// a frame.
    entry_reported: bool,
    /// Whether BOX BOX has been called by hand from the Strategy page — a
    /// reminder that lights the status border, arming nothing on the sim. A
    /// moment, not a setting, so it lives here and not in the config; cleared
    /// by toggling it off or by the stop being taken. See
    /// [`BlackBox::box_called`].
    box_called: bool,
}

impl Default for BlackBox {
    fn default() -> Self {
        Self::new()
    }
}

impl BlackBox {
    #[must_use]
    pub fn new() -> Self {
        Self::showing(Page::Relative)
    }

    /// A black box opened on `page`, for `--demo-page`.
    #[must_use]
    pub fn showing(page: Page) -> Self {
        Self {
            page,
            cursor: 0,
            scroll: 0,
            latched_target_litres: None,
            display_litres: None,
            last_auto_fuel_at: None,
            pit_lane: PitLane::Pending,
            fuel_armed_in_lane: false,
            fuel_on_entry_litres: None,
            entry_reported: false,
            box_called: false,
        }
    }

    /// Whether BOX BOX has been called by hand — for the status border.
    #[must_use]
    pub fn box_called(&self) -> bool {
        self.box_called
    }

    /// Drops a hand-called BOX BOX once the car is on pit road: the stop is
    /// being taken, so the reminder has done its job and would otherwise stay
    /// lit into the next stint.
    pub fn clear_box_call_on_pit_road(&mut self, on_pit_road: bool) {
        if on_pit_road {
            self.box_called = false;
        }
    }

    #[must_use]
    pub fn page(&self) -> Page {
        self.page
    }

    /// Brings the showing page back into `pages` when the session has stopped
    /// offering it, and reports what is showing now.
    ///
    /// Called once a frame before anything reads the page, so the rows drawn and
    /// the rows a press is applied against are always the same page's.
    pub fn settle_page(&mut self, pages: PageSet) -> Page {
        if !pages.contains(self.page) {
            self.page = pages.iter().next().unwrap_or(Page::Relative);
            self.cursor = 0;
        }
        self.page
    }

    /// Moves the scroll by `delta`, bounded both by the configured limit and
    /// by how far the field can actually be scrolled.
    ///
    /// Bounding against the field is what makes a press either move the panel
    /// or leave the count alone. Without it the number climbs against a view
    /// already pinned to the end of the list, and the presses needed to walk
    /// back appear to do nothing.
    fn scroll_by(&mut self, delta: i32, snapshot: Option<&TelemetrySnapshot>, relative: &RelativeConfig) {
        let ahead = usize::from(relative.ahead_count);
        let behind = usize::from(relative.behind_count);
        let (min, max) = snapshot.map_or((0, 0), |s| {
            crate::telemetry::relative::scroll_bounds(s.relative.len(), s.focus_index, ahead, behind)
        });
        let configured = i32::from(relative.scroll_limit);
        self.scroll = (self.scroll + delta).clamp(min.max(-configured), max.min(configured));
    }

    /// How far the Relative is scrolled from the player's own row.
    #[must_use]
    pub fn scroll(&self) -> i32 {
        self.scroll
    }

    /// Applies one action, returning any pit command it asks for.
    ///
    /// `rows` is the page as currently drawn, so a cursor never lands on a row
    /// that isn't there — the pages change shape with the car and the session.
    /// `pages` is what the session currently offers, which is what paging walks.
    pub fn apply(
        &mut self,
        action: Action,
        rows: &[Row],
        snapshot: Option<&TelemetrySnapshot>,
        settings: &mut crate::config::BlackBoxConfig,
        relative: &RelativeConfig,
        pages: PageSet,
    ) -> Option<PitRequest> {
        let on_relative = self.page() == Page::Relative;
        match action {
            Action::NextPage => {
                self.page = pages.stepped(self.page, 1);
                self.cursor = 0;
                return None;
            }
            Action::PrevPage => {
                self.page = pages.stepped(self.page, -1);
                self.cursor = 0;
                return None;
            }
            // Up and down mean "move the cursor" everywhere except the
            // Relative, which has no cursor and scrolls instead. Same gesture,
            // obvious thing in both places.
            Action::Prev if on_relative => {
                self.scroll_by(-1, snapshot, relative);
                return None;
            }
            Action::Next if on_relative => {
                self.scroll_by(1, snapshot, relative);
                return None;
            }
            // Nothing recentres the Relative: not a timer, and not a press.
            // Both were tried and both read as the panel moving on its own.
            // The view goes where it is scrolled and stays there.
            Action::Toggle if on_relative => return None,
            Action::Prev => self.move_cursor(rows, -1),
            Action::Next => self.move_cursor(rows, 1),
            Action::Increment | Action::Decrement | Action::Toggle => {}
        }

        let row = rows.get(self.cursor)?;
        let control = row.kind.control()?;
        // Auto Fuel and its margin are this app's own settings, not the sim's,
        // so they are applied here rather than becoming a pit command. The
        // load they imply is armed by `auto_fuel_request` on the next frame.
        match (control, action) {
            (Control::AutoFuel, Action::Toggle) => {
                settings.auto_fuel = !settings.auto_fuel;
                return None;
            }
            // The margin is the one setting on Auto Fuel, and it lives on the
            // same plate: a turn there changes it, while it is on. Off, a turn
            // does nothing, because the margin does nothing.
            (Control::AutoFuel, Action::Increment | Action::Decrement) => {
                if settings.auto_fuel {
                    let step =
                        if action == Action::Increment { pages::MARGIN_STEP_LAPS } else { -pages::MARGIN_STEP_LAPS };
                    settings.fuel_margin_laps = (settings.fuel_margin_laps + step).clamp(0.0, pages::MAX_MARGIN_LAPS);
                }
                return None;
            }
            // BOX BOX is a reminder, not a command: a press toggles the border
            // and sends nothing to the sim. A turn does nothing.
            (Control::BoxBox, Action::Toggle) => {
                self.box_called = !self.box_called;
                return None;
            }
            // Neither app-owned control ever becomes a pit command.
            (Control::AutoFuel | Control::BoxBox, _) => return None,
            _ => {}
        }
        let service = snapshot.map(|s| s.pit_service)?;
        pages::request_for(action, control, &row.kind, &service)
    }

    /// Applies a click, returning any pit command it asks for — see [`Click`].
    ///
    /// A click on a control lands the cursor on it first, so what happens next
    /// is exactly what the bind would have done with the cursor there. A tab
    /// for a page the session is not offering does nothing.
    pub fn click(
        &mut self,
        click: Click,
        rows: &[Row],
        snapshot: Option<&TelemetrySnapshot>,
        settings: &mut crate::config::BlackBoxConfig,
        relative: &RelativeConfig,
        pages: PageSet,
    ) -> Option<PitRequest> {
        match click {
            Click::Page(page) => {
                if pages.contains(page) && page != self.page {
                    self.page = page;
                    self.cursor = 0;
                }
                None
            }
            Click::Control { index, action } => {
                if !rows.get(index).is_some_and(|row| row.kind.is_selectable()) {
                    return None;
                }
                self.cursor = index;
                self.apply(action, rows, snapshot, settings, relative, pages)
            }
            // Danger marks are the app's to persist, not the black box's — see
            // `app`. Nothing to do here.
            Click::Danger { .. } => None,
        }
    }

    /// Moves the cursor to the next selectable row in `direction`, wrapping.
    fn move_cursor(&mut self, rows: &[Row], direction: isize) {
        let selectable: Vec<usize> =
            rows.iter().enumerate().filter(|(_, r)| r.kind.is_selectable()).map(|(i, _)| i).collect();
        if selectable.is_empty() {
            return;
        }
        let current = selectable.iter().position(|i| *i == self.cursor).unwrap_or(0);
        let next = if direction < 0 {
            (current + selectable.len() - 1) % selectable.len()
        } else {
            (current + 1) % selectable.len()
        };
        self.cursor = selectable[next];
    }

    /// The fuel load Auto Fuel wants armed, when that is worth a command.
    ///
    /// Returns `None` the rest of the time, which is what keeps this from
    /// sending a command every frame: the comparison is against iRacing's own
    /// reported figure, so once the sim has accepted the load there is nothing
    /// left to ask for.
    ///
    /// Commands are kept scarce deliberately, because each one is a
    /// machine-wide window message. Nothing is sent while the car is on track:
    /// the load is worked out there and latched, but only the pit lane arms it,
    /// and to the litre — see the body for why an on-track command changes
    /// nothing the stop does not change again. No two commands are issued
    /// closer together than [`AUTO_FUEL_MIN_INTERVAL`] either, which is what
    /// bounds the case where the sim declines to act — out of the car, service
    /// already under way — and would otherwise be asked again every frame.
    ///
    /// Once the car is on pit road the load stops being recalculated and the
    /// last one worked out on track stands. Running to a formula every frame
    /// meant the figure that actually went into the tank was the one computed
    /// while sitting in the box, from an in-lap's pace and an in-lap's fuel
    /// use — so a driver who watched it read 22 litres for a whole stint got
    /// 11 put in. Nothing about a race changes in the half minute between pit
    /// entry and the fuel rig, so there is nothing to recalculate from, and
    /// the pit lane is the worst place to be reading racing pace from anyway.
    #[must_use]
    pub fn auto_fuel_request(
        &mut self,
        snapshot: Option<&TelemetrySnapshot>,
        settings: &crate::config::BlackBoxConfig,
        now: Instant,
    ) -> Option<PitRequest> {
        if !settings.auto_fuel {
            self.latched_target_litres = None;
            self.last_auto_fuel_at = None;
            return None;
        }
        let snapshot = snapshot?;
        let service = snapshot.pit_service;
        if !service.in_car {
            return None;
        }
        // A stop that has already been served must not be re-armed while the
        // car is still in the lane — that is the double fuel load.
        self.track_pit_lane(&service);
        if self.pit_lane == PitLane::Served {
            return None;
        }
        let fresh = crate::telemetry::pit::fuel_to_finish_litres(
            snapshot.endurance.laps_remaining,
            snapshot.endurance.lap_driven_pct,
            service.fuel_per_lap_litres,
            settings.fuel_margin_laps,
        );
        if !service.on_pit_road {
            // Worked out every tick, because this is the only place racing pace
            // and racing fuel use can be read — but not *sent*. Nothing about a
            // load reaches the tank until the rig runs, and the pit lane re-arms
            // it to the litre below, so an on-track command changes nothing that
            // the stop itself does not change again. What it does do is put a
            // `SendNotifyMessage` to `HWND_BROADCAST` on the wire every couple of
            // seconds for the whole race — a window message delivered to every
            // top-level window on the machine, for a number nobody acts on yet.
            // The panel reads the latched figure straight from here instead.
            self.latched_target_litres = fresh;
            self.display_litres = fresh.map(|target| {
                crate::telemetry::pit::fuel_to_add_litres(
                    target,
                    service.fuel_level_litres,
                    service.tank_capacity_litres,
                )
            });
            return None;
        }
        // Said out loud once a stop. Every number Auto Fuel used is here, so a
        // load that came out wrong can be traced to the input that was wrong
        // rather than guessed at from the litres that went in.
        self.report_auto_fuel(snapshot, &service, settings, fresh);
        // Falls through to the fresh figure only when there is no latched one —
        // Auto Fuel switched on with the car already in the lane.
        //
        // Only the *target* comes from the lap the car drove in on. What has to
        // go in to reach it is worked out here, against the tank as it reads
        // now: the in-lap and the crawl down the lane are litres burnt since
        // that target was set, and a load latched before them arrives short by
        // exactly that much, every stop, in the direction that ends races.
        let target = self.latched_target_litres.or(fresh)?;
        let wanted =
            crate::telemetry::pit::fuel_to_add_litres(target, service.fuel_level_litres, service.tank_capacity_litres);
        self.display_litres = Some(wanted);
        // Exact, because this number is about to go into the tank.
        let request = fuel_request_for(wanted, &service)?;
        self.throttle_auto_fuel(request, now)
    }

    /// Follows the fuel rig through one visit to the pit lane.
    ///
    /// iRacing announces a served stop by unticking the fuel box, which reads
    /// exactly like "the right load is showing but nothing is set to go in" —
    /// the case arming exists for. The two are told apart by history rather
    /// than by the checkbox alone: a box that was ticked earlier on this same
    /// visit and is now clear has been acted on. A tank that has gained fuel
    /// since the car entered the lane says the same thing independently, and
    /// is trusted whatever the checkbox reports.
    ///
    /// Leaving pit road starts the next visit clean, so the load is worked
    /// out afresh from racing pace, as it is for the first stop.
    fn track_pit_lane(&mut self, service: &PitService) {
        if !service.on_pit_road {
            self.pit_lane = PitLane::Pending;
            self.fuel_armed_in_lane = false;
            self.fuel_on_entry_litres = None;
            self.entry_reported = false;
            return;
        }
        let on_entry = *self.fuel_on_entry_litres.get_or_insert(service.fuel_level_litres);
        let served = service.fuel_level_litres > on_entry + FUEL_SERVED_LITRES
            || (self.fuel_armed_in_lane && !service.fuel_armed);
        if service.fuel_armed {
            self.fuel_armed_in_lane = true;
        }
        if served {
            self.pit_lane = PitLane::Served;
            self.latched_target_litres = None;
        }
    }

    /// Prints, once per visit to the pit lane, every figure the fuel load was
    /// worked out from.
    ///
    /// A fuel load that comes out wrong has exactly five possible causes — the
    /// laps left, the measured consumption, the tank level, the tank's inferred
    /// capacity, or the margin — and after the stop there is no way to tell
    /// which. Each is a number the sim published or this app measured, so
    /// printing all five at the moment they are used turns "it under-fuelled
    /// me" into a line that says which one was wrong. `race-overlay.exe` run
    /// from a terminal writes these to it (see `attach_parent_console`).
    fn report_auto_fuel(
        &mut self,
        snapshot: &TelemetrySnapshot,
        service: &PitService,
        settings: &crate::config::BlackBoxConfig,
        fresh: Option<f32>,
    ) {
        if self.entry_reported {
            return;
        }
        self.entry_reported = true;
        let show = |value: Option<f32>| value.map_or_else(|| "?".to_owned(), |v| format!("{v:.1}"));
        println!(
            "note: auto fuel at pit entry — laps left {:?} (lap {} driven), {} l/lap, margin {:.1} laps, \
             tank {:.1}/{} l; target on board {} l (latched) / {} l (now)",
            snapshot.endurance.laps_remaining,
            show(snapshot.endurance.lap_driven_pct),
            show(service.fuel_per_lap_litres),
            settings.fuel_margin_laps,
            service.fuel_level_litres,
            show(service.tank_capacity_litres),
            show(self.latched_target_litres),
            show(fresh),
        );
    }

    /// Holds back any request issued less than [`AUTO_FUEL_MIN_INTERVAL`]
    /// after the last one. The first after a quiet spell goes straight out.
    fn throttle_auto_fuel(&mut self, request: PitRequest, now: Instant) -> Option<PitRequest> {
        if let Some(sent_at) = self.last_auto_fuel_at
            && now.duration_since(sent_at) < AUTO_FUEL_MIN_INTERVAL
        {
            return None;
        }
        self.last_auto_fuel_at = Some(now);
        Some(request)
    }

    /// The load Auto Fuel has worked out for the next stop, or `None` when it
    /// is off or has nothing to go on yet. See [`rows_for`].
    ///
    /// A display figure: it is what would go in if the stop happened at the
    /// tank reading this was last worked out against. The litres actually armed
    /// are recomputed in the lane against the live tank — see
    /// [`BlackBox::auto_fuel_request`].
    #[must_use]
    pub fn auto_fuel_litres(&self) -> Option<i16> {
        self.display_litres
    }

    /// Puts the cursor on the first selectable row if it isn't on one, which
    /// happens whenever a page changes shape underneath it.
    fn settle_cursor(&mut self, rows: &[Row]) {
        if rows.get(self.cursor).is_some_and(|r| r.kind.is_selectable()) {
            return;
        }
        self.cursor = rows.iter().position(|r| r.kind.is_selectable()).unwrap_or(0);
    }
}

/// The request that moves the sim from what it has armed to `wanted` litres,
/// or `None` when it already has exactly that, armed.
///
/// Asking for zero is not `SetFuel(0)`. The SDK reads a fuel amount of zero as
/// *keep the amount already set* — the same thing `Fuel(None)` means — so a
/// request for no fuel would arm the previous load instead of clearing it, and
/// the read-back would never move to match. That was a genuine lock-up: out of
/// the pits on a full tank, Auto Fuel wants nothing, the sim keeps reporting
/// the load it just served, and the two never agree, so the command went out
/// again on every frame. Unticking the box is what actually means no fuel, and
/// `fuel_armed` — not the amount, which the sim leaves standing — is what says
/// it took.
///
/// A load that matches but is not armed is still worth arming: iRacing unticks
/// the box once a stop is served, and leaving it that way would send the car
/// to its next stop with the right number showing and nothing set to go in.
#[must_use]
fn fuel_request_for(wanted: i16, service: &PitService) -> Option<PitRequest> {
    if wanted <= 0 {
        return service.fuel_armed.then_some(PitRequest::ClearFuel);
    }
    let armed = crate::telemetry::pit::round_litres(service.fuel_amount_litres);
    (!service.fuel_armed || wanted != armed).then_some(PitRequest::SetFuel(wanted))
}

/// Builds the current page: its controls in cursor order, and the shape they
/// are drawn in.
///
/// `auto_fuel_litres` is the load Auto Fuel has worked out but not yet sent —
/// see [`BlackBox::auto_fuel_request`]. The Fuel page shows that in preference
/// to what the sim has armed, because on track the two deliberately disagree:
/// the figure is only put on the wire at the pit entry, and a page showing the
/// sim's stale number instead would read as Auto Fuel having stopped working.
#[must_use]
pub fn layout_for(
    page: Page,
    snapshot: Option<&TelemetrySnapshot>,
    settings: &crate::config::BlackBoxConfig,
    auto_fuel_litres: Option<i16>,
    box_called: bool,
    synced: Option<&crate::sync::store::SyncedCar>,
    sync_controls: SyncControls,
) -> PageLayout {
    let rows = |controls: Vec<Row>| PageLayout { controls, shape: Shape::Rows };
    let Some(snapshot) = snapshot else {
        return rows(Vec::new());
    };
    match page {
        Page::Relative => rows(Vec::new()),
        Page::Strategy => pages::pit_window(snapshot, box_called, sync_controls),
        Page::Fuel => pages::fuel(snapshot, settings, auto_fuel_litres, synced),
        Page::Tires => pages::tires(snapshot, settings.tyre_bars),
        Page::InCarAdjustments => pages::in_car(snapshot),
        Page::Weather => pages::weather(snapshot),
    }
}

/// The single state the black box's status border shows.
///
/// One state at a time, resolved by priority — see [`resolve_status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackBoxStatus {
    /// Nothing worth a border; the widget looks as it always does.
    None,
    /// A full-course yellow or caution.
    Caution,
    /// The white flag — one lap to go.
    LastLap,
    /// Box this lap: the fuel call, or a manual one.
    Box,
    /// The chequered flag — the session is over.
    Finish,
}

impl BlackBoxStatus {
    /// The plate text and colour, or `None` for [`Self::None`].
    fn plate(self) -> Option<(&'static str, Color32)> {
        match self {
            Self::None => None,
            Self::Caution => Some(("CAUTION", CAUTION)),
            Self::LastLap => Some(("LAST LAP", Color32::WHITE)),
            Self::Box => Some(("BOX BOX", ALERT)),
            Self::Finish => Some(("FINISH", Color32::WHITE)),
        }
    }
}

/// Resolves the border state from the course flag and the two box triggers.
///
/// Priority, highest first: the chequered ends everything; a live box call is
/// the actionable one and outranks the flags behind it (boxing under a yellow
/// is exactly what you do); then the white and yellow flags. See
/// `plans/blackbox-status-border.md`.
#[must_use]
pub fn resolve_status(snapshot: Option<&TelemetrySnapshot>, box_called: bool) -> BlackBoxStatus {
    let Some(snapshot) = snapshot else {
        return BlackBoxStatus::None;
    };
    let box_now = box_called || snapshot.box_this_lap;
    match snapshot.course_flag {
        CourseFlag::Checkered => BlackBoxStatus::Finish,
        _ if box_now => BlackBoxStatus::Box,
        CourseFlag::White => BlackBoxStatus::LastLap,
        CourseFlag::Yellow => BlackBoxStatus::Caution,
        CourseFlag::Green => BlackBoxStatus::None,
    }
}

/// Metres from pit entry within which a live BOX BOX starts pulsing.
const BOX_PULSE_METRES: f32 = 400.0;

/// Paints the status border and its top-edge label plate around `content`.
///
/// A steady frame for a flag or a box call; the box call alone pulses, and
/// only inside [`BOX_PULSE_METRES`] of the stall, so the flash means "turn in
/// now". No-op for [`BlackBoxStatus::None`], which is the ordinary state.
///
/// The frame sits flush on the widget's own edge — no clear gap. On the
/// Relative, whose status gutter is inside the frame, `gutter_band` is a
/// paint slot reserved *before* the rows were drawn: the border's left limb
/// widens into a filled band across the gutter there, set into that slot so
/// the off-track and penalty markers draw on top of it — the border passes
/// under them rather than detouring around them.
fn paint_status_frame(
    ui: &Ui,
    metrics: Metrics,
    status: BlackBoxStatus,
    snapshot: Option<&TelemetrySnapshot>,
    gutter_band: Option<egui::layers::ShapeIdx>,
) {
    let Some((label, colour)) = status.plate() else {
        return;
    };

    // The pulse: a live box call inside the approach distance breathes between
    // dimmed and full. Everything else is steady. `request_repaint` keeps the
    // animation running while it pulses.
    let approaching = status == BlackBoxStatus::Box
        && snapshot.and_then(|snap| snap.metres_to_pit).is_some_and(|metres| metres <= BOX_PULSE_METRES);
    let colour = if approaching {
        ui.ctx().request_repaint();
        // ~1.4 Hz breathe between 45% and 100% alpha.
        let phase = (ui.input(|input| input.time) * std::f64::consts::TAU * 1.4).sin();
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "phase maps into 0..=255")]
        let alpha = (255.0 * (0.725 + 0.275 * phase)) as u8;
        colour.gamma_multiply(f32::from(alpha) / 255.0)
    } else {
        colour
    };

    let rect = ui.min_rect();
    let painter = ui.painter();
    let rounding = card_rounding(metrics);
    let stroke = Stroke::new(metrics.px(STATUS_BORDER_WIDTH), colour);
    // On the widget's own edge, so the frame reads as part of the box rather
    // than a halo floating a gap outside it.
    painter.rect_stroke(rect, rounding, stroke);

    if let Some(idx) = gutter_band {
        // The gutter's full run, from the frame's left edge to the card
        // beside it, filled and set into the reserved under-content slot.
        let band = Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.left() + metrics.px(super::relative::gutter_span()), rect.bottom()),
        );
        painter.set(idx, egui::Shape::rect_filled(band, super::card_rounding_side(metrics, true), colour));
    }

    // The label plate straddling the top edge, centred, straight-edged, the
    // state's colour filled with a dark ink so the word reads on it.
    let font = egui::FontId::proportional(metrics.px(STATUS_PLATE_TEXT));
    let galley = painter.layout_no_wrap(label.to_owned(), font, Color32::from_black_alpha(230));
    let pad = metrics.vec2(STATUS_PLATE_PAD_X, STATUS_PLATE_PAD_Y);
    let plate = Rect::from_center_size(egui::pos2(rect.center().x, rect.top()), galley.size() + pad * 2.0);
    painter.rect_filled(plate, rounding, colour);
    painter.galley(plate.center() - galley.size() / 2.0, galley, Color32::from_black_alpha(230));
}

/// The status border's stroke width, and the label plate's text size and
/// padding — all in design pixels, scaled.
const STATUS_BORDER_WIDTH: f32 = 3.0;
const STATUS_PLATE_TEXT: f32 = 15.0;
const STATUS_PLATE_PAD_X: f32 = 10.0;
const STATUS_PLATE_PAD_Y: f32 = 3.0;

/// Draws the black box, returning any clicks made on it.
///
/// `options` carries the switches the Relative's rows read — see [`super::RowOptions`].
pub fn draw(
    ui: &mut Ui,
    state: &mut BlackBox,
    snapshot: Option<&TelemetrySnapshot>,
    config: &RelativeConfig,
    layout: &PageLayout,
    options: super::RowOptions,
    synced: Option<&crate::sync::store::SyncedCar>,
) -> Vec<Click> {
    let page = state.page();
    let rail = Rail { pages: pages_for(snapshot, synced), current: page };
    let metrics = Metrics::new(config.scale);
    // The status border wraps whatever page is showing — the session's flag
    // and the player's box call are the widget's state, not a page's. See
    // `plans/blackbox-status-border.md`.
    let status = resolve_status(snapshot, state.box_called());
    let mut clicks = Vec::new();
    if page == Page::Relative {
        let scroll = state.scroll();
        // Reserved before the rows so the frame's gutter band can be painted
        // *under* the gutter's own markers — see `paint_status_frame`.
        let gutter_band = ui.painter().add(egui::Shape::Noop);
        super::relative::draw(ui, snapshot, config, scroll, rail, &mut clicks, options);
        paint_status_frame(ui, metrics, status, snapshot, Some(gutter_band));
        return clicks;
    }

    let controls = &layout.controls;
    state.settle_cursor(controls);
    let cursor = state.cursor;
    card_frame(metrics, PANEL_BG, margin(metrics, 0.0, 0.0), card_rounding(metrics)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.set_width(metrics.px(super::relative::outer_width(config)));
            draw_rail(ui, metrics, rail, &mut clicks);
            if layout.is_bare() {
                draw_placeholder(ui, metrics);
                draw_footer(ui, metrics, page.footer());
                return;
            }
            match &layout.shape {
                Shape::Rows => draw_rows(ui, metrics, controls, cursor),
                Shape::Corners { readouts, bars, .. } => {
                    draw_corners(ui, metrics, controls, cursor, readouts, *bars, &mut clicks);
                }
                Shape::Tiles(tiles) => draw_tiles(ui, metrics, tiles),
                Shape::PitWindow { window, this_lap_litres, labels, skip_hint } => {
                    draw_pit_window(ui, metrics, window, *this_lap_litres, labels, skip_hint.as_deref());
                    // The manual BOX BOX toggle sits under the window as an
                    // ordinary control row, so the wheel walks onto it.
                    draw_rows(ui, metrics, controls, cursor);
                }
                Shape::Fuel { gauge, fast_repairs, margin_laps } => {
                    draw_fuel(ui, metrics, controls, cursor, *gauge, fast_repairs, *margin_laps, &mut clicks);
                }
            }
            // The compound rides on the Tires footer: it is a fact about the
            // next stop the sim's own black box set, and it belongs with the
            // note saying when the page applies.
            let footer = match &layout.shape {
                Shape::Corners { compound, readouts, .. } => {
                    let mut parts = Vec::new();
                    if let Some(compound) = compound {
                        parts.push(compound_name(*compound));
                    }
                    parts.push("applies at the next stop".to_owned());
                    parts.push(
                        if readouts.iter().any(Option::is_some) {
                            "tread from the last stop"
                        } else {
                            "no stop yet this session"
                        }
                        .to_owned(),
                    );
                    Some(parts.join(" \u{00B7} "))
                }
                _ if page == Page::Weather => Some(weather_footer(snapshot)),
                _ => page.footer().map(str::to_owned),
            };
            draw_footer(ui, metrics, footer.as_deref());
        });
    });
    paint_status_frame(ui, metrics, status, snapshot, None);
    clicks
}

/// Makes `rect` a click target, recording `click` if it was clicked.
///
/// Returns whether the pointer is over it, so the caller can lift the control
/// a step: the mouse wants an affordance the wheel never needed.
fn hit(ui: &Ui, rect: Rect, id: impl std::hash::Hash, clicks: &mut Vec<Click>, click: Click) -> bool {
    let response = ui.interact(rect, ui.id().with(("black-box", id)), egui::Sense::click());
    if response.clicked() {
        clicks.push(click);
    }
    response.hovered()
}

/// The lift a hovered control gets: a wash of paper over it.
fn paint_hover(ui: &Ui, rect: Rect, rounding: f32) {
    ui.painter().rect_filled(rect, rounding, Color32::from_white_alpha(HOVER_ALPHA));
}

/// How much paper a hovered control is washed with.
const HOVER_ALPHA: u8 = 18;

/// What the tab rail shows: every page on offer, and which one is lit.
#[derive(Debug, Clone, Copy)]
pub struct Rail {
    pages: PageSet,
    current: Page,
}

/// The tab rail: the pages on offer, in the order the wheel walks them, the
/// current one lit.
///
/// A wheel button pressed at speed needs its answer on screen before the
/// press, not after, and the rail is that answer: a driver can count how
/// many presses to Fuel. Its tabs are blocks in the class-tag geometry — the
/// lit one paper with ink text, like the gutter's `PIT` chip; the rest ink
/// with quiet text.
pub fn draw_rail(ui: &mut Ui, metrics: Metrics, rail: Rail, clicks: &mut Vec<Click>) {
    let (band, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(RAIL_HEIGHT)), egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(band);
    let card = metrics.px(super::CARD_ROUNDING);
    painter.rect_filled(
        band,
        egui::Rounding { nw: card, ne: card, ..egui::Rounding::ZERO },
        Color32::from_black_alpha(90),
    );

    let tabs: Vec<Page> = rail.pages.iter().collect();
    if tabs.is_empty() {
        return;
    }
    let inner = band.shrink2(metrics.vec2(RAIL_PAD, 0.0));
    #[expect(clippy::cast_precision_loss, reason = "seven pages at most")]
    let step = inner.width() / tabs.len() as f32;
    let middle = band.center().y;
    for (slot, page) in tabs.into_iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "see above")]
        let left = inner.left() + step * slot as f32;
        let tab = Rect::from_min_max(
            egui::pos2(left + metrics.px(RAIL_TAB_GAP / 2.0), middle - metrics.px(RAIL_TAB_HEIGHT / 2.0)),
            egui::pos2(left + step - metrics.px(RAIL_TAB_GAP / 2.0), middle + metrics.px(RAIL_TAB_HEIGHT / 2.0)),
        );
        let lit = page == rail.current;
        let hovered = !lit && hit(ui, tab, ("tab", page.tab_label()), clicks, Click::Page(page));
        let (tab_fill, ink) = if lit {
            (text_primary(), Color32::from_black_alpha(230))
        } else if hovered {
            (Color32::from_white_alpha(14 + HOVER_ALPHA), text_secondary())
        } else {
            (Color32::from_white_alpha(14), text_tertiary())
        };
        painter.rect_filled(tab, metrics.px(super::BLOCK_ROUNDING), tab_fill);
        let label = RichText::new(page.tab_label()).size(metrics.px(RAIL_LABEL_SIZE)).strong().color(ink);
        let galley = egui::WidgetText::from(label).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
        let at = egui::Align2::CENTER_CENTER.anchor_size(tab.center(), galley.size());
        painter.galley(at.min, galley, Color32::WHITE);
    }
}

/// The Weather page's note: "current conditions" as always — this page
/// cannot be a forecast, since none is published — plus whatever a wet
/// session adds: the sim's track-wetness estimate, the steward's wet-tyre
/// declaration, and whether our own car is on wets. All three vanish in a
/// dry session, leaving the note exactly as it was.
fn weather_footer(snapshot: Option<&TelemetrySnapshot>) -> String {
    let mut parts = vec!["Current conditions".to_owned()];
    if let Some(weather) = snapshot.map(|s| s.weather) {
        if let Some(wetness) = weather.track_wetness.filter(|wetness| wetness.is_wet()) {
            parts.push(format!("track {}", wetness.label()));
        }
        if weather.declared_wet {
            parts.push("wet tyres allowed".to_owned());
        }
        if weather.on_wet_tyres {
            parts.push("on wets".to_owned());
        }
    }
    parts.join(" \u{00B7} ")
}

/// The footer under a page: its note, right-aligned, on a band that takes
/// the card's bottom corners. Nothing at all for a page with nothing to say.
fn draw_footer(ui: &mut Ui, metrics: Metrics, note: Option<&str>) {
    let Some(note) = note else { return };
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(FOOTER_HEIGHT)), egui::Sense::hover());
    let card = metrics.px(super::CARD_ROUNDING);
    ui.painter().rect_filled(
        rect,
        egui::Rounding { sw: card, se: card, ..egui::Rounding::ZERO },
        Color32::from_black_alpha(50),
    );
    paint_text(
        ui,
        egui::pos2(rect.right() - metrics.px(14.0), rect.center().y),
        egui::Align2::RIGHT_CENTER,
        RichText::new(note).size(metrics.px(FOOTER_SIZE)).color(text_tertiary()),
    );
}

/// The compound's name: the sim's own two where they are known, numbered
/// otherwise, because a car with three compounds would otherwise have two of
/// them called the same thing.
fn compound_name(compound: i32) -> String {
    match compound {
        0 => "DRY".to_owned(),
        1 => "WET".to_owned(),
        other => format!("C{other}"),
    }
}

/// A column of rows with the cursor running down it.
fn draw_rows(ui: &mut Ui, metrics: Metrics, rows: &[Row], cursor: usize) {
    // No trailing space: the last row's fill runs to the card's own bottom
    // edge and takes its corners, so the run reads as one instrument face
    // rather than as a list floating in a box.
    for (index, row) in rows.iter().enumerate() {
        let style = RowStyle { selected: index == cursor, odd: index % 2 == 1, seat: RowPlace::of(index, rows.len()) };
        draw_row(ui, metrics, row, style);
    }
}

/// A window of laps to box on: the recommendation, then the laps it was picked
/// from.
///
/// The recommendation is the hero because on the way to the pit entry there is
/// time to read one thing: the lap, in a position plate at pit-board size —
/// the same plate a Standings position sits on — with its reasons beside it
/// rather than under it, so the hero is one row tall. The strip under it is
/// what makes that recommendation checkable rather than merely obeyed — a
/// driver who disagrees can see what boxing a lap either side would cost, in
/// traffic and in fuel.
///
/// With no window to show — no stop needed, a caution voiding every gap, or
/// no measured lap yet — the card is the hero row and the workings and
/// nothing else. A page is as tall as what it has to say.
fn draw_pit_window(
    ui: &mut Ui,
    metrics: Metrics,
    window: &crate::telemetry::pit_window::PitWindow,
    this_lap_litres: Option<f32>,
    labels: &std::collections::HashMap<i32, String>,
    skip_hint: Option<&str>,
) {
    use crate::telemetry::pit_window::Confidence;

    let recommended = window.recommended_lap.and_then(|lap| window.candidates.iter().find(|c| c.lap == lap));
    let has_strip = recommended.is_some() && window.confidence != Confidence::Void && !window.finishes_without_stopping;
    let height = VERDICT_HEIGHT + if has_strip { WINDOW_STRIP_HEIGHT } else { 0.0 } + WORKINGS_HEIGHT;
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(height)), egui::Sense::hover());
    let inner = rect.shrink2(metrics.vec2(FUEL_SIDE_MARGIN, 0.0));

    let (plate_text, lines) = match (window.confidence, recommended) {
        (Confidence::Void, _) => (
            "VOID".to_owned(),
            vec![
                "a caution rewrites every gap in the race".to_owned(),
                "nothing projected through one is worth showing".to_owned(),
            ],
        ),
        _ if window.finishes_without_stopping => (
            "NO STOP".to_owned(),
            vec![window.fuel_window_last_lap.map_or_else(
                || "the tank already reaches the end".to_owned(),
                |lap| format!("the tank reaches lap {lap}"),
            )],
        ),
        (_, Some(best)) => (format!("L{}", best.lap), window_detail(window, best, labels)),
        (_, None) => ("\u{2014}".to_owned(), vec!["needs one racing lap and a field with pace".to_owned()]),
    };

    // The hero row: BOX, the plate, the reasons.
    let middle = rect.top() + metrics.px(VERDICT_HEIGHT / 2.0);
    let word = RichText::new("BOX").size(metrics.px(VERDICT_SUB_SIZE + 1.0)).strong().color(text_secondary());
    let word_width = text_width(ui, word.clone());
    paint_text(ui, egui::pos2(inner.left(), middle), egui::Align2::LEFT_CENTER, word);

    let value = super::readout(plate_text, metrics.px(VERDICT_SIZE)).color(Color32::from_black_alpha(230));
    let plate_width = text_width(ui, value.clone()) + metrics.px(VERDICT_PLATE_PAD * 2.0);
    let plate = Rect::from_center_size(
        egui::pos2(inner.left() + word_width + metrics.px(12.0) + plate_width / 2.0, middle),
        egui::vec2(plate_width, metrics.px(VERDICT_PLATE_HEIGHT)),
    );
    ui.painter().rect_filled(plate, metrics.px(super::BLOCK_ROUNDING), text_primary());
    paint_text(ui, plate.center(), egui::Align2::CENTER_CENTER, value);

    let lines: Vec<&String> = lines.iter().filter(|line| !line.is_empty()).collect();
    #[expect(clippy::cast_precision_loss, reason = "two lines at most")]
    let first_y = middle - metrics.px(VERDICT_SUB_HEIGHT / 2.0) * (lines.len() as f32 - 1.0);
    for (index, line) in lines.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "see above")]
        let y = first_y + metrics.px(VERDICT_SUB_HEIGHT) * index as f32;
        paint_text(
            ui,
            egui::pos2(plate.right() + metrics.px(16.0), y),
            egui::Align2::LEFT_CENTER,
            RichText::new(line.as_str()).size(metrics.px(VERDICT_SUB_SIZE)).color(text_secondary()),
        );
    }

    // And whether the rate it asks for is being held, right now.
    if let Some(litres) = this_lap_litres {
        let (note, color) =
            if litres >= 0.0 { ("on target", theme::signal()) } else { ("using too much", theme::alert()) };
        paint_text(
            ui,
            egui::pos2(inner.right(), middle),
            egui::Align2::RIGHT_CENTER,
            RichText::new(format!("{litres:+.2} L\n{note}")).size(metrics.px(TYRE_LABEL_SIZE)).strong().color(color),
        );
    }

    if has_strip {
        let strip = Rect::from_min_max(
            egui::pos2(inner.left(), rect.top() + metrics.px(VERDICT_HEIGHT)),
            egui::pos2(inner.right(), rect.bottom() - metrics.px(WORKINGS_HEIGHT)),
        );
        draw_window_strip(ui, metrics, strip, window);
    }

    // What the projection could not see, said plainly: silent truncation reads
    // as "considered everything".
    let mut notes = vec![format!("confidence {}", window.confidence.label())];
    if window.cars_skipped > 0 {
        notes.push(format!("{} cars with no lap time left out", window.cars_skipped));
    }
    if has_strip && let Some(last) = window.fuel_window_last_lap {
        notes.push(format!("tank reaches lap {last}"));
    }
    // The stop worth skipping, first among the notes: it is the one that can
    // change the whole plan rather than qualify it.
    if let Some(hint) = skip_hint {
        notes.insert(0, hint.to_owned());
    }
    paint_text(
        ui,
        egui::pos2(inner.right(), rect.bottom() - metrics.px(WORKINGS_HEIGHT / 2.0)),
        egui::Align2::RIGHT_CENTER,
        RichText::new(notes.join("   \u{00B7}   ")).size(metrics.px(WORKINGS_SIZE)).color(text_tertiary()),
    );
}

/// The two lines beside the recommendation.
fn window_detail(
    window: &crate::telemetry::pit_window::PitWindow,
    best: &crate::telemetry::pit_window::Candidate,
    labels: &std::collections::HashMap<i32, String>,
) -> Vec<String> {
    let position = best.exit_class_position.map_or_else(String::new, |p| format!("out P{p}"));
    let name = |idx: i32| labels.get(&idx).cloned().unwrap_or_else(|| "a car".to_owned());
    let between = match best.emerge_between {
        (Some(ahead), Some(behind)) => format!("between {} and {}", name(ahead), name(behind)),
        (Some(ahead), None) => format!("behind {}", name(ahead)),
        (None, Some(behind)) => format!("ahead of {}", name(behind)),
        (None, None) => "into clear air".to_owned(),
    };
    let first = best.first_conflict.map_or_else(
        || "nobody within reach for three laps".to_owned(),
        |c| {
            let verb = if c.pace_delta_secs > 0.0 { "catches you" } else { "you catch" };
            format!("{} {verb} in {:.1} laps", name(c.car_idx), c.laps_until.max(0.0))
        },
    );
    let head: Vec<String> = [position, between].into_iter().filter(|s| !s.is_empty()).collect();
    // How far off the plan the pick is, which is the thing actually being
    // decided: "a lap or two early" is a question about the plan, not about now.
    let plan = window.fuel_window_last_lap.map_or_else(String::new, |plan| match best.lap - plan {
        0 => " \u{2014} on plan".to_owned(),
        early if early < 0 => format!(" \u{2014} {} lap{} early", -early, if early == -1 { "" } else { "s" }),
        late => format!(" \u{2014} {} lap{} late", late, if late == 1 { "" } else { "s" }),
    });
    vec![format!("{}{plan}", head.join(", ")), first]
}

/// One column per candidate lap: how much traffic, and what fuel it asks for.
///
/// The recommended lap's number sits on the same plate as the hero, at
/// strip size; the last lap the tank reaches is a solid alert rule after its
/// column, so where the free choice ends is drawn rather than inferred from
/// the save row going to a cross.
fn draw_window_strip(ui: &Ui, metrics: Metrics, rect: Rect, window: &crate::telemetry::pit_window::PitWindow) {
    if window.candidates.is_empty() {
        return;
    }
    let worst = window.candidates.iter().filter_map(|c| c.traffic_score).fold(0.0_f32, f32::max);
    #[expect(clippy::cast_precision_loss, reason = "a handful of candidates, far inside f32's exact range")]
    let step = rect.width() / window.candidates.len() as f32;
    let bars_top = rect.top() + metrics.px(WINDOW_LABEL_HEIGHT + WINDOW_STOPS_HEIGHT);
    let bars_bottom = rect.bottom() - metrics.px(WINDOW_SAVE_HEIGHT);
    let rounding = metrics.px(WINDOW_BAR_ROUNDING);

    for (slot, candidate) in window.candidates.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "see above")]
        let centre = rect.left() + step * (slot as f32 + 0.5);
        let recommended = window.recommended_lap == Some(candidate.lap);

        let label_y = rect.top() + metrics.px(WINDOW_LABEL_HEIGHT / 2.0);
        let lap = super::readout(candidate.lap.to_string(), metrics.px(WINDOW_LAP_SIZE));
        if recommended {
            let width = text_width(ui, lap.clone()) + metrics.px(WINDOW_PICK_PAD * 2.0);
            let pick = Rect::from_center_size(
                egui::pos2(centre, label_y),
                egui::vec2(width, metrics.px(WINDOW_LABEL_HEIGHT - 2.0)),
            );
            ui.painter().rect_filled(pick, metrics.px(super::BLOCK_ROUNDING), text_primary());
            paint_text(ui, pick.center(), egui::Align2::CENTER_CENTER, lap.color(Color32::from_black_alpha(230)));
        } else {
            paint_text(ui, egui::pos2(centre, label_y), egui::Align2::CENTER_CENTER, lap.color(text_tertiary()));
        }

        // Stops first, because it is the consequence that dominates: an extra
        // stop is a minute, and no amount of clear air buys one back. Red
        // marks any lap costing more than the best on offer.
        if let Some(stops) = candidate.stops_to_finish {
            let costs_extra = window.min_stops.is_some_and(|min| stops > min);
            paint_text(
                ui,
                egui::pos2(centre, rect.top() + metrics.px(WINDOW_LABEL_HEIGHT + WINDOW_STOPS_HEIGHT / 2.0)),
                egui::Align2::CENTER_CENTER,
                RichText::new(format!("{stops}")).size(metrics.px(WINDOW_STOPS_SIZE)).strong().color(if costs_extra {
                    theme::alert()
                } else {
                    text_primary()
                }),
            );
        }

        // Tall is bad, which is the way round a driver reads a chart of trouble
        // without being told which way round it is.
        let column = Rect::from_min_max(
            egui::pos2(centre - metrics.px(WINDOW_BAR_WIDTH / 2.0), bars_top),
            egui::pos2(centre + metrics.px(WINDOW_BAR_WIDTH / 2.0), bars_bottom),
        );
        ui.painter().rect_filled(column, rounding, super::CONTROL_PLATE);
        if let Some(score) = candidate.traffic_score {
            let filled = if worst > 0.0 { (score / worst).clamp(0.0, 1.0) } else { 0.0 };
            let bar =
                Rect::from_min_max(egui::pos2(column.left(), column.bottom() - column.height() * filled), column.max);
            // Green only on the lap actually recommended, so nothing but the
            // pick reads as an endorsement.
            ui.painter().rect_filled(bar, rounding, if recommended { theme::signal() } else { text_secondary() });
        }

        // The second question, per lap: a dash is free, a figure is the rate to
        // drive to, a cross is past what lifting and coasting can reach.
        let (save, save_color) = if !candidate.save.reachable {
            ("\u{2717}".to_owned(), theme::alert())
        } else if candidate.save.is_free() {
            ("\u{2014}".to_owned(), text_tertiary())
        } else {
            (format!("{:.2}", candidate.save.per_lap_litres), text_primary())
        };
        paint_text(
            ui,
            egui::pos2(centre, rect.bottom() - metrics.px(WINDOW_SAVE_HEIGHT / 2.0)),
            egui::Align2::CENTER_CENTER,
            RichText::new(save).size(metrics.px(TYRE_LABEL_SIZE)).strong().color(save_color),
        );
    }

    // Where the tank runs out: a rule after the last lap it reaches, when
    // that lap is inside the window and not its last column.
    if let Some(last) = window.fuel_window_last_lap
        && let Some(slot) = window.candidates.iter().position(|c| c.lap == last)
        && slot + 1 < window.candidates.len()
    {
        #[expect(clippy::cast_precision_loss, reason = "see above")]
        let x = rect.left() + step * (slot as f32 + 1.0);
        ui.painter().rect_filled(
            Rect::from_min_max(
                egui::pos2(x - metrics.px(1.5), rect.top() + metrics.px(WINDOW_LABEL_HEIGHT)),
                egui::pos2(x + metrics.px(1.5), bars_bottom),
            ),
            0.0,
            theme::alert(),
        );
    }
}

/// The tank as a tank: what is in it, what this stop adds, where the finish
/// sits — and the four things a stop either does or doesn't, lit beneath.
///
/// The tank is the hero. Solid paper is the fuel on board; hatch is what the
/// stop will add — planned, not yet real — and a shortfall against the
/// finish is an alert-red hatch with the litres it is short written inside.
/// Both quantities are written inside their own segments, so the number and
/// the length are one object. Whether you are putting in enough is then a
/// matter of where two edges sit, not of subtracting two numbers on the way
/// into the pit lane.
///
/// `controls` runs `[add, fuel, tearoff, fast repair, auto]` — see
/// [`Shape::Fuel`]. `margin_laps` is Auto Fuel's margin while it is on, which
/// rides on the `AUTO` plate rather than taking a row of its own.
#[expect(clippy::too_many_arguments, reason = "one page, drawn in one place; a struct would only rename the list")]
fn draw_fuel(
    ui: &mut Ui,
    metrics: Metrics,
    controls: &[Row],
    cursor: usize,
    gauge: FuelGauge,
    fast_repairs: &str,
    margin_laps: Option<f32>,
    clicks: &mut Vec<Click>,
) {
    let height =
        FUEL_TOP_PAD + FUEL_HEAD_HEIGHT + TANK_HEIGHT + FUEL_LEGEND_HEIGHT + FUEL_FIGURES_HEIGHT + ARM_STRIP_HEIGHT;
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(height)), egui::Sense::hover());
    let inner = rect.shrink2(metrics.vec2(FUEL_SIDE_MARGIN, 0.0));
    let band = |top: f32, height: f32| {
        Rect::from_min_max(egui::pos2(inner.left(), top), egui::pos2(inner.right(), top + metrics.px(height)))
    };

    let mut top = rect.top() + metrics.px(FUEL_TOP_PAD);
    let head = band(top, FUEL_HEAD_HEIGHT);
    top += metrics.px(FUEL_HEAD_HEIGHT);
    let lane = band(top, TANK_HEIGHT);
    top += metrics.px(TANK_HEIGHT);
    let legend = band(top, FUEL_LEGEND_HEIGHT);
    top += metrics.px(FUEL_LEGEND_HEIGHT);
    let figures = band(top, FUEL_FIGURES_HEIGHT);
    top += metrics.px(FUEL_FIGURES_HEIGHT);
    let strip = band(top, ARM_STRIP_HEIGHT);

    // The load is the tank: a stepper while the driver sets it, a readout
    // while Auto Fuel does. Only a stepper gets chevrons and the cursor.
    let steppable = matches!(controls.first().map(|row| &row.kind), Some(RowKind::Stepper { .. }));
    let selected = steppable && cursor == 0;
    paint_text(
        ui,
        egui::pos2(head.left(), head.center().y),
        egui::Align2::LEFT_CENTER,
        RichText::new("ADD").size(metrics.px(TYRE_LABEL_SIZE)).strong().color(if selected {
            text_primary()
        } else {
            text_secondary()
        }),
    );
    let tank = lane.shrink2(metrics.vec2(TANK_CHEVRON_LANE, 0.0));
    let shortfall = draw_tank(ui, metrics, tank, head, gauge);
    if steppable {
        draw_tank_chevrons(ui, metrics, lane, selected, clicks);
    }
    if selected {
        paint_cursor_ring(ui, metrics, tank, metrics.px(TANK_ROUNDING));
    }

    draw_fuel_legend(ui, metrics, legend, shortfall);
    draw_fuel_figures(ui, metrics, figures, gauge);

    // The whole stop as a row of lit blocks. On an in-lap this is the question
    // the page is actually being asked — what have I armed? — and it is
    // answered by a pattern rather than by four ticks at four heights of a
    // list.
    let tiles: Vec<&Row> = controls.iter().skip(1).take(ARM_TILES).collect();
    #[expect(clippy::cast_precision_loss, reason = "four tiles, far inside f32's exact range")]
    let step = (strip.width() + metrics.px(ARM_TILE_GAP)) / tiles.len().max(1) as f32;
    for (slot, row) in tiles.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "see above")]
        let left = strip.left() + step * slot as f32;
        let tile = Rect::from_min_max(
            egui::pos2(left, strip.top() + metrics.px(ARM_TILE_GAP / 2.0)),
            egui::pos2(left + step - metrics.px(ARM_TILE_GAP), strip.bottom() - metrics.px(ARM_TILE_GAP / 2.0)),
        );
        let lit = matches!(row.kind, RowKind::Toggle { checked, .. } if checked);
        // Two tiles carry a note: fast repair its count, because its
        // availability is a number rather than a yes; Auto Fuel its margin,
        // because that is the one setting on it and the plate is where a turn
        // of the rotary changes it.
        let margin = margin_laps.map(|laps| format!("+{laps:.1} lap margin"));
        let note = match row.label.as_str() {
            "Fast Repair" => Some(fast_repairs),
            "Auto" => margin.as_deref(),
            _ => None,
        };
        let index = slot + 1;
        draw_arm_tile(ui, metrics, tile, &row.label, note, lit, cursor == index);
        // The plate toggles; on the AUTO plate, while it is on, its two ends
        // step the margin — the same split a turn of the rotary makes.
        let stepping = row.label == "Auto" && margin.is_some();
        let end = metrics.px(ARM_TILE_STEP_WIDTH);
        let body = if stepping { tile.shrink2(egui::vec2(end, 0.0)) } else { tile };
        if hit(ui, body, ("arm", slot), clicks, Click::Control { index, action: Action::Toggle }) {
            paint_hover(ui, tile, metrics.px(ARM_TILE_ROUNDING));
        }
        if stepping {
            for (side, name, action) in [
                (
                    Rect::from_min_max(tile.min, egui::pos2(tile.left() + end, tile.bottom())),
                    "chevron-left",
                    Action::Decrement,
                ),
                (
                    Rect::from_min_max(egui::pos2(tile.right() - end, tile.top()), tile.max),
                    "chevron-right",
                    Action::Increment,
                ),
            ] {
                let hovered = hit(ui, side, ("margin", name), clicks, Click::Control { index, action });
                if hovered {
                    paint_hover(ui, side, metrics.px(ARM_TILE_ROUNDING));
                }
                let glyph = Rect::from_center_size(side.center(), metrics.vec2(CHEVRON_SIZE, CHEVRON_SIZE));
                super::icons::svg(ui, glyph, name, Color32::from_black_alpha(if hovered { 230 } else { 120 }));
            }
        }
    }
}

/// How much of an `AUTO` plate's width, at each end, steps the margin.
const ARM_TILE_STEP_WIDTH: f32 = 28.0;

/// The stepper's chevrons, in the lanes either side of the tank.
///
/// The whole lane beside the tank is the click target, not the glyph.
fn draw_tank_chevrons(ui: &Ui, metrics: Metrics, lane: Rect, selected: bool, clicks: &mut Vec<Click>) {
    let middle = lane.center().y;
    for (x, name, action) in [
        (lane.left() + metrics.px(TANK_CHEVRON_LANE / 2.0), "chevron-left", Action::Decrement),
        (lane.right() - metrics.px(TANK_CHEVRON_LANE / 2.0), "chevron-right", Action::Increment),
    ] {
        // The whole lane beside the tank is the target, not the glyph.
        let target = Rect::from_center_size(egui::pos2(x, middle), metrics.vec2(TANK_CHEVRON_LANE, TANK_HEIGHT));
        let hovered = hit(ui, target, ("fuel", name), clicks, Click::Control { index: 0, action });
        let arrows = if selected || hovered { text_primary() } else { text_secondary() };
        let arrow = Rect::from_center_size(egui::pos2(x, middle), metrics.vec2(CHEVRON_SIZE, CHEVRON_SIZE));
        if !super::icons::svg(ui, arrow, name, arrows) {
            let glyph = if name == "chevron-left" { "\u{2039}" } else { "\u{203A}" };
            paint_text(
                ui,
                egui::pos2(x, middle),
                egui::Align2::CENTER_CENTER,
                RichText::new(glyph).size(metrics.px(ROW_SIZE + 4.0)).strong().color(arrows),
            );
        }
    }
}

/// The olive ring the cursor puts round the control it is on.
///
/// One colour for the cursor across the overlay, and it is the one that
/// already means "you" — see [`PLAYER_ROW`].
fn paint_cursor_ring(ui: &Ui, metrics: Metrics, rect: Rect, rounding: f32) {
    ui.painter().rect_stroke(
        rect.expand(metrics.px(3.0)),
        rounding + metrics.px(3.0),
        Stroke::new(metrics.px(2.0), PLAYER_ROW),
    );
}

/// The tank: what is in it, what is going in, and where the finish sits.
///
/// Returns the litres the tank falls short of the finish by, once this stop
/// has gone in, or `None` where it reaches it or nothing is known.
///
/// A finish that needs more than the tank pins the flag to the bar's far end
/// with an arrow rather than vanishing, so a two-stop race still says how far
/// short a full tank falls. With no capacity there is no scale, and a bar
/// with no scale is a picture of nothing: the trough says so and the figures
/// underneath still say what they know.
fn draw_tank(ui: &Ui, metrics: Metrics, tank: Rect, head: Rect, gauge: FuelGauge) -> Option<f32> {
    let rounding = metrics.px(TANK_ROUNDING);
    ui.painter().rect_filled(tank, rounding, super::CONTROL_PLATE);

    let Some(capacity) = gauge.capacity_litres.filter(|litres| *litres > 0.0) else {
        paint_text(
            ui,
            tank.center(),
            egui::Align2::CENTER_CENTER,
            RichText::new("no tank size yet").size(metrics.px(TYRE_LABEL_SIZE)).color(text_tertiary()),
        );
        return None;
    };
    let at = |litres: f32| tank.left() + tank.width() * (litres / capacity).clamp(0.0, 1.0);
    let held_to = at(gauge.in_tank_litres);
    let added_to = at(gauge.in_tank_litres + gauge.adding_litres);

    // What is in the tank now: solid paper, taking the trough's rounded left
    // end, and its right end too if it fills the tank.
    let on_board = Rect::from_min_max(tank.min, egui::pos2(held_to, tank.max.y));
    let held_rounding = if held_to >= tank.right() - 0.5 {
        egui::Rounding::same(rounding)
    } else {
        egui::Rounding { nw: rounding, sw: rounding, ne: 0.0, se: 0.0 }
    };
    if on_board.width() > 0.0 {
        ui.painter().rect_filled(on_board, held_rounding, text_primary());
    }

    // What this stop adds: hatched, because it is planned rather than real.
    // The hatch is clipped square, so it stops short of the trough's rounded
    // far end rather than poking out of it.
    let hatch_right = added_to.min(tank.right() - rounding);
    if gauge.adding_litres > 0.0 && hatch_right > held_to {
        let added = Rect::from_min_max(egui::pos2(held_to, tank.top()), egui::pos2(hatch_right, tank.bottom()));
        super::paint_hatch(ui, metrics, added, Color32::from_white_alpha(115));
    }

    // Where the race ends, and — if the fill stops short of it — how short.
    let needed = gauge.to_finish_litres.filter(|litres| *litres > 0.0);
    let shortfall =
        needed.map(|litres| litres - (gauge.in_tank_litres + gauge.adding_litres)).filter(|short| *short > 0.0);
    if let Some(needed) = needed {
        let beyond_the_tank = needed > capacity;
        let x = if beyond_the_tank { tank.right() } else { at(needed) };
        if let Some(short) = shortfall {
            // The gap between the fill and the flag, in the alert colour and
            // hatched: it is the one planned quantity on the page nobody
            // wants, and its length is the whole argument.
            let gap = Rect::from_min_max(
                egui::pos2(added_to, tank.top()),
                egui::pos2(x.min(tank.right() - rounding), tank.bottom()),
            );
            if gap.width() > 0.0 {
                super::paint_hatch(ui, metrics, gap, super::tint(theme::alert(), 200));
            }
            let text = super::readout(format!("-{short:.0} L"), metrics.px(TANK_VALUE_SIZE)).color(theme::alert());
            paint_inside(ui, metrics, gap, text);
        }
        let label = if beyond_the_tank { "FINISH \u{25B8}" } else { "FINISH" };
        let tab_text =
            RichText::new(label).size(metrics.px(FINISH_TAB_SIZE)).strong().color(Color32::from_black_alpha(230));
        let tab_width = text_width(ui, tab_text.clone()) + metrics.px(FINISH_TAB_PAD * 2.0);
        let centre = x.clamp(tank.left() + tab_width / 2.0, tank.right() - tab_width / 2.0);
        let tab = Rect::from_min_max(
            egui::pos2(centre - tab_width / 2.0, head.bottom() - metrics.px(FINISH_TAB_HEIGHT)),
            egui::pos2(centre + tab_width / 2.0, head.bottom()),
        );
        ui.painter().rect_filled(tab, metrics.px(super::BLOCK_ROUNDING), text_primary());
        paint_text(ui, tab.center(), egui::Align2::CENTER_CENTER, tab_text);
        ui.painter().rect_filled(
            Rect::from_min_max(
                egui::pos2(x - metrics.px(FINISH_STEM / 2.0), tab.bottom()),
                egui::pos2(x + metrics.px(FINISH_STEM / 2.0), tank.bottom()),
            ),
            0.0,
            text_primary(),
        );
    }

    // The figures, inside the segments they describe. Ink on the paper, paper
    // on the hatch; a segment too short for its number simply goes without —
    // the readouts below carry the laps either way.
    paint_inside(
        ui,
        metrics,
        on_board,
        super::readout(format!("{:.0} L", gauge.in_tank_litres), metrics.px(TANK_VALUE_SIZE))
            .color(Color32::from_black_alpha(230)),
    );
    if gauge.adding_litres > 0.0 {
        let added = Rect::from_min_max(egui::pos2(held_to, tank.top()), egui::pos2(added_to, tank.bottom()));
        paint_inside(
            ui,
            metrics,
            added,
            super::readout(format!("+{:.0} L", gauge.adding_litres), metrics.px(TANK_VALUE_SIZE)).color(text_primary()),
        );
    }
    shortfall
}

/// Paints `text` at the left of `segment`, if the segment is wide enough to
/// hold it with a pad either side.
fn paint_inside(ui: &Ui, metrics: Metrics, segment: Rect, text: RichText) {
    let width = text_width(ui, text.clone());
    if segment.width() < width + metrics.px(TANK_VALUE_PAD * 2.0) {
        return;
    }
    paint_text(
        ui,
        egui::pos2(segment.left() + metrics.px(TANK_VALUE_PAD), segment.center().y),
        egui::Align2::LEFT_CENTER,
        text,
    );
}

/// What the tank's two textures mean, said once under it — and a third, in
/// the alert colour, only while there is a shortfall to explain.
fn draw_fuel_legend(ui: &Ui, metrics: Metrics, rect: Rect, shortfall: Option<f32>) {
    let middle = rect.center().y;
    let mut x = rect.left();
    let mut item = |swatch: Option<Color32>, hatched: bool, label: &str, color: Color32| {
        let swatch_rect = Rect::from_center_size(
            egui::pos2(x + metrics.px(FUEL_LEGEND_SWATCH.0 / 2.0), middle),
            metrics.vec2(FUEL_LEGEND_SWATCH.0, FUEL_LEGEND_SWATCH.1),
        );
        if let Some(fill) = swatch {
            ui.painter().rect_filled(swatch_rect, metrics.px(2.0), fill);
        }
        if hatched {
            ui.painter().rect_filled(swatch_rect, metrics.px(2.0), super::CONTROL_PLATE);
            super::paint_hatch(ui, metrics, swatch_rect, color);
        }
        x = swatch_rect.right() + metrics.px(6.0);
        let text = RichText::new(label).size(metrics.px(FOOTER_SIZE)).color(text_tertiary());
        let width = text_width(ui, text.clone());
        paint_text(ui, egui::pos2(x, middle), egui::Align2::LEFT_CENTER, text);
        x += width + metrics.px(20.0);
    };
    item(Some(text_primary()), false, "in the tank", text_primary());
    item(None, true, "this stop adds", Color32::from_white_alpha(115));
    if shortfall.is_some() {
        item(None, true, "short of the finish", super::tint(theme::alert(), 200));
    }
}

/// Laps of fuel in the tank now, laps left, and the difference — the three
/// figures a driver wants from a fuel page, as readouts rather than a
/// sentence.
///
/// The spare is measured after the stop while one is being set up, because
/// that is the question the load answers; with nothing to add it is the tank
/// as it stands. Laps of fuel is always the tank now — with Auto Fuel on, the
/// after-stop figure is the laps left plus the margin by construction, and
/// reads as if the panel had copied the race's own countdown.
fn draw_fuel_figures(ui: &Ui, metrics: Metrics, rect: Rect, gauge: FuelGauge) {
    let dash = "\u{2014}";
    let after_stop = gauge.laps_after_stop.is_some();
    let spare = gauge.laps_after_stop.or(gauge.laps_covered).zip(gauge.laps_remaining).map(|(covered, left)| {
        #[expect(clippy::cast_precision_loss, reason = "a lap count is far inside f32's exact-integer range")]
        let left = left as f32;
        covered - left
    });
    let (spare_text, spare_label, spare_color) = match (spare, after_stop) {
        (Some(laps), true) if laps >= 0.0 => (format!("+{laps:.0}"), "SPARE AFTER STOP", theme::signal()),
        (Some(laps), true) => (format!("{laps:.0}"), "SHORT AFTER STOP", theme::alert()),
        (Some(laps), false) if laps >= 0.0 => (format!("+{laps:.0}"), "SPARE", theme::signal()),
        (Some(laps), false) => (format!("{laps:.0}"), "SHORT", theme::alert()),
        (None, _) => (dash.to_owned(), "SPARE", text_secondary()),
    };
    let figures = [
        (
            gauge.laps_covered.map_or_else(|| dash.to_owned(), |laps| format!("{laps:.1}")),
            "LAPS OF FUEL",
            text_primary(),
        ),
        (gauge.laps_remaining.map_or_else(|| dash.to_owned(), |laps| laps.to_string()), "LAPS LEFT", text_primary()),
        (spare_text, spare_label, spare_color),
    ];
    let value_y = rect.top() + metrics.px(FUEL_FIGURE_SIZE / 2.0 + 4.0);
    let label_y = rect.bottom() - metrics.px(TYRE_LABEL_SIZE / 2.0 + 6.0);
    let mut x = rect.left();
    for (value, label, color) in figures {
        let value_text = super::readout(value, metrics.px(FUEL_FIGURE_SIZE)).color(color);
        let label_text = RichText::new(label).size(metrics.px(TYRE_LABEL_SIZE)).strong().color(text_tertiary());
        let width = text_width(ui, value_text.clone()).max(text_width(ui, label_text.clone()));
        paint_text(ui, egui::pos2(x, value_y), egui::Align2::LEFT_CENTER, value_text);
        paint_text(ui, egui::pos2(x, label_y), egui::Align2::LEFT_CENTER, label_text);
        x += width + metrics.px(FUEL_FIGURE_GAP);
    }
}

/// One thing a stop either does or doesn't, as a plate that is lit or isn't.
fn draw_arm_tile(ui: &Ui, metrics: Metrics, rect: Rect, label: &str, note: Option<&str>, lit: bool, selected: bool) {
    let rounding = metrics.px(ARM_TILE_ROUNDING);
    let (fill, text, quiet) = if lit {
        (theme::caution(), Color32::from_black_alpha(230), Color32::from_black_alpha(150))
    } else {
        (super::CONTROL_PLATE, text_secondary(), text_tertiary())
    };
    ui.painter().rect_filled(rect, rounding, fill);
    // The label sits centred with no note, and lifts to make room for one.
    let raise = if note.is_some() { metrics.px(ARM_TILE_NOTE_LIFT) } else { 0.0 };
    paint_text(
        ui,
        egui::pos2(rect.center().x, rect.center().y - raise),
        egui::Align2::CENTER_CENTER,
        RichText::new(label.to_uppercase()).size(metrics.px(ARM_TILE_TEXT_SIZE)).strong().color(text),
    );
    if let Some(note) = note {
        paint_text(
            ui,
            egui::pos2(rect.center().x, rect.bottom() - metrics.px(ARM_TILE_NOTE_Y)),
            egui::Align2::CENTER_CENTER,
            RichText::new(note).size(metrics.px(ARM_TILE_NOTE_SIZE)).color(quiet),
        );
    }
    if selected {
        paint_cursor_ring(ui, metrics, rect, rounding);
    }
}

/// A small arrow in the car's own frame: nose up, `heading_rad` clockwise.
///
/// Deliberately not a compass. A ring of N/E/S/W answers "which way am I
/// facing", which is a question nobody asks mid-corner; the arrow answers
/// "which way is the air pushing me", which is the one that changes a braking
/// point.
pub fn draw_heading_arrow(ui: &Ui, centre: egui::Pos2, reach: f32, heading_rad: f32, color: Color32) {
    let (sin, cos) = heading_rad.sin_cos();
    // Screen y grows downward, so a clockwise bearing rotates as written here.
    let point = |along: f32, across: f32| {
        egui::pos2(centre.x + across * cos + along * sin, centre.y + across * sin - along * cos)
    };
    let tip = point(reach, 0.0);
    let left = point(-reach * 0.55, -reach * 0.62);
    let right = point(-reach * 0.55, reach * 0.62);
    let tail = point(-reach * 0.2, 0.0);
    ui.painter().add(egui::Shape::convex_polygon(vec![tip, right, tail, left], color, Stroke::NONE));
}

/// A run of readings, each with its name under it.
///
/// No plates and no gutter: a plate on this panel means "you can change this",
/// and nothing on a page of readings can be. It ends up the quietest page in
/// the set, which is right — nothing on it can be acted on from here.
///
/// Two tiles are twice the width of the rest: one that is a split, drawn as a
/// bar with a notch at the middle (brake bias), and one that points somewhere,
/// drawn with its arrow beside its figure (wind).
fn draw_tiles(ui: &mut Ui, metrics: Metrics, tiles: &[Tile]) {
    if tiles.is_empty() {
        return;
    }
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(TILE_STRIP_HEIGHT)), egui::Sense::hover());
    let inner = rect.shrink2(metrics.vec2(FUEL_SIDE_MARGIN, TILE_STRIP_PAD));
    let weight = |tile: &Tile| if tile.split.is_some() || tile.heading_rad.is_some() { 2.0 } else { 1.0 };
    let total: f32 = tiles.iter().map(weight).sum();
    #[expect(clippy::cast_precision_loss, reason = "a handful of tiles, far inside f32's exact range")]
    let gaps = metrics.px(TILE_GAP) * (tiles.len() as f32 - 1.0);
    let unit = (inner.width() - gaps) / total.max(1.0);

    let mut left = inner.left();
    for tile in tiles {
        let cell =
            Rect::from_min_max(egui::pos2(left, inner.top()), egui::pos2(left + unit * weight(tile), inner.bottom()));
        left = cell.right() + metrics.px(TILE_GAP);
        ui.painter().rect_filled(cell, metrics.px(TILE_ROUNDING), super::TILE_BG);
        let label = RichText::new(&tile.label).size(metrics.px(TILE_LABEL_SIZE)).strong().color(text_tertiary());

        if let Some(fraction) = tile.split {
            // A split, drawn as one: the number, then a bar with a notch at
            // the middle and the fill to the front. It is the one value on
            // this page a driver changes mid-lap, and a length can be seen
            // moving where a number cannot.
            let pad = metrics.px(TILE_PAD_X);
            paint_text(
                ui,
                egui::pos2(cell.left() + pad, cell.top() + metrics.px(TILE_VALUE_Y)),
                egui::Align2::LEFT_CENTER,
                super::readout(&tile.value, metrics.px(TILE_VALUE_SIZE)).color(text_primary()),
            );
            let bar = Rect::from_min_max(
                egui::pos2(cell.left() + pad, cell.top() + metrics.px(TILE_SPLIT_Y)),
                egui::pos2(cell.right() - pad, cell.top() + metrics.px(TILE_SPLIT_Y + TILE_SPLIT_HEIGHT)),
            );
            ui.painter().rect_filled(bar, metrics.px(2.0), Color32::from_white_alpha(30));
            let fill = Rect::from_min_max(
                bar.min,
                egui::pos2(bar.left() + bar.width() * fraction.clamp(0.0, 1.0), bar.bottom()),
            );
            ui.painter().rect_filled(fill, metrics.px(2.0), text_secondary());
            let notch = Rect::from_center_size(
                egui::pos2(bar.center().x, bar.center().y),
                egui::vec2(metrics.px(2.0), bar.height() + metrics.px(4.0)),
            );
            ui.painter().rect_filled(notch, 0.0, text_primary());
            let foot = cell.bottom() - metrics.px(TILE_LABEL_Y);
            paint_text(ui, egui::pos2(cell.left() + pad, foot), egui::Align2::LEFT_CENTER, label);
            paint_text(
                ui,
                egui::pos2(cell.right() - pad, foot),
                egui::Align2::RIGHT_CENTER,
                RichText::new("FRONT \u{2192}").size(metrics.px(TILE_LABEL_SIZE)).color(text_tertiary()),
            );
            continue;
        }

        let value = super::readout(&tile.value, metrics.px(TILE_VALUE_SIZE)).color(text_primary());
        if let Some(heading) = tile.heading_rad {
            // The arrow beside the figure, at a size that earns the tile: it
            // is the page's one spatial cue.
            let value_width = text_width(ui, value.clone());
            let arrow = metrics.px(TILE_ARROW_SIZE);
            let span = arrow + metrics.px(12.0) + value_width;
            let start = cell.center().x - span / 2.0;
            let y = cell.top() + metrics.px(TILE_VALUE_Y);
            draw_heading_arrow(ui, egui::pos2(start + arrow / 2.0, y), arrow / 2.0, heading, super::WIND);
            paint_text(ui, egui::pos2(start + arrow + metrics.px(12.0), y), egui::Align2::LEFT_CENTER, value);
        } else {
            paint_text(
                ui,
                egui::pos2(cell.center().x, cell.top() + metrics.px(TILE_VALUE_Y)),
                egui::Align2::CENTER_CENTER,
                value,
            );
        }
        paint_text(
            ui,
            egui::pos2(cell.center().x, cell.bottom() - metrics.px(TILE_LABEL_Y)),
            egui::Align2::CENTER_CENTER,
            label,
        );
    }
}

/// The four wheels where the car's wheels are, on the car, with the
/// whole-set control on its body.
///
/// A list of four is a list you have to read the labels of. Four wheels in the
/// car's own geometry are read as a shape: "fronts only" is the top pair lit,
/// which is taken in at a glance and needs no words at all. `controls` runs
/// `[all four, LF, RF, LR, RR]` — see [`Shape::Corners`].
///
/// Each wheel carries what came off it at the last stop under what goes on
/// at the next, because the one is how the other gets decided: a hot outer
/// edge on the right front and the pressure that corner is set to are one
/// fact about one corner, and they used to be two pages apart.
fn draw_corners(
    ui: &mut Ui,
    metrics: Metrics,
    controls: &[Row],
    cursor: usize,
    readouts: &[Option<TyreReadout>; 4],
    bars: crate::config::TyreBars,
    clicks: &mut Vec<Click>,
) {
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(GRID_HEIGHT)), egui::Sense::hover());

    let column = |x: f32| rect.left() + x;
    let columns = [column(metrics.px(TYRE_COLUMN_INSET)), rect.right() - metrics.px(TYRE_COLUMN_INSET)];
    let axles = [rect.top() + metrics.px(FRONT_AXLE_Y), rect.top() + metrics.px(REAR_AXLE_Y)];

    draw_body(ui, metrics, columns, axles);

    // `Corner::ALL` is LF, RF, LR, RR — across the front axle, then across the
    // rear — which is the order these four fall into reading left to right and
    // top to bottom.
    for (index, row) in controls.iter().enumerate().skip(1) {
        let wheel = index - 1;
        let Some(&x) = columns.get(wheel % 2) else { continue };
        let Some(&y) = axles.get(wheel / 2) else { continue };
        if let RowKind::Corner { armed, pressure_kpa, .. } = row.kind {
            let tyre = Rect::from_center_size(egui::pos2(x, y), metrics.vec2(TYRE_SIZE.0, TYRE_SIZE.1));
            let readout = readouts.get(wheel).copied().flatten();
            draw_tyre(ui, metrics, tyre, Wheel { armed, pressure_kpa, readout, bars, selected: index == cursor });
            // The wheel toggles; the two ends of its pressure line step it.
            let end = metrics.px(TYRE_STEP_WIDTH);
            let line = Rect::from_min_max(tyre.min, egui::pos2(tyre.right(), tyre.top() + metrics.px(TYRE_BARS_TOP)));
            let body = Rect::from_min_max(egui::pos2(tyre.left(), line.bottom()), tyre.max)
                .union(line.shrink2(egui::vec2(end, 0.0)));
            if hit(ui, body, ("tyre", wheel), clicks, Click::Control { index, action: Action::Toggle }) {
                paint_hover(ui, tyre, metrics.px(TYRE_ROUNDING));
            }
            for (side, name, action) in [
                (
                    Rect::from_min_max(line.min, egui::pos2(line.left() + end, line.bottom())),
                    "chevron-left",
                    Action::Decrement,
                ),
                (
                    Rect::from_min_max(egui::pos2(line.right() - end, line.top()), line.max),
                    "chevron-right",
                    Action::Increment,
                ),
            ] {
                let hovered = hit(ui, side, ("pressure", wheel, name), clicks, Click::Control { index, action });
                if hovered {
                    paint_hover(ui, side, metrics.px(TYRE_ROUNDING));
                }
                let glyph = Rect::from_center_size(side.center(), metrics.vec2(CHEVRON_SIZE, CHEVRON_SIZE));
                let ink = if armed {
                    Color32::from_black_alpha(if hovered { 230 } else { 110 })
                } else if hovered {
                    text_primary()
                } else {
                    text_tertiary()
                };
                super::icons::svg(ui, glyph, name, ink);
            }
        }
    }

    // The car's body: what the wheels are attached to, and the natural home
    // for the control that is about the set rather than about a corner.
    let body = f32::midpoint(rect.left(), rect.right());
    if let Some(all) = controls.first() {
        let checked = matches!(all.kind, RowKind::Toggle { checked, .. } if checked);
        let centre = egui::pos2(body, f32::midpoint(axles[0], axles[1]));
        draw_all_four(ui, metrics, centre, &all.label, checked, cursor == 0);
        let plate = Rect::from_center_size(centre, metrics.vec2(ALL_FOUR_SIZE.0, ALL_FOUR_SIZE.1));
        if hit(ui, plate, "all-four", clicks, Click::Control { index: 0, action: Action::Toggle }) {
            paint_hover(ui, plate, metrics.px(PLATE_ROUNDING));
        }
    }
}

/// How much of a wheel's pressure line, at each end, steps the pressure.
const TYRE_STEP_WIDTH: f32 = 34.0;

/// Everything one wheel is drawn from — see [`draw_tyre`].
#[derive(Debug, Clone, Copy)]
struct Wheel {
    armed: bool,
    pressure_kpa: i16,
    readout: Option<TyreReadout>,
    bars: crate::config::TyreBars,
    selected: bool,
}

/// The car the wheels are on: two axles and a spine, drawn first and under
/// them, so they read as what the wheels are attached to.
///
/// Each axle runs between the two wheel *centres*, so both its ends
/// disappear under a tyre rather than stopping in mid-air — which is what
/// makes it read as an axle going into a hub. The spine joins the axles and
/// passes behind the whole-set control, giving that control somewhere to
/// belong: it is the one on the car rather than on a corner.
///
/// Nothing here encloses anything. That is the whole point — see
/// [`AXLE_THICKNESS`].
fn draw_body(ui: &Ui, metrics: Metrics, columns: [f32; 2], axles: [f32; 2]) {
    let ink = Color32::from_white_alpha(CHASSIS_ALPHA);
    let half_axle = metrics.px(AXLE_THICKNESS) / 2.0;
    for y in axles {
        let axle = Rect::from_min_max(egui::pos2(columns[0], y - half_axle), egui::pos2(columns[1], y + half_axle));
        ui.painter().rect_filled(axle, half_axle, ink);
    }
    let centre = f32::midpoint(columns[0], columns[1]);
    let half_spine = metrics.px(SPINE_WIDTH) / 2.0;
    let spine =
        Rect::from_min_max(egui::pos2(centre - half_spine, axles[0]), egui::pos2(centre + half_spine, axles[1]));
    ui.painter().rect_filled(spine, metrics.px(SPINE_ROUNDING), ink);
}

/// One wheel, seen from above: lit when it is being changed, with the pressure
/// it will be set to at the top and what came off it at the last stop below.
///
/// The pressure lives *in* the wheel rather than beside it because they are one
/// fact about one corner, and a driver reading "159" three inches from a tick
/// has to work out which corner it belongs to.
///
/// The tread is three bars, because three numbers have to be read and
/// compared while three bars of different heights are one shape: a hot outer
/// edge on the right front is a red bar at the top right of the car. The
/// bars carry the colour and the numbers stay quiet. Wear is one fill along
/// the wheel — its lowest point, since the three were never different enough
/// to read — and the hot pressure sits under it.
///
/// Carries no corner name. Which wheel this is, is where it is — a grid whose
/// whole argument is that position holds the meaning does not then write the
/// meaning on each tile.
#[expect(clippy::too_many_lines, reason = "one wheel, drawn top to bottom in one place")]
fn draw_tyre(ui: &Ui, metrics: Metrics, rect: Rect, wheel: Wheel) {
    let Wheel { armed, pressure_kpa, readout, bars, selected } = wheel;
    let rounding = metrics.px(TYRE_ROUNDING);
    // Amber is this panel's one loud color and it means armed — so a full set
    // of tyres for the next stop is four lit wheels, and anything less is a
    // shape with a hole in it.
    let (fill, quiet, loud) = if armed {
        (theme::caution(), Color32::from_black_alpha(160), Color32::from_black_alpha(230))
    } else {
        (super::CONTROL_PLATE, text_tertiary(), text_primary())
    };
    ui.painter().rect_filled(rect, rounding, fill);

    // Grooves down the tread, which is what makes this read as a tyre rather
    // than as a rounded rectangle with a number in it.
    let groove = Color32::from_black_alpha(if armed { 45 } else { 95 });
    for fraction in TREAD_GROOVES {
        let x = rect.left() + rect.width() * fraction;
        ui.painter().rect_filled(
            Rect::from_min_max(
                egui::pos2(x, rect.top() + rounding),
                egui::pos2(x + metrics.px(2.0), rect.bottom() - rounding),
            ),
            0.0,
            groove,
        );
    }

    // What goes on: the pressure to set, in the readout face.
    paint_text(
        ui,
        egui::pos2(rect.center().x, rect.top() + metrics.px(TYRE_VALUE_Y)),
        egui::Align2::CENTER_CENTER,
        super::readout(format!("{pressure_kpa}"), metrics.px(TYRE_VALUE_SIZE)).color(loud),
    );
    paint_text(
        ui,
        egui::pos2(rect.center().x, rect.top() + metrics.px(TYRE_UNIT_Y)),
        egui::Align2::CENTER_CENTER,
        RichText::new("kPa").size(metrics.px(TYRE_UNIT_SIZE)).color(quiet),
    );

    // What came off: the tread, its temperatures, the wear, the hot pressure.
    let inner = rect.shrink2(metrics.vec2(TYRE_PAD_X, 0.0));
    let bars_top = rect.top() + metrics.px(TYRE_BARS_TOP);
    let bars_bottom = bars_top + metrics.px(TYRE_BARS_HEIGHT);
    let trough = Color32::from_black_alpha(if armed { 40 } else { 90 });
    let step = inner.width() / 3.0;
    let mean = readout.map(TyreReadout::mean_temp_c);
    let span = readout
        .map_or(1.0, |r| r.temps_c.iter().fold(0.0_f32, |worst, t| worst.max((t - r.mean_temp_c()).abs())).max(1.0));
    for slot in 0..3 {
        #[expect(clippy::cast_precision_loss, reason = "three readings, far inside f32's exact range")]
        let centre = inner.left() + step * (slot as f32 + 0.5);
        let column = Rect::from_min_max(
            egui::pos2(centre - metrics.px(TYRE_BAR_WIDTH / 2.0), bars_top),
            egui::pos2(centre + metrics.px(TYRE_BAR_WIDTH / 2.0), bars_bottom),
        );
        ui.painter().rect_filled(column, metrics.px(3.0), trough);
        let (temp, colour) = match (readout, mean, bars) {
            (Some(r), Some(mean), crate::config::TyreBars::Temps) => {
                let temp = r.temps_c[slot];
                let off = temp - mean;
                // Height reads the spread across this tyre, not an absolute
                // temperature: the shape is the diagnosis.
                let filled = f32::midpoint(off / span, 1.0).clamp(0.08, 1.0);
                let bar = Rect::from_min_max(
                    egui::pos2(column.left(), column.bottom() - column.height() * filled),
                    column.max,
                );
                let colour = temp_colour(off, armed);
                ui.painter().rect_filled(bar, metrics.px(3.0), colour);
                (format!("{temp:.0}"), loud)
            }
            (Some(r), _, crate::config::TyreBars::Wear) => {
                // Absolute, unlike temperature: a worn position is worn
                // whatever the other two are doing, so the bar is the tread
                // left and goes red once it is worth worrying about.
                let left = r.wear[slot].clamp(0.0, 1.0);
                let bar = Rect::from_min_max(
                    egui::pos2(column.left(), column.bottom() - column.height() * left.max(0.05)),
                    column.max,
                );
                let colour = if left < TYRE_WEAR_WORN { theme::alert() } else { temp_colour(0.0, armed) };
                ui.painter().rect_filled(bar, metrics.px(3.0), colour);
                (format!("{:.0}%", left * 100.0), loud)
            }
            _ => ("\u{2014}".to_owned(), quiet),
        };
        paint_text(
            ui,
            egui::pos2(centre, rect.top() + metrics.px(TYRE_TEMP_Y)),
            egui::Align2::CENTER_CENTER,
            RichText::new(temp).size(metrics.px(TYRE_TEMP_SIZE)).strong().color(colour),
        );
    }

    // The wear as one fill along the wheel — its lowest point — under the
    // temperature bars. With the bars already showing wear it would say the
    // same thing twice, so the row is left empty there.
    let wear_rect = Rect::from_min_size(
        egui::pos2(inner.left(), rect.top() + metrics.px(TYRE_WEAR_Y)),
        egui::vec2(inner.width(), metrics.px(TYRE_WEAR_HEIGHT)),
    );
    if bars == crate::config::TyreBars::Temps {
        ui.painter().rect_filled(wear_rect, metrics.px(2.0), trough);
    }
    if let Some(r) = readout {
        if bars == crate::config::TyreBars::Temps {
            let worst = r.wear.iter().copied().fold(1.0_f32, f32::min).clamp(0.0, 1.0);
            let fill = Rect::from_min_size(wear_rect.min, egui::vec2(wear_rect.width() * worst, wear_rect.height()));
            ui.painter().rect_filled(
                fill,
                metrics.px(2.0),
                if worst < TYRE_WEAR_WORN { theme::alert() } else { quiet },
            );
        }
        paint_text(
            ui,
            egui::pos2(rect.center().x, rect.top() + metrics.px(TYRE_HOT_Y)),
            egui::Align2::CENTER_CENTER,
            RichText::new(format!("{:.0} kPa hot", r.pressure_kpa)).size(metrics.px(TYRE_HOT_SIZE)).color(quiet),
        );
    }

    // A ring rather than the olive the rows use: there is no row here to wash,
    // and a ring around the object is how an instrument says "this one".
    if selected {
        paint_cursor_ring(ui, metrics, rect, rounding);
    }
}

/// How far off this tyre's own mean a reading is, as a colour.
///
/// Red for the hot edge, blue for the cold one, quiet in between — the one
/// place in the overlay a number earns a ramp, because the whole question is
/// which end of a set of three is out of line. On an armed (amber) wheel the
/// quiet bar is ink rather than paper, so it still reads as a bar.
fn temp_colour(off_mean_c: f32, armed: bool) -> Color32 {
    if off_mean_c >= TYRE_TEMP_FAR_C {
        theme::alert()
    } else if off_mean_c <= -TYRE_TEMP_FAR_C {
        super::LAPPED
    } else if armed {
        Color32::from_black_alpha(120)
    } else {
        text_secondary()
    }
}

/// The whole-set control, on a plate because it is one.
///
/// Armed, the plate itself fills amber with its label dark on top — the same
/// way an armed tyre does, and for the same reason: the four wheels and the
/// control that arms all four should light up alike, so "everything is on"
/// reads as one lit shape rather than as four lit wheels beside a word that
/// changed color.
fn draw_all_four(ui: &Ui, metrics: Metrics, centre: egui::Pos2, label: &str, checked: bool, selected: bool) {
    let plate = Rect::from_center_size(centre, metrics.vec2(ALL_FOUR_SIZE.0, ALL_FOUR_SIZE.1));
    // The cursor is the olive the rows use, as a fill rather than a ring: a
    // ring round this plate read as a border. Armed, the plate stays amber
    // and the olive is a bar along its foot.
    let (fill, label_color) = match (checked, selected) {
        (true, _) => (theme::caution(), Color32::from_black_alpha(230)),
        (false, true) => (PLAYER_ROW, text_primary()),
        (false, false) => (super::CONTROL_PLATE, text_secondary()),
    };
    super::paint_control_plate_filled(ui, plate, metrics.px(PLATE_ROUNDING), fill);
    if checked && selected {
        let bar = Rect::from_min_max(
            egui::pos2(plate.left() + metrics.px(PLATE_ROUNDING), plate.bottom() - metrics.px(ALL_FOUR_CURSOR_BAR)),
            egui::pos2(plate.right() - metrics.px(PLATE_ROUNDING), plate.bottom()),
        );
        ui.painter().rect_filled(bar, metrics.px(2.0), PLAYER_ROW);
    }

    paint_text(
        ui,
        egui::pos2(plate.center().x, centre.y),
        egui::Align2::CENTER_CENTER,
        RichText::new(label).size(metrics.px(ALL_FOUR_TEXT_SIZE)).strong().color(label_color),
    );
}

fn draw_placeholder(ui: &mut Ui, metrics: Metrics) {
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(ROW_HEIGHT * 2.0)), egui::Sense::hover());
    paint_text(
        ui,
        rect.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new("waiting for iRacing\u{2026}").size(metrics.px(ROW_SIZE)).color(text_secondary()),
    );
}

/// Where a row sits in the run.
///
/// Both ends are special and a row can be both at once: the first has the
/// title's rule above it already, and the last has to follow the card's
/// rounded bottom corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowPlace {
    First,
    Middle,
    Last,
    /// The only row on the page, which is both ends of the run.
    Only,
}

impl RowPlace {
    fn of(index: usize, len: usize) -> Self {
        match (index == 0, index + 1 >= len) {
            (true, true) => Self::Only,
            (true, false) => Self::First,
            (false, true) => Self::Last,
            (false, false) => Self::Middle,
        }
    }

    fn is_first(self) -> bool {
        matches!(self, Self::First | Self::Only)
    }

    fn is_last(self) -> bool {
        matches!(self, Self::Last | Self::Only)
    }
}

/// Where a row sits in the run, and whether the cursor is on it.
///
/// Grouped rather than passed as bare positional arguments, which at the call
/// site would be a row of indistinguishable `true`s.
#[derive(Debug, Clone, Copy)]
struct RowStyle {
    /// The cursor is on this row.
    selected: bool,
    /// Takes the alternating stripe.
    odd: bool,
    seat: RowPlace,
}

fn draw_row(ui: &mut Ui, metrics: Metrics, row: &Row, style: RowStyle) {
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), metrics.px(ROW_HEIGHT)), egui::Sense::hover());
    let middle = rect.center().y;
    let split = rect.left() + rect.width() * LABEL_FRACTION;

    // Square everywhere but at the foot of the run, where the fill has to
    // follow the card's own corners or it paints over them — the same rule the
    // Standings rows follow, for the same reason.
    let rounding = if style.seat.is_last() {
        let card = card_rounding(metrics);
        egui::Rounding { nw: 0.0, ne: 0.0, sw: card.sw, se: card.se }
    } else {
        egui::Rounding::ZERO
    };

    // The cursor takes the olive the Relative gives the player's own row.
    // "The row that is you" and "the row you are about to change" are one
    // idea, and the overlay should have one way of drawing it. It replaces an
    // outlined pill, which read as a dialog control dropped onto an
    // instrument face.
    // A slash was tried at the row's leading edge as well, borrowed from the
    // Relative. On a row whose left half is empty it had nothing to sit
    // between and read as a stray glyph rather than as a mark; the olive says
    // "this row" on its own, and the slant is already spent on the control
    // plate, which is where it earns its keep.
    if style.selected {
        ui.painter().rect_filled(rect, rounding, PLAYER_ROW);
    } else if let Some(stripe) = row_stripe(style.odd) {
        ui.painter().rect_filled(rect, rounding, stripe);
    }

    // On the row's own top edge rather than its bottom, so the run ends
    // cleanly against the card instead of on a divider with nothing under it.
    if !style.seat.is_first() {
        super::paint_row_groove(ui, rect.top(), rect.left(), rect.right());
    }

    if !row.label.is_empty() {
        // Grey, not the mockups' amber. Amber is very nearly [`super::CAUTION`],
        // and spending the loudest color in the palette on every label on every
        // page left the pages reading as loud and flat at once — with nothing
        // left over to mark the one thing on this panel worth shouting about.
        // The cursor's row brightens instead, which is a difference that means
        // something.
        paint_text(
            ui,
            egui::pos2(split - metrics.px(10.0), middle),
            egui::Align2::RIGHT_CENTER,
            RichText::new(&row.label).size(metrics.px(ROW_SIZE)).strong().color(if style.selected {
                text_primary()
            } else {
                text_secondary()
            }),
        );
    }

    match &row.kind {
        RowKind::Static { value } => paint_text(
            ui,
            egui::pos2(split, middle),
            egui::Align2::LEFT_CENTER,
            RichText::new(value).size(metrics.px(ROW_SIZE)).color(text_primary()),
        ),
        RowKind::Toggle { checked, .. } => draw_checkbox(ui, metrics, egui::pos2(split, middle), *checked),
        RowKind::Stepper { value, .. } => draw_stepper(ui, metrics, rect, split, value, style.selected),
        // A wheel belongs in the car's geometry and is never laid out as a
        // row; it reaches this arm only if a page is built wrong.
        RowKind::Corner { .. } => {}
    }
}

fn draw_checkbox(ui: &Ui, metrics: Metrics, left_center: egui::Pos2, checked: bool) {
    let size = metrics.px(14.0);
    let rect = Rect::from_min_size(egui::pos2(left_center.x, left_center.y - size / 2.0), egui::vec2(size, size));
    let rounding = metrics.px(3.0);
    if checked {
        ui.painter().rect_filled(rect, rounding, CHECK);
        // A tick drawn from two strokes rather than a glyph, so it stays
        // crisp at any scale.
        let stroke = Stroke::new(metrics.px(2.0), Color32::WHITE);
        let low = egui::pos2(rect.left() + size * 0.26, rect.center().y + size * 0.04);
        let bottom = egui::pos2(rect.center().x - size * 0.02, rect.bottom() - size * 0.28);
        let high = egui::pos2(rect.right() - size * 0.22, rect.top() + size * 0.28);
        ui.painter().line_segment([low, bottom], stroke);
        ui.painter().line_segment([bottom, high], stroke);
    } else {
        ui.painter().rect_stroke(rect, rounding, Stroke::new(metrics.px(1.0), Color32::from_gray(110)));
    }
}

/// A value you can change: the number on a raised plate, its arrows inside the
/// plate's own ends rather than adrift at either side of the row.
///
/// The plate is the whole point of the page's grammar — see
/// [`super::paint_control_plate`]. A row without one cannot be changed from
/// here, and that now reads at a glance instead of having to be inferred from
/// two thin chevrons.
fn draw_stepper(ui: &Ui, metrics: Metrics, rect: Rect, split: f32, value: &str, selected: bool) {
    let middle = rect.center().y;
    let plate = Rect::from_min_max(
        egui::pos2(split, rect.top() + metrics.px(PLATE_INSET_Y)),
        egui::pos2(rect.right() - metrics.px(PLATE_RIGHT_INSET), rect.bottom() - metrics.px(PLATE_INSET_Y)),
    );
    super::paint_control_plate(ui, plate, metrics.px(PLATE_ROUNDING));

    let left = plate.left() + metrics.px(CHEVRON_INSET);
    let right = plate.right() - metrics.px(CHEVRON_INSET);

    paint_text(
        ui,
        egui::pos2(f32::midpoint(left, right), middle),
        egui::Align2::CENTER_CENTER,
        RichText::new(value).size(metrics.px(ROW_SIZE)).strong().color(text_primary()),
    );
    // The arrows are supporting marks, so the value on the plate stays the
    // brightest thing on it; the cursor's row lifts them along with its label.
    let arrow_color = if selected { text_primary() } else { text_secondary() };
    for (x, name) in [(left, "chevron-left"), (right, "chevron-right")] {
        let arrow = Rect::from_center_size(egui::pos2(x, middle), metrics.vec2(CHEVRON_SIZE, CHEVRON_SIZE));
        if !super::icons::svg(ui, arrow, name, arrow_color) {
            // Single guillemets stand in when the asset folder is
            // missing; they are the closest the embedded face has.
            let glyph = if name == "chevron-left" { "\u{2039}" } else { "\u{203A}" };
            paint_text(
                ui,
                egui::pos2(x, middle),
                egui::Align2::CENTER_CENTER,
                RichText::new(glyph).size(metrics.px(ROW_SIZE + 4.0)).strong().color(arrow_color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn toggle_row(label: &str) -> Row {
        Row { label: label.to_owned(), kind: RowKind::Toggle { checked: false, control: Control::Tearoff } }
    }

    #[test]
    fn the_status_border_resolves_by_priority() {
        let mut snap = crate::demo::snapshot();

        // No snapshot, and a plain green race, show nothing.
        assert_eq!(resolve_status(None, false), BlackBoxStatus::None);
        snap.course_flag = CourseFlag::Green;
        snap.box_this_lap = false;
        assert_eq!(resolve_status(Some(&snap), false), BlackBoxStatus::None);

        // A hand-called box lights BOX BOX even under green.
        assert_eq!(resolve_status(Some(&snap), true), BlackBoxStatus::Box);

        // A box call outranks the yellow behind it — you pit under caution.
        snap.course_flag = CourseFlag::Yellow;
        snap.box_this_lap = true;
        assert_eq!(resolve_status(Some(&snap), false), BlackBoxStatus::Box);

        // But the flags show through when there is no call.
        snap.box_this_lap = false;
        assert_eq!(resolve_status(Some(&snap), false), BlackBoxStatus::Caution);
        snap.course_flag = CourseFlag::White;
        assert_eq!(resolve_status(Some(&snap), false), BlackBoxStatus::LastLap);

        // The chequered outranks everything, box call included.
        snap.course_flag = CourseFlag::Checkered;
        assert_eq!(resolve_status(Some(&snap), true), BlackBoxStatus::Finish);
    }

    fn static_row(label: &str) -> Row {
        Row { label: label.to_owned(), kind: RowKind::Static { value: String::new() } }
    }

    /// Every page on offer, which is what the tests page around.
    fn all_pages() -> PageSet {
        pages_for(None, None)
    }

    /// Strategy is offered in every session, whatever the session is doing.
    ///
    /// It used to come and go with the race's stop count and with the
    /// Standings widget's `endurance_mode`, which made it something to hunt for
    /// at the moment it was most wanted. It says plainly when it has no plan
    /// yet; that is a better answer than being absent.
    #[test]
    fn strategy_is_offered_in_every_session() {
        use crate::telemetry::snapshot::SessionKind;

        let mut snapshot = crate::demo::snapshot();
        for (kind, stops, note) in [
            (SessionKind::Race, true, "a multi-stop race"),
            (SessionKind::Race, false, "a race needing one stop"),
            (SessionKind::Practice, false, "practice"),
            (SessionKind::Qualifying, false, "qualifying"),
        ] {
            snapshot.relative_meta.session_kind = kind;
            snapshot.endurance.multi_stop_race = stops;
            assert!(pages_for(Some(&snapshot), None).contains(Page::Strategy), "{note}");
        }
        assert!(pages_for(None, None).contains(Page::Strategy), "and before any telemetry has arrived");
    }

    /// `--demo-page` names every page and nothing else.
    #[test]
    fn every_page_can_be_asked_for_by_name() {
        for page in Page::ALL {
            assert_eq!(Page::from_arg(page.arg_name()), Some(page));
            assert_eq!(Page::from_arg(&page.arg_name().to_uppercase()), Some(page), "case is not part of the name");
        }
        assert_eq!(Page::from_arg("nonsense"), None);
    }

    /// No snapshot at all is a driver's view.
    #[test]
    fn gating_never_takes_away_a_page_from_a_driver() {
        let offered = pages_for(None, None);
        for page in Page::ALL {
            assert!(offered.contains(page), "{page:?} went missing");
        }
    }

    /// Spectating leaves only the pages that are about somebody's race rather
    /// than about the player's car — see [`pages_for`].
    #[test]
    fn watching_somebody_else_withdraws_every_page_about_a_car() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.seat = Seat::Spectating(Arc::from("Istvan Fodor"));
        let offered = pages_for(Some(&snapshot), None);

        assert!(offered.contains(Page::Relative), "the Relative follows the camera");
        assert!(offered.contains(Page::Weather), "the weather belongs to the session");
        for page in [Page::Strategy, Page::Fuel, Page::Tires, Page::InCarAdjustments] {
            assert!(!offered.contains(page), "{page:?} would read a car nobody is driving");
        }
    }

    /// A synced car with fuel but no tyre readings yet.
    fn synced_fuel_only() -> crate::sync::store::SyncedCar {
        crate::sync::store::SyncedCar {
            driver: Some("Istvan Fodor".to_owned()),
            fuel_litres: 40.0,
            burn_per_lap: Some(2.5),
            service_fuel_litres: Some(30),
            tyres_armed: [false; 4],
            tyre_pressures_kpa: [165.0; 4],
            tyres: None,
        }
    }

    /// Once team sync is feeding the driver's real fuel, the Fuel page comes
    /// back while spectating — but the Tyres page waits for a stop's readings,
    /// and nothing unlocks on empty data.
    #[test]
    fn team_sync_unlocks_the_fuel_page_while_spectating() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.seat = Seat::Spectating(Arc::from("Istvan Fodor"));

        assert!(!pages_for(Some(&snapshot), None).contains(Page::Fuel), "no synced data, no fuel page");

        assert!(!pages_for(Some(&snapshot), None).contains(Page::Strategy), "no synced data, no stop plan");

        // Fuel present, tyres not yet: Fuel and the Strategy stop-plan unlock
        // (both live on fuel), Tyres stays withdrawn until a stop.
        let fuel_only = synced_fuel_only();
        let with_fuel = pages_for(Some(&snapshot), Some(&fuel_only));
        assert!(with_fuel.contains(Page::Fuel), "synced fuel brings the fuel page back");
        assert!(with_fuel.contains(Page::Strategy), "synced fuel brings the stop plan back");
        assert!(!with_fuel.contains(Page::Tires), "the tyres page waits for a stop's readings");
        assert!(!with_fuel.contains(Page::InCarAdjustments), "in-car controls are the driver's alone");

        // Once a stop puts tyre life on the wire, the Tyres page unlocks too.
        let mut with_tyres = synced_fuel_only();
        with_tyres.tyres = Some(crate::telemetry::snapshot::TyreInfo {
            corners: [crate::telemetry::snapshot::TyreState { temps_c: [80.0; 3], wear: [0.9; 3], pressure_kpa: 170.0 };
                4],
        });
        let offered = pages_for(Some(&snapshot), Some(&with_tyres));
        assert!(offered.contains(Page::Tires), "synced tyre life brings the tyres page back");
        assert!(!offered.contains(Page::InCarAdjustments), "still the driver's alone");
    }

    /// And a driver still gets all of them, which is the property that keeps
    /// this off anyone's racing.
    #[test]
    fn driving_offers_every_page_as_before() {
        let snapshot = crate::demo::snapshot();
        assert_eq!(snapshot.seat, Seat::Driving, "the demo is a driver's view");
        let offered = pages_for(Some(&snapshot), None);
        for page in Page::ALL {
            assert!(offered.contains(page), "{page:?} went missing");
        }
    }

    /// A team-mate's stint keeps every page about the race — the Relative,
    /// the stop plan, the fuel as a readout — and withdraws only the two that
    /// nobody but the driver can see.
    #[test]
    fn a_team_mates_stint_withdraws_only_the_pages_about_the_seat() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.seat = Seat::TeamMate(Arc::from("Istvan Fodor"));
        let offered = pages_for(Some(&snapshot), None);
        for page in [Page::Relative, Page::Weather, Page::Strategy, Page::Fuel] {
            assert!(offered.contains(page), "{page:?} is about the team's race");
        }
        for page in [Page::Tires, Page::InCarAdjustments] {
            assert!(!offered.contains(page), "{page:?} would read a seat somebody else is in");
        }
    }

    /// A driver out of their own car — the garage, a tow — still has their
    /// own car's numbers, so nothing is taken away. This is what keeps the
    /// seat off a solo driver's racing.
    #[test]
    fn a_driver_out_of_their_own_car_keeps_every_page() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.seat = Seat::OutOfCar;
        let offered = pages_for(Some(&snapshot), None);
        for page in Page::ALL {
            assert!(offered.contains(page), "{page:?} went missing");
        }
    }

    /// A spectator parked on the Fuel page when the pages are withdrawn ends up
    /// on the Relative rather than on a page that is no longer offered.
    #[test]
    fn a_spectator_is_moved_off_a_page_about_a_car() {
        let mut box_ = BlackBox::new();
        let mut settings = crate::config::BlackBoxConfig::default();
        let relative = RelativeConfig::default();
        box_.apply(Action::NextPage, &[], None, &mut settings, &relative, all_pages());
        assert_eq!(box_.page(), Page::Fuel);

        let mut snapshot = crate::demo::snapshot();
        snapshot.seat = Seat::Spectating(Arc::from("Istvan Fodor"));
        assert_eq!(box_.settle_page(pages_for(Some(&snapshot), None)), Page::Relative);
    }

    /// Someone parked on Strategy when the pages about a car are withdrawn has
    /// to end up somewhere, and paging must walk the pages that exist rather
    /// than an index into a list whose length just changed.
    #[test]
    fn a_page_that_goes_away_moves_the_driver_rather_than_stranding_them() {
        let mut box_ = BlackBox::new();
        let mut settings = crate::config::BlackBoxConfig::default();
        let relative = RelativeConfig::default();
        for _ in 0..3 {
            box_.apply(Action::NextPage, &[], None, &mut settings, &relative, all_pages());
        }
        assert_eq!(box_.page(), Page::Strategy, "Strategy follows the two pages a stop is armed from");

        let mut snapshot = crate::demo::snapshot();
        snapshot.seat = Seat::Spectating(Arc::from("Istvan Fodor"));
        let without = pages_for(Some(&snapshot), None);
        assert_eq!(box_.settle_page(without), Page::Relative);
        // And paging on from there walks only what is left.
        box_.apply(Action::NextPage, &[], None, &mut settings, &relative, without);
        assert_eq!(box_.page(), Page::Weather, "every page about a car is stepped straight over");
    }

    /// The pit window is never "waiting for iRacing".
    ///
    /// It has no controls at all — nothing on it is set — and the placeholder
    /// used to fire on an empty control list, so a fully computed window sat
    /// behind "waiting for iRacing" for as long as the page was open.
    #[test]
    fn the_demo_snapshot_produces_a_pit_window() {
        let snapshot = crate::demo::snapshot();
        let layout = pages::pit_window(&snapshot, false, SyncControls::default());
        assert!(!layout.is_bare(), "the pit window is never bare");
        let Shape::PitWindow { window, .. } = &layout.shape else { panic!("wrong shape") };
        // The fixture carries 18.7 laps of fuel with 13 laps to run, so it
        // finishes without stopping — and the page says so rather than
        // inventing a stop to place.
        assert!(window.finishes_without_stopping, "the fixture's tank reaches the flag");
        assert!(window.candidates.is_empty(), "so there is no lap to choose");
    }

    #[test]
    fn paging_wraps_both_ways() {
        let mut box_ = BlackBox::new();
        let mut settings = crate::config::BlackBoxConfig::default();
        let relative = RelativeConfig::default();
        assert_eq!(box_.page(), Page::Relative);
        box_.apply(Action::PrevPage, &[], None, &mut settings, &relative, all_pages());
        assert_eq!(box_.page(), Page::Weather, "paging back from the first page wraps to the last");
        box_.apply(Action::NextPage, &[], None, &mut settings, &relative, all_pages());
        assert_eq!(box_.page(), Page::Relative);
    }

    #[test]
    fn the_cursor_skips_rows_that_cannot_be_changed() {
        let rows =
            vec![static_row("Remaining"), toggle_row("Begin Fueling"), static_row("Est. Laps"), toggle_row("Tearoff")];
        let mut box_ = BlackBox::new();
        let mut settings = crate::config::BlackBoxConfig::default();
        let relative = RelativeConfig::default();
        box_.apply(Action::NextPage, &[], None, &mut settings, &relative, all_pages()); // off the Relative page
        box_.settle_cursor(&rows);
        assert_eq!(box_.cursor, 1, "settles on the first row that can be changed");

        box_.apply(Action::Next, &rows, None, &mut settings, &relative, all_pages());
        assert_eq!(box_.cursor, 3, "steps over the reading between them");
        box_.apply(Action::Next, &rows, None, &mut settings, &relative, all_pages());
        assert_eq!(box_.cursor, 1, "and wraps");
    }

    #[test]
    fn a_page_with_nothing_to_change_leaves_the_cursor_alone() {
        let rows = vec![static_row("Air"), static_row("Track")];
        let mut box_ = BlackBox::new();
        let mut settings = crate::config::BlackBoxConfig::default();
        let relative = RelativeConfig::default();
        box_.apply(Action::NextPage, &[], None, &mut settings, &relative, all_pages());
        box_.settle_cursor(&rows);
        box_.apply(Action::Next, &rows, None, &mut settings, &relative, all_pages());
        assert_eq!(box_.cursor, 0);
    }

    #[test]
    fn scrolling_is_bounded_by_the_field_not_just_the_configured_limit() {
        let mut box_ = BlackBox::new();
        let mut settings = crate::config::BlackBoxConfig::default();
        let relative = RelativeConfig::default();

        // With no telemetry there is no field, so there is nothing to scroll
        // into and the count must not climb — that was the bug where the
        // header read `+9` while the rows never moved.
        for _ in 0..5 {
            box_.apply(Action::Prev, &[], None, &mut settings, &relative, all_pages());
        }
        assert_eq!(box_.scroll(), 0);

        // A real field scrolls, up to where the window runs out.
        let snapshot = crate::demo::snapshot();
        for _ in 0..20 {
            box_.apply(Action::Prev, &[], Some(&snapshot), &mut settings, &relative, all_pages());
        }
        let (min, _max) = crate::telemetry::relative::scroll_bounds(
            snapshot.relative.len(),
            snapshot.focus_index,
            usize::from(relative.ahead_count),
            usize::from(relative.behind_count),
        );
        assert_eq!(box_.scroll(), min);
    }

    #[test]
    fn leaving_the_relative_page_stops_scrolling_it() {
        let mut box_ = BlackBox::new();
        let mut settings = crate::config::BlackBoxConfig::default();
        let relative = RelativeConfig::default();
        box_.apply(Action::Prev, &[], None, &mut settings, &relative, all_pages());
        box_.apply(Action::NextPage, &[], None, &mut settings, &relative, all_pages());
        let before = box_.scroll();
        box_.apply(Action::Next, &[], None, &mut settings, &relative, all_pages());
        assert_eq!(box_.scroll(), before, "up and down move the cursor once off the Relative");
    }

    fn service(armed: bool, amount: f32) -> PitService {
        PitService { fuel_armed: armed, fuel_amount_litres: amount, ..PitService::default() }
    }

    #[test]
    fn wanting_no_fuel_unticks_the_box_rather_than_asking_for_zero() {
        // `SetFuel(0)` is the SDK's "keep what is set", so asking for zero
        // that way arms the old load and the read-back never moves — the
        // frame-rate broadcast storm that locked the machine up on the lap
        // out of the pits.
        assert_eq!(fuel_request_for(0, &service(true, 43.0)), Some(PitRequest::ClearFuel));
    }

    #[test]
    fn wanting_no_fuel_asks_for_nothing_once_the_box_is_already_unticked() {
        // The sim leaves the last served amount standing in `PitSvFuel`, so
        // the amount cannot be the thing that settles this — only the tick.
        assert_eq!(fuel_request_for(0, &service(false, 43.0)), None);
    }

    #[test]
    fn a_load_the_sim_already_has_armed_asks_for_nothing() {
        assert_eq!(fuel_request_for(43, &service(true, 43.4)), None);
    }

    #[test]
    fn a_matching_load_is_re_armed_when_the_sim_has_unticked_the_box() {
        // The number is right but nothing is set to go in, which is the state
        // iRacing leaves behind after every served stop.
        assert_eq!(fuel_request_for(43, &service(false, 43.0)), Some(PitRequest::SetFuel(43)));
    }

    #[test]
    fn a_load_the_sim_already_has_needs_no_command() {
        assert_eq!(fuel_request_for(43, &service(true, 43.4)), None, "the sim already has 43 L armed");
    }

    /// Commands are only sent in the pit lane now, and there the figure is the
    /// one that actually goes into the tank — so any difference at all is worth
    /// a command. There is no on-track deadband to be inside of, because there
    /// is no on-track command.
    #[test]
    fn the_pit_lane_arms_the_latched_load_to_the_litre() {
        let armed = service(true, 43.0);
        assert_eq!(fuel_request_for(44, &armed), Some(PitRequest::SetFuel(44)));
        assert_eq!(fuel_request_for(42, &armed), Some(PitRequest::SetFuel(42)));
    }

    #[test]
    fn a_wild_read_back_widens_the_gap_rather_than_wrapping_it() {
        let huge = service(true, f32::from(i16::MAX));
        assert_eq!(fuel_request_for(1, &huge), Some(PitRequest::SetFuel(1)));
        // Negative nonsense is clamped to zero litres armed by `round_litres`
        // long before it gets here, so it reads as an empty rig, not a gap of
        // thirty thousand litres.
        assert_eq!(fuel_request_for(50, &service(true, -900.0)), Some(PitRequest::SetFuel(50)));
    }

    #[test]
    fn commands_cannot_follow_each_other_faster_than_the_interval() {
        let mut box_ = BlackBox::new();
        let start = Instant::now();

        // The first after a quiet spell is never delayed.
        assert_eq!(box_.throttle_auto_fuel(PitRequest::SetFuel(55), start), Some(PitRequest::SetFuel(55)));
        // Every frame inside the interval sends nothing, however many there
        // are and whatever they ask for — a changing figure must not be a way
        // around the ceiling.
        for frame in 1..1200 {
            let now = start + Duration::from_micros(frame * 500);
            let request = PitRequest::SetFuel(55 + i16::try_from(frame % 7).expect("small"));
            assert_eq!(box_.throttle_auto_fuel(request, now), None);
        }
        // Once the interval is up the next one goes, so a command the sim
        // refused while it was busy is not dropped for good.
        let after_interval = start + AUTO_FUEL_MIN_INTERVAL + Duration::from_millis(10);
        assert_eq!(box_.throttle_auto_fuel(PitRequest::ClearFuel, after_interval), Some(PitRequest::ClearFuel));
    }

    fn in_the_lane(armed: bool, amount: f32, level: f32) -> PitService {
        PitService {
            in_car: true,
            on_pit_road: true,
            fuel_armed: armed,
            fuel_amount_litres: amount,
            fuel_level_litres: level,
            ..PitService::default()
        }
    }

    /// The double fuel load: the crew finish, iRacing unticks the fuel box,
    /// and Auto Fuel reads that as "nothing set to go in" and arms the same
    /// load again while the car is still in its box.
    #[test]
    fn a_served_stop_is_not_armed_a_second_time() {
        let mut box_ = BlackBox::new();
        box_.latched_target_litres = Some(52.0);

        // Rolling down the lane with the load armed.
        box_.track_pit_lane(&in_the_lane(true, 40.0, 12.0));
        assert_eq!(box_.pit_lane, PitLane::Pending);

        // Served: the box is clear again and the tank is fuller.
        box_.track_pit_lane(&in_the_lane(false, 40.0, 52.0));
        assert_eq!(box_.pit_lane, PitLane::Served);
        assert_eq!(box_.latched_target_litres, None, "the figure that went in is spent");

        // Back on track, the next stop starts from scratch.
        box_.track_pit_lane(&PitService { in_car: true, ..PitService::default() });
        assert_eq!(box_.pit_lane, PitLane::Pending);
    }

    /// The whole point of arming in the lane: a load that has *not* been
    /// served yet must still go in, even though the box reads unticked.
    #[test]
    fn a_stop_still_to_come_is_armed_normally() {
        let mut box_ = BlackBox::new();
        box_.track_pit_lane(&in_the_lane(false, 0.0, 12.0));
        assert_eq!(box_.pit_lane, PitLane::Pending, "nothing has been ticked on this visit yet");
    }

    /// A tank that has plainly been filled is a served stop whatever the
    /// checkbox says, so the fix does not rest on that one signal.
    #[test]
    fn a_filled_tank_alone_counts_as_served() {
        let mut box_ = BlackBox::new();
        box_.track_pit_lane(&in_the_lane(true, 40.0, 12.0));
        box_.track_pit_lane(&in_the_lane(true, 40.0, 52.0));
        assert_eq!(box_.pit_lane, PitLane::Served);
    }

    #[test]
    fn auto_fuel_says_nothing_more_once_the_stop_is_served() {
        let mut box_ = BlackBox::new();
        let settings = crate::config::BlackBoxConfig { auto_fuel: true, ..Default::default() };
        let mut snapshot = crate::demo::snapshot();
        snapshot.pit_service = in_the_lane(false, 40.0, 52.0);
        box_.latched_target_litres = Some(52.0);
        box_.fuel_armed_in_lane = true;
        box_.fuel_on_entry_litres = Some(12.0);

        assert_eq!(box_.auto_fuel_request(Some(&snapshot), &settings, Instant::now()), None);
    }

    #[test]
    fn switching_auto_fuel_off_forgets_when_it_last_spoke() {
        let mut box_ = BlackBox::new();
        let settings = crate::config::BlackBoxConfig::default();
        assert!(!settings.auto_fuel, "off by default");
        box_.throttle_auto_fuel(PitRequest::SetFuel(55), Instant::now());
        assert_eq!(box_.auto_fuel_request(None, &settings, Instant::now()), None);
        assert!(box_.last_auto_fuel_at.is_none(), "so switching back on arms straight away");
    }
}
