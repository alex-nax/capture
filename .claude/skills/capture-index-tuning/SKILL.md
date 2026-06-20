---
name: capture-index-tuning
description: Improve capture-mcp's multimodal-index CLASSIFIER and per-type EXTRACTORS from real field data. USE THIS SKILL whenever you are tuning how the index understands screenshots — ingesting the saved index_prompts.json corpus from indexed sessions, folding frontier-model-crafted custom index prompts/schemas back into the built-in defaults, adding a new content type or extraction field, fixing mis-classification, or editing the content-type prompts / classifier prompt in crates/index/src/prompts.toml (data, executed by crates/index/src/prompts.rs). Trigger it even if the user just says "the meeting index missed names", "add a coding extractor", "the classifier mislabels X", or "update the index prompts from what we collected".
---

# capture-index-tuning

The multimodal index (`capture_index`, feature #44) understands each screenshot in two structured
stages run by a **local** vision model: **classify** the content type, then run that type's
**extractor** (a json_schema) — e.g. a meeting yields `{summary, participants[], active_speaker}`.
A **frontier model** (calling `capture_index` over MCP) can pass *custom* prompts/schemas tailored to
a session; the cheap local model executes them. Every run saves what it used to
`<session>/index_prompts.json`, building a corpus. **This skill turns that corpus into improvements to
the built-in classifier + extractors** so the defaults get better over time.

The whole index design (and the load-bearing LM Studio `reasoning_effort: "none"` constraint) is in
`docs/specs/indexing.md` — read it if you're unsure how a piece fits.

**Where the corpus comes from:** the `capture-index-eval` skill is the *producer* — it runs the 5-stage
eval on a session (basic vs custom vs frontier baseline + a frame-spot-checking judge) and emits the
`index_prompts.json` corpus plus a `recommended_schema_change` verdict. **This skill is the consumer**: it
folds those wins into the built-in defaults. If a custom prompt or schema change worked in an eval, that
verdict is your strongest signal for what to change here.

## When to reach for this
- A class of screenshot is mis-classified, or the extractor for a type misses fields the user wants.
- Frontier-model custom prompts have been used (saved in `index_prompts.json`) and worked well — fold them in.
- Adding a new content type (e.g. `spreadsheet`, `email`) or a new structured field.

## Workflow

### 1. Ingest the corpus
From the repo root, with any `python3` (the script is a pure-stdlib reader, no venv):
```bash
python3 .claude/skills/capture-index-tuning/scripts/ingest_index_prompts.py --json /tmp/index_corpus.json
```
It scans `~/.capture/runs/*/index_prompts.json` + `index.json` and reports: the content-type
distribution (flagging unknown/over-broad types), every **custom** `leaf_prompt`/`leaf_schema`/
`classify_prompt` used (with frequency), schema **fields seen in extractions but missing from the
default** for that type, and sample extractions per type so you can judge quality.

### 2. Decide the changes (read the report, not your priors)
- **Custom prompts that recur and read well** → fold their wording/fields into the matching default
  `[[content]]` entry. Don't blindly paste — generalize: keep what makes them better (a clearer field, a
  sharper instruction), drop session-specific bits.
- **New fields** the report surfaces → add to that type's `fields` (+ a line in its `prompt`).
- **Unknown content types** (⚠ in the report) → add the enum via a new `[[content]]` entry, or
  merge an over-broad/duplicate type. The classifier enum (`content_types()`) derives from the entries
  automatically.
- **Mis-classification** → sharpen `classify_prompt`'s disambiguation hints (e.g. "meeting = a video
  call; video = a media player").

### 3. Edit `crates/index/src/prompts.toml`
The built-in defaults are **data** in `crates/index/src/prompts.toml`, executed by
`crates/index/src/prompts.rs` — edit the TOML, not the Rust (see also `references/indexer-map.md`):
- Each `[[content]]` entry = `{ key, label, prompt, combine_focus, fields }` — the per-type extractor.
  The extraction schema is built from `fields` and **always prepends a `summary` string** (it feeds the
  tree), so you only list the type-specific fields. Field types are `"str"` / `"strs"`.
- `classify_prompt` (a top-level string) is the classifier; the classify enum + schema derive from the
  entries' `key`s (every key except `general`, then `other`) in `prompts.rs`.
- Prompts/combine_focus are single-line strings written as multi-line literals that `prompts.rs` trims;
  keep the existing style. A verbatim-guard test in `prompts.rs` protects the eval-tuned strings.

### 4. Verify before claiming done
- `cargo test -p capture-index` → green (the schema/derivation + verbatim-guard tests still hold).
- Empirically, against a real session + endpoint:
  `python3 tools/index_prompt_eval.py --session <dir> --preset <type> --n 8` (or `--compare`)
  — confirm the new extractor pulls out what you intended. This is the real test; the unit tests only
  guard the plumbing.

### 5. Spec + done
Update `docs/specs/indexing.md` if you added a type/field or changed behavior (the project's
same-change spec rule). Note what you changed and why.

## Notes that save time
- Schemas are executed by the LOCAL model with `reasoning_effort: "none"` (the structured-output fix);
  keep them small and concrete — many fields or vague prompts degrade extraction.
- `summary` is mandatory in every extractor schema; the hierarchical combine works on summaries.
- A custom `leaf_prompt` WITHOUT a `leaf_schema` is a free-text caption (no structured fields) — those
  show in the corpus too, but the structured customs are the richer signal.
- Don't overfit to one session. Prefer a change the report shows across several sessions.
