//! Provider and edit presets. Values mirror the Swift app (Settings.swift, Refiner.swift).

use serde::{Deserialize, Serialize};

/// Which chat API the AI features (edit, summaries) talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    Anthropic,
    /// OpenAI-compatible: OpenRouter (the default), OpenAI, Ollama, CLIProxyAPI...
    #[default]
    OpenAi,
}

impl AiProvider {
    pub fn default_base_url(self) -> &'static str {
        match self {
            AiProvider::Anthropic => "https://api.anthropic.com",
            AiProvider::OpenAi => "https://openrouter.ai/api/v1",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            AiProvider::Anthropic => "claude-haiku-5-5",
            AiProvider::OpenAi => "deepseek/deepseek-v4.1-flash",
        }
    }
}

/// Speech-to-text HTTP APIs that speak OpenAI's `/audio/transcriptions` protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscribePreset {
    OpenRouter,
    #[default]
    OpenAi,
    Groq,
    Custom,
}

impl TranscribePreset {
    /// OpenRouter has no `/audio/transcriptions`: audio goes to an audio-capable chat model instead.
    pub fn uses_chat(self) -> bool {
        self == TranscribePreset::OpenRouter
    }

    pub fn short_name(self) -> &'static str {
        match self {
            TranscribePreset::OpenRouter => "OpenRouter",
            TranscribePreset::OpenAi => "OpenAI",
            TranscribePreset::Groq => "Groq",
            TranscribePreset::Custom => "",
        }
    }

    pub fn base_url(self) -> &'static str {
        match self {
            TranscribePreset::OpenRouter => "https://openrouter.ai/api/v1",
            TranscribePreset::OpenAi => "https://api.openai.com/v1",
            TranscribePreset::Groq => "https://api.groq.com/openai/v1",
            TranscribePreset::Custom => "",
        }
    }

    pub fn models(self) -> &'static [&'static str] {
        match self {
            TranscribePreset::OpenRouter => &[
                "google/gemini-3.5-flash",
                "google/gemini-3.8-flash",
                "google/gemini-3.5-flash-lite",
            ],
            TranscribePreset::OpenAi => {
                &["whisper-1", "gpt-4o-transcribe", "gpt-4o-mini-transcribe"]
            }
            TranscribePreset::Groq => &["whisper-large-v3-turbo", "whisper-large-v3"],
            TranscribePreset::Custom => &[],
        }
    }

    pub fn key_url(self) -> Option<&'static str> {
        match self {
            TranscribePreset::OpenRouter => Some("https://openrouter.ai/keys"),
            TranscribePreset::OpenAi => Some("https://platform.openai.com/api-keys"),
            TranscribePreset::Groq => Some("https://console.groq.com/keys"),
            TranscribePreset::Custom => None,
        }
    }
}

/// One of the built-in ways to edit a dictated transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditPreset {
    pub id: &'static str,
    /// Sent to the model in English whatever the UI language is.
    pub instruction: &'static str,
}

pub static EDIT_PRESETS: [EditPreset; 8] = [
    EditPreset {
        id: "clean",
        instruction: "Fix speech-recognition mistakes, spelling, grammar and punctuation. Remove filler words (uh, um, ờ, à, ừm, thì là…), stutters and accidental repetitions. Keep the original meaning, tone and wording as much as possible.",
    },
    EditPreset {
        id: "formal",
        instruction: "Clean up the transcript and rewrite it in a clear, polished, professional tone.",
    },
    EditPreset {
        id: "concise",
        instruction: "Clean up the transcript and make it concise, keeping all key points.",
    },
    EditPreset {
        id: "bullets",
        instruction: "Clean up the transcript and restructure it as a well-organized bullet list.",
    },
    EditPreset {
        id: "prompt",
        instruction: "Turn this spoken request into a clear, well-structured prompt for an AI coding/chat assistant. Keep every requirement the speaker mentioned; do not invent new ones.",
    },
    EditPreset {
        id: "email",
        instruction: "Turn this into a well-written email (greeting, body, closing). No placeholders such as [Your Name]: leave out what you don't know.",
    },
    EditPreset {
        id: "en",
        instruction: "Clean up the transcript and translate it into natural, fluent English.",
    },
    EditPreset {
        id: "vi",
        instruction: "Clean up the transcript and translate it into natural, fluent Vietnamese.",
    },
];

