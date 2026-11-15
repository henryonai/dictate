//! What is left of Dictate on this machine: its settings, models and dictations, which Loah carries over once at the
//! first start, and the old app itself, which may still be installed, running or set to start at login.
//!
//! Loah never quits, unregisters or deletes the old app. When it is still there Settings says so (it uses the same
//! shortcut and the same meetings folder), and what to do about it is the README's section on upgrading. The one thing it
//! removes is on Linux: the login entry that the package swap leaves, which runs `/usr/bin/dictate` (now the script that
//! starts Loah) and would start Loah a second time.
//!
//! Finding the old app is best effort: where Loah can't tell, it says nothing. A notice about an app that is not there
//! would be worse than none, so every check asks for the thing itself (the program is on the disk and is Dictate's), not
//! only for a name that a registry or a file carries.

use loah_services::migrate::{self, Fallbacks, LegacyDirs, MigrationReport};

/// The identifier of the Tauri Dictate (2.0.0), and the one of the Swift Dictate before it.
#[cfg(any(test, target_os = "macos"))]
const OLD_APP_IDS: [&str; 2] = ["io.github.henryonai.dictate", "local.dictate"];

/// Whether this is the first start: Loah has no settings file, and Dictate's settings are not being read in its place
/// (that is the case when they could not be copied).
pub fn is_first_launch(settings_path: &std::path::Path, fallbacks: &Fallbacks) -> bool {
    !settings_path.exists() && fallbacks.settings.is_none()
}

/// What the migration did at this start, and what follows from it.
#[derive(Debug)]
pub struct Carried {
    pub report: MigrationReport,
    /// Where each item that could not be copied is still read from: Dictate's own copy, read only.
    pub fallbacks: Fallbacks,
    /// Loah's folders, which hold the flag of the login item.
    new: Option<LegacyDirs>,
}

/// What `Carried::settle_login_item` did.
#[derive(Debug, PartialEq, Eq)]
pub enum LoginItem {
    /// The migration did not leave it to be done at this start, or has been done since.
    NotPending,
    /// The settings that came over do not ask for it.
    NotWanted,
    Registered,
    /// It could not be done; it is tried again at the next start.
    Failed(String),
}

/// Carries Dictate's settings, models and dictations over to Loah when that is due (see `loah_services::migrate`), and
/// logs each step. Before anything reads the settings: a person whose settings came over is not at a first launch.
pub fn carry_over_from_dictate() -> Carried {
    carry_over(LegacyDirs::dictate(), LegacyDirs::loah())
}

fn carry_over(old: Option<LegacyDirs>, new: Option<LegacyDirs>) -> Carried {
    let (Some(old), Some(new)) = (old, new) else {
        return Carried {
            report: MigrationReport::default(),
            fallbacks: Fallbacks::default(),
            new: None,
        };
    };
    let report = migrate::migrate_from_dictate(&old, &new);
    for item in &report.copied {
        log::info!("{}", migrate::step_message(item));
    }
    if let Some(failed) = report.failure_list() {
        log::warn!("Could not copy all of Dictate's files to Loah, reading Dictate's own meanwhile: {failed}");
    }
    let fallbacks = report.fallbacks(&old);
    Carried {
        report,
        fallbacks,
        new: Some(new),
    }
}

impl Carried {
    /// Registers Loah as a login item when the run that finished the migration left that to be done and the settings that
    /// came over (`launch_at_login`) ask for it. The setting is otherwise applied only when it changes, and Loah has no login
    /// item of its own before this. `register` does it. Once it is done, or not wanted, the flag is removed; a registration
    /// that failed leaves it, and the next start tries again.
    ///
    /// Also once it is settled, `drop_wrapper_entry` removes the login entry that Dictate left behind where it would start Loah
    /// a second time (see `remove_dictate_wrapper_autostart`): Loah's own entry, or the person's choice of none, is what counts now.
    pub fn settle_login_item(
        &self,
        launch_at_login: bool,
        register: impl FnOnce() -> Result<(), String>,
        drop_wrapper_entry: impl FnOnce(),
    ) -> LoginItem {
        let Some(new) = self
            .new
            .as_ref()
            .filter(|new| migrate::login_item_pending(new))
        else {
            return LoginItem::NotPending;
        };
        let settled = || {
            drop_wrapper_entry();
            if let Err(e) = migrate::clear_login_item_pending(new) {
                log::warn!("Could not remove the mark of the login item: {e}");
            }
        };
        if !launch_at_login {
            settled();
            return LoginItem::NotWanted;
        }
        match register() {
            Ok(()) => {
                settled();
                LoginItem::Registered
            }
            Err(e) => LoginItem::Failed(e),
        }
    }
}

/// Whether Dictate, the Tauri app or the Swift one, is still on this machine in a way that matters: running, or registered
/// to start at login, with its program on the disk. Not asked at the start but when Settings is opened, so that it is
/// true to the moment: someone who quits Dictate and looks again sees the notice go.
pub fn dictate_still_present() -> bool {
    #[cfg(target_os = "macos")]
    return macos_present();
    #[cfg(windows)]
    return windows_present();
    #[cfg(target_os = "linux")]
    return linux_present();
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    false
}

