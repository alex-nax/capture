//! Per-content-type index prompts + schemas (port of `core/indexer.py` lines 36–248).
//!
//! Pure DATA: the classify→extract prompt strings + structured-output json_schemas that the
//! indexer uses. In "auto" mode it first CLASSIFIES each frame (structured output, enum
//! `content_type`) and routes to the matching extraction prompt below; a fixed preset (e.g.
//! "meeting") skips classification. Each entry pairs a leaf EXTRACTION prompt with a
//! `combine_focus` line that steers the range summaries. These strings are eval-tuned —
//! they are copied verbatim from the Python and must NOT be paraphrased.
//!
//! The prompt DATA itself lives in `prompts.toml` (embedded at compile time via `include_str!`)
//! so the eval-tuned strings read cleanly instead of as clunky Rust string-concat literals.
//! This module parses that TOML once and rebuilds the public API on top of it. The `.trim()`s
//! below strip the leading/trailing newline a `'''…'''` literal block carries, so each loaded
//! value equals the single-line joined Python string byte-for-byte (see `prompts_are_verbatim`).

use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// The prompt data, embedded at compile time (single binary, no runtime file dependency).
const PROMPTS_TOML: &str = include_str!("prompts.toml");

/// A leaf extraction prompt + schema + combine-focus for one content type.
pub struct ContentPrompt {
    pub label: String,
    pub prompt: String,
    /// The leaf-extraction json_schema (a `summary` string + the type-specific fields).
    pub schema: Value,
    pub combine_focus: String,
}

/// One content-type entry as stored in `prompts.toml`. `fields` is an ordered list of
/// `[name, kind]` (kind = `"str"` or `"strs"`) from which the json_schema is rebuilt.
#[derive(Deserialize)]
struct Entry {
    key: String,
    label: String,
    prompt: String,
    combine_focus: String,
    fields: Vec<(String, String)>,
}

/// The whole parsed `prompts.toml`.
#[derive(Deserialize)]
struct Config {
    default_preset: String,
    code_types: Vec<String>,
    classify_prompt: String,
    content: Vec<Entry>,
}

/// Parse `prompts.toml` exactly once.
fn config() -> &'static Config {
    static C: OnceLock<Config> = OnceLock::new();
    C.get_or_init(|| toml::from_str(PROMPTS_TOML).expect("prompts.toml parses"))
}

/// Default preset: classify each frame and route to the matching extractor. Mirrors Python
/// `DEFAULT_PRESET`.
pub fn default_preset() -> &'static str {
    config().default_preset.trim()
}

/// The classify-stage prompt (verbatim from `core/indexer.py` `CLASSIFY_PROMPT`).
pub fn classify_prompt() -> &'static str {
    config().classify_prompt.trim()
}

/// `_STR` — a plain string field.
fn str_field() -> Value {
    json!({ "type": "string" })
}

/// `_STRS` — an array-of-strings field.
fn strs_field() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

/// Build a leaf-extraction json_schema from an entry's ordered `fields`: always a `summary`
/// string first, then each type-specific field. Mirrors Python `_schema`.
fn schema_for(fields: &[(String, String)]) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert("summary".to_string(), str_field());
    for (name, kind) in fields {
        let v = match kind.as_str() {
            "strs" => strs_field(),
            // "str" (and any unexpected kind) → plain string field.
            _ => str_field(),
        };
        properties.insert(name.clone(), v);
    }
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": ["summary"],
    })
}

/// Find the entry for a content type, falling back to "general" for "other"/unknown.
fn entry_for(content_type: &str) -> &'static Entry {
    let cfg = config();
    cfg.content
        .iter()
        .find(|e| e.key == content_type)
        .or_else(|| cfg.content.iter().find(|e| e.key == "general"))
        .expect("prompts.toml has a `general` entry")
}

/// The extraction prompt + schema + combine focus for a content type
/// ("other"/unknown → general). Mirrors Python `_content_prompts` over `CONTENT_PROMPTS`.
pub fn content_prompt(content_type: &str) -> ContentPrompt {
    let e = entry_for(content_type);
    ContentPrompt {
        label: e.label.trim().to_string(),
        prompt: e.prompt.trim().to_string(),
        schema: schema_for(&e.fields),
        combine_focus: e.combine_focus.trim().to_string(),
    }
}

/// The enum the classifier picks from: every `CONTENT_PROMPTS` key EXCEPT "general", in the
/// same order, then "other" appended. Mirrors Python `CONTENT_TYPES`. ORDER MATTERS.
pub fn content_types() -> Vec<String> {
    let mut out: Vec<String> = config()
        .content
        .iter()
        .map(|e| e.key.clone())
        .filter(|k| k != "general")
        .collect();
    out.push("other".to_string());
    out
}

/// Content types whose small, dense text benefits from a higher-resolution extraction pass (#49).
/// Mirrors Python `CODE_TYPES`.
pub fn code_types() -> Vec<String> {
    config().code_types.clone()
}

