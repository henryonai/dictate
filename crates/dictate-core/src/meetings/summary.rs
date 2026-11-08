//! The summary Markdown, split into the sections the summary prompt asks for.

use unicode_normalization::UnicodeNormalization;

use super::transcript::{normalize_newlines, split_stamp};

pub const SUMMARY_NAMES: &[&str] = &["Tóm tắt", "Summary"];
pub const KEY_POINT_NAMES: &[&str] = &["Ý chính", "Key points"];
pub const DECISION_NAMES: &[&str] = &["Quyết định", "Decisions"];
pub const ACTION_NAMES: &[&str] = &["Việc cần làm", "Action items"];
pub const QUESTION_NAMES: &[&str] = &["Câu hỏi", "Open questions"];
pub const TIMELINE_NAMES: &[&str] = &["Mốc thời gian", "Timeline"];

/// The owners the summary prompt writes when nobody was named.
const UNKNOWN_OWNERS: &[&str] = &["chưa rõ", "unknown", "tbd"];

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub heading: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionItem {
    pub owner: Option<String>,
    pub text: String,
}

/// A "[mm:ss] topic — detail" line of the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub seconds: u32,
    pub label: String,
    pub text: String,
}

/// The summary Markdown split into sections, so the library can show action items as checkboxes and the
/// timeline as seek chips.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryDoc {
    pub title: Option<String>,
    pub sections: Vec<Section>,
}

impl SummaryDoc {
    /// The first "# " line is the title; each "## " line starts a section. Text before the first "## " is a
    /// section without a heading.
    pub fn parse(md: &str) -> SummaryDoc {
        let md = normalize_newlines(md);
        let mut title = None;
        let mut sections = Vec::new();
        let mut heading = "";
        let mut body: Vec<&str> = Vec::new();
        for line in md.split('\n') {
            if title.is_none() {
                if let Some(text) = line.strip_prefix("# ") {
                    title = Some(text.to_string());
                    continue;
                }
            }
            if let Some(text) = line.strip_prefix("## ") {
                flush(&mut sections, heading, &mut body);
                heading = trim_spaces(text);
                continue;
            }
            body.push(line);
        }
        flush(&mut sections, heading, &mut body);
        SummaryDoc { title, sections }
    }

    /// First section whose heading starts with one of `names` (case-insensitive; English fallbacks included).
    pub fn section(&self, names: &[&str]) -> Option<&Section> {
        let names: Vec<String> = names.iter().map(|name| canonical_lowercase(name)).collect();
        self.sections.iter().find(|section| {
            let heading = canonical_lowercase(&section.heading);
            names.iter().any(|name| heading.starts_with(name.as_str()))
        })
    }