// ---- macOS: the running apps, by identifier, and the LaunchAgent that the autostart plugin of Dictate wrote.

/// Whether an app with one of the identifiers of the old Dictate is running.
#[cfg(any(test, target_os = "macos"))]
fn old_app_running(running: impl Fn(&str) -> bool) -> bool {
    OLD_APP_IDS.iter().any(|id| running(id))
}

/// The text of each `<string>` of a property list.
#[cfg(any(test, target_os = "macos"))]
fn plist_strings(plist: &str) -> Vec<&str> {
    plist
        .split("<string>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</string>").map(|(text, _)| text))
        .collect()
}

/// Whether the LaunchAgent `plist` starts a Dictate: one of its program arguments is `.../Contents/MacOS/dictate` and
/// `is_old_app` says that this program is the old app's.
#[cfg(any(test, target_os = "macos"))]
fn launch_agent_starts_old_app(plist: &str, is_old_app: impl Fn(&std::path::Path) -> bool) -> bool {
    plist_strings(plist)
        .into_iter()
        .filter(|program| program.ends_with("/Contents/MacOS/dictate"))
        .any(|program| is_old_app(std::path::Path::new(program)))
}

/// Whether `exe`, the program inside an app bundle, is there and its bundle is the Tauri Dictate (by the identifier in its
/// Info.plist): another app that has a LaunchAgent called Dictate is not it.
#[cfg(any(test, target_os = "macos"))]
fn is_old_app_executable(exe: &std::path::Path) -> bool {
    // <bundle>/Contents/MacOS/dictate, and <bundle>/Contents/Info.plist.
    let Some(contents) = exe.ancestors().nth(2) else {
        return false;
    };
    let info = std::fs::read_to_string(contents.join("Info.plist")).unwrap_or_default();
    exe.is_file() && plist_strings(&info).contains(&OLD_APP_IDS[0])
}

// ---- Windows: the value "Dictate" of the Run key, which the autostart plugin of Dictate wrote.

/// Whether the output of `reg query HKCU\...\Run /v Dictate` shows a value `Dictate` that starts `dictate.exe`, and the
/// program is there (`exists`): the entry that an uninstall leaves behind starts nothing.
#[cfg(any(test, windows))]
fn run_entry_starts_old_app(output: &str, exists: impl Fn(&std::path::Path) -> bool) -> bool {
    const PROGRAM: &str = "dictate.exe";
    output.lines().any(|line| {
        // "    Dictate    REG_SZ    C:\...\dictate.exe " (REG_SZ is the same in every language of Windows).
        let Some((name, data)) = line.split_once("REG_SZ") else {
            return false;
        };
        if name.trim() != "Dictate" {
            return false;
        }
        // The path up to and including the program's name: what follows is arguments, and the path may hold spaces.
        let data = data.trim().trim_start_matches('"');
        let lower = data.to_ascii_lowercase();
        let Some(end) = lower.find(PROGRAM).map(|at| at + PROGRAM.len()) else {
            return false;
        };
        let (path, rest) = data.split_at(end);
        let named = path
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|file| file.eq_ignore_ascii_case(PROGRAM));
        named
            && (rest.is_empty() || rest.starts_with(['"', ' ']))
            && exists(std::path::Path::new(path))
    })
}

/// Whether the output of `reg query HKCU\...\Explorer\StartupApproved\Run /v Dictate` says that the entry was turned off (in
/// Task Manager's Startup tab or in Settings). The value has twelve bytes, the last eight of which are zero while the entry is
/// on and a time once it is off (what the autostart plugin itself reads); no value, or one too short to tell, leaves the
/// entry as it is.
#[cfg(any(test, windows))]
fn startup_is_turned_off(output: &str) -> bool {
    output.lines().any(|line| {
        let Some((name, data)) = line.split_once("REG_BINARY") else {
            return false;
        };
        let hex = data.trim();
        if name.trim() != "Dictate" || hex.len() < 16 || hex.len() % 2 != 0 {
            return false;
        }
        // The last eight bytes, as sixteen hex digits.
        let last_eight = &hex[hex.len() - 16..];
        last_eight.chars().any(|digit| digit != '0')
    })
}

// ---- Linux: the autostart entry of Dictate and the processes that run its program.

/// The command that Dictate 2.0.0 installed, and that its autostart entry runs. After an upgrade it is the wrapper of the
/// Loah package, a script that starts Loah.
#[cfg(any(test, target_os = "linux"))]
const DICTATE_COMMAND: &str = "/usr/bin/dictate";

/// The program that the autostart entry `desktop` runs: the first word of its `Exec`. None when the entry runs nothing: the
/// desktop's own switches for an entry (it is in the folder and nothing starts), or no command.
#[cfg(any(test, target_os = "linux"))]
fn autostart_program(desktop: &str) -> Option<String> {
    let value = |key: &str| {
        desktop.lines().find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim().to_ascii_lowercase())
        })
    };
    if value("Hidden").as_deref() == Some("true")
        || value("X-GNOME-Autostart-enabled").as_deref() == Some("false")
    {
        return None;
    }
    value_of_exec(desktop)?
        .split_whitespace()
        .next()
        .map(str::to_owned)
}

