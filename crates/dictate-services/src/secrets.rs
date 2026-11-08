//! API keys, kept in the OS credential store and nowhere else.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};

/// Credential store service name.
pub const SERVICE: &str = "Dictate";

/// The credentials the app keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Account {
    /// Key for the AI provider (edit, summaries).
    Ai,
    /// Key for the transcription service.
    Transcribe,
}

impl Account {
    /// The account name in the credential store.
    pub fn name(self) -> &'static str {
        match self {
            Account::Ai => "default",
            Account::Transcribe => "transcribe",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// No usable credential store, e.g. Linux without a Secret Service, or one that is locked.
    Unavailable,
    Other(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Unavailable => f.write_str("No system keyring available"),
            SecretError::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SecretError {}

pub trait SecretStore: Send + Sync {
    /// `Ok(None)` when no key is stored.
    fn get(&self, a: Account) -> Result<Option<String>, SecretError>;
    fn set(&self, a: Account, v: &str) -> Result<(), SecretError>;
    /// Deleting a key that isn't stored succeeds.
    fn delete(&self, a: Account) -> Result<(), SecretError>;
}

/// The OS credential store: macOS Keychain, Windows Credential Manager, Linux Secret Service.
#[derive(Debug, Default)]
pub struct KeyringStore;

impl KeyringStore {
    pub fn new() -> Self {
        KeyringStore
    }
}

fn entry(a: Account) -> Result<keyring::Entry, SecretError> {
    keyring::Entry::new(SERVICE, a.name()).map_err(map_error)
}

fn map_error(e: keyring::Error) -> SecretError {
    match e {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            SecretError::Unavailable
        }
        other => SecretError::Other(other.to_string()),
    }
}

fn read_result(r: keyring::Result<String>) -> Result<Option<String>, SecretError> {
    match r {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(map_error(e)),
    }
}

fn delete_result(r: keyring::Result<()>) -> Result<(), SecretError> {
    match r {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(map_error(e)),
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, a: Account) -> Result<Option<String>, SecretError> {
        read_result(entry(a)?.get_password())
    }

    fn set(&self, a: Account, v: &str) -> Result<(), SecretError> {
        entry(a)?.set_password(v).map_err(map_error)
    }

    fn delete(&self, a: Account) -> Result<(), SecretError> {
        delete_result(entry(a)?.delete_credential())
    }
}

/// Keys held in memory, for tests.
#[derive(Default)]
pub struct MemoryStore {
    keys: Mutex<HashMap<Account, String>>,
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore").finish_non_exhaustive()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, a: Account) -> Result<Option<String>, SecretError> {
        let keys = self.keys.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(keys.get(&a).cloned())
    }

    fn set(&self, a: Account, v: &str) -> Result<(), SecretError> {
        let mut keys = self.keys.lock().unwrap_or_else(PoisonError::into_inner);
        keys.insert(a, v.to_string());
        Ok(())
    }

    fn delete(&self, a: Account) -> Result<(), SecretError> {
        let mut keys = self.keys.lock().unwrap_or_else(PoisonError::into_inner);
        keys.remove(&a);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn platform_error(msg: &str) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(io::Error::other(msg.to_string()))
    }

    #[test]
    fn keyring_unavailable_is_reported() {
        assert_eq!(
            SecretError::Unavailable.to_string(),
            "No system keyring available"
        );
        assert_eq!(
            map_error(keyring::Error::NoStorageAccess(platform_error("locked"))),
            SecretError::Unavailable
        );
        assert_eq!(
            map_error(keyring::Error::PlatformFailure(platform_error(
                "no secret service on the bus"
            ))),
            SecretError::Unavailable
        );
    }

    #[test]
    fn other_keyring_errors_keep_their_message() {
        let err = map_error(keyring::Error::TooLong("password".into(), 2560));
        assert_eq!(
            err,
            SecretError::Other(
                "Attribute 'password' is longer than platform limit of 2560 chars".into()
            )
        );
        assert_eq!(
            err.to_string(),
            "Attribute 'password' is longer than platform limit of 2560 chars"
        );
        assert!(matches!(
            map_error(keyring::Error::BadEncoding(vec![0xff])),
            SecretError::Other(_)
        ));
    }

    #[test]
    fn a_missing_entry_is_not_an_error() {
        assert_eq!(read_result(Err(keyring::Error::NoEntry)), Ok(None));
        assert_eq!(read_result(Ok("sk-1".into())), Ok(Some("sk-1".into())));
        assert_eq!(delete_result(Err(keyring::Error::NoEntry)), Ok(()));
        assert_eq!(delete_result(Ok(())), Ok(()));
    }

    #[test]
    fn unavailable_keyring_fails_reads_and_deletes_too() {
        let down = || keyring::Error::NoStorageAccess(platform_error("locked"));
        assert_eq!(read_result(Err(down())), Err(SecretError::Unavailable));
        assert_eq!(delete_result(Err(down())), Err(SecretError::Unavailable));
    }

    #[test]
    fn accounts_and_service_match_the_spec() {
        assert_eq!(SERVICE, "Dictate");
        assert_eq!(Account::Ai.name(), "default");
        assert_eq!(Account::Transcribe.name(), "transcribe");
    }

    #[test]
    fn keyring_store_is_a_secret_store() {
        // Building it must not touch the OS keyring: entries are opened per call.
        let store: Box<dyn SecretStore> = Box::new(KeyringStore::new());
        drop(store);
    }

    #[test]
    fn memory_store_keeps_accounts_apart() {
        let store = MemoryStore::default();
        assert_eq!(store.get(Account::Ai), Ok(None));

        store.set(Account::Ai, "ai-key").unwrap();
        store.set(Account::Transcribe, "stt-key").unwrap();
        assert_eq!(store.get(Account::Ai), Ok(Some("ai-key".into())));
        assert_eq!(store.get(Account::Transcribe), Ok(Some("stt-key".into())));

        store.set(Account::Ai, "ai-key-2").unwrap();
        assert_eq!(store.get(Account::Ai), Ok(Some("ai-key-2".into())));

        store.delete(Account::Ai).unwrap();
        assert_eq!(store.get(Account::Ai), Ok(None));
        assert_eq!(store.get(Account::Transcribe), Ok(Some("stt-key".into())));
        // Deleting what isn't there is fine.
        store.delete(Account::Ai).unwrap();
    }

    #[test]
    fn memory_store_never_prints_its_secrets() {
        let store = MemoryStore::default();
        store.set(Account::Ai, "sk-very-secret").unwrap();
        assert!(!format!("{store:?}").contains("sk-very-secret"));
    }
}
