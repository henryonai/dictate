//! Recognition languages and how the hotkey is shown (sections R1 and R2 of the plan 3 dictation reference).

/// The languages at the top of the language menu, in the order they are shown.
pub const FAVOURITES: [&str; 8] = [
    "en-US", "vi-VN", "en-GB", "ja-JP", "ko-KR", "zh-CN", "fr-FR", "de-DE",
];

/// The languages under "Other languages", in the order they are shown.
pub const OTHERS: &[&str] = &[
    "es-ES", "it-IT", "pt-BR", "pt-PT", "nl-NL", "pl-PL", "ru-RU", "uk-UA", "tr-TR", "id-ID",
    "ms-MY", "th-TH", "hi-IN", "ar-SA", "he-IL", "el-GR", "cs-CZ", "sv-SE", "da-DK", "fi-FI",
    "nb-NO", "hu-HU", "ro-RO", "zh-TW",
];

/// The two-letter label of a language in the bubble and the pill: "EN" for "en-US".
pub fn short_label(lang: &str) -> String {
    lang.chars().take(2).flat_map(char::to_uppercase).collect()
}

/// A hotkey as the user reads it. `accel` is a global-shortcut accelerator such as "Ctrl+Alt+Space". On the Mac
/// the modifiers become their symbols and the "+" goes, as in "⌃⌥Space". Elsewhere it stays as it is, except that
/// "CmdOrCtrl" is "Ctrl". Modifiers keep the order they were given in.
pub fn display_hotkey(accel: &str, mac: bool) -> String {
    let keys = accel.split('+').map(|part| {
        let name = part.trim();
        match (modifier(name), mac) {
            (Some(Modifier::Ctrl), true) => "⌃",
            (Some(Modifier::Alt), true) => "⌥",
            (Some(Modifier::Shift), true) => "⇧",
            (Some(Modifier::Cmd | Modifier::CmdOrCtrl), true) => "⌘",
            (Some(Modifier::CmdOrCtrl), false) => "Ctrl",
            (_, true) => name,
            (_, false) => part,
        }
    });
    if mac {
        keys.collect()
    } else {
        keys.collect::<Vec<_>>().join("+")
    }
}

enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Cmd,
    CmdOrCtrl,
}

/// The modifier an accelerator part names, in any of the spellings and cases the shortcut plugin takes.
fn modifier(name: &str) -> Option<Modifier> {
    let is = |names: &[&str]| names.iter().any(|n| name.eq_ignore_ascii_case(n));
    if is(&["ctrl", "control"]) {
        Some(Modifier::Ctrl)
    } else if is(&["alt", "option"]) {
        Some(Modifier::Alt)
    } else if is(&["shift"]) {
        Some(Modifier::Shift)
    } else if is(&["cmd", "command", "super", "meta"]) {
        Some(Modifier::Cmd)
    } else if is(&[
        "cmdorctrl",
        "cmdorcontrol",
        "commandorctrl",
        "commandorcontrol",
    ]) {
        Some(Modifier::CmdOrCtrl)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_lists_start_with_english_and_have_no_repeats() {
        assert_eq!(FAVOURITES[0], "en-US");
        let all: Vec<&str> = FAVOURITES.iter().chain(OTHERS).copied().collect();
        let mut unique = all.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), all.len(), "a language is listed twice");
        assert_eq!(all.len(), 8 + 24);
    }

    #[test]
    fn language_codes_are_language_and_region() {
        for code in FAVOURITES.iter().chain(OTHERS) {
            let (language, region) = code.split_once('-').expect(code);
            assert!(
                language.len() == 2 && language.chars().all(|c| c.is_ascii_lowercase()),
                "{code}"
            );
            assert!(
                region.len() == 2 && region.chars().all(|c| c.is_ascii_uppercase()),
                "{code}"
            );
        }
    }

    #[test]
    fn short_labels() {
        assert_eq!(short_label("en-US"), "EN");
        assert_eq!(short_label("vi-VN"), "VI");
        assert_eq!(short_label("zh-TW"), "ZH");
        assert_eq!(short_label("ja"), "JA");
        assert_eq!(short_label(""), "");
    }

    #[test]
    fn display_hotkey_mac_symbols() {
        assert_eq!(display_hotkey("Ctrl+Alt+Space", true), "⌃⌥Space");
        assert_eq!(display_hotkey("Cmd+Shift+K", true), "⌘⇧K");
        assert_eq!(display_hotkey("F5", true), "F5");
        assert_eq!(display_hotkey("Alt+Ctrl+Space", true), "⌥⌃Space");
    }

    #[test]
    fn display_hotkey_mac_knows_every_modifier_spelling() {
        assert_eq!(display_hotkey("Control+Option+Space", true), "⌃⌥Space");
        for cmd in [
            "Cmd",
            "Command",
            "Super",
            "Meta",
            "CmdOrCtrl",
            "CommandOrControl",
            "CmdOrControl",
        ] {
            assert_eq!(display_hotkey(&format!("{cmd}+A"), true), "⌘A", "{cmd}");
        }
        assert_eq!(display_hotkey("ctrl+SHIFT+a", true), "⌃⇧a");
    }

    #[test]
    fn display_hotkey_cmd_or_ctrl_is_ctrl_off_the_mac() {
        assert_eq!(
            display_hotkey("CmdOrCtrl+Shift+Space", false),
            "Ctrl+Shift+Space"
        );
        assert_eq!(display_hotkey("CommandOrControl+K", false), "Ctrl+K");
    }

    #[test]
    fn display_hotkey_off_the_mac_is_unchanged() {
        for accel in ["Ctrl+Alt+Space", "Cmd+Shift+K", "Super+A", "F5", ""] {
            assert_eq!(display_hotkey(accel, false), accel);
        }
    }
}
