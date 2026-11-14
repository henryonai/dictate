//! Search that ignores case and Vietnamese diacritics: "hop doi" finds "Họp đội".

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// A query split into folded words; see [`TextSearch::matches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSearch {
    pub words: Vec<String>,
}

impl TextSearch {
    pub fn new(query: &str) -> Self {
        Self {
            words: fold(query).split_whitespace().map(str::to_owned).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Every word of the query appears in `text`.
    pub fn matches(&self, text: &str) -> bool {
        if self.words.is_empty() {
            return true;
        }
        let folded = fold(text);
        self.words.iter().all(|word| folded.contains(word.as_str()))
    }

    /// `text` on one line, or a window of about `length` characters around the
    /// first word found in it, with `…` marking each cut end.
    pub fn snippet(&self, text: &str, length: usize) -> String {
        // NFC first so one Vietnamese letter is one `char`, whatever form the file used.
        let flat: Vec<char> = text
            .nfc()
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        if flat.len() <= length {
            return flat.into_iter().collect();
        }
        let flat_text: String = flat.iter().collect();
        let folded = fold(&flat_text);
        // Folding keeps one char per char for Vietnamese; when it doesn't, show the start.
        let center = if folded.chars().count() == flat.len() {
            self.words
                .first()
                .and_then(|word| char_position(&folded, word))
                .unwrap_or(0)
        } else {
            0
        };
        let start = center.saturating_sub(length / 3).min(flat.len() - length);
        let end = start + length;
        let mut out = String::new();
        if start > 0 {
            out.push('…');
        }
        out.extend(&flat[start..end]);
        if end < flat.len() {
            out.push('…');
        }
        out
    }
}

/// Lower-cases and strips combining marks (any Unicode `Mark`: Mn, Mc, Me) and `đ`,
/// so "Họp Đội" becomes "hop doi".
pub fn fold(s: &str) -> String {
    s.nfd()
        .filter(|&c| !is_combining_mark(c))
        // Per char (not `str::to_lowercase`) so a letter folds the same wherever it stands.
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'đ' { 'd' } else { c })
        .collect()
}

/// Index, counted in chars, of the first occurrence of `needle` in `haystack`.
fn char_position(haystack: &str, needle: &str) -> Option<usize> {
    let byte = haystack.find(needle)?;
    let before = haystack.char_indices().take_while(|&(i, _)| i < byte);
    Some(before.count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_vietnamese() {
        assert_eq!(fold("Họp Đội đẹp"), "hop doi dep");
    }

    #[test]
    fn fold_lowercases_ascii_and_keeps_other_text() {
        assert_eq!(fold("Hello, WORLD 42"), "hello, world 42");
    }

    #[test]
    fn fold_treats_decomposed_and_precomposed_alike() {
        assert_eq!(fold("e\u{0323}\u{0302}"), "e");
        assert_eq!(fold("\u{1EC7}"), "e");
    }

    #[test]
    fn matches_all_words() {
        let s = TextSearch::new("hop doi");
        assert!(s.matches("Họp đội tuần"));
        assert!(!s.matches("Họp tuần"));
        assert!(TextSearch::new("  ").matches("anything"));
    }

    #[test]
    fn new_splits_the_folded_query_into_words() {
        let s = TextSearch::new("  Họp\tĐỘI \n tuần ");
        assert_eq!(s.words, vec!["hop", "doi", "tuan"]);
        assert!(!s.is_empty());
        assert!(TextSearch::new(" \t ").is_empty());
    }

    #[test]
    fn snippet_short_text_unchanged() {
        assert_eq!(TextSearch::new("x").snippet("a\nb", 280), "a b");
    }

    #[test]
    fn snippet_exact_length_is_not_cut() {
        let at_limit = "a".repeat(280);
        assert_eq!(TextSearch::new("a").snippet(&at_limit, 280), at_limit);
        let over = "a".repeat(281);
        let out = TextSearch::new("zzz").snippet(&over, 280);
        assert_eq!(out, format!("{}…", "a".repeat(280)));
    }

    #[test]
    fn snippet_multibyte_window() {
        let text = format!(
            "{}refactor{}",
            "Chào mọi người ".repeat(40),
            " đồng ý ".repeat(40)
        );
        let out = TextSearch::new("refactor").snippet(&text, 280);
        assert!(out.starts_with('…') && out.ends_with('…'));
        assert!(out.contains("refactor"));
        assert_eq!(out.trim_matches('…').chars().count(), 280);
    }

    #[test]
    fn snippet_centres_a_third_of_the_window_before_the_match() {
        let text = format!("{}needle{}", "a".repeat(500), "b".repeat(500));
        let out = TextSearch::new("needle").snippet(&text, 300);
        let body = out.trim_matches('…');
        assert_eq!(body.chars().count(), 300);
        assert_eq!(body.find("needle"), Some(100));
    }

    #[test]
    fn snippet_without_a_match_shows_the_start() {
        let text = "x".repeat(1000);
        let out = TextSearch::new("zzz").snippet(&text, 280);
        assert_eq!(out, format!("{}…", "x".repeat(280)));
    }

    #[test]
    fn snippet_match_near_the_end_shows_the_tail() {
        let text = format!("{}needle", "a".repeat(1000));
        let out = TextSearch::new("needle").snippet(&text, 280);
        assert!(out.starts_with('…') && !out.ends_with('…'));
        assert!(out.ends_with("needle"));
        assert_eq!(out.trim_matches('…').chars().count(), 280);
    }

    #[test]
    fn snippet_match_near_the_start_has_no_leading_ellipsis() {
        let text = format!("needle{}", "a".repeat(1000));
        let out = TextSearch::new("needle").snippet(&text, 280);
        assert!(out.starts_with("needle") && out.ends_with('…'));
    }

    #[test]
    fn snippet_finds_an_unaccented_query_in_accented_text() {
        let text = format!("{}Họp đội{}", "x".repeat(600), "y".repeat(600));
        let out = TextSearch::new("hop doi").snippet(&text, 280);
        assert!(out.contains("Họp đội"));
    }

    #[test]
    fn snippet_counts_decomposed_vietnamese_as_one_char_per_letter() {
        let composed = format!("{}Họp đội{}", "x".repeat(600), "y".repeat(600));
        let decomposed = composed
            .replace('ọ', "o\u{0323}")
            .replace('ộ', "o\u{0302}\u{0323}");
        assert_ne!(composed, decomposed);
        let q = TextSearch::new("hop doi");
        assert_eq!(q.snippet(&decomposed, 280), q.snippet(&composed, 280));
    }

    #[test]
    fn snippet_shows_the_start_when_folding_changes_the_char_count() {
        // U+20DD (enclosing circle) is dropped by fold but survives NFC, so the
        // folded and original texts differ in length and the match cannot be located.
        let text = format!("{}\u{20DD}needle{}", "a".repeat(600), "b".repeat(600));
        let out = TextSearch::new("needle").snippet(&text, 280);
        assert_eq!(out, format!("{}…", "a".repeat(280)));
    }
}
