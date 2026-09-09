// Rust guideline compliant 2026-02-16

//! The notification-area (system tray) icon and its menu.
//!
//! The overlay has no window of its own worth switching to — it's a
//! transparent, click-through sheet over the whole screen — so it keeps out
//! of the taskbar and alt-tab entirely (see `app::hide_from_taskbar`) and
//! lives in the tray instead. That makes the tray the only way to quit it,
//! which is why the menu is built before the window comes up rather than
//! lazily.
//!
//! It is also the quickest way to choose which panels are on screen — one
//! tickable entry per panel — and where the settings window is opened from.

use anyhow::Context;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// One of the overlay's panels, as the tray menu names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// The Relative, and with it the whole black box it is page one of.
    Relative,
    Standings,
    RadarBars,
    /// The warning that a quicker class is coming up behind.
    FasterClass,
}

impl Panel {
    /// Every panel, in the order the menu lists them — top of the screen to
    /// bottom, roughly, as the default layout places them.
    pub const ALL: [Self; 4] = [Self::Standings, Self::Relative, Self::RadarBars, Self::FasterClass];

    /// The menu entry's text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Relative => "Relative & Black Box",
            Self::Standings => "Standings",
            Self::RadarBars => "Radar Bars",
            Self::FasterClass => "Faster Class",
        }
    }
}

/// The tray icon, kept alive for the process's lifetime.
///
/// Dropping this removes the icon from the notification area, so it is held
/// rather than discarded even though nothing reads it back.
pub struct Tray {
    #[expect(dead_code, reason = "held only so the icon stays in the notification area until exit")]
    icon: TrayIcon,
    quit_id: MenuId,
    settings_id: MenuId,
}

/// What the user asked of the tray menu since the last frame.
#[derive(Debug, Clone, Default)]
pub struct TrayActions {
    /// The overlay should shut down.
    pub quit: bool,
    /// The settings window should come up.
    pub open_settings: bool,
}

/// `TrayIcon` itself is not `Debug`, but the crate's lint set requires every
/// public type to be — and the useful thing to print is whether the icon
/// exists at all, not its internals.
impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").field("quit_id", &self.quit_id).finish_non_exhaustive()
    }
}

impl Tray {
    /// Builds the tray icon and its menu.
    ///
    /// # Errors
    /// Returns an error if the icon image can't be decoded or the shell
    /// refuses to register the notification-area entry.
    pub fn new() -> anyhow::Result<Self> {
        let menu = Menu::new();

        let settings = MenuItem::new("Open Settings", true, None);
        let settings_id = settings.id().clone();
        menu.append(&settings).context("adding the tray menu's settings item")?;
        menu.append(&PredefinedMenuItem::separator()).context("adding a tray menu separator")?;

        let quit = MenuItem::new("Quit Race Overlay", true, None);
        let quit_id = quit.id().clone();
        menu.append(&quit).context("adding the tray menu's quit item")?;

        let icon = TrayIconBuilder::new()
            .with_tooltip("Race Overlay")
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .with_menu(Box::new(menu))
            .with_icon(load_icon()?)
            .build()
            .context("registering the tray icon")?;

        Ok(Self { icon, quit_id, settings_id })
    }

    /// Drains pending tray-menu events into the actions they stand for.
    ///
    /// Called once per frame from the paint loop, which is what pumps the
    /// thread's message queue that the tray icon's own hidden window
    /// receives on.
    pub fn poll(&self) -> TrayActions {
        let mut actions = TrayActions::default();
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                actions.open_settings = true;
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.quit_id {
                actions.quit = true;
            } else if event.id == self.settings_id {
                actions.open_settings = true;
            }
        }
        actions
    }
}

/// Decodes the embedded icon into the RGBA buffer the tray expects.
fn load_icon() -> anyhow::Result<tray_icon::Icon> {
    /// The same artwork the executable's own icon is built from, embedded so
    /// the tray never depends on a file being present beside the exe.
    const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

    let decoder = png::Decoder::new(ICON_PNG);
    let mut reader = decoder.read_info().context("reading the tray icon's PNG header")?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).context("decoding the tray icon")?;
    buffer.truncate(info.buffer_size());

    tray_icon::Icon::from_rgba(buffer, info.width, info.height).context("building the tray icon from its pixels")
}
