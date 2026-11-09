//! Test doubles the tests of several modules share.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use dictate_services::secrets::{Account, MemoryStore, SecretError, SecretStore};
use tauri_plugin_global_shortcut::Shortcut;

use crate::hotkey::Registry;
use crate::state::lock;

/// A key store in memory that counts how often it is asked for a key, and can take its time answering like a
/// Keychain that waits for a prompt.
#[derive(Default)]
pub struct CountingStore {
    inner: MemoryStore,
    gets: AtomicUsize,
    delay: Duration,
}

impl CountingStore {
    /// Every `get` takes `delay`.
    pub fn slow(delay: Duration) -> CountingStore {
        CountingStore {
            delay,
            ..CountingStore::default()
        }
    }

    /// How many times a key was read so far.
    pub fn gets(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }
}

impl SecretStore for CountingStore {
    fn get(&self, a: Account) -> Result<Option<String>, SecretError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        thread::sleep(self.delay);
        self.inner.get(a)
    }

    fn set(&self, a: Account, v: &str) -> Result<(), SecretError> {
        self.inner.set(a, v)
    }

    fn delete(&self, a: Account) -> Result<(), SecretError> {
        self.inner.delete(a)
    }
}

/// A key store that is down, like a Linux desktop without a Secret Service.
pub struct NoKeyring;

impl SecretStore for NoKeyring {
    fn get(&self, _: Account) -> Result<Option<String>, SecretError> {
        Err(SecretError::Unavailable)
    }
    fn set(&self, _: Account, _: &str) -> Result<(), SecretError> {
        Err(SecretError::Unavailable)
    }
    fn delete(&self, _: Account) -> Result<(), SecretError> {
        Err(SecretError::Unavailable)
    }
}

/// The shortcuts of a desktop, in memory: which ones this app has registered, which ones another app owns (they
/// can't be registered), and how often it was asked to register or let go of one.
#[derive(Default)]
pub struct FakeRegistry {
    registered: Mutex<Vec<Shortcut>>,
    owned_by_another_app: Mutex<Vec<Shortcut>>,
    calls: AtomicUsize,
}

impl FakeRegistry {
    /// Another app owns this shortcut: registering it fails.
    pub fn refuse(&self, shortcut: Shortcut) {
        lock(&self.owned_by_another_app).push(shortcut);
    }

    pub fn registered(&self) -> Vec<Shortcut> {
        lock(&self.registered).clone()
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Registry for FakeRegistry {
    fn register(&self, shortcut: Shortcut) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut registered = lock(&self.registered);
        if lock(&self.owned_by_another_app).contains(&shortcut) || registered.contains(&shortcut) {
            return Err("the shortcut is already registered".into());
        }
        registered.push(shortcut);
        Ok(())
    }

    fn unregister(&self, shortcut: Shortcut) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        lock(&self.registered).retain(|s| *s != shortcut);
        Ok(())
    }
}

/// Writes a runnable script into `dir` and returns its path: a shell script with the body `unix` on Unix, a batch file
/// with the body `windows` on Windows. A stand-in for a program such as yt-dlp.
pub fn script(dir: &Path, name: &str, unix: &str, windows: &str) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{unix}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = windows;
        path
    }
    #[cfg(windows)]
    {
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(&path, format!("@echo off\r\n{windows}\r\n")).unwrap();
        let _ = unix;
        path
    }
}