/// Whether the autostart entry `desktop` (`~/.config/autostart/Dictate.desktop`) starts a program that is the old
/// Dictate: one that is there and is not a script. After an upgrade `/usr/bin/dictate` is the two-line wrapper of the
/// package, which starts Loah, and that is not the old app.
#[cfg(any(test, target_os = "linux"))]
fn autostart_entry_starts_old_app(
    desktop: &str,
    is_program: impl Fn(&std::path::Path) -> bool,
) -> bool {
    autostart_program(desktop).is_some_and(|program| is_program(std::path::Path::new(&program)))
}

/// Whether the autostart entry `desktop` of Dictate starts the wrapper of the Loah package (`is_wrapper` says which program
/// that is), and so starts Loah. Loah registers its own entry, so while this one stays two Loah start at every login (the
/// second one only shows Settings), and "Open Loah at login" turned off would not stop the first.
#[cfg(any(test, target_os = "linux"))]
fn autostart_entry_starts_wrapper(
    desktop: &str,
    is_wrapper: impl Fn(&std::path::Path) -> bool,
) -> bool {
    autostart_program(desktop).is_some_and(|program| is_wrapper(std::path::Path::new(&program)))
}

/// Removes the autostart entry file `entry` when it starts the wrapper (`is_wrapper`). Whether it did is the answer: no
/// entry, one that can't be read, and one that starts anything else (a Dictate that is there) are all left as they are.
#[cfg(any(test, target_os = "linux"))]
fn remove_wrapper_entry(
    entry: &std::path::Path,
    is_wrapper: impl Fn(&std::path::Path) -> bool,
) -> Result<bool, String> {
    let Ok(desktop) = std::fs::read_to_string(entry) else {
        return Ok(false);
    };
    if !autostart_entry_starts_wrapper(&desktop, is_wrapper) {
        return Ok(false);
    }
    std::fs::remove_file(entry)
        .map(|()| true)
        .map_err(|e| format!("Could not remove {}: {e}", entry.display()))
}

/// The command of a desktop entry (`Exec=`).
#[cfg(any(test, target_os = "linux"))]
fn value_of_exec(desktop: &str) -> Option<String> {
    desktop
        .lines()
        .find_map(|line| line.trim().strip_prefix("Exec="))
        .map(str::to_owned)
}

/// Whether a file starts with `#!`: `None` when that can't be told (it is not there, or it is shorter than that).
#[cfg(any(test, target_os = "linux"))]
fn starts_with_shebang(path: &std::path::Path) -> Option<bool> {
    use std::io::Read;
    let mut start = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut start))
        .ok()
        .map(|()| start == *b"#!")
}

/// Whether a file is a program and not a script (`#!`): the wrapper of the package is one.
#[cfg(any(test, target_os = "linux"))]
fn is_program(path: &std::path::Path) -> bool {
    starts_with_shebang(path) == Some(false)
}

/// Whether `path` is `wrapper` and a script: the wrapper of the package, which starts Loah. The program of a person's own script
/// in another place is not it, and nor is the program of the old Dictate.
#[cfg(any(test, target_os = "linux"))]
fn is_wrapper_script(path: &std::path::Path, wrapper: &std::path::Path) -> bool {
    path == wrapper && starts_with_shebang(path) == Some(true)
}

/// Whether a process whose `/proc/<pid>/exe` points at `exe_link` is the old Dictate: `/usr/bin/dictate` when that is a
/// program, or what was `/usr/bin/dictate` when the package was removed or replaced while it ran.
#[cfg(any(test, target_os = "linux"))]
fn process_is_old_app(exe_link: &str, is_program: impl Fn(&std::path::Path) -> bool) -> bool {
    exe_link == format!("{DICTATE_COMMAND} (deleted)")
        || (exe_link == DICTATE_COMMAND && is_program(std::path::Path::new(DICTATE_COMMAND)))
}

/// Whether a process under `proc_root` (`/proc`) is the old Dictate. (Its test needs symbolic links, so on Windows the
/// function is left out of the test build too.)
#[cfg(any(all(test, unix), target_os = "linux"))]
fn old_app_in_proc(proc_root: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        // A process of another user, or one that has just ended, can't be read: it counts as not Dictate.
        .filter_map(|entry| std::fs::read_link(entry.path().join("exe")).ok())
        .any(|exe| process_is_old_app(&exe.to_string_lossy(), is_program))
}

// ---- The questions to the system, one thin function per OS: the decisions are above.

#[cfg(target_os = "macos")]
fn macos_present() -> bool {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    // NSRunningApplication can be asked from any thread.
    let is_running = |id: &str| {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(id))
            .iter()
            .any(|app| !app.isTerminated())
    };
    if old_app_running(is_running) {
        return true;
    }
    // The autostart plugin of Dictate wrote ~/Library/LaunchAgents/Dictate.plist.
    dirs::home_dir()
        .map(|home| home.join("Library/LaunchAgents/Dictate.plist"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|plist| launch_agent_starts_old_app(&plist, is_old_app_executable))
}

