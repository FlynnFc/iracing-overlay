// Rust guideline compliant 2026-02-16

//! Wheel and keyboard binds for the black box.
//!
//! The overlay never has keyboard focus — it can't, it's a click-through
//! sheet over the sim — so both sources read device state directly rather
//! than waiting for input events to be routed to a window.

pub mod device;
pub mod keys;
pub mod typed;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One thing the black box can be told to do.
///
/// Named after the iRacing control they mirror, so a user reading their own
/// sim bindings recognizes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Show the next page.
    NextPage,
    /// Show the previous page.
    PrevPage,
    /// Move the cursor down the current page's list. On the Relative page,
    /// scroll the field toward the cars behind.
    Next,
    /// Move the cursor up. On the Relative page, scroll toward the cars ahead.
    Prev,
    /// Raise the selected value.
    Increment,
    /// Lower the selected value.
    Decrement,
    /// Tick or untick the selected option. On the Relative page, recenter on
    /// the player.
    Toggle,
}

impl Action {
    /// Every action, in the order `--bind` walks through them.
    pub const ALL: [Self; 7] =
        [Self::NextPage, Self::PrevPage, Self::Next, Self::Prev, Self::Increment, Self::Decrement, Self::Toggle];

    /// The name used on the command line and as the config key.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Self::NextPage => "next_page",
            Self::PrevPage => "prev_page",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Increment => "increment",
            Self::Decrement => "decrement",
            Self::Toggle => "toggle",
        }
    }

    /// What the settings window calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NextPage => "Next page",
            Self::PrevPage => "Previous page",
            Self::Next => "Cursor down / scroll behind",
            Self::Prev => "Cursor up / scroll ahead",
            Self::Increment => "Increase value",
            Self::Decrement => "Decrease value",
            Self::Toggle => "Toggle / recentre",
        }
    }

    /// Parses a slug, accepting hyphens as well as underscores so
    /// `--bind next-page` works the way a command line reads.
    #[must_use]
    pub fn from_slug(text: &str) -> Option<Self> {
        let normalized = text.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL.into_iter().find(|action| action.slug() == normalized)
    }
}

/// A physical control bound to an action.
///
/// Devices are identified by name rather than by index: gamepad indices are
/// assignment order, so unplugging a pedal set can renumber a wheel and
/// silently move every bind onto the wrong device.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Bind {
    /// A button on a wheel, button box or pad.
    Button { device: String, button: u32 },
    /// A keyboard key, by Windows virtual-key code.
    Key { vk: u16 },
}

impl Bind {
    /// The control as a driver would name it: `MOZA FSR button 21`, `F3`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Button { device, button } => format!("{device} button {button}"),
            Self::Key { vk } => keys::key_name(*vk),
        }
    }
}

/// The key that cancels a capture rather than being captured.
///
/// Escape is the one key nobody wants bound to a black box action, and a
/// capture that could not be backed out of would leave a mis-click as a bind.
const VK_ESCAPE: u16 = 0x1B;

/// How often the input thread reads the wheel.
///
/// Deliberately much faster than the display rate, and the reason this is a
/// thread rather than a call once a frame: a rotary encoder's detent often
/// reports as a pulse only a few tens of milliseconds long, and a poll at
/// frame rate can step straight over one. `--bind` samples at the same rate,
/// for the same reason.
const POLL_INTERVAL: Duration = Duration::from_millis(4);

/// How long the input thread idles while nothing at all is bound.
///
/// With no binds there is no device to open and no button to read, so the only
/// thing to wake up for is a `--bind` writing the first one.
const IDLE_INTERVAL: Duration = Duration::from_millis(250);

