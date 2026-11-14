//! Carrying what Dictate 2.0.0 saved over to Loah, once, when Loah starts.
//!
//! Only the four things that Dictate 2.0.0 wrote are taken: `settings.json` from its config folder, `whisper-models/`,
//! `diarization-models/` and `Dictations/` from its data folder. Not `permission-guide-shown`: Loah has a bundle id of its
//! own and no permission yet, so its guide has to show again. Not the folders as a whole:
//! on macOS `~/Library/Application Support/Dictate` is both of them and also holds the Swift app's `Models/` and
//! `Recordings/` (many gigabytes), and on Windows `%LOCALAPPDATA%\Dictate` is the install folder of the old program.
//!
//! Nothing of Dictate's is ever moved, changed or deleted: files are copied, and the model files hard-linked where the
//! system allows it (they never change, and a link takes no space and no time).
//!
//! What makes it safe to stop at any moment is the order of the work, not the speed of it:
//!  - a file is written under a temporary name and renamed into place, so a file that has its final name is whole (a
//!    model file that is there with the wrong size is a leftover, and is replaced);
//!  - a single file that is there already (`settings.json`), and a file of `Dictations/` that is there with something in
//!    it, are Loah's own from then on and are never replaced: a copy that is tried again after a failure must not undo
//!    what the person has changed in Loah since;
//!  - a download that is not finished (`*.part`, which the downloader resumes by appending to it) is never linked or
//!    copied: a hard link would make Loah write into Dictate's file;
//!  - what a copy writes is on the disk (`sync_all`) before it has its name, and a file that changed while it was being
//!    copied is a failure of its item and not a copy;
//!  - the marker `migrated-from-dictate` is written last, and only when every item went well. Without it the next start
//!    does the work again, and finds the finished items already complete.
//!
//! An item that fails is not lost and not reset: Loah goes on reading Dictate's own copy of it, read only (`Fallbacks`),
//! and writes only in its own folders.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file in Loah's config folder that says the migration is finished.
pub const MARKER: &str = "migrated-from-dictate";

/// The file that is written together with the marker, by the run that writes it, and that says Loah has yet to register
/// itself as a login item (when the settings that came over ask for it). The app removes it once that is done, so that a
/// registration that failed is tried again at the next start.
pub const LOGIN_ITEM_PENDING: &str = "login-item-pending";

/// What a file that is being copied is called until it is whole.
const TEMPORARY_SUFFIX: &str = ".migrating";

/// What a download that is not finished is called (`whisper::models`): it is not a file to carry over.
const PART_SUFFIX: &str = ".part";

/// How an item is carried over.
#[derive(Clone, Copy)]
enum How {
    /// One file, copied. It is left alone when Loah has it already.
    File,
    /// A folder of files that never change: each is hard-linked, or copied where that does not work.
    LinkedFolder,
    /// A folder of the person's files: each is copied.
    CopiedFolder,
}

/// Where an item is: the config folder or the data folder.
#[derive(Clone, Copy)]
enum Place {
    Config,
    Data,
}

/// The four items, in the order they are carried over: the settings first, so that they are there whatever happens to the
/// long part.
const ITEMS: [(&str, Place, How); 4] = [
    ("settings.json", Place::Config, How::File),
    ("whisper-models", Place::Data, How::LinkedFolder),
    ("diarization-models", Place::Data, How::LinkedFolder),
    ("Dictations", Place::Data, How::CopiedFolder),
];

/// The two folders an app keeps its files in: the config folder (`dirs::config_dir()/<name>`, with the settings) and the data
/// folder (`dirs::data_local_dir()/<name>`, with the models and the dictations). They are one folder on macOS
/// (`~/Library/Application Support/<name>`), `%APPDATA%` and `%LOCALAPPDATA%` on Windows, `~/.config` and `~/.local/share`
/// on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDirs {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl LegacyDirs {
    /// The folders of Dictate 2.0.0. `None` when the system has no config or data folder.
    pub fn dictate() -> Option<LegacyDirs> {
        LegacyDirs::named("Dictate")
    }

    /// The folders of Loah.
    pub fn loah() -> Option<LegacyDirs> {
        LegacyDirs::named("Loah")
    }

    fn named(folder: &str) -> Option<LegacyDirs> {
        Some(LegacyDirs {
            config: dirs::config_dir()?.join(folder),
            data: dirs::data_local_dir()?.join(folder),
        })
    }

    fn place(&self, place: Place) -> &Path {
        match place {
            Place::Config => &self.config,
            Place::Data => &self.data,
        }
    }
}

/// What a migration did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// The items that were carried over in this run (`settings.json`, `whisper-models`, ...); one that was complete
    /// already is not in it.
    pub copied: Vec<String>,
    /// The items that could not be carried over, with what the system said. They stay as Dictate left them, and Loah starts
    /// without them (the others are there); the next start tries again.
    pub failed: Vec<(String, String)>,
    /// The marker is there: the migration is finished and will not run again.
    pub done: bool,
}

