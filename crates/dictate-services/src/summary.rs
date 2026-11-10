//! Summaries of a recording's transcript, and questions about a recording. The prompts and the limits are the Swift
//! app's (Summarizer.swift and MeetingManager.ask), word for word: the Vietnamese headings are part of the library's
//! format, and `dictate-mcp` and the summary view read them back.

use std::time::Duration;

use crate::ai::{complete, AiConfig};
use crate::http::ApiError;

/// The system prompt of a summary: English instructions, Vietnamese headings.
pub const SUMMARY_SYSTEM: &str = r#"You turn transcripts of recordings (meetings, calls, lectures, videos) into clear summaries.
The transcript comes from automatic speech recognition: expect misheard words, missing punctuation and mixed Vietnamese/English. Infer the intended words from context, but never invent facts, names, numbers or commitments that are not supported by the transcript.
Lines look like "[mm:ss] Speaker: text". The speaker is a generic label ("Speaker 1"), a real name the user gave, or missing, and labels may be imperfect. Refer to people by those real names when there are some (e.g. as owners).

Write in the dominant language of the transcript (usually Vietnamese), keeping English technical terms as spoken.
Output GitHub-flavored Markdown with exactly these sections (omit a section only if it would be empty, except Tóm tắt):

# <short descriptive title>
## Tóm tắt
3–6 sentences: what this was about and the outcome.
## Ý chính
- key points, grouped logically, concrete (numbers, names, examples)
## Quyết định
- decisions that were made
## Việc cần làm
- [ ] **Owner** — task (deadline if mentioned)  ← no owner: "Chưa rõ", or "Unknown" when writing in English
## Câu hỏi còn mở
- unresolved questions / risks
## Mốc thời gian
- [mm:ss] topic — one line per major topic shift

Use the section headings above translated to the output language if it is not Vietnamese. No preamble before the title."#;

/// The system prompt of the notes taken from one part of a transcript that is too long for one request.
pub const NOTES_SYSTEM: &str = r#"You take detailed, factual notes from part of a speech-recognition transcript. Keep timestamps [mm:ss], speakers, decisions, action items, numbers and open questions. Write in the transcript's language. Output Markdown bullet notes only."#;

/// The system prompt of a question about a recording.
pub const ASK_SYSTEM: &str = r#"Answer questions about a recording using only its transcript (lines "[mm:ss] Speaker: text"). Cite the timestamps you rely on as [mm:ss]. If the transcript does not contain the answer, say so. Answer in the language of the question, concisely, in plain text or short Markdown bullets."#;

/// A transcript of more than this many characters (not bytes) is summarized in parts, and each part holds at most
/// this many.
const CHUNK_LIMIT: usize = 100_000;
/// How much of the transcript goes with a question, in characters.
const ASK_LIMIT: usize = 300_000;
/// The longest answer asked for: a summary or a merge, the notes of one part, the answer to a question.
const SUMMARY_TOKENS: u32 = 8000;
const NOTES_TOKENS: u32 = 4000;
const ASK_TOKENS: u32 = 1500;
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(300);
const ASK_TIMEOUT: Duration = Duration::from_secs(120);

/// What separates two paragraphs of a transcript.
const PARAGRAPH_BREAK: &str = "\n\n";

/// The system prompt of a summary, plus the user's vocabulary when there is one.
pub fn summary_system(vocabulary: &str) -> String {
    let mut system = String::from(SUMMARY_SYSTEM);
    let vocabulary = vocabulary.trim();
    if !vocabulary.is_empty() {
        system.push_str("\n\nCustom vocabulary / context (prefer these spellings):\n");
        system.push_str(vocabulary);
    }
    system
}