#[cfg(windows)]
fn windows_present() -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    /// `CREATE_NO_WINDOW` of the Win32 API: no console flashes by for the question.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // reg.exe is in every Windows; `/v` asks for the one value, and the exit code says whether it is there.
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let reg = std::path::Path::new(&system)
        .join("System32")
        .join("reg.exe");
    let ask = |key: &str| {
        Command::new(&reg)
            .args(["query", key, "/v", "Dictate"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let starts = ask(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run")
        .is_some_and(|output| run_entry_starts_old_app(&output, |path| path.is_file()));
    starts
        && !ask(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run")
            .is_some_and(|output| startup_is_turned_off(&output))
}

/// Removes `~/.config/autostart/Dictate.desktop` when it starts the wrapper of the Loah package (see `remove_wrapper_entry`).
/// Dictate 2.0.0 wrote it for "Open at login"; after the package swap `/usr/bin/dictate` starts Loah, which has an entry of
/// its own, so that the old one is a second start (and is not turned off by Loah's setting). An entry that starts a real
/// Dictate is not touched, and there is nothing to do on the other systems. Best effort: what fails is logged.
pub fn remove_dictate_wrapper_autostart() {
    #[cfg(target_os = "linux")]
    {
        let Some(entry) =
            dirs::config_dir().map(|dir| dir.join("autostart").join("Dictate.desktop"))
        else {
            return;
        };
        let wrapper = |program: &std::path::Path| {
            is_wrapper_script(program, std::path::Path::new(DICTATE_COMMAND))
        };
        match remove_wrapper_entry(&entry, wrapper) {
            Ok(true) => log::info!(
                "Removed the login entry of Dictate, which only starts Loah (Loah has its own)"
            ),
            Ok(false) => {}
            Err(e) => log::warn!("{e}"),
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_present() -> bool {
    let entry = dirs::config_dir()
        .map(|config| config.join("autostart").join("Dictate.desktop"))
        .and_then(|path| std::fs::read_to_string(path).ok());
    entry.is_some_and(|desktop| autostart_entry_starts_old_app(&desktop, is_program))
        || old_app_in_proc(std::path::Path::new("/proc"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn without_the_folders_of_the_system_nothing_is_carried_over() {
        assert_eq!(carry_over(None, None).report, MigrationReport::default());
        let dirs = LegacyDirs {
            config: "c".into(),
            data: "d".into(),
        };
        assert_eq!(
            carry_over(Some(dirs.clone()), None).report,
            MigrationReport::default()
        );
        assert_eq!(
            carry_over(None, Some(dirs)).report,
            MigrationReport::default()
        );
    }

    /// Dictate's folders and Loah's, in a temporary folder, with Dictate's settings in place.
    fn folders(settings: &str) -> (tempfile::TempDir, LegacyDirs, LegacyDirs) {
        let root = tempfile::tempdir().unwrap();
        let old = LegacyDirs {
            config: root.path().join("old-config"),
            data: root.path().join("old-data"),
        };
        let new = LegacyDirs {
            config: root.path().join("new-config"),
            data: root.path().join("new-data"),
        };
        fs::create_dir_all(&old.config).unwrap();
        fs::write(old.config.join("settings.json"), settings).unwrap();
        (root, old, new)
    }

    #[test]
    fn the_carry_over_hands_back_what_the_migration_did() {
        let (_root, old, new) = folders("{}");

        let carried = carry_over(Some(old), Some(new.clone()));

        assert_eq!(carried.report.copied, ["settings.json"]);
        assert!(carried.report.done);
        assert_eq!(carried.fallbacks, Fallbacks::default());
        assert!(new.config.join("settings.json").is_file());
    }

    #[test]
    fn a_start_without_settings_is_a_first_launch_unless_dictates_settings_are_being_read() {
        let (_root, old, new) = folders("{}");
        let settings = new.config.join("settings.json");
        let none = Fallbacks::default();
        let reading = Fallbacks {
            settings: Some(old.config.join("settings.json")),
            ..Fallbacks::default()
        };

        assert!(is_first_launch(&settings, &none));
        // Settings that could not be copied are settings all the same.
        assert!(!is_first_launch(&settings, &reading));
        // Loah's own file, copied or saved since.
        fs::create_dir_all(&new.config).unwrap();
        fs::write(&settings, "{}").unwrap();
        assert!(!is_first_launch(&settings, &none));
    }

    #[test]
    fn an_item_that_could_not_be_copied_is_handed_back_as_a_fallback() {
        let (_root, old, new) = folders("{}");
        fs::create_dir_all(old.data.join("Dictations")).unwrap();
        fs::write(old.data.join("Dictations").join("a.txt"), "hello").unwrap();
        // A file where Loah's folder has to be.
        fs::create_dir_all(&new.data).unwrap();
        fs::write(new.data.join("Dictations"), "in the way").unwrap();

        let carried = carry_over(Some(old.clone()), Some(new));

        assert!(!carried.report.done);
        assert_eq!(
            carried.fallbacks,
            Fallbacks {
                dictations: Some(old.data.join("Dictations")),
                ..Fallbacks::default()
            }
        );
    }

    #[test]
    fn the_login_item_is_registered_at_the_start_that_wrote_the_marker_when_the_settings_ask_for_it(
    ) {
        let (_root, old, new) = folders(r#"{"launch_at_login": true}"#);
        let carried = carry_over(Some(old.clone()), Some(new.clone()));
        assert!(migrate::login_item_pending(&new));

        let calls = std::cell::Cell::new(0);
        let outcome = carried.settle_login_item(
            true,
            || {
                calls.set(calls.get() + 1);
                Ok(())
            },
            || {},
        );

        assert_eq!(outcome, LoginItem::Registered);
        assert_eq!(calls.get(), 1);
        assert!(
            !migrate::login_item_pending(&new),
            "done: not again at the next start"
        );
        // The next start: nothing is pending, the setting is applied only when it changes, as always.
        let again = carry_over(Some(old), Some(new)).settle_login_item(
            true,
            || {
                calls.set(calls.get() + 1);
                Ok(())
            },
            || {},
        );
        assert_eq!(again, LoginItem::NotPending);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_registration_that_failed_is_tried_again_at_the_next_start() {
        let (_root, old, new) = folders(r#"{"launch_at_login": true}"#);
        let first = carry_over(Some(old.clone()), Some(new.clone()));

        let outcome = first.settle_login_item(true, || Err("no permission".to_string()), || {});

        assert_eq!(outcome, LoginItem::Failed("no permission".to_string()));
        assert!(migrate::login_item_pending(&new), "kept for the next start");

        // The next start has no migration to do (the marker is there) and still has the flag.
        let second = carry_over(Some(old), Some(new.clone()));
        assert!(second.report.done && second.report.copied.is_empty());
        assert_eq!(
            second.settle_login_item(true, || Ok(()), || {}),
            LoginItem::Registered
        );
        assert!(!migrate::login_item_pending(&new));
    }

    #[test]
    fn a_login_item_that_the_settings_do_not_ask_for_is_not_registered_and_is_forgotten() {
        let (_root, old, new) = folders(r#"{"launch_at_login": false}"#);
        let carried = carry_over(Some(old), Some(new.clone()));

        let outcome = carried.settle_login_item(false, || panic!("registered"), || {});

        assert_eq!(outcome, LoginItem::NotWanted);
        assert!(!migrate::login_item_pending(&new));
    }

    #[test]
    fn a_start_that_is_not_after_a_migration_registers_nothing() {
        // No Dictate on this machine: no marker, no flag.
        let root = tempfile::tempdir().unwrap();
        let none = LegacyDirs {
            config: root.path().join("nowhere-config"),
            data: root.path().join("nowhere-data"),
        };
        let new = LegacyDirs {
            config: root.path().join("new-config"),
            data: root.path().join("new-data"),
        };
        let carried = carry_over(Some(none), Some(new));
        assert_eq!(
            carried.settle_login_item(true, || panic!("registered"), || panic!("tidied")),
            LoginItem::NotPending
        );
        // Nor without the folders of the system.
        assert_eq!(
            carry_over(None, None).settle_login_item(
                true,
                || panic!("registered"),
                || panic!("tidied")
            ),
            LoginItem::NotPending
        );
    }

    #[test]
    fn dictates_leftover_entry_goes_once_the_login_item_is_settled_and_not_before() {
        // Registered: Loah has its own entry now, and the one of Dictate would start a second Loah.
        let (_root, old, new) = folders(r#"{"launch_at_login": true}"#);
        let tidied = std::cell::Cell::new(0);
        let outcome = carry_over(Some(old), Some(new)).settle_login_item(
            true,
            || Ok(()),
            || tidied.set(tidied.get() + 1),
        );
        assert_eq!(outcome, LoginItem::Registered);
        assert_eq!(tidied.get(), 1);

        // Not wanted: the entry would start Loah at login all the same, against the setting.
        let (_root, old, new) = folders(r#"{"launch_at_login": false}"#);
        let tidied = std::cell::Cell::new(0);
        let outcome = carry_over(Some(old), Some(new)).settle_login_item(
            false,
            || panic!("registered"),
            || tidied.set(tidied.get() + 1),
        );
        assert_eq!(outcome, LoginItem::NotWanted);
        assert_eq!(tidied.get(), 1);
    }

    #[test]
    fn dictates_leftover_entry_stays_while_loahs_own_could_not_be_made() {
        // The wrapper entry still starts Loah: it goes at the start that registers Loah's own.
        let (_root, old, new) = folders(r#"{"launch_at_login": true}"#);
        let outcome = carry_over(Some(old), Some(new)).settle_login_item(
            true,
            || Err("no permission".to_string()),
            || panic!("tidied"),
        );
        assert_eq!(outcome, LoginItem::Failed("no permission".to_string()));
    }

    // ---- macOS

    #[test]
    fn a_running_app_with_either_identifier_of_the_old_dictate_counts() {
        assert!(old_app_running(|id| id == "io.github.henryonai.dictate"));
        assert!(old_app_running(|id| id == "local.dictate"));
        // Loah itself, and an app that only has a similar name.
        assert!(!old_app_running(|id| id == "io.github.henryonai.loah"));
        assert!(!old_app_running(|id| id == "com.example.dictate"));
        assert!(!old_app_running(|_| false));
    }

    const AGENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
  <key>Label</key>
  <string>Dictate</string>
  <key>ProgramArguments</key>
  <array><string>/Applications/Dictate.app/Contents/MacOS/dictate</string></array>
  <key>RunAtLoad</key>
  <true/>
  </dict>
</plist>"#;

    #[test]
    fn the_strings_of_a_launch_agent_are_read() {
        assert_eq!(
            plist_strings(AGENT),
            [
                "Dictate",
                "/Applications/Dictate.app/Contents/MacOS/dictate"
            ]
        );
        assert!(plist_strings("").is_empty());
    }

    #[test]
    fn a_launch_agent_that_starts_the_old_program_is_the_old_app_when_that_program_is() {
        let exe = Path::new("/Applications/Dictate.app/Contents/MacOS/dictate");
        assert!(launch_agent_starts_old_app(AGENT, |path| path == exe));
        // The program is gone (the app was deleted and the agent was left behind) or it is another app's.
        assert!(!launch_agent_starts_old_app(AGENT, |_| false));
    }

    #[test]
    fn a_launch_agent_of_loah_is_not_the_old_app() {
        let agent = AGENT
            .replace("Dictate.app", "Loah.app")
            .replace("MacOS/dictate", "MacOS/loah");
        // Not asked about at all: it names no program of Dictate's.
        assert!(!launch_agent_starts_old_app(&agent, |_| panic!("asked")));
    }

    fn bundle(root: &Path, identifier: &str) -> std::path::PathBuf {
        let contents = root.join("Dictate.app").join("Contents");
        fs::create_dir_all(contents.join("MacOS")).unwrap();
        fs::write(contents.join("MacOS").join("dictate"), b"\xcf\xfa").unwrap();
        fs::write(
            contents.join("Info.plist"),
            format!("<dict><key>CFBundleIdentifier</key>\n<string>{identifier}</string></dict>"),
        )
        .unwrap();
        contents.join("MacOS").join("dictate")
    }

    #[test]
    fn the_program_of_a_bundle_is_the_old_app_by_the_identifier_in_its_info_plist() {
        let root = tempfile::tempdir().unwrap();
        let exe = bundle(root.path(), "io.github.henryonai.dictate");
        assert!(is_old_app_executable(&exe));

        // Another app that has a program called dictate, and a LaunchAgent called Dictate.
        let other = tempfile::tempdir().unwrap();
        assert!(!is_old_app_executable(&bundle(
            other.path(),
            "com.example.dictate"
        )));
        // Loah's own identifier is not the old app's.
        let loah = tempfile::tempdir().unwrap();
        assert!(!is_old_app_executable(&bundle(
            loah.path(),
            "io.github.henryonai.loah"
        )));
    }

    #[test]
    fn a_program_that_is_not_there_is_not_the_old_app() {
        let root = tempfile::tempdir().unwrap();
        let exe = bundle(root.path(), "io.github.henryonai.dictate");
        fs::remove_file(&exe).unwrap();
        assert!(!is_old_app_executable(&exe));
        assert!(!is_old_app_executable(&root.path().join("nowhere/dictate")));
    }

    // ---- Windows

    const DICTATE_EXE: &str = r"C:\Users\Người Dùng\AppData\Local\Dictate\dictate.exe";

    fn reg_output(name: &str, data: &str) -> String {
        format!(
            "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    {name}    REG_SZ    {data}\r\n\r\n"
        )
    }

    #[test]
    fn a_run_entry_that_starts_the_old_program_is_the_old_app_when_it_is_there() {
        // The autostart plugin writes the path as it is, with a space after it for the arguments there are none of.
        let output = reg_output("Dictate", &format!("{DICTATE_EXE} "));
        assert!(run_entry_starts_old_app(&output, |path| {
            path == Path::new(DICTATE_EXE)
        }));
        // A quoted path and arguments after it read the same.
        let quoted = reg_output("Dictate", &format!("\"{DICTATE_EXE}\" --hidden"));
        assert!(run_entry_starts_old_app(&quoted, |path| {
            path == Path::new(DICTATE_EXE)
        }));
    }

    #[test]
    fn an_entry_that_an_uninstall_left_behind_is_not_the_old_app() {
        let output = reg_output("Dictate", &format!("{DICTATE_EXE} "));
        assert!(!run_entry_starts_old_app(&output, |_| false));
    }

    #[test]
    fn a_run_entry_of_another_program_or_name_is_not_the_old_app() {
        let there = |_: &Path| true;
        assert!(!run_entry_starts_old_app(
            &reg_output("Dictate", r"C:\Tools\other.exe "),
            there
        ));
        assert!(!run_entry_starts_old_app(
            &reg_output("Loah", r"C:\Users\x\AppData\Local\Loah\loah.exe "),
            there
        ));
        assert!(!run_entry_starts_old_app(
            &reg_output("DictateX", DICTATE_EXE),
            there
        ));
        assert!(!run_entry_starts_old_app("", there));
        assert!(!run_entry_starts_old_app(
            "ERROR: The system was unable to find the specified registry key or value.",
            there
        ));
    }

    #[test]
    fn a_run_entry_that_task_manager_has_turned_off_is_not_started() {
        // StartupApproved\Run: twelve bytes; the last eight are zero while the entry is on, and a time once it is turned off.
        let approved = |hex: &str| {
            format!(
                "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run\r\n    Dictate    REG_BINARY    {hex}\r\n\r\n"
            )
        };
        assert!(!startup_is_turned_off(&approved(
            "020000000000000000000000"
        )));
        assert!(!startup_is_turned_off(&approved(
            "060000000000000000000000"
        )));
        assert!(startup_is_turned_off(&approved("03000000A1B2C3D4E5F60718")));
        // No value, nothing readable, or too short to tell: the entry stands.
        assert!(!startup_is_turned_off(""));
        assert!(!startup_is_turned_off(
            "ERROR: The system was unable to find the specified registry key or value."
        ));
        assert!(!startup_is_turned_off(&approved("0300")));
        // A value of another name is not Dictate's.
        assert!(!startup_is_turned_off(
            &approved("03000000A1B2C3D4E5F60718").replace("Dictate", "Other")
        ));
    }

    // ---- Linux

    const WRAPPER: &str = "#!/bin/sh\nexec /usr/bin/loah \"$@\"\n";

    #[test]
    fn a_script_is_not_a_program_and_a_binary_is() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("dictate");
        fs::write(&script, WRAPPER).unwrap();
        let binary = dir.path().join("real");
        fs::write(&binary, b"\x7fELF....").unwrap();
        assert!(!is_program(&script));
        assert!(is_program(&binary));
        assert!(!is_program(&dir.path().join("missing")));
    }

    #[test]
    fn an_autostart_entry_that_starts_a_program_of_the_old_app_counts() {
        let entry = "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Dictate\nComment=Dictate startup script\nExec=/usr/bin/dictate \nStartupNotify=false\nTerminal=false\n";
        assert!(autostart_entry_starts_old_app(entry, |path| path == Path::new("/usr/bin/dictate")));
        // After the upgrade that same entry starts the wrapper, which starts Loah: nothing of the old app is left.
        assert!(!autostart_entry_starts_old_app(entry, |_| false));
    }

    #[test]
    fn an_autostart_entry_that_is_turned_off_is_not_the_old_app() {
        // The desktop's own switches for a startup entry: the entry is there and nothing starts.
        for off in [
            "Hidden=true",
            "X-GNOME-Autostart-enabled=false",
            "Hidden = true",
        ] {
            let entry = format!("[Desktop Entry]\nName=Dictate\nExec=/usr/bin/dictate\n{off}\n");
            assert!(!autostart_entry_starts_old_app(&entry, |_| true), "{off}");
        }
        for on in ["Hidden=false", "X-GNOME-Autostart-enabled=true"] {
            let entry = format!("[Desktop Entry]\nName=Dictate\nExec=/usr/bin/dictate\n{on}\n");
            assert!(autostart_entry_starts_old_app(&entry, |_| true), "{on}");
        }
    }

    #[test]
    fn an_autostart_entry_without_a_command_is_not_the_old_app() {
        assert!(!autostart_entry_starts_old_app(
            "[Desktop Entry]\nName=Dictate\n",
            |_| true
        ));
        assert!(!autostart_entry_starts_old_app("", |_| true));
    }

    // ---- Linux: the entry of Dictate that the package swap leaves as a second login item of Loah

    /// What Dictate 2.0.0 wrote for "Open at login": it runs `/usr/bin/dictate`, which after the upgrade is the wrapper.
    const DICTATES_ENTRY: &str = "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Dictate\nComment=Dictate startup script\nExec=/usr/bin/dictate \nStartupNotify=false\nTerminal=false\n";

    /// The check of a test: `/usr/bin/dictate` is the wrapper (as it is after the upgrade), whatever else is not.
    fn is_the_wrapper(path: &Path) -> bool {
        path == Path::new("/usr/bin/dictate")
    }

    /// `autostart/Dictate.desktop` in a temporary folder, holding `text`.
    fn dictates_entry(text: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("autostart").join("Dictate.desktop");
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::write(&entry, text).unwrap();
        (root, entry)
    }

    #[test]
    fn an_entry_that_starts_the_wrapper_is_one_that_starts_loah_a_second_time() {
        assert!(autostart_entry_starts_wrapper(
            DICTATES_ENTRY,
            is_the_wrapper
        ));
        // The same entry when `/usr/bin/dictate` is the old program: that is the old app's, not the wrapper's.
        assert!(!autostart_entry_starts_wrapper(DICTATES_ENTRY, |_| false));
        // An entry that starts something else, or nothing, or that the desktop has turned off.
        let appimage = DICTATES_ENTRY.replace("/usr/bin/dictate", "/home/me/Apps/Dictate.AppImage");
        assert!(!autostart_entry_starts_wrapper(&appimage, is_the_wrapper));
        assert!(!autostart_entry_starts_wrapper(
            "[Desktop Entry]\nName=Dictate\n",
            |_| true
        ));
        assert!(!autostart_entry_starts_wrapper(
            &format!("{DICTATES_ENTRY}Hidden=true\n"),
            |_| true
        ));
    }

    #[test]
    fn an_entry_that_starts_the_wrapper_is_removed() {
        let (_root, entry) = dictates_entry(DICTATES_ENTRY.as_bytes());

        assert_eq!(remove_wrapper_entry(&entry, is_the_wrapper), Ok(true));

        assert!(!entry.exists());
        // The folder is the desktop's, with its other entries in it: only the file goes.
        assert!(entry.parent().unwrap().is_dir());
    }

    #[test]
    fn an_entry_that_starts_a_real_dictate_is_kept() {
        // An AppImage, or an install in another place: a Dictate that is still there and is started at login.
        let own = DICTATES_ENTRY.replace("/usr/bin/dictate", "/home/me/Apps/Dictate.AppImage");
        let (_root, entry) = dictates_entry(own.as_bytes());
        assert_eq!(remove_wrapper_entry(&entry, is_the_wrapper), Ok(false));
        assert_eq!(fs::read_to_string(&entry).unwrap(), own);

        // `/usr/bin/dictate` itself, when it is the program of the old app.
        let (_root, entry) = dictates_entry(DICTATES_ENTRY.as_bytes());
        assert_eq!(remove_wrapper_entry(&entry, |_| false), Ok(false));
        assert!(entry.exists());
    }

    #[test]
    fn without_an_entry_there_is_nothing_to_remove() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("autostart").join("Dictate.desktop");

        assert_eq!(remove_wrapper_entry(&entry, |_| panic!("asked")), Ok(false));

        assert!(!root.path().join("autostart").exists(), "nothing is made");
    }

    #[test]
    fn an_entry_that_cannot_be_read_is_kept() {
        // Not text: what it says can't be known, and a file that is not Loah's to judge is not removed.
        let mut bytes = DICTATES_ENTRY.as_bytes().to_vec();
        bytes.extend_from_slice(b"\xff\xfe");
        let (_root, entry) = dictates_entry(&bytes);

        assert_eq!(remove_wrapper_entry(&entry, is_the_wrapper), Ok(false));

        assert_eq!(fs::read(&entry).unwrap(), bytes);
    }

    #[test]
    fn an_entry_that_is_turned_off_is_kept() {
        // The desktop's own switch: nothing starts, so nothing is in the way, and the person's choice is left as it is.
        let off = format!("{DICTATES_ENTRY}X-GNOME-Autostart-enabled=false\n");
        let (_root, entry) = dictates_entry(off.as_bytes());

        assert_eq!(remove_wrapper_entry(&entry, is_the_wrapper), Ok(false));

        assert!(entry.exists());
    }

    #[test]
    fn the_wrapper_is_the_script_of_the_package_and_not_a_program_or_another_script() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("dictate");
        fs::write(&wrapper, WRAPPER).unwrap();
        assert!(is_wrapper_script(&wrapper, &wrapper));

        // The same path, a program: the old Dictate.
        let program = dir.path().join("real");
        fs::write(&program, b"\x7fELF....").unwrap();
        assert!(!is_wrapper_script(&program, &program));
        // A script of the person's own, at another path: not the package's.
        let own = dir.path().join("start-dictate.sh");
        fs::write(&own, WRAPPER).unwrap();
        assert!(!is_wrapper_script(&own, &wrapper));
        // Nothing there.
        let missing = dir.path().join("missing");
        assert!(!is_wrapper_script(&missing, &missing));
    }

    #[test]
    fn a_process_of_usr_bin_dictate_is_the_old_app_when_that_is_a_program() {
        assert!(process_is_old_app("/usr/bin/dictate", |_| true));
        // The wrapper of the package is the same path: a script, and not the old app.
        assert!(!process_is_old_app("/usr/bin/dictate", |_| false));
        assert!(!process_is_old_app("/usr/bin/loah", |_| true));
        assert!(!process_is_old_app("/usr/bin/dictate-mcp", |_| true));
    }

    #[test]
    fn a_process_that_runs_what_was_usr_bin_dictate_is_the_old_app() {
        // The package was replaced while the app ran: the path now is the wrapper, and the process is the old binary.
        assert!(process_is_old_app("/usr/bin/dictate (deleted)", |_| false));
    }

    #[cfg(unix)]
    #[test]
    fn the_processes_are_read_from_proc() {
        let proc = tempfile::tempdir().unwrap();
        let process = |pid: &str, exe: &str| {
            let dir = proc.path().join(pid);
            fs::create_dir(&dir).unwrap();
            std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
        };
        // Other processes, and entries of /proc that are not processes.
        process("101", "/usr/bin/loah");
        process("102", "/bin/dash");
        fs::create_dir(proc.path().join("self-not-a-pid")).unwrap();
        fs::write(proc.path().join("uptime"), "1 1").unwrap();
        assert!(!old_app_in_proc(proc.path()));

        process("4242", "/usr/bin/dictate (deleted)");
        assert!(old_app_in_proc(proc.path()));
        assert!(!old_app_in_proc(&proc.path().join("nowhere")));
    }
}
