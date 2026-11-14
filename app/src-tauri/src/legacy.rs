//! What is left of Dictate on this machine: its settings, models and dictations, which Loah carries over once at the
//! first start, and the old app itself, which may still be installed, running or set to start at login.
//!
//! Loah never quits, unregisters or deletes the old app. When it is still there Settings says so (it uses the same
//! shortcut and the same meetings folder), and what to do about it is the README's section on upgrading.
//!
//! Finding the old app is best effort: where Loah can't tell, it says nothing. A notice about an app that is not there
//! would be worse than none, so every check asks for the thing itself (the program is on the disk and is Dictate's), not
//! only for a name that a registry or a file carries.

use loah_services::migrate::{self, LegacyDirs, MigrationReport};

/// The identifier of the Tauri Dictate (2.0.0), and the one of the Swift Dictate before it.
#[cfg(any(test, target_os = "macos"))]
const OLD_APP_IDS: [&str; 2] = ["io.github.henryonai.dictate", "local.dictate"];

/// Carries Dictate's settings, models and dictations over to Loah when that is due (see `loah_services::migrate`), and
/// logs each step. Before anything reads the settings: a person whose settings came over is not at a first launch.
pub fn carry_over_from_dictate() -> MigrationReport {
    carry_over(LegacyDirs::dictate(), LegacyDirs::loah())
}

fn carry_over(old: Option<LegacyDirs>, new: Option<LegacyDirs>) -> MigrationReport {
    let (Some(old), Some(new)) = (old, new) else {
        return MigrationReport::default();
    };
    let report = migrate::migrate_from_dictate(&old, &new);
    for item in &report.copied {
        log::info!("{}", migrate::step_message(item));
    }
    if let Some(failed) = report.failure_list() {
        log::warn!("Could not copy all of Dictate's files to Loah: {failed}");
    }
    report
}

/// Whether Loah registers itself as a login item at this start: the migration finished in this very run with something
/// copied (so, once), and the settings that came over have launch at login on. The setting is otherwise applied only when it
/// changes, and Loah has no login item of its own before this.
pub fn registers_login_item(report: &MigrationReport, launch_at_login: bool) -> bool {
    report.finished_now() && launch_at_login
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

// ---- Linux: the autostart entry of Dictate and the processes that run its program.

/// Whether the autostart entry `desktop` (`~/.config/autostart/Dictate.desktop`) starts a program that is the old
/// Dictate: one that is there and is not a script. After an upgrade `/usr/bin/dictate` is the two-line wrapper of the
/// package, which starts Loah, and that is not the old app.
#[cfg(any(test, target_os = "linux"))]
fn autostart_entry_starts_old_app(
    desktop: &str,
    is_program: impl Fn(&std::path::Path) -> bool,
) -> bool {
    desktop
        .lines()
        .find_map(|line| line.trim().strip_prefix("Exec="))
        .and_then(|command| command.split_whitespace().next())
        .is_some_and(|program| is_program(std::path::Path::new(program)))
}

/// Whether a file is a program and not a script (`#!`): the wrapper of the package is one.
#[cfg(any(test, target_os = "linux"))]
fn is_program(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut start = [0u8; 2];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut start));
    read.is_ok() && start != *b"#!"
}

/// Whether a process whose `/proc/<pid>/exe` points at `exe_link` is the old Dictate: `/usr/bin/dictate` when that is a
/// program, or what was `/usr/bin/dictate` when the package was removed or replaced while it ran.
#[cfg(any(test, target_os = "linux"))]
fn process_is_old_app(exe_link: &str, is_program: impl Fn(&std::path::Path) -> bool) -> bool {
    const PROGRAM: &str = "/usr/bin/dictate";
    exe_link == format!("{PROGRAM} (deleted)")
        || (exe_link == PROGRAM && is_program(std::path::Path::new(PROGRAM)))
}

/// Whether a process under `proc_root` (`/proc`) is the old Dictate.
#[cfg(any(test, target_os = "linux"))]
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
    let output = Command::new(reg)
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Dictate",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    output.is_ok_and(|output| {
        output.status.success()
            && run_entry_starts_old_app(&String::from_utf8_lossy(&output.stdout), |path| {
                path.is_file()
            })
    })
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
        assert_eq!(carry_over(None, None), MigrationReport::default());
        let dirs = LegacyDirs {
            config: "c".into(),
            data: "d".into(),
        };
        assert_eq!(
            carry_over(Some(dirs.clone()), None),
            MigrationReport::default()
        );
        assert_eq!(carry_over(None, Some(dirs)), MigrationReport::default());
    }

    #[test]
    fn the_carry_over_hands_back_what_the_migration_did() {
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
        fs::write(old.config.join("settings.json"), "{}").unwrap();

        let report = carry_over(Some(old), Some(new.clone()));

        assert_eq!(report.copied, ["settings.json"]);
        assert!(report.done && report.finished_now());
        assert!(new.config.join("settings.json").is_file());
    }

    #[test]
    fn the_login_item_is_registered_once_when_the_migration_finishes_and_the_settings_ask_for_it() {
        let finished = MigrationReport {
            copied: vec!["settings.json".into()],
            failed: vec![],
            done: true,
        };
        assert!(registers_login_item(&finished, true));
        // The copied settings don't ask for it.
        assert!(!registers_login_item(&finished, false));
        // A start that has nothing to do with a migration (it is done, or there was none): the setting is applied only
        // when it changes, as always.
        let done_before = MigrationReport {
            done: true,
            ..MigrationReport::default()
        };
        assert!(!registers_login_item(&done_before, true));
        assert!(!registers_login_item(&MigrationReport::default(), true));
        // A migration that is not finished is not the one that registers: the next start, which finishes it, is.
        let unfinished = MigrationReport {
            copied: vec!["settings.json".into()],
            failed: vec![("Dictations".into(), "x".into())],
            done: false,
        };
        assert!(!registers_login_item(&unfinished, true));
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
    fn an_autostart_entry_without_a_command_is_not_the_old_app() {
        assert!(!autostart_entry_starts_old_app(
            "[Desktop Entry]\nName=Dictate\n",
            |_| true
        ));
        assert!(!autostart_entry_starts_old_app("", |_| true));
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