/// Edge-detects held inputs so one press produces exactly one action.
///
/// Both sources are level-triggered — "is this button down right now" — and
/// the UI runs at display rate, so without this a single press of the page
/// button would advance sixty pages a second.
///
/// **Reading the devices happens on its own thread, and must.** DirectInput
/// enumeration walks every HID device on the machine through SetupAPI, and
/// when a device it is looking for is absent — a wheel powered down, or asleep
/// on a suspended USB port — that walk blocks for *seconds* while consuming no
/// CPU at all. On the render thread that is a window which has stopped pumping
/// messages, which is precisely what Windows puts the "Not Responding" title on.
/// It was reproducible: with every bind on a wheel that was switched off, the
/// overlay stalled for seconds at a time, every two seconds, for as long as it
/// was left running. Nothing here is fast enough to be allowed near a frame.
#[derive(Debug)]
#[expect(
    clippy::doc_markdown,
    reason = "DirectInput and SetupAPI are Windows API names; backticking them would read as though they were items in this crate, as in `device`"
)]
pub struct Actions {
    /// The binds the input thread reads, republished whenever they change so a
    /// `--bind` takes effect without a restart.
    binds: Arc<Mutex<Vec<(Action, Bind)>>>,
    /// Presses the input thread has seen since the last drain.
    pressed: Receiver<Action>,
    /// Whether the input thread is waiting for a fresh press to report as a
    /// bind rather than acting on the binds it has — the settings window's
    /// **Bind…**. Cleared by the thread once it has one, or on Escape.
    capturing: Arc<AtomicBool>,
    /// The control the thread caught, if a capture has finished.
    captured: Receiver<Bind>,
}

impl Default for Actions {
    fn default() -> Self {
        Self::new()
    }
}

impl Actions {
    /// Starts the input thread.
    #[must_use]
    pub fn new() -> Self {
        let binds = Arc::new(Mutex::new(Vec::new()));
        let (tx, pressed) = mpsc::channel();
        let capturing = Arc::new(AtomicBool::new(false));
        let (capture_tx, captured) = mpsc::channel();
        let worker = Arc::clone(&binds);
        let worker_capturing = Arc::clone(&capturing);
        // `Devices` is built inside the closure rather than passed in: it owns
        // DirectInput COM interfaces, and creating them on the thread that uses
        // them keeps the whole of that API on one thread, the same way the
        // telemetry `Session` is confined to its own.
        std::thread::spawn(move || {
            // Polls the wheel continuously; keep it off the sim's cores — see
            // `crate::perf`.
            crate::perf::mark_background_thread();
            read_inputs(&Reader { binds: &worker, pressed: &tx, capturing: &worker_capturing, captured: &capture_tx });
        });
        Self { binds, pressed, capturing, captured }
    }

    /// Asks the input thread to report the next fresh press as a bind
    /// instead of acting on it. Whatever is already held when this is called
    /// does not count, so a shifter resting in gear cannot be the answer.
    pub fn start_capture(&self) {
        // Anything left from an earlier capture nobody collected is stale.
        while self.captured.try_recv().is_ok() {}
        self.capturing.store(true, Ordering::Release);
    }

    /// Calls a capture off. Harmless when none is running.
    pub fn cancel_capture(&self) {
        self.capturing.store(false, Ordering::Release);
    }

    /// Whether a capture is still waiting for a press.
    #[must_use]
    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::Acquire)
    }

    /// The control a capture caught, once it has. Each result is handed out
    /// once.
    pub fn captured(&self) -> Option<Bind> {
        self.captured.try_recv().ok()
    }

    /// Returns the actions whose bind went down since the last call.
    ///
    /// `binds` is the live configuration; handing it over each frame is what
    /// lets a rebind take effect without a restart. Never blocks — the reading
    /// itself happens elsewhere, and this only publishes the binds and drains
    /// whatever the input thread has already found.
    pub fn poll(&mut self, binds: &[(Action, Bind)]) -> Vec<Action> {
        if let Ok(mut shared) = self.binds.lock()
            && shared.as_slice() != binds
        {
            *shared = binds.to_vec();
        }
        self.pressed.try_iter().collect()
    }
}

/// What the input thread reads from and reports to — see [`Actions`].
struct Reader<'a> {
    binds: &'a Mutex<Vec<(Action, Bind)>>,
    pressed: &'a Sender<Action>,
    capturing: &'a AtomicBool,
    captured: &'a Sender<Bind>,
}

/// What was already held when a capture began, so only a fresh press counts.
struct CaptureBaseline {
    buttons: Vec<(String, u32)>,
    key: Option<u16>,
}