/// The classify-stage json_schema: `content_type` (enum of `content_types()`) + `app`.
/// Mirrors Python `_classify_schema`.
pub fn classify_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "content_type": { "type": "string", "enum": content_types() },
            "app": str_field(),
        },
        "required": ["content_type"],
    })
}

/// The combine prompt fusing two consecutive range summaries + the transcript slice.
/// Mirrors Python `_combine_prompt` exactly (transcript shows `(none)` when empty).
pub fn combine_prompt(left: &str, right: &str, transcript: &str, focus: &str) -> String {
    let transcript = if transcript.is_empty() { "(none)" } else { transcript };
    format!(
        "You are building a hierarchical summary of a screen-recording session. Below are \
         summaries of two consecutive time ranges, plus the transcript of what was said during \
         the combined range. Write a concise summary (2-4 sentences) of the COMBINED range, \
         capturing {focus}.\n\n\
         EARLIER RANGE:\n{left}\n\nLATER RANGE:\n{right}\n\n\
         TRANSCRIPT (may be empty):\n{transcript}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_schema_fields() {
        let cp = content_prompt("meeting");
        let props = cp.schema["properties"].as_object().unwrap();
        let mut keys: Vec<&String> = props.keys().collect();
        keys.sort();
        let mut expected = vec![
            "summary",
            "participants",
            "active_speaker",
            "shared_content",
            "task_assignments",
            "data_points",
            "decisions",
        ];
        expected.sort();
        let expected: Vec<String> = expected.into_iter().map(String::from).collect();
        let expected_refs: Vec<&String> = expected.iter().collect();
        assert_eq!(keys, expected_refs);
        assert_eq!(cp.schema["required"], json!(["summary"]));
    }

    #[test]
    fn other_and_unknown_fall_back_to_general() {
        assert_eq!(content_prompt("other").label, "General");
        assert_eq!(content_prompt("nonsense").label, "General");
        assert_eq!(content_prompt("general").label, "General");
    }

    #[test]
    fn content_types_order_no_general_other_last() {
        let types = content_types();
        assert_eq!(
            types,
            vec![
                "meeting", "lecture", "coding", "terminal", "browsing", "video", "gameplay",
                "document", "design", "other"
            ]
        );
        assert!(!types.iter().any(|t| t == "general"));
        assert_eq!(types.last().unwrap(), "other");
    }

    #[test]
    fn classify_schema_enum_matches_content_types() {
        let sch = classify_schema();
        let en = sch["properties"]["content_type"]["enum"].clone();
        assert_eq!(en, json!(content_types()));
    }

    #[test]
    fn combine_prompt_empty_and_nonempty_transcript() {
        let empty = combine_prompt("L", "R", "", "FOCUS_TEXT");
        assert!(empty.contains("(none)"));
        assert!(empty.contains("FOCUS_TEXT"));
        let full = combine_prompt("L", "R", "spoken words here", "FOCUS_TEXT");
        assert!(full.contains("spoken words here"));
        assert!(!full.contains("(none)"));
        assert!(full.contains("FOCUS_TEXT"));
    }

    #[test]
    fn code_types_set() {
        assert_eq!(code_types(), vec!["coding".to_string(), "terminal".to_string()]);
    }

    #[test]
    fn default_preset_is_auto() {
        assert_eq!(default_preset(), "auto");
    }

    /// Verbatim fidelity guard: known eval-tuned substrings must survive the TOML round-trip
    /// EXACTLY (catches accidental whitespace drift / bad joining when editing `prompts.toml`).
    #[test]
    fn prompts_are_verbatim() {
        // No leading/trailing whitespace leaked from the `'''…'''` literal blocks.
        let meeting = content_prompt("meeting");
        assert!(!meeting.prompt.starts_with(char::is_whitespace));
        assert!(!meeting.prompt.ends_with(char::is_whitespace));
        assert!(meeting.prompt.starts_with("This is a video meeting"));
        assert!(meeting.prompt.ends_with("Do not invent names, owners, or tasks."));
        assert!(meeting.prompt.contains(
            "Set `active_speaker` to the name of the person whose tile is highlighted/outlined/enlarged"
        ));

        let coding = content_prompt("coding");
        assert!(coding
            .prompt
            .contains("transcribed VERBATIM — preserve identifiers"));

        assert!(classify_prompt().contains(
            "a screen recording or a YouTube/Twitch video OF an IDE or code is `coding`"
        ));
        assert!(!classify_prompt().starts_with(char::is_whitespace));
        assert!(!classify_prompt().ends_with(char::is_whitespace));
        assert!(classify_prompt().starts_with("Classify what this screenshot PRIMARILY shows"));

        // combine_focus also round-trips clean.
        assert_eq!(
            content_prompt("general").combine_focus,
            "what happened, in order, and the salient topics, entities, and on-screen text"
        );
    }
}