/// The preset with this id; an unknown id falls back to `clean`.
pub fn find_edit_preset(id: &str) -> &'static EditPreset {
    EDIT_PRESETS
        .iter()
        .find(|p| p.id == id)
        .unwrap_or(&EDIT_PRESETS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_presets_match_swift() {
        assert_eq!(
            EDIT_PRESETS.map(|p| p.id),
            ["clean", "formal", "concise", "bullets", "prompt", "email", "en", "vi"]
        );
        assert!(EDIT_PRESETS[0]
            .instruction
            .starts_with("Fix speech-recognition mistakes"));
        assert_eq!(find_edit_preset("nope").id, "clean");
        assert_eq!(find_edit_preset("bullets").id, "bullets");
        assert_eq!(find_edit_preset("vi").id, "vi");
    }

    #[test]
    fn edit_preset_instructions_are_verbatim() {
        assert_eq!(
            find_edit_preset("clean").instruction,
            "Fix speech-recognition mistakes, spelling, grammar and punctuation. Remove filler words (uh, um, ờ, à, ừm, thì là…), stutters and accidental repetitions. Keep the original meaning, tone and wording as much as possible."
        );
        assert_eq!(
            find_edit_preset("formal").instruction,
            "Clean up the transcript and rewrite it in a clear, polished, professional tone."
        );
        assert_eq!(
            find_edit_preset("concise").instruction,
            "Clean up the transcript and make it concise, keeping all key points."
        );
        assert_eq!(
            find_edit_preset("bullets").instruction,
            "Clean up the transcript and restructure it as a well-organized bullet list."
        );
        assert_eq!(
            find_edit_preset("prompt").instruction,
            "Turn this spoken request into a clear, well-structured prompt for an AI coding/chat assistant. Keep every requirement the speaker mentioned; do not invent new ones."
        );
        assert_eq!(
            find_edit_preset("email").instruction,
            "Turn this into a well-written email (greeting, body, closing). No placeholders such as [Your Name]: leave out what you don't know."
        );
        assert_eq!(
            find_edit_preset("en").instruction,
            "Clean up the transcript and translate it into natural, fluent English."
        );
        assert_eq!(
            find_edit_preset("vi").instruction,
            "Clean up the transcript and translate it into natural, fluent Vietnamese."
        );
    }

    #[test]
    fn the_default_provider_is_openai_compatible_on_openrouter() {
        assert_eq!(AiProvider::default(), AiProvider::OpenAi);
        assert_eq!(
            AiProvider::OpenAi.default_base_url(),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            AiProvider::Anthropic.default_base_url(),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn each_provider_has_its_own_default_model() {
        assert_eq!(AiProvider::Anthropic.default_model(), "claude-haiku-5-5");
        assert_eq!(
            AiProvider::OpenAi.default_model(),
            "deepseek/deepseek-v4.1-flash"
        );
    }

    #[test]
    fn transcribe_presets_match_swift() {
        use TranscribePreset::*;
        assert_eq!(OpenRouter.base_url(), "https://openrouter.ai/api/v1");
        assert_eq!(OpenAi.base_url(), "https://api.openai.com/v1");
        assert_eq!(Groq.base_url(), "https://api.groq.com/openai/v1");
        assert_eq!(Custom.base_url(), "");

        assert_eq!(
            OpenRouter.models(),
            [
                "google/gemini-3.5-flash",
                "google/gemini-3.8-flash",
                "google/gemini-3.5-flash-lite"
            ]
        );
        assert_eq!(
            OpenAi.models(),
            ["whisper-1", "gpt-4o-transcribe", "gpt-4o-mini-transcribe"]
        );
        assert_eq!(
            Groq.models(),
            ["whisper-large-v3-turbo", "whisper-large-v3"]
        );
        assert!(Custom.models().is_empty());

        assert!(OpenRouter.uses_chat());
        assert!(!OpenAi.uses_chat() && !Groq.uses_chat() && !Custom.uses_chat());

        assert_eq!(OpenRouter.key_url(), Some("https://openrouter.ai/keys"));
        assert_eq!(
            OpenAi.key_url(),
            Some("https://platform.openai.com/api-keys")
        );
        assert_eq!(Groq.key_url(), Some("https://console.groq.com/keys"));
        assert_eq!(Custom.key_url(), None);

        assert_eq!(OpenRouter.short_name(), "OpenRouter");
        assert_eq!(OpenAi.short_name(), "OpenAI");
        assert_eq!(Groq.short_name(), "Groq");
        assert_eq!(Custom.short_name(), "");
        assert_eq!(TranscribePreset::default(), OpenAi);
    }

    #[test]
    fn presets_use_the_swift_raw_values_in_json() {
        assert_eq!(
            serde_json::to_string(&AiProvider::OpenAi).unwrap(),
            "\"openai\""
        );
        assert_eq!(
            serde_json::to_string(&TranscribePreset::OpenRouter).unwrap(),
            "\"openrouter\""
        );
        assert_eq!(
            serde_json::from_str::<TranscribePreset>("\"groq\"").unwrap(),
            TranscribePreset::Groq
        );
    }
}
