---
name: capture-index-eval
description: Run the proven 5-stage evaluation that measures how well capture-mcp's CHEAP local-vision text index reconstructs a capture session's content versus an expensive frontier "view every frame" baseline — and at what token cost. USE THIS SKILL whenever the user wants to evaluate / benchmark / score a capture index, compare the cheap local index against a frontier baseline, measure index fidelity or token savings on a session, build a custom leaf_prompt+schema and judge it, or asks "how good is the index for session X", "is the local model good enough", "how much does the index lose vs looking at every frame", or "eval session 5806dc". It RESOLVES the session, builds basic + custom indexes, runs baseline / reconstruction / judge subagents, and writes scores + a verdict. It PRODUCES the corpus + verdicts that the capture-index-tuning skill then INGESTS to improve the built-in extractors — reach for it even if the user just names a session id and says "evaluate the index."
---

# capture-index-eval

Evaluate how faithfully capture-mcp's **cheap local-vision text index** (a 9B VLM + a custom index prompt)
reconstructs a session's content, versus an expensive **frontier "look at every frame"** baseline — and at
what token cost. Given a session id (e.g. `5806dc`), this runs a fixed 5-stage routine and outputs
`scores.json` + `verdict.md` plus the saved custom prompt corpus.

This is the **producer** in a pair: it runs the eval and emits verdicts + `index_prompts.json` corpus;
**`capture-index-tuning` is the consumer** that folds those wins back into the built-in
`CONTENT_PROMPTS` / `CLASSIFY_PROMPT` extractors. Run this skill to learn what to change; run that one to
change it. The index design itself (and the `reasoning_effort:"none"` constraint) is in
`docs/specs/indexing.md`.

## Predict the regime FIRST (this is the payoff — read before running)
Don't run blind. Across four captures the custom (cheap text-index) arm's fidelity followed
**fidelity ≈ narration_richness × error_tolerance — NOT font size** (full table + reasoning in
`references/findings.md`). So before stage 1, predict from the session's nature:
- **Rich narration + prose-tolerant target** (explainer, meeting minutes) → the cheap path will likely
  win; design a lean custom prompt that leans on the transcript.
- **Thin narration and/or zero-error-tolerant target** (live-coding mini-notation, tiny-font IDE code) →
  the cheap path will likely break on verbatim content, because nothing in the text path reads pixels.
  Design the custom prompt with a `code_legible`/`verbatim_uncertain` flag and expect to recommend a
  targeted frontier image-fetch on the few load-bearing frames.

Stating this prediction up front tells you what "good" looks like and stops you over-trusting a confident
text reconstruction (the worst failure mode — see Strudel in findings.md).

## Setup (load these — they bite otherwise)
- Run from the repo root with any `python3` — the eval scripts are pure-stdlib `/v1` clients (no venv,
  no deps); they proxy the daemon via `tools/capture_v1.py`.
- The **daemon must be running** — every build goes through `/v1`. The daemon is the native Rust
  `captured` (`cargo run -p capture-daemon`). `drive_index.py` discovers it via `~/.capture/daemon.json`.
- The daemon needs a vision **`endpoint`+`model`** passed: LM Studio (or similar) serving the local VLM,
  exported as `CAPTURE_INDEX_URL` (e.g. `http://HOST:1234/v1/chat/completions`) and a model like
  `qwen/qwen3.5-9b`. Structured output requires `reasoning_effort:"none"` — already handled in the client.
- Put all artifacts under a workspace dir, mirroring the existing study:
  `~/.capture/evals/<short>-<slug>/` with `basic/`, `custom/`, `baseline/`, `dense/`, `judge/`.

## The 5-stage routine
Do the heavy lifting in **subagents** (templates in `references/subagent-prompts.md`) to keep this context
clean. The key invariant: `select_leaves` is **deterministic** for a given `--sample-rate`, so the
baseline (images), custom (text), and dense (denser images) arms all see the **same** leaf frames and the
scores are directly comparable. Pick one `sample_rate` and reuse it across arms.

### 1. Inspect the session
Resolve the **FULL** daemon session_id — sessions are keyed by the full stamp like
`2026-06-17T10-10-36.393Z-5806dc`, **not** the short `5806dc` suffix. Query the daemon and match the
suffix:
```bash
python3 - <<'PY'
import sys; sys.path.insert(0, "tools")
from capture_v1 import Daemon
d = Daemon.discover()
s = d.find_session("5806dc")
print(s["session_id"]); print(s["dir"]); print("transcript_segments:", s.get("transcript_segments"))
PY
```
Save the full id + dir to `session_id.txt` / `session_dir.txt`. Count frames
(`ls <dir>/screenshots | wc -l`) and read the transcript head + length
(`<dir>/transcript.jsonl`). **The transcript's RICHNESS is the single biggest predictor** of how well the
cheap path will do — note it now and revisit your regime prediction.

