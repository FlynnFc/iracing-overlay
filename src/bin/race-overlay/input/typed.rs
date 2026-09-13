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
//! synthetic events. The numeric set and clipboard shortcuts are watched:
//! digits, minus, decimal point, editing keys, and Ctrl+A/C/V/X. Ordinary
//! letters are left alone deliberately —
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
    shortcut_down: [bool; 4],
}

impl TypedKeys {
    /// Appends this frame's key edges to `events` while `active`.
    ///
    /// Inactive frames still clear the held state, so a key held across the
    /// moment a field gains focus is not replayed as a fresh press, and one
    /// held across losing it cannot ghost a release into the next field.
    /// Returns true only on a Paste shortcut edge. The window backend reads
    /// the clipboard on that explicit request, not on ordinary frames.
    pub fn poll(&mut self, active: bool, events: &mut Vec<egui::Event>) -> bool {
        self.poll_keys(active, events, is_key_down)
    }

    fn poll_keys(&mut self, active: bool, events: &mut Vec<egui::Event>, key_down: impl Fn(u16) -> bool) -> bool {
        if !active {
            self.down.clear();
            self.shortcut_down = [false; 4];
            return false;
        }
        let ctrl = key_down(0x11);
        let modifiers =
            egui::Modifiers { ctrl, command: ctrl, shift: key_down(0x10), alt: key_down(0x12), ..Default::default() };
        let mut paste = false;
        for (index, vk) in [0x41, 0x43, 0x56, 0x58].into_iter().enumerate() {
            let down = key_down(vk);
            if ctrl && down && !self.shortcut_down[index] {
                match vk {
                    0x41 => events.push(egui::Event::Key {
                        key: egui::Key::A,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }),
                    0x43 => events.push(egui::Event::Copy),
                    0x56 => paste = true,
                    0x58 => events.push(egui::Event::Cut),
                    _ => unreachable!(),
                }
            }
            self.shortcut_down[index] = down;
        }
        self.down.resize(WATCHED.len(), false);
        for (watched, was_down) in WATCHED.iter().zip(&mut self.down) {
            let is_down = key_down(watched.vk);
            if is_down == *was_down {
                continue;
            }
            *was_down = is_down;
            if let Some(key) = watched.key {
                events.push(egui::Event::Key { key, physical_key: None, pressed: is_down, repeat: false, modifiers });
            }
            if is_down
                && !modifiers.ctrl
                && !modifiers.alt
                && let Some(text) = watched.text
            {
                events.push(egui::Event::Text(text.to_owned()));
            }
        }
        paste
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
        let mut typed = TypedKeys { down: vec![true; WATCHED.len()], ..Default::default() };
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

    #[test]
    fn clipboard_shortcuts_are_edge_triggered_and_require_a_focused_field() {
        let mut typed = TypedKeys::default();
        let mut events = Vec::new();
        assert!(!typed.poll_keys(false, &mut events, |vk| [0x11, 0x56].contains(&vk)));
        assert!(typed.poll_keys(true, &mut events, |vk| [0x11, 0x56].contains(&vk)));
        assert!(
            !typed.poll_keys(true, &mut events, |vk| [0x11, 0x56].contains(&vk)),
            "holding Ctrl+V must not repeat paste"
        );
        assert!(events.is_empty());
        typed.poll_keys(true, &mut events, |_| false);
        typed.poll_keys(true, &mut events, |vk| [0x11, 0x43].contains(&vk));
        assert!(matches!(events.last(), Some(egui::Event::Copy)));
        typed.poll_keys(true, &mut events, |vk| [0x11, 0x58].contains(&vk));
        assert!(matches!(events.last(), Some(egui::Event::Cut)));
    }

    #[test]
    fn select_all_and_shift_selection_preserve_modifiers_without_typing_control_digits() {
        let mut typed = TypedKeys::default();
        let mut events = Vec::new();
        typed.poll_keys(true, &mut events, |vk| [0x11, 0x41, 0x31].contains(&vk));
        assert!(
            matches!(events.as_slice(), [egui::Event::Key { key: egui::Key::A, modifiers, .. }] if modifiers.command)
        );
        events.clear();
        typed.poll_keys(true, &mut events, |vk| [0x10, 0x25].contains(&vk));
        assert!(
            matches!(events.as_slice(), [egui::Event::Key { key: egui::Key::ArrowLeft, modifiers, .. }] if modifiers.shift)
        );
    }
}