/// The transcript in parts of at most `limit` characters, cut only between paragraphs (blank lines): the parts joined
/// by a blank line are the transcript again (less any blank lines at its very start). A paragraph longer than
/// `limit` is a part of its own, whole.
pub fn summary_parts(transcript: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut part_chars = 0;
    // Two characters, and two bytes: it is ASCII.
    let break_chars = PARAGRAPH_BREAK.len();
    for paragraph in transcript.split(PARAGRAPH_BREAK) {
        let chars = paragraph.chars().count();
        if !part.is_empty() && part_chars + break_chars + chars > limit {
            parts.push(std::mem::take(&mut part));
            part_chars = 0;
        }
        if part.is_empty() {
            part_chars = chars;
        } else {
            part.push_str(PARAGRAPH_BREAK);
            part_chars += break_chars + chars;
        }
        part.push_str(paragraph);
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// Where a long summary is: the notes of one part (counted from 1) are being taken, or the notes are being merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryProgress {
    Part { index: usize, count: usize },
    Merging,
}

/// The summary of a transcript, as Markdown, trimmed. `untitled`: the recording has no title of its own, so the model
/// is asked for one (the first `# ` line, see `title_from_summary`). A transcript of more than 100,000 characters is
/// summarized in two steps: notes on each part, one request after the other, then a summary of the notes.
/// `on_progress` is told before each of those requests.
///
/// The returned future is `Send` as long as `on_progress` is, with no need for it to be `Sync`: it is only ever
/// called, never shared.
pub async fn summarize(
    c: &AiConfig,
    title: &str,
    untitled: bool,
    transcript: &str,
    mut on_progress: impl FnMut(SummaryProgress) + Send,
) -> Result<String, ApiError> {
    let title = if untitled {
        format!("untitled ({title}) — write a short descriptive title from the content")
    } else {
        title.to_owned()
    };
    let system = summary_system(&c.vocabulary);
    // `complete` hands back trimmed text, and fails on an empty one; its errors never hold the key.
    if transcript.chars().count() <= CHUNK_LIMIT {
        let user = format!("Recording: {title}\n\n<transcript>\n{transcript}\n</transcript>");
        return complete(c, &system, &user, SUMMARY_TOKENS, SUMMARY_TIMEOUT, false).await;
    }

    let parts = summary_parts(transcript, CHUNK_LIMIT);
    let count = parts.len();
    let mut notes = Vec::with_capacity(count);
    for (i, part) in parts.iter().enumerate() {
        let index = i + 1;
        on_progress(SummaryProgress::Part { index, count });
        let user = format!(
            "Part {index} of {count} of \"{title}\":\n\n<transcript>\n{part}\n</transcript>"
        );
        let part_notes =
            complete(c, NOTES_SYSTEM, &user, NOTES_TOKENS, SUMMARY_TIMEOUT, false).await?;
        notes.push(format!("### Phần {index}\n{part_notes}"));
    }
    on_progress(SummaryProgress::Merging);
    let user = format!(
        "Recording: {title}\nThe transcript was too long, so here are detailed notes of each consecutive part instead. Summarize the whole recording from them.\n\n<notes>\n{}\n</notes>",
        notes.join("\n\n")
    );
    complete(c, &system, &user, SUMMARY_TOKENS, SUMMARY_TIMEOUT, false).await
}

/// The title the model gave: the text of the first line that starts with `# `, trimmed. `None` when there is no such
/// line, or its text is blank.
pub fn title_from_summary(md: &str) -> Option<String> {
    let title = md.lines().find_map(|line| line.strip_prefix("# "))?.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// The answer to a question about a recording, from its transcript alone (the first 300,000 characters of it; the
/// summary is not sent). Each question stands by itself: nothing of an earlier one goes along.
pub async fn ask(
    c: &AiConfig,
    title: &str,
    transcript: &str,
    question: &str,
) -> Result<String, ApiError> {
    // The cut counts characters, so it is always on a character boundary: the text is mostly Vietnamese.
    let transcript = match transcript.char_indices().nth(ASK_LIMIT) {
        Some((end, _)) => &transcript[..end],
        None => transcript,
    };
    let user = format!(
        "Recording: {title}\n\n<transcript>\n{transcript}\n</transcript>\n\nQuestion: {question}"
    );
    complete(c, ASK_SYSTEM, &user, ASK_TOKENS, ASK_TIMEOUT, false).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::{json, Value};
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::presets::AiProvider;

    /// Long enough that hiding it in a message cannot garble an ordinary word.
    const KEY: &str = "sk-test-key-123";
    const PROVIDERS: [AiProvider; 2] = [AiProvider::Anthropic, AiProvider::OpenAi];

    /// The summary prompt as the reference (R4) and the Swift app have it, a line each, kept apart from the constant so
    /// that a slip in either one shows. Vietnamese headings, an en dash, an arrow and two spaces before it included.
    const R4_SYSTEM_LINES: [&str; 22] = [
        r#"You turn transcripts of recordings (meetings, calls, lectures, videos) into clear summaries."#,
        r#"The transcript comes from automatic speech recognition: expect misheard words, missing punctuation and mixed Vietnamese/English. Infer the intended words from context, but never invent facts, names, numbers or commitments that are not supported by the transcript."#,
        r#"Lines look like "[mm:ss] Speaker: text". The speaker is a generic label ("Speaker 1"), a real name the user gave, or missing, and labels may be imperfect. Refer to people by those real names when there are some (e.g. as owners)."#,
        "",
        r#"Write in the dominant language of the transcript (usually Vietnamese), keeping English technical terms as spoken."#,
        r#"Output GitHub-flavored Markdown with exactly these sections (omit a section only if it would be empty, except Tóm tắt):"#,
        "",
        r#"# <short descriptive title>"#,
        r#"## Tóm tắt"#,
        r#"3–6 sentences: what this was about and the outcome."#,
        r#"## Ý chính"#,
        r#"- key points, grouped logically, concrete (numbers, names, examples)"#,
        r#"## Quyết định"#,
        r#"- decisions that were made"#,
        r#"## Việc cần làm"#,
        r#"- [ ] **Owner** — task (deadline if mentioned)  ← no owner: "Chưa rõ", or "Unknown" when writing in English"#,
        r#"## Câu hỏi còn mở"#,
        r#"- unresolved questions / risks"#,
        r#"## Mốc thời gian"#,
        r#"- [mm:ss] topic — one line per major topic shift"#,
        "",
        r#"Use the section headings above translated to the output language if it is not Vietnamese. No preamble before the title."#,
    ];

    const R4_NOTES_SYSTEM: &str = r#"You take detailed, factual notes from part of a speech-recognition transcript. Keep timestamps [mm:ss], speakers, decisions, action items, numbers and open questions. Write in the transcript's language. Output Markdown bullet notes only."#;

    const R6_ASK_SYSTEM: &str = r#"Answer questions about a recording using only its transcript (lines "[mm:ss] Speaker: text"). Cite the timestamps you rely on as [mm:ss]. If the transcript does not contain the answer, say so. Answer in the language of the question, concisely, in plain text or short Markdown bullets."#;

    fn config(provider: AiProvider, base_url: &str) -> AiConfig {
        AiConfig {
            provider,
            base_url: base_url.to_string(),
            model: "test-model".to_string(),
            api_key: Some(KEY.to_string()),
            vocabulary: String::new(),
        }
    }

    /// What one request asked the model, whichever provider's body it came in.
    #[derive(Debug)]
    struct Sent {
        system: String,
        user: String,
        /// `None` for an OpenAI-compatible server: its body carries no limit (as in the Swift app).
        max_tokens: Option<u64>,
    }

    fn sent(provider: AiProvider, request: &Request) -> Sent {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let text = |v: &Value| v.as_str().unwrap().to_string();
        match provider {
            AiProvider::Anthropic => Sent {
                system: text(&body["system"]),
                user: text(&body["messages"][0]["content"]),
                max_tokens: body["max_tokens"].as_u64(),
            },
            AiProvider::OpenAi => Sent {
                system: text(&body["messages"][0]["content"]),
                user: text(&body["messages"][1]["content"]),
                max_tokens: None,
            },
        }
    }

    fn reply(provider: AiProvider, text: &str) -> ResponseTemplate {
        match provider {
            AiProvider::Anthropic => ResponseTemplate::new(200)
                .set_body_json(json!({"content": [{"type": "text", "text": text}]})),
            AiProvider::OpenAi => ResponseTemplate::new(200).set_body_json(
                json!({"choices": [{"message": {"role": "assistant", "content": text}}]}),
            ),
        }
    }

    /// A server that decides each answer from the request it gets.
    async fn server_with(
        provider: AiProvider,
        answer: impl Fn(&Sent) -> ResponseTemplate + Send + Sync + 'static,
    ) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(move |request: &Request| answer(&sent(provider, request)))
            .mount(&server)
            .await;
        server
    }

    async fn server_replying(provider: AiProvider, text: &'static str) -> MockServer {
        server_with(provider, move |_| reply(provider, text)).await
    }

    /// Everything the server was asked, in order.
    async fn requests(server: &MockServer, provider: AiProvider) -> Vec<Sent> {
        let received = server.received_requests().await.unwrap();
        received.iter().map(|r| sent(provider, r)).collect()
    }

    fn assert_limit(provider: AiProvider, sent: &Sent, tokens: u64) {
        // Only the Anthropic body carries a limit.
        if provider == AiProvider::Anthropic {
            assert_eq!(sent.max_tokens, Some(tokens), "{sent:?}");
        }
    }

    // ---- the prompts

    #[test]
    fn summary_prompt_is_verbatim() {
        assert_eq!(SUMMARY_SYSTEM, R4_SYSTEM_LINES.join("\n"));
        assert_eq!(summary_system(""), SUMMARY_SYSTEM);
    }

    #[test]
    fn notes_and_ask_prompts_are_verbatim() {
        assert_eq!(NOTES_SYSTEM, R4_NOTES_SYSTEM);
        assert_eq!(ASK_SYSTEM, R6_ASK_SYSTEM);
    }

    #[test]
    fn summary_system_appends_the_trimmed_vocabulary() {
        assert_eq!(summary_system(" \n\t "), SUMMARY_SYSTEM);
        assert_eq!(
            summary_system("  Kubernetes, Tauri \n"),
            format!(
                "{SUMMARY_SYSTEM}\n\nCustom vocabulary / context (prefer these spellings):\nKubernetes, Tauri"
            )
        );
    }

    #[test]
    fn limits_follow_the_reference() {
        assert_eq!(CHUNK_LIMIT, 100_000);
        assert_eq!(ASK_LIMIT, 300_000);
        assert_eq!(
            (SUMMARY_TOKENS, NOTES_TOKENS, ASK_TOKENS),
            (8000, 4000, 1500)
        );
        assert_eq!(SUMMARY_TIMEOUT, Duration::from_secs(300));
        assert_eq!(ASK_TIMEOUT, Duration::from_secs(120));
    }

    // ---- summarize: one call

    #[tokio::test]
    async fn summarize_sends_title_and_transcript() {
        for provider in PROVIDERS {
            let server =
                server_replying(provider, "  # Họp kế hoạch\n\n## Tóm tắt\nXong.\n\n ").await;
            let mut c = config(provider, &server.uri());
            c.vocabulary = "Dictate".to_string();
            let transcript = "[00:00] Speaker 1: Xin chào\n\n[00:05] Speaker 2: Chào anh";
            let mut events = Vec::new();

            let md = summarize(&c, "Họp tuần", false, transcript, |p| events.push(p))
                .await
                .unwrap();

            assert_eq!(md, "# Họp kế hoạch\n\n## Tóm tắt\nXong.", "{provider:?}");
            let sent = requests(&server, provider).await;
            assert_eq!(sent.len(), 1, "{provider:?}");
            assert_eq!(sent[0].system, summary_system("Dictate"));
            assert_eq!(
                sent[0].user,
                format!("Recording: Họp tuần\n\n<transcript>\n{transcript}\n</transcript>")
            );
            assert_limit(provider, &sent[0], 8000);
            assert!(events.is_empty(), "a short transcript reports no part");
        }
    }

    #[tokio::test]
    async fn summarize_untitled_asks_for_a_title() {
        for provider in PROVIDERS {
            let server = server_replying(provider, "# Tiêu đề\n\n## Tóm tắt\nXong.").await;
            let c = config(provider, &server.uri());

            summarize(&c, "Screen recording 10-11", true, "[00:00] hello", |_| {})
                .await
                .unwrap();

            let sent = requests(&server, provider).await;
            assert_eq!(
                sent[0].user,
                "Recording: untitled (Screen recording 10-11) — write a short descriptive title from the content\n\n<transcript>\n[00:00] hello\n</transcript>",
                "{provider:?}"
            );
        }
    }

    #[tokio::test]
    async fn summarize_counts_characters_not_bytes() {
        // Each "ế" is three bytes: 100,000 of them are 300,000 bytes and still go in one call.
        let server = server_replying(AiProvider::OpenAi, "# T\n\nok").await;
        let c = config(AiProvider::OpenAi, &server.uri());
        let mut events = Vec::new();

        summarize(&c, "T", false, &"ế".repeat(100_000), |p| events.push(p))
            .await
            .unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn summarize_above_the_limit_takes_notes_even_of_one_part() {
        // One character over, in a single paragraph: that paragraph stays whole, as one part.
        let server = server_replying(AiProvider::OpenAi, "- note").await;
        let c = config(AiProvider::OpenAi, &server.uri());
        let mut events = Vec::new();

        summarize(&c, "T", false, &"ế".repeat(100_001), |p| events.push(p))
            .await
            .unwrap();

        assert_eq!(
            events,
            [
                SummaryProgress::Part { index: 1, count: 1 },
                SummaryProgress::Merging
            ]
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    // ---- summarize: map, then merge

    /// Six paragraphs of 41,665 characters (Vietnamese letters, three bytes each): 250,000 characters with the blank
    /// lines, so 100,000 per part holds two of them.
    fn long_transcript() -> (Vec<String>, String) {
        let paragraphs: Vec<String> = (0..6)
            .map(|i| format!("[0{i}:00] Speaker 1: {}", "ế".repeat(41_665 - 19)))
            .collect();
        let transcript = paragraphs.join("\n\n");
        assert_eq!(transcript.chars().count(), 250_000);
        (paragraphs, transcript)
    }

    #[tokio::test]
    async fn summarize_long_transcript_maps_then_merges() {
        for provider in PROVIDERS {
            let (paragraphs, transcript) = long_transcript();
            // Each part's notes say which part they are; the merge gets the summary.
            let server = server_with(provider, move |sent| {
                match sent.user.strip_prefix("Part ") {
                    Some(rest) => reply(
                        provider,
                        &format!("notes {}", rest.split(' ').next().unwrap()),
                    ),
                    None => reply(provider, "# Họp tuần\n\n## Tóm tắt\nXong.\n"),
                }
            })
            .await;
            let c = config(provider, &server.uri());
            let mut events = Vec::new();

            let md = summarize(&c, "Weekly sync", false, &transcript, |p| events.push(p))
                .await
                .unwrap();

            assert_eq!(md, "# Họp tuần\n\n## Tóm tắt\nXong.", "{provider:?}");
            assert_eq!(
                events,
                [
                    SummaryProgress::Part { index: 1, count: 3 },
                    SummaryProgress::Part { index: 2, count: 3 },
                    SummaryProgress::Part { index: 3, count: 3 },
                    SummaryProgress::Merging
                ],
                "{provider:?}"
            );
            let sent = requests(&server, provider).await;
            assert_eq!(sent.len(), 4, "{provider:?}: 3 notes, then 1 merge");
            for (i, pair) in paragraphs.chunks(2).enumerate() {
                assert_eq!(sent[i].system, NOTES_SYSTEM);
                assert_eq!(
                    sent[i].user,
                    format!(
                        "Part {} of 3 of \"Weekly sync\":\n\n<transcript>\n{}\n</transcript>",
                        i + 1,
                        pair.join("\n\n")
                    ),
                    "{provider:?}, part {}",
                    i + 1
                );
                assert_limit(provider, &sent[i], 4000);
            }
            assert_eq!(sent[3].system, summary_system(""));
            assert_eq!(
                sent[3].user,
                "Recording: Weekly sync\nThe transcript was too long, so here are detailed notes of each consecutive part instead. Summarize the whole recording from them.\n\n<notes>\n### Phần 1\nnotes 1\n\n### Phần 2\nnotes 2\n\n### Phần 3\nnotes 3\n</notes>",
                "{provider:?}"
            );
            assert!(sent[3].user.contains("### Phần 1") && sent[3].user.contains("### Phần 3"));
            assert_limit(provider, &sent[3], 8000);
        }
    }

    #[tokio::test]
    async fn summarize_a_long_untitled_recording_names_the_placeholder_in_every_call() {
        let (_, transcript) = long_transcript();
        let server = server_replying(AiProvider::Anthropic, "- note").await;
        let c = config(AiProvider::Anthropic, &server.uri());
        let title = "untitled (Cuộc gọi) — write a short descriptive title from the content";

        summarize(&c, "Cuộc gọi", true, &transcript, |_| {})
            .await
            .unwrap();

        let sent = requests(&server, AiProvider::Anthropic).await;
        assert!(sent[0]
            .user
            .starts_with(&format!("Part 1 of 3 of \"{title}\":")));
        assert!(sent[3].user.starts_with(&format!("Recording: {title}\n")));
    }

    #[tokio::test]
    async fn a_failed_note_call_ends_the_run_before_the_merge() {
        let (_, transcript) = long_transcript();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let server = server_with(AiProvider::OpenAi, move |_| {
            match seen.fetch_add(1, Ordering::SeqCst) {
                1 => ResponseTemplate::new(500)
                    .set_body_json(json!({"error": {"message": "overloaded"}})),
                _ => reply(AiProvider::OpenAi, "- note"),
            }
        })
        .await;
        let c = config(AiProvider::OpenAi, &server.uri());
        let mut events = Vec::new();

        let err = summarize(&c, "T", false, &transcript, |p| events.push(p))
            .await
            .unwrap_err();

        assert_eq!(
            err,
            ApiError::Status {
                status: 500,
                message: "overloaded".into()
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2, "no third part, no merge");
        assert_eq!(
            events,
            [
                SummaryProgress::Part { index: 1, count: 3 },
                SummaryProgress::Part { index: 2, count: 3 }
            ]
        );
    }

    // ---- the parts

    #[test]
    fn summary_parts_keeps_oversized_paragraph() {
        let big = "b".repeat(25);
        let transcript = format!("aaaa\n\n{big}\n\ncccc");

        let parts = summary_parts(&transcript, 10);

        assert_eq!(parts, ["aaaa".to_string(), big, "cccc".to_string()]);
    }

    #[test]
    fn summary_parts_packs_paragraphs_up_to_the_limit() {
        // "aaaa\n\naaaa" is exactly 10 characters.
        let parts = summary_parts("aaaa\n\naaaa\n\naaaa\n\naaaa\n\naaaa", 10);
        assert_eq!(parts, ["aaaa\n\naaaa", "aaaa\n\naaaa", "aaaa"]);
        assert_eq!(summary_parts("short", 100), ["short"]);
    }

    #[test]
    fn summary_parts_count_the_blank_line_between_paragraphs() {
        // 4 + 2 + 4 = 10 does not fit in 9.
        assert_eq!(summary_parts("aaaa\n\nbbbb", 9), ["aaaa", "bbbb"]);
    }

    #[test]
    fn summary_parts_count_characters_and_cut_only_between_paragraphs() {
        let paragraph = "Xin chào, đây là một đoạn tiếng Việt có dấu.";
        let transcript = vec![paragraph; 20].join("\n\n");
        let limit = paragraph.chars().count() * 3 + 4;

        let parts = summary_parts(&transcript, limit);

        assert_eq!(parts.len(), 7);
        for part in &parts {
            assert!(part.chars().count() <= limit, "{}", part.chars().count());
            assert!(part.split("\n\n").all(|p| p == paragraph));
        }
        assert_eq!(parts.join("\n\n"), transcript, "nothing is lost or cut");
    }

    #[test]
    fn summary_parts_of_nothing_is_nothing() {
        assert!(summary_parts("", 10).is_empty());
    }

    // ---- the title

    #[test]
    fn title_from_summary_reads_first_heading() {
        assert_eq!(
            title_from_summary("# Họp kế hoạch quý 4\n\n## Tóm tắt\nXong."),
            Some("Họp kế hoạch quý 4".to_string())
        );
        // The first one, even when it is not on the first line, and trimmed (a CRLF reply too).
        assert_eq!(
            title_from_summary("\n\n#   Một tiêu đề  \r\n# Another\n"),
            Some("Một tiêu đề".to_string())
        );
        // A section heading is not the title.
        assert_eq!(title_from_summary("## Tóm tắt\nNội dung\n"), None);
        assert_eq!(title_from_summary("#hashtag\n#\n"), None);
        assert_eq!(title_from_summary("# \n## Tóm tắt"), None);
        assert_eq!(title_from_summary(""), None);
    }

    // ---- ask

    #[tokio::test]
    async fn ask_cuts_transcript_and_formats_question() {
        for provider in PROVIDERS {
            let server = server_replying(provider, "  Anh Nam phụ trách. [00:05]\n").await;
            let c = config(provider, &server.uri());
            // 300,001 letters of three bytes each: the cut counts characters and lands between them.
            let transcript = "ế".repeat(300_001);

            let answer = ask(&c, "Họp tuần", &transcript, "Ai phụ trách tài liệu?")
                .await
                .unwrap();

            assert_eq!(answer, "Anh Nam phụ trách. [00:05]", "{provider:?}");
            let sent = requests(&server, provider).await;
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].system, ASK_SYSTEM);
            assert_eq!(
                sent[0].user,
                format!(
                    "Recording: Họp tuần\n\n<transcript>\n{}\n</transcript>\n\nQuestion: Ai phụ trách tài liệu?",
                    "ế".repeat(300_000)
                ),
                "{provider:?}"
            );
            assert_limit(provider, &sent[0], 1500);
        }
    }

    #[tokio::test]
    async fn ask_sends_a_short_transcript_whole_and_without_the_vocabulary() {
        let server = server_replying(AiProvider::Anthropic, "No.").await;
        let mut c = config(AiProvider::Anthropic, &server.uri());
        c.vocabulary = "Dictate".to_string();

        ask(&c, "T", "[00:00] Speaker 1: hi", "Q?").await.unwrap();

        let sent = requests(&server, AiProvider::Anthropic).await;
        assert_eq!(sent[0].system, ASK_SYSTEM);
        assert_eq!(
            sent[0].user,
            "Recording: T\n\n<transcript>\n[00:00] Speaker 1: hi\n</transcript>\n\nQuestion: Q?"
        );
    }

    // ---- errors

    #[tokio::test]
    async fn errors_pass_through_and_never_carry_the_key() {
        for provider in PROVIDERS {
            let server = MockServer::start().await;
            Mock::given(any())
                .respond_with(ResponseTemplate::new(401).set_body_json(
                    json!({"error": {"message": format!("Incorrect API key: {KEY}")}}),
                ))
                .mount(&server)
                .await;
            let c = config(provider, &server.uri());

            let from_summary = summarize(&c, "T", false, "text", |_| {}).await.unwrap_err();
            let from_ask = ask(&c, "T", "text", "Q?").await.unwrap_err();

            for err in [from_summary, from_ask] {
                assert!(
                    matches!(err, ApiError::Status { status: 401, .. }),
                    "{err:?}"
                );
                assert!(!format!("{err} {err:?}").contains(KEY), "{err}");
            }
        }
    }

    #[tokio::test]
    async fn an_empty_reply_is_an_error() {
        let server = server_replying(AiProvider::OpenAi, "  \n").await;
        let c = config(AiProvider::OpenAi, &server.uri());

        assert_eq!(
            summarize(&c, "T", false, "text", |_| {}).await.unwrap_err(),
            ApiError::Empty
        );
        assert_eq!(
            ask(&c, "T", "text", "Q?").await.unwrap_err(),
            ApiError::Empty
        );
    }

    // ---- Send

    /// Compile-time: what `summarize` is allowed to hold across an await. The progress closure is `Send` but not
    /// `Sync` (it owns a `Cell`), as the pipeline's may be; a backend behind `#[async_trait]` needs the future to be
    /// `Send` all the same.
    #[test]
    fn summarize_future_is_send_when_the_progress_closure_is_not_sync() {
        fn assert_send<T: Send>(_: T) {}
        let c = config(AiProvider::Anthropic, "http://localhost");
        let seen = std::cell::Cell::new(0_usize);

        assert_send(summarize(&c, "title", false, "text", move |_| {
            seen.set(seen.get() + 1)
        }));
        assert_send(ask(&c, "title", "text", "question"));
    }
}
