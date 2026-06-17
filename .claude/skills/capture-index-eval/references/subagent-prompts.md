# Subagent prompt templates

All the heavy lifting runs in **subagents** so the orchestrating context stays clean — the baseline agent
burns ~100–170k tokens viewing images, the reconstruction agent reads a ~20–35k-token scaffold, and the
judge re-views frames. Spawn each with the general-purpose agent and have it write its output file, then
return only a short summary.

Fill the `{{slots}}`. Keep them content-agnostic: the per-content tuning lives in the custom prompt
(`custom-prompt-cookbook.md`), not here. The crucial invariant is that **all three agents see the SAME leaf
frames** — `select_leaves` is deterministic for a given `--sample-rate`, so baseline (images), custom
(text), and dense (denser images) align frame-for-frame and the scores are comparable.

`{{target}}` examples: "the C++ source and Blueprint graphs being built", "the Marching Cubes algorithm and
its code", "the meeting minutes: attendees, decisions, and per-person task assignments", "the exact Strudel
patterns/code played".

---

## A. Baseline (gold) extraction — frontier VIEWS every leaf frame
This is the high-token reference everything is scored against. The agent must look at pixels.

```
You are extracting GROUND TRUTH from a screen+audio capture, for use as the reference an
eval is scored against. Be exhaustive and precise; this is the expensive arm.

Inputs:
- Leaf frame list: {{leaf_frames_txt}}  (one image path per line, in capture order)
- Transcript: {{transcript_path}}  (read it in full)

Do:
1. Read the transcript fully for narration context.
2. VIEW every leaf frame image with the Read tool (it renders images). Do not skip frames.
3. Extract {{target}}. Transcribe code/formulas/notation VERBATIM from the pixels. Note the
   frame index / timestamp for each piece so the judge can spot-check it.
4. Where the on-screen content is genuinely illegible, say so — do NOT guess.

Write:
- {{out_dir}}/extraction.md  — human-readable reconstruction of {{target}}
- {{out_dir}}/extraction.json — structured: e.g. {"code":[{label,language,code,frame}], "decisions":[...], ...}

Return only: how many frames you viewed, and a 3-line summary of what you found.
```

## B. Custom text-only reconstruction — NO images, distilled index only
Reconstructs the same target from ONLY the cheap text scaffold. This is the arm under test.

```
You are reconstructing {{target}} from a DISTILLED TEXT INDEX of a screen capture.
You may NOT view any images — work only from the text below. This tests how well a cheap
local-vision text index preserves the content.

Input:
- {{leaves_json}} — {"root_summary", "leaves":[{i,t,surface,<content fields>,transcript}]}
  produced by the local vision model. The `transcript` field is often your most reliable signal;
  the per-frame `code`/`figure`/etc. fields are noisier local OCR.

Do:
1. Read the whole scaffold. Use `surface` to find the load-bearing frames for {{target}}.
2. Reconstruct {{target}} as faithfully as the text allows.
3. CRITICAL on verbatim content (code, notation, formulas): reproduce only what the text actually
   contains. If a field is empty or looks like noisy OCR, mark it uncertain — do NOT "repair" it into
   plausible-looking tokens, and do NOT synthesize tokens from cross-frame consensus. Hallucinated
   confidence is the failure mode we are measuring.

Write:
- {{out_dir}}/extraction.md and {{out_dir}}/extraction.json (same shape as the baseline's, so they
  can be diffed field-for-field).

Return only: a 3-line summary and which parts you were unsure about.
```

## C. Judge — spot-check frames, score baseline vs custom
Adjudicates with real ground truth, not vibes. It must open a sample of frames itself.

```
You are the JUDGE for a capture-index eval. Score the CUSTOM (cheap text-only) reconstruction
against the BASELINE (frontier-viewed-all-frames) reconstruction, using real frames as ground truth.

Inputs:
- Baseline: {{baseline_dir}}/extraction.(md|json)
- Custom:   {{custom_dir}}/extraction.(md|json)
- Leaf frames: {{leaf_frames_txt}} — VIEW 10–12 frames yourself, chosen to cover the load-bearing
  content (the code/notation/board frames, not the title cards), to establish ground truth.

Score each arm 0.0–1.0 on (name them for {{target}}, e.g.):
- coverage      — fraction of the real content that was captured
- fidelity      — verbatim correctness of code/notation/formulas (the strict one)
- {{quality}}   — e.g. algorithm_quality / minutes_quality / musical_intent
- hallucination_rate — fraction of asserted content that is NOT on the frames you checked (lower better)

Write:
- {{judge_dir}}/scores.json:
  {"baseline": {...scores...}, "custom": {...scores...},
   "verdict": "<2-3 sentences: where custom held up and where it broke, with frame evidence>",
   "recommended_schema_change": "<concrete change for capture-index-tuning to fold in>"}
- {{judge_dir}}/verdict.md — the full frame-by-frame reasoning.

Be specific about WHICH frames you checked and what was wrong. Return only the scores + one-line verdict.
```

## Token accounting
Capture each subagent's `total_tokens` from its completion notification and write `cost_summary.json`:
```json
{"baseline_images":   {"frontier_tokens": 173385, "what": "agent viewed N leaf frames + transcript"},
 "custom_text_index": {"frontier_tokens": 73906, "local_model": "qwen3.5-9b 338s (free/on-prem)",
                       "what": "agent read ~19k-token text scaffold, no images"},
 "frontier_token_savings_pct": 57.4,
 "note": "savings are per-extraction; the local index builds (basic + custom) are free/on-prem"}
```
