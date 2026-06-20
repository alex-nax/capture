//! capture-index — the v3 multimodal index (port of `core/{indexer,live_index,vision_client}.py`).
//!
//! Pure logic + HTTP, no capture/permissions: it captions a session's screenshots with a remote
//! OpenAI-compatible vision LLM and summarizes the timeline as a binary merge-tree (transcript fused),
//! emits per-node artifacts + `AGENTS.md`. Validated against the 7 existing eval corpora as regression
//! fixtures. See `docs/specs/indexing.md`.
//!
//! Port pieces (#62), landing incrementally:
//! - `vision` — the OpenAI-compatible chat/vision client (reasoning_effort:"none" + json_schema).
//! - `prompts` — CONTENT_PROMPTS / CLASSIFY_PROMPT (the classify→type-extractor schemas; #56).
//! - `build` — build_index: classify → extract → binary combine-to-root; #49/#51 image handling.
//! - `live` — the incremental append→O(log n) merge-tree (#55).
//! - `agents` — AGENTS.md generation (#57).
//! - `providers` — index vision-LLM provider catalog + URL composition + model listing (#52/#53).

pub mod build;
pub mod live;
pub mod prompts;
pub mod providers;
pub mod vision;
