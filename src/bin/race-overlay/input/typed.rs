// Rust guideline compliant 2026-02-16

//! Typed keys for the settings window, polled the way the binds are.
//!
//! The overlay window is `WS_EX_NOACTIVATE` and must stay that way — the one
//! time it became foreground the compositor stopped compositing it (see
//! `app::report_window_state`) — so Windows will never route real keyboard
//! events to it. Clicking into a slider's number field therefore focused an
//! egui text edit that no keystroke could ever reach.
//!
//! Instead, while such a field has egui's focus, the keys a number needs are
//! read off [`super::keys::is_key_down`] each frame and replayed to egui as
//! synthetic events. Only the numeric set is watched: digits, minus, the
//! decimal point, and editing keys. Letters are left alone deliberately —
//! this is a number pad for sliders, not a keyboard hook, and the smaller
//! the set the smaller the surprise when some other window has the driver's
//! actual attention.

use super::keys::is_key_down;

/// One watched key: its virtual-key code, the text it types (if any), and
/// the egui key event it maps to (if any).
///
/// Both are sent where both apply — egui reads `Text` for the character and
/// `Key` for cursor movement and shortcuts, the same pair a real key press
/// delivers.
struct Watched {
    vk: u16,
    text: Option<&'static str>,
    key: Option<egui::Key>,
}

/// Every key the settings window's number fields need.
///
/// Digits appear twice because the top row and the numpad have different
/// virtual-key codes. Escape is deliberately absent: the settings window
/// already closes on it, and a synthetic escape would close the window under
/// the field being edited.
const WATCHED: &[Watched] = &[
    Watched { vk: 0x30, text: Some("0"), key: None },
    Watched { vk: 0x31, text: Some("1"), key: None },
    Watched { vk: 0x32, text: Some("2"), key: None },
    Watched { vk: 0x33, text: Some("3"), key: None },
    Watched { vk: 0x34, text: Some("4"), key: None },
    Watched { vk: 0x35, text: Some("5"), key: None },
    Watched { vk: 0x36, text: Some("6"), key: None },
    Watched { vk: 0x37, text: Some("7"), key: None },
    Watched { vk: 0x38, text: Some("8"), key: None },
    Watched { vk: 0x39, text: Some("9"), key: None },
    // VK_NUMPAD0..=VK_NUMPAD9.
    Watched { vk: 0x60, text: Some("0"), key: None },
    Watched { vk: 0x61, text: Some("1"), key: None },
    Watched { vk: 0x62, text: Some("2"), key: None },
    Watched { vk: 0x63, text: Some("3"), key: None },
    Watched { vk: 0x64, text: Some("4"), key: None },
    Watched { vk: 0x65, text: Some("5"), key: None },
    Watched { vk: 0x66, text: Some("6"), key: None },
    Watched { vk: 0x67, text: Some("7"), key: None },
    Watched { vk: 0x68, text: Some("8"), key: None },
    Watched { vk: 0x69, text: Some("9"), key: None },
    // VK_OEM_PERIOD and VK_DECIMAL.
    Watched { vk: 0xBE, text: Some("."), key: None },
    Watched { vk: 0x6E, text: Some("."), key: None },
    // VK_OEM_MINUS and VK_SUBTRACT.
    Watched { vk: 0xBD, text: Some("-"), key: Some(egui::Key::Minus) },
    Watched { vk: 0x6D, text: Some("-"), key: Some(egui::Key::Minus) },
    // Editing: backspace, delete, enter, and the cursor keys.
    Watched { vk: 0x08, text: None, key: Some(egui::Key::Backspace) },
    Watched { vk: 0x2E, text: None, key: Some(egui::Key::Delete) },
    Watched { vk: 0x0D, text: None, key: Some(egui::Key::Enter) },
    Watched { vk: 0x25, text: None, key: Some(egui::Key::ArrowLeft) },
    Watched { vk: 0x27, text: None, key: Some(egui::Key::ArrowRight) },
    Watched { vk: 0x26, text: None, key: Some(egui::Key::ArrowUp) },
    Watched { vk: 0x28, text: None, key: Some(egui::Key::ArrowDown) },
    Watched { vk: 0x24, text: None, key: Some(egui::Key::Home) },
    Watched { vk: 0x23, text: None, key: Some(egui::Key::End) },
];

/// Edge-detects the watched keys and replays them as egui events.
#[derive(Debug, Default)]
pub struct TypedKeys {
    /// Which watched keys were down last frame, by index into [`WATCHED`].
    down: Vec<bool>,
}

impl TypedKeys {
    /// Appends this frame's key edges to `events` while `active`.
    ///
    /// Inactive frames still clear the held state, so a key held across the
    /// moment a field gains focus is not replayed as a fresh press, and one
    /// held across losing it cannot ghost a release into the next field.
    pub fn poll(&mut self, active: bool, events: &mut Vec<egui::Event>) {
        if !active {
            self.down.clear();
            return;
        }
        self.down.resize(WATCHED.len(), false);
        for (watched, was_down) in WATCHED.iter().zip(&mut self.down) {
            let is_down = is_key_down(watched.vk);
            if is_down == *was_down {
                continue;
            }
            *was_down = is_down;
            if let Some(key) = watched.key {
                events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: is_down,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                });
            }
            if is_down && let Some(text) = watched.text {
                events.push(egui::Event::Text(text.to_owned()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Going inactive must drop the held state: a key still physically down
    /// when the field regains focus would otherwise never register again —
    /// and, worse, a release edge would be sent into a fresh session.
    #[test]
    fn deactivating_forgets_held_keys() {
        let mut typed = TypedKeys { down: vec![true; WATCHED.len()] };
        let mut events = Vec::new();
        typed.poll(false, &mut events);
        assert!(events.is_empty());
        assert!(typed.down.is_empty());
    }

    /// Escape must not be watched: the settings window closes on it, so a
    /// synthetic escape would slam the window shut mid-edit.
    #[test]
    fn escape_is_not_replayed() {
        assert!(WATCHED.iter().all(|watched| watched.vk != 0x1B));
    }

    /// Every digit both keyboards offer, so a number can be typed from
    /// either.
    #[test]
    fn both_digit_rows_are_covered() {
        for offset in 0..10_u16 {
            assert!(WATCHED.iter().any(|watched| watched.vk == 0x30 + offset), "top-row digit {offset}");
            assert!(WATCHED.iter().any(|watched| watched.vk == 0x60 + offset), "numpad digit {offset}");
        }
    }
}
