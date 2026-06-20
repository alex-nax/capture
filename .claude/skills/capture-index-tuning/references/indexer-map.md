# index prompts map (what to edit when tuning)

`crates/index/src/prompts.toml` — the **data** you edit to change classification/extraction.
`crates/index/src/prompts.rs` — the code that parses + executes it (you rarely edit this).

## Structures (in `prompts.toml`)

- `[[content]]` entries — the per-content-type **extractor**. Each is
  `{ key, label, prompt, combine_focus, fields }`:
  - `key`: the content-type id (e.g. `meeting`, `coding`); `general` is the fallback for `other`/unknown.
  - `prompt`: the extraction instruction the local model follows.
  - `fields`: the type-specific fields, as `[name, type]` pairs where type is `"str"` or `"strs"`.
    `prompts.rs` builds the json_schema from these and **always prepends a mandatory `summary` string**,
    so you only list the extra fields (e.g. meeting has `participants` (`strs`), `active_speaker` (`str`),
    `shared_content` (`str`)).
  - `combine_focus`: the phrase that steers the range summaries for this type up the tree.
  - Add a new content type by adding a `[[content]]` entry.

- `classify_prompt` (top-level string) — the stage-1 **classifier** prompt. Sharpen the disambiguation
  hints here when types get confused. The classify enum + schema (`{content_type, app}`) are DERIVED in
  `prompts.rs` from the entries' `key`s (every key except `general`, then `other`); don't hand-edit the
  enum, add/remove a `[[content]]` entry instead.

- `default_preset` (`"auto"`) and `code_types` (e.g. `["coding", "terminal"]`) — top-level config.

## How `prompts.rs` uses it
- `classify_prompt()` / `classify_schema()` — stage-1 classifier prompt + derived schema.
- `content_prompt(content_type)` — the matching `[[content]]` entry's prompt/schema/combine_focus
  (`other`/unknown → `general`).
- `content_types()` — the classifier enum, derived from the entry keys (ORDER MATTERS).
- `schema_for(fields)` — builds the extraction json_schema, always prepending `summary`.

## Flow (build_index)
- `auto` preset: per leaf → classify (`classify_prompt` + `classify_schema`) → route to the matching
  `[[content]]` entry → extract (its `prompt` + derived schema) → node `data` = the structured dict,
  node `summary` = `data["summary"]`.
- A fixed preset (e.g. `meeting`) skips stage 1. A caller `leaf_prompt`+`leaf_schema` is a custom extractor
  (`content_type="custom"`); a caller `classify_prompt` overrides stage 1.
- Internal nodes: combine of child summaries (free-text, reasoning off).

## Invariants
- Every extractor schema keeps a `summary` string (the tree combines on it) — handled by `schema_for`.
- All model calls use `reasoning_effort: "none"` (LM Studio structured-output fix) — keep schemas tight.
- Prompt/combine_focus strings are eval-tuned; a verbatim-guard test in `prompts.rs` protects them.
- Each index writes `<session>/index_prompts.json` (the corpus this skill ingests).
