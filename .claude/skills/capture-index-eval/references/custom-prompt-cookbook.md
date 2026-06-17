# Custom-prompt cookbook — designing `leaf_prompt` + `leaf_schema` per content type

The custom arm lives or dies on this one artifact. A `leaf_prompt` + `leaf_schema` is what the
**local** vision model runs on each sampled frame; `drive_index.py --custom-json` ships them, the
content_type becomes `custom`, and the pair is also saved to `<session>/index_prompts.json` as corpus
for `capture-index-tuning` to fold back into the built-in extractors.

## The recipe (applies to every content type)
1. **Open with what the frame IS and who's reading.** State the app/medium ("a single frame from a
   live-coding music session in Strudel") and the contract: *"a reader who CANNOT see this image must be
   able to reproduce X."* That framing is what stops the 9B from summarizing detail away.
2. **`summary` (required string).** One sentence. It feeds the hierarchical tree combine — don't skip it.
3. **`surface` (enum) to ROUTE.** This is the workhorse that fixes `auto`'s mis-classification: instead of
   one wrong top-level `video` label, every frame self-sorts into `code` / `diagram` / `meeting_grid` /
   `strudel_editor` / etc. The reconstruction agent filters on `surface` to find the load-bearing frames.
4. **Content fields tuned to the target artifact.** Only what you'll actually reconstruct. More fields and
   vaguer prompts *degrade* a 9B's extraction — keep them concrete.
5. **Quarantine verbatim risk.** For anything error-intolerant (code, mini-notation, formulas) say
   explicitly *"transcribe VERBATIM… if illegible, leave empty, do NOT invent plausible code."* The 9B's
   instinct is to confabulate legible-looking tokens; this is the single biggest fidelity lever.

`leaf_schema` must be valid JSON Schema with `"required": ["summary", "surface"]` and small, concrete
fields. The local model runs it with `reasoning_effort:"none"` (the structured-output constraint — already
handled in the client), so a tight schema matters.

## Predict the regime BEFORE you write the prompt
`custom-arm fidelity ≈ narration_richness × error_tolerance` (see `findings.md`). Design accordingly:
- **Rich narration + prose-tolerant** (explainer, meeting) → lean on `transcript`; free-text fields are
  fine; expect high fidelity. Don't over-engineer verbatim capture.
- **Thin narration and/or zero-error-tolerant** (live-coding, tiny-font code) → the text path cannot read
  pixels for you. Add a `code_legible` / `verbatim_uncertain` flag (see findings.md → converged fix) and
  plan a targeted frontier image-fetch on the few load-bearing frames. Do NOT trust cheap cross-frame
  consensus to synthesize unseen tokens.

---

## The four worked examples (real, used in the cross-session study)

Copy these from `~/.capture/evals/<id>/custom_prompt.json` and adapt. Headline fidelity in parens is the
custom-arm `code_fidelity` (or the main content score) the judge gave — it tells you which regime you're in.

### 1. Explainer video — Marching Cubes (`17fc41`, fidelity 0.82, BEST)
Rich narration, large slide font, prose-tolerant algorithm. The winning shape: a `surface` enum
(`code/diagram/visualization/presenter/...`), VERBATIM `code` + `code_language`, a `math` field for
formulas, a `figure` field that describes diagrams *in terms of what they teach*, and a `takeaway`. The
`topic` field lets the reconstruction agent group frames by algorithm concept.
→ `~/.capture/evals/17fc41-marching-cubes/custom_prompt.json`

### 2. Recorded meeting — Google Meet standup (`432498`, tasks/decisions high; small-font board dragged it)
Max narration, paraphrasable minutes. `surface` enum covers the meeting surfaces
(`meeting_grid/shared_screen/slide/document/spreadsheet/task_board/code/...`). Capture
`active_speaker` + `participants` from Meet's tile name-labels and the highlighted/outlined active-speaker
tile, plus `shared_content`, `task_assignments` (`"<owner>: <task>"`), `data_points`, `decisions`. Note: a
small-font Linear board is the one part the text path can't read — that's the verbatim-risk surface here.
→ `~/.capture/evals/432498-standup/custom_prompt.json`

### 3. IDE coding tutorial — UE5 C++/Blueprint (`5806dc`, fidelity 0.45)
Small IDE font, rich narration, but a 9B can't OCR tiny code and confabulates — the cautionary case.
`surface` routes `cpp_ide/blueprint_graph/ue_editor_ui/viewport/...`; capture `file_or_asset`, VERBATIM
`code` (incl. UCLASS/UPROPERTY/GENERATED_BODY macros), `blueprint_nodes[]` in execution order,
`blueprint_connections`, and `ue_action`. Despite a strong prompt, fidelity was low — **this is the case
that needs the targeted frontier image-fetch on the few code frames.**
→ `~/.capture/evals/5806dc-ue5/custom_prompt.json`

### 4. Live-coding music — Strudel (`88fe12`, fidelity 0.40, WORST despite LARGEST font)
Thin narration + zero-error-tolerant symbol-dense mini-notation. The prompt is meticulous about VERBATIM
mini-notation (`"bd*2 hh sd"`, `<c a f e>`, method chains) and a `pattern_change` field for incremental
edits — yet fidelity was worst, because nothing in the text path reads pixels and there's no narration to
anchor the noisy OCR. Cheap consensus over 44 noisy reads *launders* OCR noise into confident-wrong tokens
(hallucination 0.30). **Do NOT rely on consensus here; route inferred globals (`setcps`, headers) to a
separate field, never into authoritative `final_code`.**
→ `~/.capture/evals/88fe12-strudel/custom_prompt.json`

## Anti-patterns (learned the hard way)
- A custom `leaf_prompt` **without** a `leaf_schema` is just a free-text caption — no structured routing.
  Always pair them for an eval.
- Don't let one authoritative `code`/`final_code` field hold inferred tokens; split inferred vs. verbatim.
- Don't pile on fields "just in case" — a 9B's extraction quality drops with schema size.
