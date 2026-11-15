//! API keys, kept in the OS credential store and nowhere else.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Mutex, PoisonError};

/// Credential store service name.
pub const SERVICE: &str = "Loah";

/// The service name of Dictate 2.0.0, whose keys Loah reads when it has none of its own (the Swift Dictate used another
/// service, `local.dictate.apikey`, and its keys are not imported).
pub const LEGACY_SERVICE: &str = "Dictate";

/// What Loah's own entry holds after a key that came from Dictate was removed in Loah: without it the next read would
/// find Dictate's entry again, and the key the person took out would be back. It is never read as a key.
const REMOVED: &str = "(removed)";

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
    /// No usable credential store, e.g. one that is locked or whose keychain is missing.
    Unavailable,
    /// The credential store failed at runtime: a Linux without a Secret Service to talk to, but as well a prompt
    /// that the user denied or cancelled. Holds what the platform said, which tells these apart.
    PlatformFailure(String),
    Other(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Unavailable => f.write_str("No system keyring available"),
            SecretError::PlatformFailure(detail) => write!(
                f,
                "No system keyring available, or access to it was denied ({detail})"
            ),
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

/// A credential store as `KeyringStore` uses it: entries by service and account. Apart from the OS one a test has its own.
trait Vault: Send + Sync {
    /// `Ok(None)` when there is no such entry.
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretError>;
    fn write(&self, service: &str, account: &str, value: &str) -> Result<(), SecretError>;
    /// Removing an entry that isn't there succeeds.
    fn remove(&self, service: &str, account: &str) -> Result<(), SecretError>;
}

/// The credential store of the OS: macOS Keychain, Windows Credential Manager, Linux Secret Service.
struct OsVault;

fn entry(service: &str, account: &str) -> Result<keyring::Entry, SecretError> {
    keyring::Entry::new(service, account).map_err(map_error)
}

fn map_error(e: keyring::Error) -> SecretError {
    match e {
        keyring::Error::NoStorageAccess(_) => SecretError::Unavailable,
        keyring::Error::PlatformFailure(detail) => SecretError::PlatformFailure(detail.to_string()),
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

impl Vault for OsVault {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretError> {
        read_result(entry(service, account)?.get_password())
    }

    fn write(&self, service: &str, account: &str, value: &str) -> Result<(), SecretError> {
        entry(service, account)?
            .set_password(value)
            .map_err(map_error)
    }

    fn remove(&self, service: &str, account: &str) -> Result<(), SecretError> {
        delete_result(entry(service, account)?.delete_credential())
    }
}

/// The keys of the app, in the credential store of the OS: under the service `Loah`, and, until there is one there, the
/// key that Dictate 2.0.0 saved under `Dictate`, which is copied to `Loah` when it is first read and left where it is.
pub struct KeyringStore {
    vault: Box<dyn Vault>,
    /// The accounts for which Dictate's entry was asked in this run and gave no key. Asking again would, on a Mac, show
    /// the Keychain's prompt again (for an entry another app made) after the person refused it.
    legacy_asked: Mutex<HashSet<Account>>,
    /// Held while Loah's own entry is written: by a key that is saved, and by the copy of Dictate's key (which looks at the entry
    /// again before it writes). Not held while Dictate's entry is read, which can wait a long time for a prompt: a key that is
    /// saved then must not wait for it, and must not be overwritten by it either.
    writing: Mutex<()>,
}

impl fmt::Debug for KeyringStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyringStore").finish_non_exhaustive()
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        KeyringStore::new()
    }
}

impl KeyringStore {
    /// Building it must not touch the OS keyring: entries are opened per call.
    pub fn new() -> Self {
        KeyringStore::with_vault(Box::new(OsVault))
    }

    fn with_vault(vault: Box<dyn Vault>) -> Self {
        KeyringStore {
            vault,
            legacy_asked: Mutex::new(HashSet::new()),
            writing: Mutex::new(()),
        }
    }

    /// The key that Dictate saved, which is saved under Loah as well. A refusal of the store is the error, once per run.
    fn key_of_dictate(&self, a: Account) -> Result<Option<String>, SecretError> {
        let mut asked = self
            .legacy_asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Looked at again under the lock: another thread may have saved Loah's entry (or removed the key) while this one waited,
        // and asking Dictate's entry then would be a second prompt for nothing.
        match self.vault.read(SERVICE, a.name())? {
            Some(removed) if removed == REMOVED => return Ok(None),
            Some(key) => return Ok(Some(key)),
            None => {}
        }
        if asked.contains(&a) {
            return Ok(None);
        }
        let found = self.vault.read(LEGACY_SERVICE, a.name());
        if !matches!(found, Ok(Some(_))) {
            asked.insert(a);
        }
        let Some(key) = found? else {
            return Ok(None);
        };
        // The read of Dictate's entry may have waited for the person (the prompt of the Keychain), who may have saved a key in
        // Loah or removed it meanwhile. That is not undone by a copy of Dictate's key: Loah's entry is written only if it is still
        // empty, and nobody else writes it between the look and the write.
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        match self.vault.read(SERVICE, a.name()) {
            Ok(Some(removed)) if removed == REMOVED => return Ok(None),
            Ok(Some(saved)) => return Ok(Some(saved)),
            Ok(None) => {}
            // Loah's entry can't be looked at: Dictate's key is used, and copied when it can.
            Err(_) => return Ok(Some(key)),
        }
        // A key that can't be saved under Loah now is still used, and copied the next time.
        let _ = self.vault.write(SERVICE, a.name(), &key);
        Ok(Some(key))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, a: Account) -> Result<Option<String>, SecretError> {
        match self.vault.read(SERVICE, a.name())? {
            Some(removed) if removed == REMOVED => Ok(None),
            Some(key) => Ok(Some(key)),
            None => self.key_of_dictate(a),
        }
    }

    fn set(&self, a: Account, v: &str) -> Result<(), SecretError> {
        // Not while the copy of Dictate's key looks at Loah's entry and writes it: that would be written over this key.
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        self.vault.write(SERVICE, a.name(), v)
    }

    fn delete(&self, a: Account) -> Result<(), SecretError> {
        self.vault.remove(SERVICE, a.name())?;
        // Dictate's entry is Dictate's, and stays. But the next read must not find it and bring the key back: where there
        // is one (or the store can't say), Loah's entry says that the key was removed.
        if !matches!(self.vault.read(LEGACY_SERVICE, a.name()), Ok(None)) {
            self.vault.write(SERVICE, a.name(), REMOVED)?;
        }
        Ok(())
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
    use std::sync::{mpsc, Arc};

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
    }

    #[test]
    fn a_platform_failure_keeps_what_the_platform_said() {
        let err = map_error(keyring::Error::PlatformFailure(platform_error(
            "no secret service on the bus",
        )));
        assert_eq!(
            err,
            SecretError::PlatformFailure("no secret service on the bus".into())
        );
        // Not the flat text of an unavailable keyring: it may as well be a prompt that was denied.
        let text = err.to_string();
        assert!(text.starts_with("No system keyring available"), "{text}");
        assert!(text.contains("no secret service on the bus"), "{text}");
        assert_ne!(text, SecretError::Unavailable.to_string());
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
    fn service_is_loah() {
        assert_eq!(SERVICE, "Loah");
        assert_eq!(LEGACY_SERVICE, "Dictate");
    }

    #[test]
    fn the_account_names_are_the_ones_dictate_used() {
        assert_eq!(Account::Ai.name(), "default");
        assert_eq!(Account::Transcribe.name(), "transcribe");
    }

    #[test]
    fn keyring_store_is_a_secret_store() {
        // Building it must not touch the OS keyring: entries are opened per call.
        let store: Box<dyn SecretStore> = Box::new(KeyringStore::new());
        drop(store);
    }

    /// The credential store as a test sees it: entries by service and account, with the reads that were asked and the
    /// services that refuse to be read or written.
    #[derive(Default)]
    struct MemoryVault {
        entries: Mutex<HashMap<(String, String), String>>,
        reads: Mutex<Vec<(String, String)>>,
        unreadable: Mutex<Vec<String>>,
        unwritable: Mutex<Vec<String>>,
        /// A service whose first read sees nothing, as a read that was made just before another thread saved the entry.
        stale_first_read: Mutex<Option<String>>,
        /// The prompt of the Keychain: the first read of Dictate's entry says that it is waiting (`opened`), and goes on when
        /// the person answers (`answered`).
        prompt: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    }

    impl MemoryVault {
        fn with(entries: &[(&str, &str, &str)]) -> Arc<MemoryVault> {
            let vault = MemoryVault::default();
            for (service, account, value) in entries {
                vault.put(service, account, value);
            }
            Arc::new(vault)
        }

        fn put(&self, service: &str, account: &str, value: &str) {
            let key = (service.to_string(), account.to_string());
            self.entries.lock().unwrap().insert(key, value.to_string());
        }

        fn peek(&self, service: &str, account: &str) -> Option<String> {
            let key = (service.to_string(), account.to_string());
            self.entries.lock().unwrap().get(&key).cloned()
        }

        fn reads_of(&self, service: &str) -> usize {
            let reads = self.reads.lock().unwrap();
            reads.iter().filter(|(read, _)| read == service).count()
        }
    }

    impl Vault for Arc<MemoryVault> {
        fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretError> {
            self.reads
                .lock()
                .unwrap()
                .push((service.to_string(), account.to_string()));
            if self.unreadable.lock().unwrap().iter().any(|s| s == service) {
                return Err(SecretError::PlatformFailure("denied".into()));
            }
            let mut stale = self.stale_first_read.lock().unwrap();
            if stale.as_deref() == Some(service) {
                *stale = None;
                return Ok(None);
            }
            drop(stale);
            if service == "Dictate" {
                let prompt = self.prompt.lock().unwrap().take();
                if let Some((opened, answered)) = prompt {
                    opened.send(()).unwrap();
                    answered.recv().unwrap();
                }
            }
            Ok(self.peek(service, account))
        }

        fn write(&self, service: &str, account: &str, value: &str) -> Result<(), SecretError> {
            if self.unwritable.lock().unwrap().iter().any(|s| s == service) {
                return Err(SecretError::PlatformFailure("read only".into()));
            }
            self.put(service, account, value);
            Ok(())
        }

        fn remove(&self, service: &str, account: &str) -> Result<(), SecretError> {
            let key = (service.to_string(), account.to_string());
            self.entries.lock().unwrap().remove(&key);
            Ok(())
        }
    }

    fn store_on(vault: &Arc<MemoryVault>) -> KeyringStore {
        KeyringStore::with_vault(Box::new(vault.clone()))
    }

    #[test]
    fn a_key_saved_by_dictate_is_found_and_saved_under_loah() {
        let vault = MemoryVault::with(&[("Dictate", "default", "sk-old")]);
        let store = store_on(&vault);

        assert_eq!(store.get(Account::Ai), Ok(Some("sk-old".into())));

        // Now it is Loah's own, and Dictate's entry is left where it was: the old app may still be in use.
        assert_eq!(vault.peek("Loah", "default"), Some("sk-old".into()));
        assert_eq!(vault.peek("Dictate", "default"), Some("sk-old".into()));
    }

    #[test]
    fn the_key_of_each_account_is_found_apart() {
        let vault = MemoryVault::with(&[("Dictate", "transcribe", "sk-stt")]);
        let store = store_on(&vault);

        assert_eq!(store.get(Account::Ai), Ok(None));
        assert_eq!(store.get(Account::Transcribe), Ok(Some("sk-stt".into())));
        assert_eq!(vault.peek("Loah", "default"), None);
    }

    #[test]
    fn a_key_saved_in_loah_wins_over_the_one_of_dictate_and_dictate_is_not_asked() {
        let vault = MemoryVault::with(&[
            ("Loah", "default", "sk-new"),
            ("Dictate", "default", "sk-old"),
        ]);
        let store = store_on(&vault);

        assert_eq!(store.get(Account::Ai), Ok(Some("sk-new".into())));
        assert_eq!(
            vault.reads_of("Dictate"),
            0,
            "the old entry is not even read"
        );
    }

    #[test]
    fn a_key_saved_under_loah_since_the_first_read_is_used_and_dictate_is_not_asked() {
        // Two reads at the same time: one finds nothing and goes to Dictate's entry, and the other has saved Loah's meanwhile.
        // The one that waits for its turn looks at Loah's entry again before it asks anything of Dictate.
        let vault = MemoryVault::with(&[
            ("Loah", "default", "sk-new"),
            ("Dictate", "default", "sk-old"),
        ]);
        *vault.stale_first_read.lock().unwrap() = Some("Loah".into());
        let store = store_on(&vault);

        assert_eq!(store.get(Account::Ai), Ok(Some("sk-new".into())));
        assert_eq!(vault.reads_of("Dictate"), 0, "no second prompt");
        assert_eq!(vault.peek("Loah", "default"), Some("sk-new".into()));
    }

    #[test]
    fn a_key_removed_under_loah_since_the_first_read_stays_removed() {
        let vault = MemoryVault::with(&[
            ("Loah", "default", "(removed)"),
            ("Dictate", "default", "sk-old"),
        ]);
        *vault.stale_first_read.lock().unwrap() = Some("Loah".into());

        assert_eq!(store_on(&vault).get(Account::Ai), Ok(None));
        assert_eq!(vault.reads_of("Dictate"), 0);
    }

    #[test]
    fn with_no_key_anywhere_nothing_is_found_and_nothing_is_written() {
        let vault = MemoryVault::with(&[]);
        let store = store_on(&vault);

        assert_eq!(store.get(Account::Ai), Ok(None));
        assert!(vault.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn saving_a_key_writes_loah_and_never_dictate() {
        let vault = MemoryVault::with(&[("Dictate", "default", "sk-old")]);
        let store = store_on(&vault);

        store.set(Account::Ai, "sk-new").unwrap();

        assert_eq!(store.get(Account::Ai), Ok(Some("sk-new".into())));
        assert_eq!(vault.peek("Dictate", "default"), Some("sk-old".into()));
    }

    #[test]
    fn a_key_removed_in_loah_does_not_come_back_key_of_dictate() {
        let vault = MemoryVault::with(&[("Dictate", "default", "sk-old")]);
        let store = store_on(&vault);
        assert_eq!(store.get(Account::Ai), Ok(Some("sk-old".into())));

        store.delete(Account::Ai).unwrap();

        assert_eq!(store.get(Account::Ai), Ok(None));
        // Not at the next start either: what says "removed" is in the store, not in this run's memory.
        assert_eq!(store_on(&vault).get(Account::Ai), Ok(None));
        // Dictate's own entry is untouched.
        assert_eq!(vault.peek("Dictate", "default"), Some("sk-old".into()));
        // And a key saved after that is read as any other.
        store.set(Account::Ai, "sk-newer").unwrap();
        assert_eq!(
            store_on(&vault).get(Account::Ai),
            Ok(Some("sk-newer".into()))
        );
    }

    #[test]
    fn removing_a_key_that_dictate_never_had_leaves_no_entry_behind() {
        let vault = MemoryVault::with(&[]);
        let store = store_on(&vault);
        store.set(Account::Ai, "sk-1").unwrap();

        store.delete(Account::Ai).unwrap();

        assert!(vault.entries.lock().unwrap().is_empty());
        assert_eq!(store.get(Account::Ai), Ok(None));
        // Deleting what isn't there is fine.
        store.delete(Account::Transcribe).unwrap();
    }

    #[test]
    fn an_entry_of_dictate_that_cannot_be_read_is_an_error_and_is_not_asked_again_in_this_run() {
        // A prompt that the person refused: asking at every dictation would show it at every dictation.
        let vault = MemoryVault::with(&[("Dictate", "default", "sk-old")]);
        vault.unreadable.lock().unwrap().push("Dictate".into());
        let store = store_on(&vault);

        assert_eq!(
            store.get(Account::Ai),
            Err(SecretError::PlatformFailure("denied".into()))
        );
        assert_eq!(store.get(Account::Ai), Ok(None));
        assert_eq!(vault.reads_of("Dictate"), 1);
        // A new start asks again.
        assert!(store_on(&vault).get(Account::Ai).is_err());
    }

    #[test]
    fn a_key_that_could_not_be_saved_under_loah_is_still_used_and_tried_again_next_time() {
        let vault = MemoryVault::with(&[("Dictate", "default", "sk-old")]);
        vault.unwritable.lock().unwrap().push("Loah".into());

        assert_eq!(store_on(&vault).get(Account::Ai), Ok(Some("sk-old".into())));
        assert_eq!(vault.peek("Loah", "default"), None);

        vault.unwritable.lock().unwrap().clear();
        assert_eq!(store_on(&vault).get(Account::Ai), Ok(Some("sk-old".into())));
        assert_eq!(vault.peek("Loah", "default"), Some("sk-old".into()));
    }

    /// A store, and the two ends of a prompt for Dictate's entry: `opened` hears that the read is waiting for the person, and
    /// `answer` is what the person does.
    fn store_with_a_prompt(
        vault: &Arc<MemoryVault>,
    ) -> (Arc<KeyringStore>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (opened, opened_rx) = mpsc::channel();
        let (answer, answered) = mpsc::channel();
        *vault.prompt.lock().unwrap() = Some((opened, answered));
        (Arc::new(store_on(vault)), opened_rx, answer)
    }

    #[test]
    fn a_key_saved_while_the_prompt_for_dictates_key_waits_is_not_overwritten_by_it() {
        let vault = MemoryVault::with(&[("Dictate", "default", "sk-old")]);
        let (store, opened, answer) = store_with_a_prompt(&vault);
        let reading = {
            let store = store.clone();
            std::thread::spawn(move || store.get(Account::Ai))
        };
        opened.recv().unwrap();

        // The person pastes a key while the Keychain asks, and then allows the use of Dictate's.
        store.set(Account::Ai, "sk-pasted").unwrap();
        answer.send(()).unwrap();
        let read = reading.join().unwrap();

        assert_eq!(vault.peek("Loah", "default"), Some("sk-pasted".into()));
        assert_eq!(
            read,
            Ok(Some("sk-pasted".into())),
            "the read gives the key that is Loah's now"
        );
        assert_eq!(store.get(Account::Ai), Ok(Some("sk-pasted".into())));
        assert_eq!(vault.peek("Dictate", "default"), Some("sk-old".into()));
    }

    #[test]
    fn a_key_removed_while_the_prompt_for_dictates_key_waits_does_not_come_back() {
        let vault = MemoryVault::with(&[("Dictate", "default", "sk-old")]);
        let (store, opened, answer) = store_with_a_prompt(&vault);
        let reading = {
            let store = store.clone();
            std::thread::spawn(move || store.get(Account::Ai))
        };
        opened.recv().unwrap();

        // Cleared in Settings meanwhile: Loah's entry now says that the key was removed.
        store.delete(Account::Ai).unwrap();
        answer.send(()).unwrap();
        let read = reading.join().unwrap();

        assert_eq!(vault.peek("Loah", "default"), Some("(removed)".into()));
        assert_eq!(read, Ok(None));
        assert_eq!(store.get(Account::Ai), Ok(None));
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