/// The input thread: reads every bound control and reports each fresh press.
///
/// Returns when the receiving end is gone, which is how it is shut down — the
/// overlay's window closing drops the `Actions` that owns it.
fn read_inputs(reader: &Reader<'_>) {
    let mut devices = device::Devices::new();
    let mut held: Vec<Action> = Vec::new();
    let mut baseline: Option<CaptureBaseline> = None;
    // Set once a capture ends, so the press that was captured — which may
    // well also be an existing bind — isn't then acted on as one.
    let mut swallow_next = false;
    loop {
        if reader.capturing.load(Ordering::Acquire) {
            // Every device, not only the bound ones: the point of a capture is
            // usually a control on a device with nothing bound yet.
            devices.pump(&[]);
            let baseline =
                baseline.get_or_insert_with(|| CaptureBaseline { buttons: devices.pressed(), key: keys::pressed() });
            let fresh_button = devices.pressed().into_iter().find(|held| !baseline.buttons.contains(held));
            let fresh_key = keys::pressed().filter(|vk| Some(*vk) != baseline.key);
            let result = if keys::is_key_down(VK_ESCAPE) {
                None
            } else if let Some((device, button)) = fresh_button {
                Some(Bind::Button { device, button })
            } else if let Some(vk) = fresh_key.filter(|vk| *vk != VK_ESCAPE) {
                Some(Bind::Key { vk })
            } else {
                std::thread::sleep(POLL_INTERVAL);
                continue;
            };
            reader.capturing.store(false, Ordering::Release);
            swallow_next = true;
            if let Some(bind) = result
                && reader.captured.send(bind).is_err()
            {
                return; // the overlay has gone
            }
            continue;
        }
        baseline = None;

        let Ok(current) = reader.binds.lock().map(|binds| binds.clone()) else {
            return; // the UI thread panicked while holding the lock
        };
        if current.is_empty() {
            std::thread::sleep(IDLE_INTERVAL);
            continue;
        }

        // Which devices are actually wanted, so one that isn't open yet is
        // looked for again rather than written off — a wheel is often still
        // coming up when the launcher starts this at boot.
        let wanted: Vec<&str> = current
            .iter()
            .filter_map(|(_, bind)| match bind {
                Bind::Button { device, .. } => Some(device.as_str()),
                Bind::Key { .. } => None,
            })
            .collect();
        devices.pump(&wanted);

        let mut down = Vec::new();
        for (action, bind) in &current {
            let is_down = match bind {
                Bind::Button { device, button } => devices.is_button_down(device, *button),
                Bind::Key { vk } => keys::is_key_down(*vk),
            };
            if is_down {
                down.push(*action);
            }
        }

        if swallow_next {
            swallow_next = false;
        } else {
            for action in down.iter().copied().filter(|action| !held.contains(action)) {
                if reader.pressed.send(action).is_err() {
                    return; // the overlay has gone
                }
            }
        }
        held = down;

        // Fast only while there is something a fast poll could catch. With the
        // wheel switched off, no device is open and every button bind is dead
        // until one appears, so 250 reads a second buys nothing — and that is
        // the state the overlay spends most of its life in, sitting on the
        // desktop between races.
        let has_key_bind = current.iter().any(|(_, bind)| matches!(bind, Bind::Key { .. }));
        let interval = if devices.any_open() || has_key_bind { POLL_INTERVAL } else { IDLE_INTERVAL };
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_round_trip() {
        for action in Action::ALL {
            assert_eq!(Action::from_slug(action.slug()), Some(action));
        }
    }

    #[test]
    fn command_line_hyphens_are_accepted() {
        assert_eq!(Action::from_slug("next-page"), Some(Action::NextPage));
        assert_eq!(Action::from_slug("  NEXT-PAGE "), Some(Action::NextPage));
        assert_eq!(Action::from_slug("nope"), None);
    }

    #[test]
    fn binds_round_trip_through_toml() {
        let bind = Bind::Button { device: "Moza Racing FSR".to_owned(), button: 21 };
        let text = toml::to_string(&bind).expect("a bind must serialize");
        let parsed: Bind = toml::from_str(&text).expect("a bind must round-trip");
        assert_eq!(parsed, bind);
    }
}
