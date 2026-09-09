// Rust guideline compliant 2026-02-16

//! Keyboard binds, polled rather than hooked.
//!
//! `GetAsyncKeyState` reports a key's physical state regardless of which
//! window has focus, which is what this needs: the overlay never has focus,
//! and iRacing does. A low-level keyboard hook would also work but installs
//! itself system-wide and blocks the input queue if it ever stalls — a poll
//! that reads a bit each frame cannot.

use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

/// Function keys `F1`-`F12`, whose virtual-key codes are contiguous from
/// `VK_F1`. iRacing binds its own black box pages to these by default, so
/// they're the natural fallback when no wheel is connected.
pub const VK_F1: u16 = 0x70;

/// The high bit of a `GetAsyncKeyState` result: "this key is down now".
/// The low bit means "was pressed since the last call", which is deliberately
/// not used — it is consumed by whoever reads it first, so two callers race.
const KEY_DOWN_MASK: i16 = -0x8000; // 0x8000 as i16

/// Whether the key with this virtual-key code is physically down.
#[must_use]
pub fn is_key_down(vk: u16) -> bool {
    // SAFETY: `GetAsyncKeyState` takes an integer virtual-key code and reads
    // only process-global input state; it has no preconditions and cannot
    // fail. Out-of-range codes simply report not-pressed.
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    state & KEY_DOWN_MASK != 0
}

/// The range of virtual-key codes worth offering as binds.
///
/// Starts past the mouse buttons, which would fire constantly, and stops
/// before the OEM punctuation whose codes vary by keyboard layout.
const BINDABLE_FIRST: u16 = 0x08;
const BINDABLE_LAST: u16 = 0x87;

/// Whichever bindable key is held right now, if any.
///
/// Used by `--bind` so a keyboard bind can be captured the same way a wheel
/// button is, rather than needing a virtual-key code written into the config
/// by hand.
#[must_use]
pub fn pressed() -> Option<u16> {
    // Searched from the top down, so a function key wins over a modifier
    // that happens to be held alongside it.
    (BINDABLE_FIRST..=BINDABLE_LAST).rfind(|vk| is_key_down(*vk))
}

/// A readable name for a virtual-key code, for config files and `--bind`.
#[must_use]
pub fn key_name(vk: u16) -> String {
    match vk {
        0x70..=0x7B => format!("F{}", vk - VK_F1 + 1),
        // Digits and letters use their ASCII value as their virtual-key code.
        0x30..=0x39 | 0x41..=0x5A => char::from(u8::try_from(vk).unwrap_or(b'?')).to_string(),
        _ => format!("VK 0x{vk:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bindable_range_covers_the_function_keys_but_not_the_mouse() {
        let bindable = BINDABLE_FIRST..=BINDABLE_LAST;
        assert!(bindable.contains(&VK_F1));
        assert!(bindable.contains(&0x7B), "F12");
        assert!(!bindable.contains(&0x01), "left mouse button");
    }

    #[test]
    fn key_names_read_the_way_a_user_would_write_them() {
        assert_eq!(key_name(0x70), "F1");
        assert_eq!(key_name(0x7B), "F12");
        assert_eq!(key_name(0x41), "A");
        assert_eq!(key_name(0x31), "1");
        assert_eq!(key_name(0x01), "VK 0x01");
    }

    /// The mask must select the "down now" bit, not the "pressed since last
    /// call" bit, which is consumed on read and so races between callers.
    #[test]
    fn the_down_mask_is_the_high_bit() {
        assert_eq!(KEY_DOWN_MASK, i16::MIN, "0x8000 is the sign bit of an i16");
    }
}
