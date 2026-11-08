//! The paste shortcut of Windows and of X11, pressed through enigo (`SendInput` on Windows, the XTest extension on
//! X11). macOS posts its own key events, and Wayland has no way to press keys that every compositor allows.

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use super::paste::PasteError;

/// Presses Ctrl+V, or Ctrl+Shift+V (what terminals paste with) when `shift` is set. The modifiers always come up
/// again, even when the V could not be sent.
pub fn press_paste(shift: bool) -> Result<(), PasteError> {
    let failed = |error: enigo::InputError| PasteError::Failed(error.to_string());
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|error| PasteError::Failed(error.to_string()))?;
    enigo.key(Key::Control, Direction::Press).map_err(failed)?;
    let shifted = if shift {
        enigo.key(Key::Shift, Direction::Press)
    } else {
        Ok(())
    };
    let v = shifted.and_then(|()| enigo.key(v_key(), Direction::Click));
    let shift_up = if shift {
        enigo.key(Key::Shift, Direction::Release)
    } else {
        Ok(())
    };
    let control_up = enigo.key(Key::Control, Direction::Release);
    v.and(shift_up).and(control_up).map_err(failed)
}

/// The V of the paste shortcut on Windows: the virtual key VK_V, which Ctrl+V is bound to on every layout.
/// `Key::Unicode('v')` would ask the layout for a key that types a "v" instead (`VkKeyScanExW`), and Cyrillic,
/// Greek, Arabic and Hebrew layouts have none: enigo then types the character as Unicode text, which a shortcut
/// never sees, so nothing would be pasted (or a stray "v" would be typed) while the paste is reported as done.
#[cfg(target_os = "windows")]
fn v_key() -> Key {
    Key::V
}

/// The V of the paste shortcut on X11: the key that the keymap has for the keysym "v" (enigo has no
/// virtual-key V there, and finds or maps a key for it).
#[cfg(not(target_os = "windows"))]
fn v_key() -> Key {
    Key::Unicode('v')
}

// These run in CI on Windows and Linux (and compile in the scratch checks): the file is built on no other system.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn on_windows_the_paste_key_is_the_virtual_key_and_not_a_character() {
        assert_eq!(v_key(), Key::V);
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn on_x11_the_paste_key_is_the_keysym_v() {
        assert_eq!(v_key(), Key::Unicode('v'));
    }
}
