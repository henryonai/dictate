//! Carrying what Dictate 2.0.0 saved over to Loah, once, when Loah starts.
//!
//! Only the five things that Dictate 2.0.0 wrote are taken: `settings.json` and `permission-guide-shown` from its config
//! folder, `whisper-models/`, `diarization-models/` and `Dictations/` from its data folder. Not the folders as a whole:
//! on macOS `~/Library/Application Support/Dictate` is both of them and also holds the Swift app's `Models/` and
//! `Recordings/` (many gigabytes), and on Windows `%LOCALAPPDATA%\Dictate` is the install folder of the old program.
//!
//! Nothing of Dictate's is ever moved, changed or deleted: files are copied, and the model files hard-linked where the
//! system allows it (they never change, and a link takes no space and no time).
//!
//! What makes it safe to stop at any moment is the order of the work, not the speed of it:
//!  - a file is written under a temporary name and renamed into place, so a file that has its final name is whole (a
//!    model file that is there with the wrong size is a leftover, and is replaced);
//!  - a single file that is there already (`settings.json`) is Loah's own from then on and is never replaced: a copy
//!    that is tried again after a failure must not undo what the person has changed in Loah since;
//!  - the marker `migrated-from-dictate` is written last, and only when every item went well. Without it the next start
//!    does the work again, and finds the finished items already complete.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file in Loah's config folder that says the migration is finished.
pub const MARKER: &str = "migrated-from-dictate";

/// What a file that is being copied is called until it is whole.
const TEMPORARY_SUFFIX: &str = ".migrating";

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

/// The five items, in the order they are carried over: the settings first, so that they are there whatever happens to the
/// long part.
const ITEMS: [(&str, Place, How); 5] = [
    ("settings.json", Place::Config, How::File),
    ("permission-guide-shown", Place::Config, How::File),
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
    /// Whether this run is the one that finished the migration with something carried over: what happens once, such as
    /// registering Loah to start at login when the copied settings ask for it, is done now.
    pub fn finished_now(&self) -> bool {
        self.done && !self.copied.is_empty()
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
        match write_marker(&marker) {
            Ok(()) => report.done = true,
            Err(e) => report.failed.push((MARKER.to_string(), e.to_string())),
        }
    }
    report
}

/// Carries one item over. `Ok(true)` when something was written, `Ok(false)` when there was nothing to do: Dictate has
/// no such item, or Loah has it whole.
fn carry_over(from: &Path, to: &Path, how: How) -> io::Result<bool> {
    // A symbolic link at the top is followed (someone may keep their models on another disk, and link the folder): the
    // ones inside a folder are not.
    let Ok(source) = fs::metadata(from) else {
        return Ok(false);
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
    Ok(true)
}

/// The files of a folder, and of the folders in it, that Loah does not have whole yet. A file counts as whole when it is
/// there with the same size.
fn carry_folder(from: &Path, to: &Path, link: bool) -> io::Result<bool> {
    let mut written = false;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if kind.is_dir() {
            written |= carry_folder(&source, &target, link)?;
        } else if kind.is_file() && !is_whole(&source, &target) {
            place(&source, &target, link)?;
            written = true;
        }
        // A link, a socket: not a file of Dictate's to bring along.
    }
    Ok(written)
}

fn is_whole(source: &Path, target: &Path) -> bool {
    match (fs::metadata(source), fs::metadata(target)) {
        (Ok(source), Ok(target)) => target.is_file() && target.len() == source.len(),
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
        fs::copy(from, &temporary).map(|_| ())
    };
    let done = put.and_then(|()| fs::rename(&temporary, to));
    if done.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    done
}

fn write_marker(marker: &Path) -> io::Result<()> {
    if let Some(folder) = marker.parent() {
        fs::create_dir_all(folder)?;
    }
    fs::write(
        marker,
        "Loah copied its settings, models and dictations from Dictate once. Delete this file to have it done again.\n",
    )
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
                "migrated-from-dictate",
                "permission-guide-shown",
                "settings.json",
                "whisper-models",
            ]
        );
        assert_eq!(
            report,
            MigrationReport {
                copied: vec![
                    "settings.json".into(),
                    "permission-guide-shown".into(),
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
                "migrated-from-dictate",
                "permission-guide-shown",
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
        assert!(!report.finished_now());
        assert_eq!(names(&world.new.config), ["migrated-from-dictate"]);
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
        assert!(!report.finished_now());
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
            [
                "permission-guide-shown",
                "whisper-models",
                "diarization-models",
                "Dictations"
            ]
        );
        assert!(report.finished_now());
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
        assert!(!report.finished_now());

        // Next start, with the way cleared.
        fs::remove_file(world.new.data.join("Dictations")).unwrap();
        let report = world.migrate();
        assert!(report.done && report.failed.is_empty());
        assert_eq!(report.copied, ["Dictations"]);
        assert!(report.finished_now());
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

    #[test]
    fn a_migration_that_copied_nothing_has_not_finished_now() {
        let nothing_new = MigrationReport {
            done: true,
            ..MigrationReport::default()
        };
        assert!(!nothing_new.finished_now());
        let unfinished = MigrationReport {
            copied: vec!["settings.json".into()],
            ..MigrationReport::default()
        };
        assert!(!unfinished.finished_now());
    }
}
