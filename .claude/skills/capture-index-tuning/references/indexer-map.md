# indexer.py map (what to edit when tuning)

`src/capture_mcp/core/indexer.py` — the only file you edit to change classification/extraction.

## Structures

- `_schema(props)` / `_STR` / `_STRS` — helpers. `_schema({...})` returns a json_schema object with a
  mandatory `summary` string plus your `props`. Use them so every extractor is shaped consistently.

- `CONTENT_PROMPTS: dict[str, {label, prompt, schema, combine_focus}]` — the per-content-type **extractor**.
  - `prompt`: the extraction instruction the local model follows.
  - `schema`: the json_schema it must return (always includes `summary`; add type fields, e.g. meeting has
    `participants` (`_STRS`), `active_speaker` (`_STR`), `shared_content` (`_STR`)).
  - `combine_focus`: the phrase that steers the range summaries for this type up the tree.
  - `"general"` is the fallback (used for `other`/unknown). Add a new type by adding a key here.

- `CONTENT_TYPES` — DERIVED from `CONTENT_PROMPTS` (`[k for k in CONTENT_PROMPTS if k != "general"] + ["other"]`).
  It's the classifier enum. Don't hand-edit; add/remove a `CONTENT_PROMPTS` entry instead.

- `CLASSIFY_PROMPT` + `_classify_schema()` — stage-1 **classifier** (returns `{content_type, app}`). Sharpen
  the disambiguation hints here when types get confused.

## Flow (build_index)
- `auto` preset: per leaf → `structured_image(CLASSIFY_PROMPT, _classify_schema())` → route to
  `CONTENT_PROMPTS[type]` → `structured_image(prompt, schema)` → node `data` = the structured dict,
  node `summary` = `data["summary"]`.
- A fixed preset (e.g. `meeting`) skips stage 1. A caller `leaf_prompt`+`leaf_schema` is a custom extractor
  (`content_type="custom"`); `classify_prompt` overrides stage 1.
- Internal nodes: `combine(...)` of child summaries (free-text, reasoning off).

## Invariants
- Every extractor `schema` keeps a `summary` string (the tree combines on it).
- All model calls use `reasoning_effort: "none"` (LM Studio structured-output fix) — keep schemas tight.
- Each index writes `<session>/index_prompts.json` (the corpus this skill ingests) — that recording lives in
  `_write_prompts_record`; if you add fields worth tracking for tuning, record them there too.
