//! Per-content-type index prompts + schemas (port of `core/indexer.py` lines 36–248).
//!
//! Pure DATA: the classify→extract prompt strings + structured-output json_schemas that the
//! indexer uses. In "auto" mode it first CLASSIFIES each frame (structured output, enum
//! `content_type`) and routes to the matching extraction prompt below; a fixed preset (e.g.
//! "meeting") skips classification. Each entry pairs a leaf EXTRACTION prompt with a
//! `combine_focus` line that steers the range summaries. These strings are eval-tuned —
//! they are copied verbatim from the Python and must NOT be paraphrased.

use serde_json::{json, Value};

/// A leaf extraction prompt + schema + combine-focus for one content type.
pub struct ContentPrompt {
    pub label: &'static str,
    pub prompt: &'static str,
    /// The leaf-extraction json_schema (a `summary` string + the type-specific fields).
    pub schema: Value,
    pub combine_focus: &'static str,
}

/// Default preset: classify each frame and route to the matching extractor.
pub const DEFAULT_PRESET: &str = "auto";

/// The classify-stage prompt (verbatim from `core/indexer.py` `CLASSIFY_PROMPT`).
pub const CLASSIFY_PROMPT: &str =
    "Classify what this screenshot PRIMARILY shows: set `content_type` to the single best fit and `app` to the \
     application/site in focus. Classify by the CONTENT on screen, NOT the window around it — a screen recording \
     or a YouTube/Twitch video OF an IDE or code is `coding`, OF a slide-based tutorial/explainer is `lecture`, \
     OF a video call is `meeting`, OF a document/spreadsheet is `document`. Use `video` ONLY when the media itself \
     is the subject (a film, vlog, music or gameplay footage) with nothing to read or extract. \
     (meeting = a video call/conference; lecture = anything that teaches — a tutorial/explainer/screencast, even \
     inside a video player; coding = an IDE/code editor, even inside a video player; terminal = a console; video = \
     entertainment media with no code/slides/meeting/document to extract; browsing = a web page that is NOT a call; \
     document = docs/notes/PDF; design = Figma/Photoshop.)";

/// A leaf-extraction json_schema: always a `summary` string + the type-specific fields.
/// Mirrors Python `_schema`.
fn schema(props: Value) -> Value {
    let mut properties = json!({ "summary": { "type": "string" } });
    if let (Some(base), Some(extra)) = (properties.as_object_mut(), props.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": ["summary"],
    })
}

/// `_STR` — a plain string field.
fn str_field() -> Value {
    json!({ "type": "string" })
}

/// `_STRS` — an array-of-strings field.
fn strs_field() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

