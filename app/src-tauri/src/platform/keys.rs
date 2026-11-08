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
    // The key is the V of the layout: the shortcut is "Ctrl and the key that types a v".
    let v = shifted.and_then(|()| enigo.key(Key::Unicode('v'), Direction::Click));
    let shift_up = if shift {
        enigo.key(Key::Shift, Direction::Release)
    } else {
        Ok(())
    };
    let control_up = enigo.key(Key::Control, Direction::Release);
    v.and(shift_up).and(control_up).map_err(failed)
}