impl MigrationReport {
    /// Where each item that failed is still read from: Dictate's own copy, when it is there.
    pub fn fallbacks(&self, old: &LegacyDirs) -> Fallbacks {
        let mut fallbacks = Fallbacks::default();
        for (item, _) in &self.failed {
            let Some((_, place, _)) = ITEMS.iter().find(|(name, _, _)| name == item) else {
                continue;
            };
            let there = old.place(*place).join(item);
            // Only an item that is in Dictate's folder has anything to be read.
            let there = there.exists().then_some(there);
            match item.as_str() {
                "settings.json" => fallbacks.settings = there,
                "whisper-models" => fallbacks.whisper_models = there,
                "diarization-models" => fallbacks.diarization_models = there,
                "Dictations" => fallbacks.dictations = there,
                _ => {}
            }
        }
        fallbacks
    }

    /// What could not be carried over, as one line ("settings.json (Permission denied); Dictations (...)"), or `None`.
    pub fn failure_list(&self) -> Option<String> {
        if self.failed.is_empty() {
            return None;
        }
        let items: Vec<String> = self
            .failed
            .iter()
            .map(|(item, why)| format!("{item} ({why})"))
            .collect();
        Some(items.join("; "))
    }
}

/// Where an item that could not be carried over is still read from. Dictate's folder is only ever read: what Loah writes
/// (a save of the settings, a model that is downloaded) goes to its own folders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fallbacks {
    /// `settings.json` of Dictate: read when Loah has none.
    pub settings: Option<PathBuf>,
    /// `whisper-models/`: the models that are there are used, and a download goes to Loah's folder.
    pub whisper_models: Option<PathBuf>,
    /// `diarization-models/`: the same.
    pub diarization_models: Option<PathBuf>,
    /// `Dictations/`: where Open folder opens, as that is where the saved dictations are.
    pub dictations: Option<PathBuf>,
}

/// Whether the run that wrote the marker has left the login item to be registered (see `LOGIN_ITEM_PENDING`).
pub fn login_item_pending(new: &LegacyDirs) -> bool {
    new.config.join(LOGIN_ITEM_PENDING).exists()
}

