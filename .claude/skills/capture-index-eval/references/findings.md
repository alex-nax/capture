# Findings — the governing model and the levers under investigation

The living record of this whole study is `~/.capture/evals/CROSS_SESSION_SYNTHESIS.md`. Read it for the
full write-up; this file is the load-bearing summary the skill teaches.

## The governing model (predict the regime BEFORE running)

Across four screen-recorded captures, the custom (cheap text-index) arm's fidelity followed:

> **custom-arm fidelity ≈ narration_richness × error_tolerance** — NOT font size.

| Session | Content | narration | error-tolerance | custom fidelity |
|---|---|---|---|---|
| `17fc41` marching-cubes | explainer, large slide font | rich, step-by-step | prose-tolerant | **0.82** |
| `432498` standup | Google Meet, screen-share | max (paraphrasable) | high; small-font board dragged it | ~0.78 |
| `5806dc` ue5 | C++/Blueprint IDE tutorial | rich | low — verbatim C++ | **0.45** |
| `88fe12` strudel | live-coding music | THIN (flavor only) | zero — one wrong glyph breaks it | **0.40** |

The first two cases suggested "font size drives fidelity." **Strudel refutes that**: it had the
*largest, crispest* font and the *worst* fidelity. Why — nothing in the text-index path reads pixels (the
local model already did), so **font size only ever helps the baseline (image) arm.** The custom arm is
driven by:

1. **Narration richness.** The clean transcript is the load-bearing signal. Rich step-by-step narration
   rescues a text-only reconstruction; thin/flavor commentary cannot, leaving the reconstruction stuck with
   the noisy local OCR alone.
2. **Error-tolerance of the target.** Prose algorithm descriptions tolerate fuzz (0.95); C++ tolerates some
   (0.45); Strudel mini-notation is zero-tolerance (0.40). Symbol density compounds it.

So when you scope a new eval: **rich-narration + prose target → expect the cheap path to win; thin-narration
or verbatim-symbol target → expect it to fail, and that's exactly where the escalation below pays off.**

## The constant: `auto` mis-classifies screen-recorded video
`prompt_preset="auto"` classifies *every* screen-recorded YouTube/Twitch capture as content_type `video`
(it sees player chrome) -> the default extractor yields titles and zero code/detail. Real Google Meet
captures classify correctly as `meeting`. The custom `leaf_prompt`+`leaf_schema` (content_type `custom`)
bypasses this, and its `surface` enum routes every frame correctly. The free-text combine still gives a
usable root summary — but on thin narration it *embellishes* (Strudel's root summary invented a "Twitch
channel" backstory). Treat the root summary as lossy on thin-narration content.

## The converged fix (the recommendation the skill produces)
A **hybrid escalation**, and it matters MOST exactly where narration is thin:
1. The local pass emits, per code block, a **`code_legible` / `verbatim_uncertain`** confidence flag (and
   for live-coding, detect a stable full-editor view).
2. Trigger a **targeted frontier image-fetch of only the ~2 best anchor frames** for verbatim
   transcription; let cheap consensus handle only the diffs between anchors.
3. **Never** let consensus synthesize unseen tokens or emit inferred globals (`setcps`, headers) into an
   authoritative `final_code` field; route inferred content to a separate field and downgrade confidence
   rather than dropping legible data.

This recovers near-baseline fidelity at ~80% frontier savings and is robust across content types, because
it spends frontier tokens precisely on the few frames whose verbatim content neither the local OCR nor the
narration can supply. This is the change `capture-index-tuning` folds into the built-in extractors.

## Token economics (per-extraction frontier savings)
Baseline (view all frames) vs custom (read text scaffold): **39–57% frontier savings**, the expensive
vision work moving to the free on-prem local model. Savings shrink when the per-leaf data is rich
(standup's tasks/data x 75 leaves made a bigger scaffold) — a real quality/savings tradeoff, not a bug.

## Levers still under investigation
- **Sampling density.** `run_dense_indexes.sh` re-runs each custom index at ~2x density (lower
  `--sample-rate`). Hypothesis: more frames -> cheap consensus stabilizes the skeleton. Reality so far: on
  thin narration it *launders* OCR noise into confident-wrong tokens. Denser sampling helps coverage, not
  verbatim fidelity. Each session's `dense/index.json` holds these runs.
- **Capture/vision resolution.** The vision client downscales each frame to `max_px`
  (`CAPTURE_INDEX_MAX_PX`, default 1024). Raising it tests whether higher resolution lets the 9B OCR tiny
  IDE fonts — i.e. whether the UE5 0.45 is a resolution wall or a model-capability wall.
- **Local model size.** A bigger local VLM than the 9B may move the verbatim line; the escalation exists
  precisely because a 9B can't be trusted on tiny/symbol-dense frames regardless of resolution.
