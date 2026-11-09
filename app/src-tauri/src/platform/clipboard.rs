//! The clipboard of Windows and Linux, through arboard.
//!
//! arboard reads the text, the HTML, a picture and a list of files, and writes one kind of them at a time, so a
//! snapshot keeps all four and the restore puts back the one that matters most (see [`choose_restore`]); formats
//! of apps' own (RTF, private ones) are lost. The text of a paste is marked so that clipboard history and the
//! cloud clipboard skip it: with `ExcludeClipboardContentFromMonitorProcessing` on Windows (history and cloud),
//! and with the `x-kde-passwordManagerHint` type that managers on Linux honour.
//!
//! On X11 the clipboard lives in the process that put something on it, so the arboard clipboard is made once and
//! kept: the text must still be there when the app that is pasted into asks for it, and when the user pastes it
//! by hand after a paste that could not be done.

use std::sync::{Mutex, MutexGuard, PoisonError};

use arboard::{Clipboard as Board, ImageData, Set};

use super::paste::{choose_restore, ChangeMark, ClipSnapshot, Clipboard, Contents, Image, Restore};

pub struct DesktopClipboard {
    /// Made when it is first needed, and kept.
    board: Mutex<Option<Board>>,
    /// Linux: how many texts were put on the clipboard, and the last one. There is no counter to ask, so "nobody
    /// copied since" is "our text is still there".
    #[cfg(target_os = "linux")]
    written: Mutex<(u64, String)>,
}

impl DesktopClipboard {
    pub fn new() -> DesktopClipboard {
        DesktopClipboard {
            board: Mutex::new(None),
            #[cfg(target_os = "linux")]
            written: Mutex::new((0, String::new())),
        }
    }

    /// Runs `job` with the clipboard, which is opened the first time.
    fn with_board<T>(&self, job: impl FnOnce(&mut Board) -> T) -> Result<T, String> {
        let mut guard: MutexGuard<'_, Option<Board>> =
            self.board.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(Board::new().map_err(|error| error.to_string())?);
        }
        match guard.as_mut() {
            Some(board) => Ok(job(board)),
            None => Err("no clipboard".into()),
        }
    }

    /// The mark of the clipboard after `text` was put on it.
    #[cfg(target_os = "windows")]
    fn mark_after_write(&self, _text: &str) -> ChangeMark {
        ChangeMark(sequence_number())
    }

    #[cfg(target_os = "linux")]
    fn mark_after_write(&self, text: &str) -> ChangeMark {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        written.0 += 1;
        written.1 = text.to_owned();
        ChangeMark(written.0)
    }

    #[cfg(target_os = "windows")]
    fn changed(&self, mark: &ChangeMark) -> bool {
        sequence_number() != mark.0
    }

    #[cfg(target_os = "linux")]
    fn changed(&self, mark: &ChangeMark) -> bool {
        let (count, text) = self
            .written
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        count != mark.0
            || self
                .with_board(|board| board.get_text().ok())
                .ok()
                .flatten()
                .as_deref()
                != Some(text.as_str())
    }
}

/// The number that Windows counts up whenever the clipboard changes.
#[cfg(target_os = "windows")]
fn sequence_number() -> u64 {
    // SAFETY: a call with no arguments.
    u64::from(unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() })
}

/// Marks what is set as not to be kept by a clipboard history.
fn excluded(set: Set<'_>) -> Set<'_> {
    #[cfg(target_os = "windows")]
    {
        use arboard::SetExtWindows;
        set.exclude_from_cloud().exclude_from_history()
    }
    #[cfg(target_os = "linux")]
    {
        use arboard::SetExtLinux;
        set.exclude_from_history()
    }
}

impl Clipboard for DesktopClipboard {
    fn snapshot(&self) -> ClipSnapshot {
        let contents = self
            .with_board(|board| Contents {
                text: board.get_text().ok(),
                html: board.get().html().ok(),
                image: board.get_image().ok().map(|image| Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.bytes.into_owned(),
                }),
                files: board.get().file_list().unwrap_or_default(),
            })
            .unwrap_or_default();
        ClipSnapshot::Contents(contents)
    }

    fn write_transient(&self, text: &str) -> Result<ChangeMark, String> {
        self.with_board(|board| excluded(board.set()).text(text).map_err(|e| e.to_string()))??;
        Ok(self.mark_after_write(text))
    }

    fn changed_since(&self, mark: &ChangeMark) -> bool {
        self.changed(mark)
    }

    fn restore(&self, snapshot: ClipSnapshot) {
        let ClipSnapshot::Contents(contents) = snapshot else {
            return;
        };
        // What is put back is the user's own, so it is not marked.
        let done = self.with_board(|board| match choose_restore(&contents) {
            Restore::Files => board.set().file_list(&contents.files),
            Restore::Text => {
                let text = contents.text.as_deref().unwrap_or_default();
                match contents.html.as_deref() {
                    Some(html) => board.set().html(html, Some(text)),
                    None => board.set().text(text),
                }
            }
            Restore::Image => match &contents.image {
                Some(image) => board.set_image(ImageData {
                    width: image.width,
                    height: image.height,
                    bytes: image.rgba.as_slice().into(),
                }),
                None => Ok(()),
            },
            Restore::Html => board
                .set()
                .html(contents.html.as_deref().unwrap_or_default(), None),
            Restore::Nothing => board.clear(),
        });
        match done {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!("could not give the clipboard back: {error}"),
            Err(error) => log::warn!("could not open the clipboard to give it back: {error}"),
        }
    }

    fn copy(&self, text: &str) -> Result<(), String> {
        self.with_board(|board| board.set_text(text).map_err(|e| e.to_string()))??;
        // A copy is a change that a restore that is waiting must see (Windows counts it by itself).
        let _ = self.mark_after_write(text);
        Ok(())
    }
}