    pub fn decisions(&self) -> Vec<String> {
        self.bullets(DECISION_NAMES)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    pub fn questions(&self) -> Vec<String> {
        self.bullets(QUESTION_NAMES)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// In summary order: `tasks.json` stores the done ones by their index in this list.
    pub fn action_items(&self) -> Vec<ActionItem> {
        self.bullets(ACTION_NAMES)
            .into_iter()
            .map(action_item)
            .collect()
    }

    /// "[mm:ss] topic — detail" lines.
    pub fn timeline(&self) -> Vec<Mark> {
        self.bullets(TIMELINE_NAMES)
            .into_iter()
            .filter_map(|line| {
                let (stamp, rest) = split_stamp(line)?;
                let seconds = stamp
                    .split(':')
                    .filter_map(|part| part.parse::<u32>().ok())
                    .fold(0_u32, |total, part| {
                        total.saturating_mul(60).saturating_add(part)
                    });
                let text = rest.split_once(" — ").map_or(rest, |(topic, _)| topic);
                Some(Mark {
                    seconds,
                    label: stamp.to_string(),
                    text: text.to_string(),
                })
            })
            .collect()
    }

    /// The "- " and "* " lines of the section, without the marker.
    fn bullets(&self, names: &[&str]) -> Vec<&str> {
        let Some(section) = self.section(names) else {
            return Vec::new();
        };
        section
            .body
            .split('\n')
            .map(trim_spaces)
            .filter_map(|line| line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")))
            .collect()
    }
}

/// Ends the section being read: it is kept when it has a heading or a body.
fn flush(sections: &mut Vec<Section>, heading: &str, body: &mut Vec<&str>) {
    let text = body.join("\n");
    let text = text.trim();
    if !heading.is_empty() || !text.is_empty() {
        sections.push(Section {
            heading: heading.to_string(),
            body: text.to_string(),
        });
    }
    body.clear();
}

/// "[ ] **Owner** — task": the checkbox and the "— " are dropped; an owner that is not known is `None`.
fn action_item(raw: &str) -> ActionItem {
    let mut text = raw;
    for checkbox in ["[ ] ", "[x] ", "[X] "] {
        if let Some(rest) = text.strip_prefix(checkbox) {
            text = rest;
        }
    }
    let owned = text
        .strip_prefix("**")
        .and_then(|after| after.split_once("**"));
    let Some((owner, rest)) = owned else {
        return ActionItem {
            owner: None,
            text: text.to_string(),
        };
    };
    let mut rest = trim_spaces(rest);
    for separator in ["— ", "– ", "- ", ": "] {
        if let Some(task) = rest.strip_prefix(separator) {
            rest = task;
        }
    }
    let unknown = UNKNOWN_OWNERS.contains(&canonical_lowercase(owner).as_str());
    ActionItem {
        owner: (!unknown).then(|| owner.to_string()),
        text: rest.to_string(),
    }
}

/// Lowercase and composed, so "Quyết" matches however its accents are written.
fn canonical_lowercase(text: &str) -> String {
    text.to_lowercase().nfc().collect()
}

/// Trims what Foundation's `.whitespaces` does: spaces and tabs, but not line ends.
pub(crate) fn trim_spaces(text: &str) -> &str {
    text.trim_matches(|c: char| {
        c == '\t'
            || (c.is_whitespace()
                && !matches!(
                    c,
                    '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
                ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixture::SUMMARY;

    fn doc(md: &str) -> SummaryDoc {
        SummaryDoc::parse(md)
    }

    fn owner_and_text(item: &ActionItem) -> (Option<&str>, &str) {
        (item.owner.as_deref(), item.text.as_str())
    }

    #[test]
    fn summary_sections() {
        let doc = doc(SUMMARY);
        assert_eq!(doc.title.as_deref(), Some("Họp kế hoạch refactor"));
        let summary = doc.section(SUMMARY_NAMES).unwrap();
        assert!(summary.body.starts_with("Nhóm thống nhất"));
        assert_eq!(doc.decisions(), ["Tách repo test riêng"]);
        let items = doc.action_items();
        assert_eq!(items.len(), 2);
        assert!(
            items[0].owner.as_deref() == Some("Henry")
                && items[0].text == "Tạo repo test mới (thứ Sáu)"
        );
        assert!(items[1].owner.is_none()); // "Chưa rõ"
        assert_eq!(
            doc.timeline().iter().map(|m| m.seconds).collect::<Vec<_>>(),
            [0, 65]
        );
    }

    #[test]
    fn summary_crlf() {
        let d = SummaryDoc::parse(&SUMMARY.replace('\n', "\r\n"));
        assert_eq!(d.title.as_deref(), Some("Họp kế hoạch refactor"));
        assert_eq!(d.action_items().len(), 2);
        assert_eq!(d.timeline()[1].seconds, 65);
    }

    #[test]
    fn the_title_is_the_first_h1_and_later_ones_are_body_text() {
        let d = doc("# Một\n\n## A\n# Hai\nnội dung");
        assert_eq!(d.title.as_deref(), Some("Một"));
        assert_eq!(d.sections[0].body, "# Hai\nnội dung");
        assert_eq!(doc("## A\nx").title, None);
    }

    #[test]
    fn text_before_the_first_heading_is_a_section_without_a_name() {
        let d = doc("# Tiêu đề\nLời mở đầu\n## Tóm tắt\nNội dung");
        assert_eq!(d.sections.len(), 2);
        assert_eq!(d.sections[0].heading, "");
        assert_eq!(d.sections[0].body, "Lời mở đầu");
        assert_eq!(d.sections[1].heading, "Tóm tắt");
    }

    #[test]
    fn a_heading_without_a_body_is_still_a_section() {
        let d = doc("## Câu hỏi\n\n## Quyết định\n- Chốt");
        assert_eq!(d.sections.len(), 2);
        assert_eq!(d.sections[0].heading, "Câu hỏi");
        assert_eq!(d.sections[0].body, "");
    }

    #[test]
    fn headings_and_bodies_are_trimmed() {
        let d = doc("##  Tóm tắt \t\n\n  Nội dung  \n\n");
        assert_eq!(d.sections[0].heading, "Tóm tắt");
        assert_eq!(d.sections[0].body, "Nội dung");
    }

    #[test]
    fn a_section_is_found_by_the_start_of_its_heading_in_any_case() {
        let d = doc("## DECISIONS (2)\n- A\n\n## Câu hỏi còn mở\n- B\n\n## Action items\n- C");
        assert_eq!(d.decisions(), ["A"]);
        assert_eq!(d.questions(), ["B"]);
        assert_eq!(d.action_items()[0].text, "C");
        assert!(d.section(TIMELINE_NAMES).is_none());
    }

    #[test]
    fn a_heading_matches_whichever_way_its_accents_are_written() {
        // "Quyết định" with "ế" and "ị" as base letter + combining marks.
        let d = doc("## Quye\u{302}\u{301}t \u{111}i\u{323}nh\n- A");
        assert_eq!(d.decisions(), ["A"]);
    }

    #[test]
    fn only_dash_and_star_lines_are_bullets() {
        let d = doc("## Quyết định\n- A\n* B\n   - C\n-D\nDòng thường\n1. E");
        assert_eq!(d.decisions(), ["A", "B", "C"]);
    }

    #[test]
    fn action_item_checkboxes_and_separators_are_dropped() {
        let d = doc(
            "## Việc cần làm\n- [x] **Lan** – Gửi bản nháp\n- [X] **Nam** - Đặt phòng\n\
             - [ ] **Henry**: Tạo repo\n- [ ] **An** —Không có dấu cách\n- Không có chủ",
        );
        let items = d.action_items();
        let got: Vec<_> = items.iter().map(owner_and_text).collect();
        assert_eq!(
            got,
            [
                (Some("Lan"), "Gửi bản nháp"),
                (Some("Nam"), "Đặt phòng"),
                (Some("Henry"), "Tạo repo"),
                (Some("An"), "—Không có dấu cách"),
                (None, "Không có chủ"),
            ]
        );
    }

    #[test]
    fn an_unclosed_owner_is_left_in_the_text() {
        let d = doc("## Việc cần làm\n- **Lan — Gửi bản nháp");
        assert_eq!(
            owner_and_text(&d.action_items()[0]),
            (None, "**Lan — Gửi bản nháp")
        );
    }

    #[test]
    fn an_owner_that_is_not_known_is_none_in_any_case() {
        let d = doc(
            "## Action items\n- **TBD** — a\n- **Unknown** — b\n- **CHƯA RÕ** — c\n- **Lan** — d",
        );
        let owners: Vec<_> = d.action_items().into_iter().map(|i| i.owner).collect();
        assert_eq!(owners, [None, None, None, Some("Lan".to_string())]);
    }

    #[test]
    fn the_timeline_reads_hours_and_cuts_the_detail() {
        let d = doc(
            "## Timeline\n- [1:02:03] Kết luận — chốt lịch — thứ Sáu\n- [00:30]   Mở đầu\n\
             - Không có mốc\n- [12:34] Giữa buổi",
        );
        let marks = d.timeline();
        assert_eq!(
            marks,
            [
                Mark {
                    seconds: 3723,
                    label: "1:02:03".into(),
                    text: "Kết luận".into()
                },
                Mark {
                    seconds: 30,
                    label: "00:30".into(),
                    text: "Mở đầu".into()
                },
                Mark {
                    seconds: 754,
                    label: "12:34".into(),
                    text: "Giữa buổi".into()
                },
            ]
        );
    }
}