/// Says that the login item is settled (registered, or not wanted). Settling it twice is fine.
pub fn clear_login_item_pending(new: &LegacyDirs) -> io::Result<()> {
    match fs::remove_file(new.config.join(LOGIN_ITEM_PENDING)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The sentence for the log about an item that was carried over.
pub fn step_message(item: &str) -> String {
    match item {
        "settings.json" => "Copied your settings from Dictate".to_string(),
        other => format!("Copied {other} from Dictate"),
    }
}

/// Carries what Dictate 2.0.0 saved in `old` over to `new`, when that is due: the marker is not in `new.config` yet, and
/// `old` has a folder. Takes the time that copying takes (a model on another volume is copied, which is the slow case), so it
/// is done before the app opens its settings.
pub fn migrate_from_dictate(old: &LegacyDirs, new: &LegacyDirs) -> MigrationReport {
    let marker = new.config.join(MARKER);
    if marker.exists() {
        return MigrationReport {
            done: true,
            ..MigrationReport::default()
        };
    }
    if old == new || !(old.config.is_dir() || old.data.is_dir()) {
        return MigrationReport::default();
    }

    let mut report = MigrationReport::default();
    for (name, place, how) in ITEMS {
        let from = old.place(place).join(name);
        let to = new.place(place).join(name);
        match carry_over(&from, &to, how) {
            Ok(true) => report.copied.push(name.to_string()),
            Ok(false) => {}
            Err(e) => report.failed.push((name.to_string(), e.to_string())),
        }
    }
    if report.failed.is_empty() {
        // The flag first: a stop between the two leaves no marker, and the next run writes both.
        let finished =
            write_flag(&new.config.join(LOGIN_ITEM_PENDING)).and_then(|()| write_marker(&marker));
        match finished {
            Ok(()) => report.done = true,
            Err(e) => report.failed.push((MARKER.to_string(), e.to_string())),
        }
    }
    report
}

/// Carries one item over. `Ok(true)` when something was written, `Ok(false)` when there was nothing to do: Dictate has
/// no such item, or Loah has it whole.
fn carry_over(from: &Path, to: &Path, how: How) -> io::Result<bool> {
    // Only an item that is not there is "nothing to carry over": one that is there and can't be looked at (no permission,
    // a link that leads nowhere) is a failure, which keeps the marker away and has Loah use Dictate's copy meanwhile.
    let source = match fs::symlink_metadata(from) {
        Ok(source) => source,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    // A symbolic link at the top is followed (someone may keep their models on another disk, and link the folder): the
    // ones inside a folder are not.
    let source = if source.file_type().is_symlink() {
        fs::metadata(from)?
    } else {
        source
    };
    match how {
        How::File if source.is_file() => copy_whole_file(from, to),
        How::LinkedFolder if source.is_dir() => carry_folder(from, to, true),
        How::CopiedFolder if source.is_dir() => carry_folder(from, to, false),
        // Something else under that name is not Dictate's.
        _ => Ok(false),
    }
}

/// A single file: written when Loah has none.
fn copy_whole_file(from: &Path, to: &Path) -> io::Result<bool> {
    if to.exists() {
        return Ok(false);
    }
    place(from, to, false)?;
    sync_dir(to.parent());
    Ok(true)
}

/// The files of a folder, and of the folders in it, that Loah does not have whole yet (see `is_whole`).
fn carry_folder(from: &Path, to: &Path, link: bool) -> io::Result<bool> {
    let mut written = false;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        // A download that is not finished, or a copy of ours that is: neither is a file to carry over.
        let name_text = name.to_string_lossy();
        if name_text.ends_with(PART_SUFFIX) || name_text.ends_with(TEMPORARY_SUFFIX) {
            continue;
        }
        let (source, target) = (entry.path(), to.join(&name));
        if kind.is_dir() {
            written |= carry_folder(&source, &target, link)?;
        } else if kind.is_file() && !is_whole(&source, &target, link) {
            place(&source, &target, link)?;
            written = true;
        }
        // A link, a socket: not a file of Dictate's to bring along.
    }
    if written {
        sync_dir(Some(to));
    }
    Ok(written)
}

/// Whether Loah has the file `source` as `target`. A model (`linked`) never changes, so it is whole when it is there with the
/// same size, and a file of another size is a leftover that is replaced. A file of the person's (a dictation) is whole when it
/// is there with something in it, whatever its size, so that a retry never replaces what the person has changed in Loah; an
/// empty one is a leftover too, unless Dictate's is empty as well.
fn is_whole(source: &Path, target: &Path, linked: bool) -> bool {
    match (fs::metadata(source), fs::metadata(target)) {
        (Ok(source), Ok(target)) if target.is_file() => {
            if linked {
                target.len() == source.len()
            } else {
                target.len() > 0 || source.len() == 0
            }
        }
        _ => false,
    }
}

/// Puts `from` at `to` under a temporary name and then renames it, so that `to` is never half a file. With `link` the
/// file is hard-linked when the system can, and copied when it can't (another volume, a file system without links).
fn place(from: &Path, to: &Path, link: bool) -> io::Result<()> {
    if let Some(folder) = to.parent() {
        fs::create_dir_all(folder)?;
    }
    let mut name = to.file_name().unwrap_or_default().to_owned();
    name.push(TEMPORARY_SUFFIX);
    let temporary = to.with_file_name(name);
    // What an earlier run left when it was stopped.
    let _ = fs::remove_file(&temporary);
    let put = if link && fs::hard_link(from, &temporary).is_ok() {
        Ok(())
    } else {
        copy_checked(from, &temporary, || {})
    };
    let done = put.and_then(|()| fs::rename(&temporary, to));
    if done.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    done
}

/// Copies `from` to `to` and has the bytes on the disk. A source that is not the length it was before, or is not the
/// length of what was copied, changed while it was being read (Dictate is still running and writes): that is an error, and
/// what was written is not a copy of anything. `between` runs after the copy and before the source is looked at again, for
/// the test.
fn copy_checked(from: &Path, to: &Path, between: impl FnOnce()) -> io::Result<()> {
    let before = fs::metadata(from)?.len();
    let copied = fs::copy(from, to)?;
    between();
    let after = fs::metadata(from)?.len();
    if before != after || copied != after {
        let name = from.file_name().unwrap_or_default().to_string_lossy();
        return Err(io::Error::other(format!(
            "{name} changed while it was copied"
        )));
    }
    // Write access, which Windows asks for to flush a file. A copy of a read-only file is read-only too, and is left as it is
    // (the system has it on the disk in its own time).
    if let Ok(file) = fs::OpenOptions::new().write(true).open(to) {
        file.sync_all()?;
    }
    Ok(())
}

/// Has the folder's own entries (the names of what was put in it) on the disk, where the system lets a folder be flushed.
fn sync_dir(folder: Option<&Path>) {
    #[cfg(unix)]
    if let Some(Ok(folder)) = folder.map(fs::File::open) {
        let _ = folder.sync_all();
    }
    #[cfg(not(unix))]
    let _ = folder;
}

fn write_flag(flag: &Path) -> io::Result<()> {
    write_durably(
        flag,
        "Loah is to register itself as a login item, if the settings that came over from Dictate ask for it. It removes this file when that is done.\n",
    )
}

fn write_marker(marker: &Path) -> io::Result<()> {
    write_durably(
        marker,
        "Loah copied its settings, models and dictations from Dictate once. Delete this file to have it done again.\n",
    )
}

/// Writes a small file whole and has it on the disk, with its folder's entry.
fn write_durably(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let mut file = fs::File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    sync_dir(path.parent());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A system with Dictate's folders and Loah's, in a temporary folder.
    struct World {
        _root: tempfile::TempDir,
        old: LegacyDirs,
        new: LegacyDirs,
    }

    impl World {
        /// The layout of Windows and Linux: a config folder and a data folder, each with a Dictate folder and a Loah folder.
        fn split() -> World {
            let root = tempfile::tempdir().unwrap();
            let (config, data) = (root.path().join("config"), root.path().join("data"));
            World {
                old: LegacyDirs {
                    config: config.join("Dictate"),
                    data: data.join("Dictate"),
                },
                new: LegacyDirs {
                    config: config.join("Loah"),
                    data: data.join("Loah"),
                },
                _root: root,
            }
        }

        /// The layout of macOS: one folder for both.
        fn shared() -> World {
            let root = tempfile::tempdir().unwrap();
            let support = root.path().join("Application Support");
            let old = support.join("Dictate");
            let new = support.join("Loah");
            World {
                old: LegacyDirs {
                    config: old.clone(),
                    data: old,
                },
                new: LegacyDirs {
                    config: new.clone(),
                    data: new,
                },
                _root: root,
            }
        }

        fn migrate(&self) -> MigrationReport {
            migrate_from_dictate(&self.old, &self.new)
        }
    }

    /// Writes `bytes` to `path`, with the folders above it.
    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// Everything below `folder`: the relative path of each file and folder, with the bytes of the files.
    fn snapshot(folder: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
        fn walk(root: &Path, at: &Path, into: &mut BTreeMap<String, Option<Vec<u8>>>) {
            let Ok(entries) = fs::read_dir(at) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                // With `/` on every system, as the tests write the names.
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/");
                if path.is_dir() {
                    into.insert(name, None);
                    walk(root, &path, into);
                } else {
                    into.insert(name, Some(fs::read(&path).unwrap()));
                }
            }
        }
        let mut found = BTreeMap::new();
        walk(folder, folder, &mut found);
        found
    }

    /// What Dictate 2.0.0 leaves on a Mac that also has the Swift app's files, in a folder named `data` (and `config`).
    fn put_dictate_2_in(old: &LegacyDirs) {
        write(
            &old.config.join("settings.json"),
            b"{\"launch_at_login\": true}\n",
        );
        write(&old.config.join("permission-guide-shown"), b"");
        write(&old.data.join("whisper-models/ggml-base.bin"), &[1; 4096]);
        write(&old.data.join("whisper-models/ggml-small.bin"), &[2; 8192]);
        write(
            &old.data.join("diarization-models/segmentation.onnx"),
            &[3; 512],
        );
        write(&old.data.join("Dictations/20261109-101530.wav"), &[4; 2048]);
        write(
            &old.data.join("Dictations/20261109-101530.txt"),
            b"hello world",
        );
        // What is not Dictate 2.0.0's: the Swift app's, the old program, anything else that is in the folder.
        write(&old.data.join("Models/WhisperKit/big.bin"), &[5; 16384]);
        write(&old.data.join("Recordings/call.m4a"), &[6; 4096]);
        write(&old.data.join("dictate.exe"), b"MZ");
        write(&old.data.join("dictate-mcp.exe"), b"MZ");
        write(&old.data.join("notes.txt"), b"not for Loah");
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn migration_copies_only_the_known_items() {
        // The Mac: one folder for everything, with the Swift app's gigabytes in it.
        let world = World::shared();
        put_dictate_2_in(&world.old);
        let before = snapshot(&world.old.config);

        let report = world.migrate();

        assert_eq!(
            names(&world.new.config),
            [
                "Dictations",
                "diarization-models",
                "login-item-pending",
                "migrated-from-dictate",
                "settings.json",
                "whisper-models",
            ]
        );
        assert_eq!(
            report,
            MigrationReport {
                copied: vec![
                    "settings.json".into(),
                    "whisper-models".into(),
                    "diarization-models".into(),
                    "Dictations".into(),
                ],
                failed: vec![],
                done: true,
            }
        );
        let after = snapshot(&world.new.config);
        assert_eq!(
            after["settings.json"],
            Some(b"{\"launch_at_login\": true}\n".to_vec())
        );
        assert_eq!(after["whisper-models/ggml-small.bin"], Some(vec![2; 8192]));
        assert_eq!(
            after["diarization-models/segmentation.onnx"],
            Some(vec![3; 512])
        );
        assert_eq!(
            after["Dictations/20261109-101530.txt"],
            Some(b"hello world".to_vec())
        );
        // Dictate's folder is as it was, byte for byte.
        assert_eq!(snapshot(&world.old.config), before);
    }

    #[test]
    fn the_same_holds_where_the_config_and_the_data_are_apart() {
        // Windows: the data folder is the install folder of the old program, with its programs in it.
        let world = World::split();
        put_dictate_2_in(&world.old);
        let before = (snapshot(&world.old.config), snapshot(&world.old.data));

        let report = world.migrate();

        assert!(report.done && report.failed.is_empty());
        assert_eq!(
            names(&world.new.config),
            [
                "login-item-pending",
                "migrated-from-dictate",
                "settings.json"
            ]
        );
        assert_eq!(
            names(&world.new.data),
            ["Dictations", "diarization-models", "whisper-models"]
        );
        assert_eq!(
            (snapshot(&world.old.config), snapshot(&world.old.data)),
            before
        );
    }

    #[test]
    fn no_old_dir_means_nothing_to_do() {
        let world = World::split();

        let report = world.migrate();

        assert_eq!(report, MigrationReport::default());
        assert!(!world.new.config.exists() && !world.new.data.exists());
    }

    #[test]
    fn an_old_dir_without_any_of_the_items_is_finished_with_nothing_copied() {
        // A Mac that only ever had the Swift app: its folder is there, and none of it is Dictate 2.0.0's.
        let world = World::shared();
        write(
            &world.old.data.join("Models/WhisperKit/big.bin"),
            &[5; 1024],
        );
        write(&world.old.data.join("Recordings/call.m4a"), &[6; 1024]);
        let before = snapshot(&world.old.config);

        let report = world.migrate();

        assert!(report.done && report.copied.is_empty() && report.failed.is_empty());
        assert_eq!(
            names(&world.new.config),
            ["login-item-pending", "migrated-from-dictate"]
        );
        assert_eq!(snapshot(&world.old.config), before);
    }

    #[test]
    fn a_finished_migration_is_not_repeated() {
        let world = World::split();
        put_dictate_2_in(&world.old);
        assert!(world.migrate().done);
        // The person deletes a model in Loah, and Dictate saves a new dictation and changes its settings.
        fs::remove_file(world.new.data.join("whisper-models/ggml-base.bin")).unwrap();
        write(
            &world.old.data.join("Dictations/20261110-080000.txt"),
            b"later",
        );
        write(
            &world.old.config.join("settings.json"),
            b"{\"hotkey\": \"F6\"}",
        );
        let loah_before = (snapshot(&world.new.config), snapshot(&world.new.data));

        let report = world.migrate();

        assert_eq!(
            report,
            MigrationReport {
                done: true,
                ..MigrationReport::default()
            }
        );
        assert_eq!(
            (snapshot(&world.new.config), snapshot(&world.new.data)),
            loah_before
        );
    }

    #[test]
    fn an_interrupted_migration_is_finished_next_time() {
        let world = World::split();
        put_dictate_2_in(&world.old);
        // What a start that was killed in the middle of the models leaves: the settings, a first model whole, a second
        // that is half (its final name is there, as after a copy that was cut), a third that is only the temporary one;
        // no marker. The person has changed a setting since, in Loah.
        write(
            &world.new.config.join("settings.json"),
            b"{\"hotkey\": \"F9\"}",
        );
        write(
            &world.new.data.join("whisper-models/ggml-base.bin"),
            &[1; 4096],
        );
        write(
            &world.new.data.join("whisper-models/ggml-small.bin"),
            &[2; 100],
        );
        write(
            &world
                .new
                .data
                .join("diarization-models/segmentation.onnx.migrating"),
            &[3; 7],
        );

        let report = world.migrate();

        assert!(report.done && report.failed.is_empty());
        // The half file is replaced by the whole one, the one that was whole is left, the missing ones come.
        let data = snapshot(&world.new.data);
        assert_eq!(data["whisper-models/ggml-small.bin"], Some(vec![2; 8192]));
        assert_eq!(data["whisper-models/ggml-base.bin"], Some(vec![1; 4096]));
        assert_eq!(
            data["diarization-models/segmentation.onnx"],
            Some(vec![3; 512])
        );
        assert_eq!(data["Dictations/20261109-101530.wav"], Some(vec![4; 2048]));
        assert!(
            data.keys().all(|name| !name.ends_with(".migrating")),
            "no temporary file is left: {:?}",
            data.keys().collect::<Vec<_>>()
        );
        // The settings are not lost, and not replaced by Dictate's older ones.
        assert_eq!(
            fs::read(world.new.config.join("settings.json")).unwrap(),
            b"{\"hotkey\": \"F9\"}"
        );
        assert_eq!(
            report.copied,
            ["whisper-models", "diarization-models", "Dictations"]
        );
        assert!(world.new.config.join(MARKER).is_file());
    }

    #[test]
    fn the_marker_is_not_written_until_every_item_is_there() {
        // A migration that stops after the first items has no marker: the next start sees the folder half filled and
        // finishes it, and never takes it for complete.
        let world = World::split();
        put_dictate_2_in(&world.old);
        // `Dictations` is a file in Loah's way: that item can't be written.
        write(&world.new.data.join("Dictations"), b"in the way");

        let report = world.migrate();

        assert!(!report.done);
        assert!(!world.new.config.join(MARKER).exists());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "Dictations");
        // The rest arrived, though.
        assert!(world.new.config.join("settings.json").is_file());
        assert!(world
            .new
            .data
            .join("whisper-models/ggml-base.bin")
            .is_file());
        assert!(world
            .new
            .data
            .join("diarization-models/segmentation.onnx")
            .is_file());

        // Next start, with the way cleared.
        fs::remove_file(world.new.data.join("Dictations")).unwrap();
        let report = world.migrate();
        assert!(report.done && report.failed.is_empty());
        assert_eq!(report.copied, ["Dictations"]);
    }

    #[test]
    fn a_failure_is_named_with_what_the_system_said() {
        let world = World::split();
        put_dictate_2_in(&world.old);
        write(&world.new.data.join("Dictations"), b"in the way");

        let report = world.migrate();

        let list = report.failure_list().expect("a failure");
        assert!(list.starts_with("Dictations ("), "{list}");
        assert!(list.ends_with(')'), "{list}");
        assert_eq!(MigrationReport::default().failure_list(), None);
        assert_eq!(
            MigrationReport {
                failed: vec![("a".into(), "x".into()), ("b".into(), "y".into())],
                ..MigrationReport::default()
            }
            .failure_list(),
            Some("a (x); b (y)".to_string())
        );
    }

    #[test]
    fn a_settings_file_that_is_in_loah_already_is_never_replaced() {
        // The settings were copied, another item failed, the person changed a setting in Loah, and the next start tries
        // again: their change stays.
        let world = World::split();
        put_dictate_2_in(&world.old);
        write(
            &world.new.config.join("settings.json"),
            b"{\"push_to_talk\": true}",
        );

        world.migrate();

        assert_eq!(
            fs::read(world.new.config.join("settings.json")).unwrap(),
            b"{\"push_to_talk\": true}"
        );
    }

    #[test]
    fn a_dictate_that_has_no_settings_file_still_brings_the_rest() {
        let world = World::split();
        write(
            &world.old.data.join("whisper-models/ggml-base.bin"),
            &[1; 64],
        );

        let report = world.migrate();

        assert_eq!(report.copied, ["whisper-models"]);
        assert!(report.done);
        // No settings: Loah is told that this is a first start.
        assert!(!world.new.config.join("settings.json").exists());
    }

    #[test]
    fn a_folder_of_dictate_is_not_replaced_by_something_that_is_not_a_folder() {
        // A file called `whisper-models` is nobody's models.
        let world = World::split();
        write(&world.old.data.join("whisper-models"), b"a file");
        write(&world.old.config.join("settings.json"), b"{}");

        let report = world.migrate();

        assert_eq!(report.copied, ["settings.json"]);
        assert!(!world.new.data.join("whisper-models").exists());
    }

    #[test]
    fn the_permission_guide_mark_is_not_carried_over() {
        // Loah has a bundle id of its own and no permission yet: the guide has to show again.
        let world = World::split();
        put_dictate_2_in(&world.old);
        assert!(world.old.config.join("permission-guide-shown").is_file());

        world.migrate();

        assert!(!world.new.config.join("permission-guide-shown").exists());
    }

    #[test]
    fn a_download_that_is_not_finished_is_not_carried_over() {
        // A `.part` is resumed by appending to it: a hard link to Dictate's would make Loah write into Dictate's file.
        let world = World::split();
        put_dictate_2_in(&world.old);
        write(
            &world.old.data.join("whisper-models/ggml-tiny.bin.part"),
            &[7; 300],
        );
        write(
            &world.old.data.join("diarization-models/model.onnx.part"),
            &[8; 300],
        );
        write(
            &world
                .old
                .data
                .join("whisper-models/ggml-base.bin.migrating"),
            &[9; 5],
        );
        write(
            &world.old.data.join("Dictations/20261109-101600.wav.part"),
            &[1; 5],
        );
        let before = (snapshot(&world.old.config), snapshot(&world.old.data));

        let report = world.migrate();

        assert!(report.done && report.failed.is_empty());
        let data = snapshot(&world.new.data);
        assert!(
            data.keys()
                .all(|name| !name.ends_with(".part") && !name.ends_with(".migrating")),
            "nothing in progress arrives: {:?}",
            data.keys().collect::<Vec<_>>()
        );
        // The finished files do.
        assert!(data.contains_key("whisper-models/ggml-base.bin"));
        assert!(data.contains_key("diarization-models/segmentation.onnx"));
        assert_eq!(
            (snapshot(&world.old.config), snapshot(&world.old.data)),
            before
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_at_the_top_is_a_failure_and_not_an_absent_item() {
        let world = World::split();
        put_dictate_2_in(&world.old);
        fs::remove_dir_all(world.old.data.join("whisper-models")).unwrap();
        std::os::unix::fs::symlink(
            world.old.data.join("nowhere"),
            world.old.data.join("whisper-models"),
        )
        .unwrap();

        let report = world.migrate();

        assert!(
            !report.done,
            "the models are there in name only: that is not nothing to copy"
        );
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "whisper-models");
        assert!(!world.new.config.join(MARKER).exists());
        // The others arrived.
        assert!(world.new.data.join("Dictations").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_looked_into_is_a_failure_and_not_an_absent_item() {
        use std::os::unix::fs::PermissionsExt;
        let world = World::split();
        put_dictate_2_in(&world.old);
        fs::set_permissions(&world.old.data, fs::Permissions::from_mode(0o000)).unwrap();
        // A superuser looks into it anyway, and then there is nothing to test.
        if fs::read_dir(&world.old.data).is_ok() {
            fs::set_permissions(&world.old.data, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let report = world.migrate();
        fs::set_permissions(&world.old.data, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!report.done);
        let failed: Vec<&str> = report
            .failed
            .iter()
            .map(|(item, _)| item.as_str())
            .collect();
        assert_eq!(
            failed,
            ["whisper-models", "diarization-models", "Dictations"]
        );
        // The settings are in the other folder and came over.
        assert_eq!(report.copied, ["settings.json"]);
        assert!(!world.new.config.join(MARKER).exists());
    }

    #[test]
    fn a_dictation_that_is_in_loah_already_is_never_replaced() {
        // Loah's own file wins, whatever its size: a person may have edited the text, and a retry must not undo it.
        let world = World::split();
        put_dictate_2_in(&world.old);
        write(
            &world.new.data.join("Dictations/20261109-101530.txt"),
            b"hello world, as edited in Loah",
        );

        world.migrate();

        assert_eq!(
            fs::read(world.new.data.join("Dictations/20261109-101530.txt")).unwrap(),
            b"hello world, as edited in Loah"
        );
        // The rest of the folder came.
        assert!(world
            .new
            .data
            .join("Dictations/20261109-101530.wav")
            .is_file());
    }

    #[test]
    fn an_empty_dictation_file_in_loah_is_filled_from_dictates() {
        // A file with nothing in it is a leftover of a cut copy, and is not "there".
        let world = World::split();
        put_dictate_2_in(&world.old);
        write(&world.new.data.join("Dictations/20261109-101530.txt"), b"");

        world.migrate();

        assert_eq!(
            fs::read(world.new.data.join("Dictations/20261109-101530.txt")).unwrap(),
            b"hello world"
        );
    }

    #[test]
    fn a_file_that_changes_while_it_is_copied_is_not_taken_for_a_copy() {
        // Dictate is still running and writes a dictation while it is copied: what arrived is not that file, and the item
        // fails (and is tried again at the next start) instead of keeping half of it.
        let dir = tempfile::tempdir().unwrap();
        let (source, target) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        fs::write(&source, vec![1u8; 1000]).unwrap();

        let append = || {
            use std::io::Write;
            let mut file = fs::OpenOptions::new().append(true).open(&source).unwrap();
            file.write_all(&[2u8; 10]).unwrap();
        };
        let err = copy_checked(&source, &target, append).unwrap_err();
        assert!(
            err.to_string().contains("changed while it was copied"),
            "{err}"
        );

        // A file that stays as it is is copied whole.
        fs::write(&source, vec![3u8; 1000]).unwrap();
        copy_checked(&source, &target, || {}).unwrap();
        assert_eq!(fs::read(&target).unwrap(), vec![3u8; 1000]);
    }

    #[test]
    fn the_login_item_flag_is_written_with_the_marker_and_only_then() {
        let world = World::split();
        put_dictate_2_in(&world.old);
        // A failure in the way: no marker, and no flag either.
        write(&world.new.data.join("Dictations"), b"in the way");
        assert!(!world.migrate().done);
        assert!(!login_item_pending(&world.new));

        fs::remove_file(world.new.data.join("Dictations")).unwrap();
        assert!(world.migrate().done);
        assert!(login_item_pending(&world.new), "this run wrote the marker");
        assert!(world.new.config.join(MARKER).is_file());
    }

    #[test]
    fn the_flag_is_written_even_when_nothing_was_copied_and_is_not_written_again() {
        let world = World::shared();
        write(&world.old.data.join("Models/WhisperKit/big.bin"), &[5; 64]);
        assert!(world.migrate().done);
        assert!(login_item_pending(&world.new));

        clear_login_item_pending(&world.new).unwrap();
        // Clearing what is not there is fine.
        clear_login_item_pending(&world.new).unwrap();
        assert!(!login_item_pending(&world.new));

        // The marker is there: a later start does not bring it back.
        assert!(world.migrate().done);
        assert!(!login_item_pending(&world.new));
    }

    #[test]
    fn a_failed_item_is_still_read_from_dictates_own_folder() {
        let world = World::split();
        put_dictate_2_in(&world.old);
        write(&world.new.data.join("Dictations"), b"in the way");

        let report = world.migrate();

        assert_eq!(
            report.fallbacks(&world.old),
            Fallbacks {
                dictations: Some(world.old.data.join("Dictations")),
                ..Fallbacks::default()
            }
        );
        // The ones that went well have none.
        assert_eq!(report.failed.len(), 1);
    }

    #[test]
    fn every_item_that_failed_has_its_place_in_dictate_and_nothing_else_has_one() {
        let world = World::split();
        put_dictate_2_in(&world.old);
        let failed = |items: &[&str]| MigrationReport {
            failed: items
                .iter()
                .map(|item| (item.to_string(), "x".to_string()))
                .collect(),
            ..MigrationReport::default()
        };

        let all = failed(&[
            "settings.json",
            "whisper-models",
            "diarization-models",
            "Dictations",
        ]);
        assert_eq!(
            all.fallbacks(&world.old),
            Fallbacks {
                settings: Some(world.old.config.join("settings.json")),
                whisper_models: Some(world.old.data.join("whisper-models")),
                diarization_models: Some(world.old.data.join("diarization-models")),
                dictations: Some(world.old.data.join("Dictations")),
            }
        );
        // A marker that could not be written says nothing of what is used; an item that is not in Dictate has nothing to read.
        assert_eq!(
            failed(&[MARKER]).fallbacks(&world.old),
            Fallbacks::default()
        );
        let nothing_there = World::split();
        assert_eq!(all.fallbacks(&nothing_there.old), Fallbacks::default());
        assert_eq!(
            MigrationReport::default().fallbacks(&world.old),
            Fallbacks::default()
        );
    }

    #[cfg(unix)]
    #[test]
    fn model_files_are_hard_linked_and_the_dictations_are_copied() {
        use std::os::unix::fs::MetadataExt;
        let world = World::split();
        put_dictate_2_in(&world.old);

        world.migrate();

        let inode = |path: &Path| fs::metadata(path).unwrap().ino();
        let model = "whisper-models/ggml-base.bin";
        assert_eq!(
            inode(&world.old.data.join(model)),
            inode(&world.new.data.join(model)),
            "a model is a link to the same file"
        );
        let dictation = "Dictations/20261109-101530.txt";
        assert_ne!(
            inode(&world.old.data.join(dictation)),
            inode(&world.new.data.join(dictation)),
            "a dictation is a copy: a change to one is not a change to the other"
        );
        let settings = "settings.json";
        assert_ne!(
            inode(&world.old.config.join(settings)),
            inode(&world.new.config.join(settings))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_a_folder_is_not_followed() {
        // A link in Dictations that points out of the folder (to anything at all) is not something to carry over.
        let world = World::split();
        write(&world.old.data.join("Dictations/real.txt"), b"mine");
        let outside = world.old.data.join("outside.txt");
        write(&outside, b"not a dictation");
        std::os::unix::fs::symlink(&outside, world.old.data.join("Dictations/link.txt")).unwrap();

        world.migrate();

        assert_eq!(names(&world.new.data.join("Dictations")), ["real.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_read_fails_its_item_only_and_is_tried_again() {
        use std::os::unix::fs::PermissionsExt;
        let world = World::split();
        put_dictate_2_in(&world.old);
        let locked = world.old.data.join("Dictations/20261109-101530.wav");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // A superuser reads it anyway, and then there is nothing to test.
        if fs::File::open(&locked).is_ok() {
            return;
        }

        let report = world.migrate();

        assert!(!report.done);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "Dictations");
        assert!(world.new.config.join("settings.json").is_file());
        assert!(world
            .new
            .data
            .join("whisper-models/ggml-small.bin")
            .is_file());
        assert!(!world.new.config.join(MARKER).exists());
        assert!(
            snapshot(&world.new.data)
                .keys()
                .all(|name| !name.ends_with(".migrating")),
            "a copy that failed leaves no temporary file"
        );

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        let report = world.migrate();
        assert!(report.done && report.failed.is_empty());
        assert!(world
            .new
            .data
            .join("Dictations/20261109-101530.wav")
            .is_file());
    }

    #[test]
    fn the_folders_are_those_of_the_app_with_the_names_of_the_two_apps() {
        let (dictate, loah) = (LegacyDirs::dictate(), LegacyDirs::loah());
        if let (Some(config), Some(data)) = (dirs::config_dir(), dirs::data_local_dir()) {
            let dictate = dictate.expect("there are folders");
            assert_eq!(dictate.config, config.join("Dictate"));
            assert_eq!(dictate.data, data.join("Dictate"));
            let loah = loah.expect("there are folders");
            assert_eq!(loah.config, config.join("Loah"));
            assert_eq!(loah.data, data.join("Loah"));
        }
    }

    #[test]
    fn the_log_says_what_was_copied() {
        assert_eq!(
            step_message("settings.json"),
            "Copied your settings from Dictate"
        );
        assert_eq!(
            step_message("whisper-models"),
            "Copied whisper-models from Dictate"
        );
    }
}