### 2. Basic auto index
Build with the built-in classifier via the daemon:
```bash
python3 .claude/skills/capture-index-eval/scripts/drive_index.py \
  --session "$SID" --session-dir "$SDIR" --out basic/index.json \
  --endpoint "$CAPTURE_INDEX_URL" --model qwen/qwen3.5-9b --sample-rate 0.5 --preset auto
```
Record `root_summary` and the classified `content_type`. **Expected finding:** screen-recorded
YouTube/Twitch captures mis-classify as `video` (player chrome) and lose all code/detail; real Google Meet
captures classify correctly as `meeting`. This is the gap the custom prompt closes.

### 3. Baseline (gold) extraction
Build a leaf-frame manifest at this sample_rate, then spawn the **baseline subagent** (template A) to VIEW
every leaf frame (via the Read tool, which renders images) + read the transcript, and extract the target
content (code / algorithm / minutes / task assignments — content-dependent). This is the high-token
reference. Capture its `total_tokens`. Write `baseline/extraction.{md,json}`.

### 4. Custom index
Craft a content-tuned `leaf_prompt` + `leaf_schema` — see `references/custom-prompt-cookbook.md` and copy
the closest of the four real examples. The schema MUST include a `summary` string; add a `surface` enum to
route frames, plus content fields. Save the pair to `custom_prompt.json`, then build:
```bash
python3 .claude/skills/capture-index-eval/scripts/drive_index.py \
  --session "$SID" --session-dir "$SDIR" --out custom/index.json \
  --endpoint "$CAPTURE_INDEX_URL" --model qwen/qwen3.5-9b --sample-rate 0.5 \
  --custom-json custom_prompt.json
```
content_type becomes `custom`; the prompt+schema are also saved to `<session>/index_prompts.json` (the
corpus `capture-index-tuning` ingests). The cheap local model executes the schema.

### 5. Text-only reconstruction + judge
Distill the custom index to the lean reconstruction input:
```bash
python3 .claude/skills/capture-index-eval/scripts/distill_leaves.py \
  --index custom/index.json --out custom/leaves.json --drop-empty
```
Spawn the **reconstruction subagent** (template B) to rebuild the target from ONLY `custom/leaves.json`
(no images). Then spawn the **judge subagent** (template C): it spot-checks ~10–12 real frames for ground
truth and scores baseline vs custom on coverage / fidelity / quality / hallucination. Write
`judge/scores.json` + `judge/verdict.md`, and a `cost_summary.json` with both arms' `total_tokens` and the
frontier savings %.

## Output
- `judge/scores.json` — `{baseline:{...}, custom:{...}, verdict, recommended_schema_change}`.
- `judge/verdict.md` — frame-by-frame reasoning.
- `cost_summary.json` — frontier tokens per arm + savings %.
- `custom_prompt.json` + `<session>/index_prompts.json` — the corpus for `capture-index-tuning`.
Finish by stating the regime you predicted vs. what the judge found, and the one concrete
`recommended_schema_change` — that hand-off is the point of the eval.

## Optional: density / resolution sweeps
To test whether more frames or higher resolution help (they help coverage, not verbatim fidelity — see
findings.md), re-run the custom build at a lower `--sample-rate` into `dense/`
(`~/.capture/evals/run_dense_indexes.sh` is the worked example), and/or raise `CAPTURE_INDEX_MAX_PX` (the
vision client downscales each frame to this, default 1024) before building.

## Gotchas that save time
- **Full session_id, not the suffix** — `/v1/sessions/{id}` 404s on the short form. Always resolve via the
  daemon (stage 1).
- **Deterministic leaves** — keep `--sample-rate` identical across baseline/custom/dense so frames align;
  otherwise the scores aren't comparable.
- **Daemon needs endpoint+model** — without them the build can't reach the VLM. Export `CAPTURE_INDEX_URL`.
- **`reasoning_effort:"none"`** is required for structured output and is already set by the client; if a
  custom schema returns prose instead of JSON, suspect a different model/endpoint, not this.
- **`max_px` is the resolution knob** — `CAPTURE_INDEX_MAX_PX` (default 1024); raise to test tiny-font OCR.
- **Don't trust cheap cross-frame consensus** to synthesize unseen verbatim tokens — that's the Strudel
  failure (confident-but-wrong). Route inferred content to a separate field; recommend a targeted frontier
  image-fetch on the few load-bearing frames instead (the converged fix in findings.md).

## Bundled resources
- `scripts/drive_index.py` — builds any arm via the daemon `/v1` SSE; discovers the repo root robustly.
- `scripts/distill_leaves.py` — index.json → lean `leaves.json` for the text-only reconstruction agent.
- `references/custom-prompt-cookbook.md` — how to design `leaf_prompt`+`leaf_schema`, with 4 real examples.
- `references/subagent-prompts.md` — baseline / reconstruction / judge templates (content-agnostic slots).
- `references/findings.md` — the governing model, the converged fix, and the open levers.
- `~/.capture/evals/CROSS_SESSION_SYNTHESIS.md` — the living cross-session record.