/// The extraction prompt + schema + combine focus for a content type
/// ("other"/unknown → general). Mirrors Python `_content_prompts` over `CONTENT_PROMPTS`.
pub fn content_prompt(content_type: &str) -> ContentPrompt {
    match content_type {
        "meeting" => ContentPrompt {
            label: "Meeting / call",
            prompt: "This is a video meeting (Google Meet / Zoom / Teams), often with a SHARED SCREEN (doc, \
                     slide, task board). Read the participant NAMES verbatim from the tile name-labels into \
                     `participants`. Set `active_speaker` to the name of the person whose tile is highlighted/\
                     outlined/enlarged or shows a speaking indicator (empty string if no cue is visible — do NOT \
                     guess). Put the shared screen/slide/board text verbatim in `shared_content`. Capture the WORK \
                     CONTENT: `task_assignments` = any '<owner>: <task>' assignments visible on a board/doc or shown \
                     in this frame; `data_points` = concrete data (ticket refs, dates/deadlines, project/initiative \
                     names, statuses, metrics); `decisions` = any decisions or action items evident. Leave a field \
                     empty if nothing supports it. `summary` = a 1-2 sentence description naming who is speaking. \
                     Do not invent names, owners, or tasks.",
            schema: schema(json!({
                "participants": strs_field(),
                "active_speaker": str_field(),
                "shared_content": str_field(),
                "task_assignments": strs_field(),
                "data_points": strs_field(),
                "decisions": strs_field(),
            })),
            combine_focus: "WHO said or did WHAT (attribute to the named speakers), task assignments (owner→task), \
                            decisions, action items, ticket refs/dates, and topics, in order",
        },
        "lecture" => ContentPrompt {
            label: "Lecture / tutorial",
            prompt: "This is an educational video / screencast / tutorial / explainer (often inside a video player). \
                     `summary` = 1-2 sentences on what is being taught. `topic` = the slide title or current topic. \
                     `key_points` = key terms, definitions, or takeaways (verbatim where shown). `code` = any source \
                     code on screen transcribed verbatim (else \"\"). `formulas` = any equations/formulas shown \
                     verbatim (else []).",
            schema: schema(json!({
                "topic": str_field(),
                "key_points": strs_field(),
                "code": str_field(),
                "formulas": strs_field(),
            })),
            combine_focus: "the concepts taught and how the material progresses, with key terms, code, formulas, and definitions",
        },
        "coding" => ContentPrompt {
            label: "Coding / IDE",
            prompt: "This is a code editor / IDE (possibly shown inside a video player). `summary` = 1-2 sentences on \
                     the task. `language` = the programming language. `file` = the open file name (read from the tab/\
                     title bar). `code` = the visible source code transcribed VERBATIM — preserve identifiers, \
                     signatures, and structure exactly; do not paraphrase or invent; leave \"\" if illegible. \
                     `symbols` = key function/class/identifier names or errors (verbatim).",
            schema: schema(json!({
                "language": str_field(),
                "file": str_field(),
                "code": str_field(),
                "symbols": strs_field(),
            })),
            combine_focus: "the coding task, the files and the actual code involved, and the changes or problems",
        },
        "terminal" => ContentPrompt {
            label: "Terminal",
            prompt: "This is a terminal / console. `summary` = 1-2 sentences on the task. `commands` = the commands \
                     run and any salient output/errors (verbatim).",
            schema: schema(json!({
                "commands": strs_field(),
            })),
            combine_focus: "the commands run and their outcomes",
        },
        "browsing" => ContentPrompt {
            label: "Web browsing",
            prompt: "This is a web browser (not a video call). `summary` = 1-2 sentences on the page/content. \
                     `site` = the site or page title (and URL if visible). `headings` = visible headings (verbatim).",
            schema: schema(json!({
                "site": str_field(),
                "headings": strs_field(),
            })),
            combine_focus: "the pages/sites visited and what was read or done",
        },
        "video" => ContentPrompt {
            label: "Video / media",
            prompt: "This is a video / media player (e.g. YouTube). `summary` = 1-2 sentences on what's on screen. \
                     `title` = the video title (verbatim). `channel` = the channel/uploader if shown.",
            schema: schema(json!({
                "title": str_field(),
                "channel": str_field(),
            })),
            combine_focus: "what the video showed, in order, and its topics",
        },
        "gameplay" => ContentPrompt {
            label: "Game",
            prompt: "This is a video game frame. `summary` = 1-2 sentences on what's happening. `game` = the game \
                     name if identifiable. `scene` = the scene/level/mode and any salient HUD text.",
            schema: schema(json!({
                "game": str_field(),
                "scene": str_field(),
            })),
            combine_focus: "the gameplay progression, objectives, and events",
        },
        "document" => ContentPrompt {
            label: "Document",
            prompt: "This is a document / text editor (Docs, Word, PDF, Notion). `summary` = 1-2 sentences on the \
                     content. `title` = the document title. `section` = the visible heading/section (verbatim).",
            schema: schema(json!({
                "title": str_field(),
                "section": str_field(),
            })),
            combine_focus: "the document's content and any edits",
        },
        "design" => ContentPrompt {
            label: "Design tool",
            prompt: "This is a design / creative tool (Figma, Sketch, Photoshop). `summary` = 1-2 sentences on what \
                     is being designed. `tool` = the app. `elements` = salient layers/elements/labels visible.",
            schema: schema(json!({
                "tool": str_field(),
                "elements": strs_field(),
            })),
            combine_focus: "the design work and its elements",
        },
        // "general", "other", unknown, and any type not in the set → general fallback.
        _ => ContentPrompt {
            label: "General",
            prompt: "Describe this screenshot. Put a 1-2 sentence factual description in `summary`; the app/site \
                     in `app`; and any salient on-screen text/names you can read verbatim in `on_screen_text`.",
            schema: schema(json!({
                "app": str_field(),
                "on_screen_text": strs_field(),
            })),
            combine_focus: "what happened, in order, and the salient topics, entities, and on-screen text",
        },
    }
}

/// The enum the classifier picks from: every `CONTENT_PROMPTS` key EXCEPT "general", in the
/// same order, then "other" appended. Mirrors Python `CONTENT_TYPES`. ORDER MATTERS.
pub fn content_types() -> Vec<&'static str> {
    vec![
        "meeting", "lecture", "coding", "terminal", "browsing", "video", "gameplay", "document",
        "design", "other",
    ]
}

/// Content types whose small, dense text benefits from a higher-resolution extraction pass (#49).
/// Mirrors Python `CODE_TYPES`.
pub fn code_types() -> &'static [&'static str] {
    &["coding", "terminal"]
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
        assert!(!types.contains(&"general"));
        assert_eq!(*types.last().unwrap(), "other");
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
        assert_eq!(code_types(), &["coding", "terminal"]);
    }
}
