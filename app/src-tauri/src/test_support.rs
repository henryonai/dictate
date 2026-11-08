//! Test doubles the tests of several modules share.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use dictate_services::secrets::{Account, MemoryStore, SecretError, SecretStore};

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
