//! The recording bar: the small window that shows the time, the meters and the buttons while a recording goes on. It is shown
//! when a recording begins and hidden when it is over.

use tauri::{AppHandle, Runtime};

/// The label of the window of the bar. The recorder tells it its state, as it tells the meetings window.
pub const LABEL: &str = "recorder";

/// Shows the bar. The bar is not made yet: this does nothing until it is.
pub fn show<R: Runtime>(_app: &AppHandle<R>) {}

/// Hides the bar. The bar is not made yet: this does nothing until it is.
pub fn hide<R: Runtime>(_app: &AppHandle<R>) {}
