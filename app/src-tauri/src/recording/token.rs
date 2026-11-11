//! Where the restore token of the ScreenCast portal is kept between recordings: in the settings (`screencast_restore_token`), the
//! text as the capture gave it.
//!
//! The portal asks for it, and hands it over, on the thread that all of the app's calls to the portal share, so neither may wait
//! for the disk: the token is kept in memory and written by a thread of its own.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use dictate_services::record::capture::TokenStore;
use dictate_services::settings::SettingsStore;

/// What writes a token down.
type Persist = Box<dyn FnMut(&str) + Send>;

/// A [`TokenStore`] that answers from memory and has `persist` write what it was given on a thread of its own, one at a time. A token
/// that comes while another is being written replaces the ones that wait: only the last of them is written.
pub struct AsyncTokens {
    current: Mutex<Option<String>>,
    /// To the writer, which is started by the first token to be saved: the systems that have no portal never have one.
    writer: Mutex<Option<Sender<String>>>,
    persist: Mutex<Option<Persist>>,
}

impl AsyncTokens {
    /// `initial` is what was kept the last time; `persist` writes a token down.
    pub fn new(initial: Option<String>, persist: impl FnMut(&str) + Send + 'static) -> AsyncTokens {
        AsyncTokens {
            current: Mutex::new(initial),
            writer: Mutex::new(None),
            persist: Mutex::new(Some(Box::new(persist))),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TokenStore for AsyncTokens {
    fn load(&self) -> Option<String> {
        lock(&self.current).clone()
    }

    fn save(&self, token: &str) {
        *lock(&self.current) = Some(token.to_owned());
        let mut writer = lock(&self.writer);
        if writer.is_none() {
            let Some(mut persist) = lock(&self.persist).take() else {
                return;
            };
            let (tx, rx) = mpsc::channel::<String>();
            let started = std::thread::Builder::new()
                .name("restore-token".into())
                .spawn(move || {
                    while let Ok(mut token) = rx.recv() {
                        // What came while the last one was being written: only the newest is worth the disk.
                        while let Ok(newer) = rx.try_recv() {
                            token = newer;
                        }
                        persist(&token);
                    }
                });
            match started {
                Ok(_) => *writer = Some(tx),
                Err(e) => {
                    log::warn!("Could not start the writer of the restore token: {e}");
                    return;
                }
            }
        }
        if let Some(tx) = writer.as_ref() {
            let _ = tx.send(token.to_owned());
        }
    }
}

/// The tokens of the app: read from the settings now, and written to them as they come.
pub fn for_settings(settings: Arc<SettingsStore>) -> Arc<AsyncTokens> {
    let initial = settings.get().screencast_restore_token;
    Arc::new(AsyncTokens::new(initial, move |token| {
        let saved = settings.update(|s| s.screencast_restore_token = Some(token.to_owned()));
        if let Err(e) = saved {
            log::warn!("Could not keep the restore token of the screen: {e}");
        }
    }))
}
