# Spec: Hierarchical multimodal index (remote vision LLM)

> **Status: BUILT + LIVE-VERIFIED (2026-06-17).** Implemented per the decisions below and
> verified end-to-end against the real **`qwen/qwen3.5-9b`** model on LM Studio at
> `192.168.31.217:1234`: a 12 s screen recording → 6 accurate frame captions → a coherent
> root summary (11-node tree, ~230 s). Also covered by a hermetic fake-server test + the
> daemon e2e. NOTE the model id carries a publisher prefix (`qwen/qwen3.5-9b`).

## Purpose
Build a **multi-resolution semantic index** of a finished session's visual capture by
feeding its screenshots to a remote multimodal LLM (an **LM Studio** server on the LAN
running a Qwen vision model) and summarizing the timeline as a **binary tree**: leaves
describe individual frames; internal nodes fuse their children into a coarser summary;
the root is a whole-session synopsis. Every node is indexed with its time range +
metadata, so you can read the session at any zoom level (whole session → a 2-minute
slice → one frame) and, later, search it.

This mirrors the divide-and-conquer ("binary search") shape you described:
**descend** by repeatedly taking the middle frame of a range (coarse-to-fine visual
sampling), then **conquer** by combining leaves back up to the root.

---

## Decisions (approved)

| # | Decision | Resolution |
|---|----------|------------|
| **D0** | Remote endpoint | Host **`192.168.31.217:1234`** (private LAN; the `192.186` in the request was a typo). Model `qwen3.5-9B`. LM Studio speaks the **OpenAI** `/v1/chat/completions` API (vision via base64 `image_url`), usually **no API key**. Configured via env + GUI (see Configuration). |
| **D1** | Frame selection — **tunable, not heuristic** | Leaf frames are chosen by a configurable **index sampling rate** `0 < rate ≤ 1` (default `0.5`): keep every `round(1/rate)`-th captured frame (rate `0.5` → every other frame; `1.0` → all; `0.25` → every 4th). A `max_leaves` cap is a backstop. **Also**: the **capture interval** itself becomes configurable (down to `0.5 s`) so the source cadence is tunable too. Perceptual-hash dedup is a deferred *optional* secondary filter — the rate is the primary knob so it can be tuned empirically. |
| **D2** | Where vision runs | **Vision only at leaves**; internal nodes are pure text-combine of their children (~N vision + N−1 text). A "dense" mode (midpoint vision on internal nodes too) is deferred behind a flag. |
| **D3** | Fuse the transcript — **and keep raw artifacts** | The time-aligned transcript slice is fed into each node's **combine** step (multimodal nodes), **but every node also stores its raw artifacts** — the raw `vision_caption` and the raw `transcript_slice` — alongside the fused `summary`. So the index stays inspectable and re-combinable without re-running vision. Skipped cleanly when a session has no audio. |
| **D4** | Node output shape | Start with **plain-text** summaries; a strict-JSON node mode (`{summary, topics[], entities[]}`) is deferred behind a flag once the 9B proves reliable. |
| **D5** | Run model | Daemon-orchestrated **background job** + SSE progress + **resumable checkpointing** (a flaky LAN link resumes, never restarts). Same job pattern as import/retranscribe. |

---

## Files (planned)
- `src/capture_mcp/core/indexer.py` — the tree builder (descend → conquer), checkpointing.
- `src/capture_mcp/core/vision_client.py` — stdlib-only OpenAI-compatible vision/chat client
  (mirrors `asr/openai_compat.py`): base64 image messages + text-combine messages, retries, `available()` preflight.
- `src/capture_mcp/core/frames.py` — frame listing (fs_stamp→offset) + `select_leaves` (sampling-rate decimation + cap).
- `src/capture_mcp/core/indexer.py` — the tree builder (vision leaves → combine up), transcript fusion, checkpointing, `load_index`.
- Daemon: `start_index` + `POST /v1/sessions/{id}/index`, `GET …/index`, `GET /v1/index/status`;
  `daemon/models.py` `IndexRequest`; `daemon/client.py` `index()/get_index()/index_status()`.
- MCP: `capture_index(session_id, endpoint?, model?, sample_rate?)`.
- GUI: an **Index** action on a finished session (Manage panel) + live progress + the root summary;
  the endpoint URL + reachability badge in Settings (`gui-settings.json` `index_url`). A browsable
  node tree is a follow-on.

## Public contract
- **`POST /v1/sessions/{id}/index {endpoint?, model?, sample_rate?, max_leaves?, fuse_transcript?}`**
  → `202 {session_id, started}`; runs in the background. No-op if already indexing (`started:false`).
  400 if no screenshots (`can_index=false`) / still live / bad params; **503 if the endpoint is unset
  or unreachable** (the gate — see below).
- **SSE** on `/v1/events`: `index` `{session_id, phase: "caption"|"combine", done, total, fraction}`
  → `index_done {session_id, node_count, leaf_count}` / `index_error {session_id, error}`.
- **`GET /v1/sessions/{id}/index`** → the built tree (or 404 if not indexed yet).
- **`GET /v1/index/status[?url=&model=]`** → `{available, configured, url, model}` — drives the GUI gate.
- **Capability flag** `can_index` (= `has_screenshots`) joins `session_capabilities`; combined with a
  reachable endpoint it gates the route + GUI. Indexing is **off unless a working LM Studio endpoint is
  provided** — both must hold.
- **MCP** `capture_index(session_id, endpoint=None, model=None, sample_rate=None, prompt_preset=None, leaf_prompt=None, leaf_schema=None, classify_prompt=None, max_px=None)` — daemon-first, like `capture_retranscribe`. `max_px` raises the base image resolution for a code-heavy build (#49).

## Per-frame extraction: classify → structured, type-specific (the "auto" preset)
A universal description prompt is wrong (meeting ≠ lecture ≠ gameplay), so each frame is handled in
**two structured stages** (the default `auto` preset; a fixed preset like `meeting` skips stage 1):
1. **Classify** — a structured call returns `{content_type ∈ enum, app}`. The classifier classifies by the
   **content shown, not the window around it** (the eval study found every screen-recorded YouTube/Twitch
   capture was mis-routed to `video`, losing all code/algorithm detail): a recording *of* an IDE is `coding`,
   *of* a tutorial/explainer is `lecture`, *of* a call is `meeting`; `video` is reserved for entertainment
   media with nothing to extract.
2. **Extract** — the content type routes to a type-specific **json_schema** (e.g. `meeting` →
   `{summary, participants[], active_speaker, shared_content, task_assignments[], data_points[], decisions[]}`;
   `lecture` → `{summary, topic, key_points[], code, formulas[]}`; `coding` →
   `{summary, language, file, code, symbols[]}` — `coding`/`terminal` are extracted at higher resolution so the
   verbatim `code` is legible, see #49). The
   structured fields are stored on the leaf node's `data`; `summary` feeds the tree. So the index carries real
   structured data (e.g. who is on the call and **who is speaking**, read from tile labels + the active-speaker
   highlight, plus the **task assignments / ticket refs / decisions** off a shared doc or board), and the root
   summary can be speaker-attributed. Presets/schemas live in `indexer.CONTENT_PROMPTS`; tune with
   `tools/index_prompt_eval.py`.

### Resolution-adaptive extraction (the OCR lever, #49)
The vision client downscales every frame to a longest-edge `max_px` (`CAPTURE_INDEX_MAX_IMAGE_PX`, default
**1024**) before upload. A field study (six captures, with a *controlled* resolution sweep on one high-res UE
capture — same frames, same prompt, only `max_px`) found that for **small-font / dense code** the binding
constraint is OCR accuracy, and **resolution is the lever**: 1024→2048 lifted UE C++ `code_fidelity`
**0.42→0.88** and file-name accuracy **0.30→0.95** (hallucination 0.55→0.10), while *sampling density* could
not move systematic misreads. So the extract stage is **resolution-adaptive**: leaves classified
`coding`/`terminal` (`indexer.CODE_TYPES`) are extracted at `CAPTURE_INDEX_CODE_MAX_PX` (default **2048**) via a
per-call `max_px` override on `structured_image`; classification and non-code types stay at the cheaper base.
`IndexRequest.max_px` (and the `capture_index` MCP arg) raise the base for a whole code-heavy build. Slides/
meeting/video gain nothing from the extra pixels, so the base stays 1024 there (~14% token / ~43% local-time
cost is spent only where it pays).

### OCR-reliability flags (#51, flag-now half)
After the tree is built, `_flag_code_reliability` post-processes code leaves (`coding`/`terminal`/`lecture`/
`custom`): it sets **`data.ocr_uncertain: true`** where the OCR'd `file`/`file_or_asset` is a *singleton amid
several differently-named code leaves* — the local model's confabulation signature (the study saw it invent a
different class every frame), validated to flag exactly the fake file names while leaving the real recurring
ones alone. It also attaches **`data.narration_values`** — numbers/identifiers spoken in that frame's
transcript slice — so a consumer can prefer the *dictated* token over a garbled OCR (the 1c0c0d finding). The
`AGENTS.md` lists the flagged frames as "re-read these first." The **automated re-read** of flagged frames
(higher-res / frontier re-extraction at build time) is the deferred second half — for now the flags steer the
*consuming* agent's targeted re-read.
### LM Studio structured output (the load-bearing constraint)
LM Studio enforces `response_format: json_schema` with **llama.cpp grammar-constrained sampling**, which
forbids a **reasoning model**'s `<think>` block → the model returns **empty `content`**. Neither `/no_think`
nor `chat_template_kwargs:{enable_thinking:false}` are honored by Qwen3.5 here. The fix (verified): send
**`reasoning_effort: "none"`** (the OpenAI-standard param LM Studio honors) on every structured/extract/combine
call — reasoning off, the grammar applies cleanly, AND the calls are ~8× faster (~1.5 s). Free-text caption
calls also pass a generous `max_tokens` (`CAPTURE_INDEX_MAX_TOKENS`, default 2048) so reasoning doesn't
exhaust the budget and empty the content; an empty reply is retried then degrades to `""` rather than aborting.

## Behavior — the algorithm
Input: the session's screenshots, time-ordered `[(stamp_0, path_0) … (stamp_{n-1}, path_{n-1})]`.

1. **Select (D1)**: decimate the captured frames by the **index sampling rate** — keep every
   `round(1/rate)`-th frame → the **leaf set** `L` (further capped at `max_leaves` as a backstop).
   Each retained frame keeps its real timestamp (and the span of frames it represents). The rate
   and the capture interval are both tunable, so leaf density can be dialed in empirically.
2. **Build the balanced tree**: recursively split `L` at its midpoint. A range of one frame = a
   **leaf**; otherwise an **internal** node with the left/right halves as children. (Balanced ⇒
   depth ≈ log₂|L|.)
3. **Descend (D2)**: vision-caption each node's representative (midpoint) frame — by default only
   the leaves; in "dense" mode internal midpoints too. A caption is a concise read of the screen
   (app, activity, salient text/entities).
4. **Conquer (ascend)**: at each internal node, `summary = combine(left.summary, right.summary,
   self.caption?, transcript_slice?)` (D3) — climbing to the **root summary**.
5. **Persist** the tree to `index.json` + a human-readable `index_summary.txt` (root).

Each node records (this is the "metadata preserved on the way back up") — **including raw
artifacts** (D3) so the index is inspectable and re-combinable without re-running vision:
`{id, depth, lo_idx, hi_idx, t_lo, t_hi, repr_frame, represents_n_frames,
vision_caption (raw), transcript_slice (raw, the in-range segments/text), summary (fused),
children:[…], parent, model, created_at}`.

**Resumability (D5)**: `index.json` is checkpointed as nodes complete; a re-run skips nodes whose
`(id, frame-content-hash, model, params)` already match — so a dropped LAN connection resumes.
Re-indexing a session backs up the prior `index.json` → `index.prev.json` (like re-transcribe).

## Invariants & constraints
- **No new heavy deps**: the vision client is **stdlib-only** (urllib + base64 + the existing image
  files); dedup uses a tiny perceptual hash we compute from the PNGs (no Pillow requirement — reuse
  the existing PNG reader / downscale path).
- **Off-device, opt-in, disabled by default**: indexing **sends screenshots to the configured LAN
  server**, so it is **off unless a working endpoint is configured** (URL set + reachable) and the
  user explicitly triggers it — never automatically. (Privacy note belongs in the GUI affordance.)
- **GUI ↔ MCP parity** (hard rule): the feature lands on the daemon, the Python client, the MCP tool,
  the Rust client, and the GUI together.
- Node timestamps are the screenshots' real `fs_stamp`s, so the index lines up with the playback
  scrubber and the transcript.

## Failure modes & handling
- **Server unreachable / offline PC**: preflight `GET /v1/models`; per-call retries with backoff;
  on give-up → `index_error` with a clear message, partial `index.json` left on disk for resume.
- **A frame the model rejects** (too large, decode error): downscale + JPEG-encode under
  `max_image_px`; on persistent failure caption = "(unreadable frame)" and continue (one bad frame
  never aborts the run).
- **No screenshots** (audio-only / pruned session): 400 `can_index=false`.
- **Model returns junk / non-JSON** (D4 structured mode): fall back to treating the raw text as the
  summary; never crash the tree.

## Outputs / artifacts
- `index.json` — the full node tree (+ `index_version`, `model`, `params`, `created_at`).
- `index.prev.json` — the previous index, kept on re-index.
- `index_summary.txt` — the root summary, human-readable.
- `index_prompts.json` — the model + prompts/schemas used (the corpus the tuning skill ingests).
- `AGENTS.md` (#57) — a per-capture **trust-calibration + usage guide** for any agent that later consumes
  the capture: the artifact map plus content-aware reliability rules (the local model's `data`/`code` is a
  hallucination-prone scaffold; the transcript is authoritative; re-read `repr_frame.path` for verbatim
  tokens). Written by `_write_agents_md` after every build; tailored by the leaf `content_type` mix.

## Configuration (env, mirrors `asr/openai_compat.py`; also set from the GUI)
**Indexing is DISABLED by default.** It is enabled only when a **working** LM Studio endpoint
is configured — `CAPTURE_INDEX_URL` set AND a startup/preflight health check (`GET /v1/models`)
succeeds. With no URL, or an unreachable server, the daemon reports `index_available:false`, the
`POST …/index` route returns `400`/`503`, and the GUI's Index control is hidden/disabled with a
"configure an LM Studio endpoint" hint. No frames are ever sent without a configured, reachable server.

Endpoint config (env, read by `vision_client.load`; `endpoint`/`model` are also overridable per
request, which is how the GUI carries its `index_url`):
- `CAPTURE_INDEX_URL`   e.g. `http://192.168.31.217:1234/v1/chat/completions` (**required to enable**)
- `CAPTURE_INDEX_MODEL` e.g. `qwen3.5-9b` (LM Studio model id; many LM Studio builds ignore it)
- `CAPTURE_INDEX_KEY`   bearer token (optional; LM Studio usually needs none)
- `CAPTURE_INDEX_TIMEOUT` per-call seconds (default 120)
- `CAPTURE_INDEX_MAX_IMAGE_PX` base longest-edge downscale before upload (default 1024; `sips` → JPEG)
- `CAPTURE_INDEX_CODE_MAX_PX` higher downscale for `coding`/`terminal` extraction (default 2048, #49)

Build params (per `IndexRequest`, with defaults — not env):
- `sample_rate` leaf sampling rate `0 < rate ≤ 1` (default 0.5; keep every `round(1/rate)`-th frame)
- `max_leaves` backstop cap on leaf count (default 512)
- `fuse_transcript` fold the time-aligned transcript into combines (default true)
- `max_px` override the base downscale for a whole build (default unset → env; code/terminal still auto-bump)

The **capture interval** (screenshot cadence, `screenshot_interval`, configurable down to `0.5 s`) is a
related capture-side knob exposed in the GUI Settings, so the source frame density is tunable too.

## Known limitations / open items
- Semantic **search** over the index (embeddings) is a follow-on, not in this slice.
- "Dense" mode (D2-i), structured-JSON nodes (D4-b), and perceptual-hash dedup are deferred (the
  sampling rate is the leaf knob for now).
- The transcript fed to a combine is capped (`TRANSCRIPT_FEED_CAP`) so the root combine stays bounded;
  the full slice is stored raw on the node regardless.
- A rich GUI **index browser** (click a node → see its frame + summary + jump the scrubber) is a
  follow-on; this slice ships the build + progress + the root summary.
- Cross-session indexing / a global timeline is out of scope.
- LM Studio model ids carry a publisher prefix (`qwen/qwen3.5-9b`); the GUI exposes both a URL and a
  **model** field (a box can have several models loaded — embeddings, chat, vision — so the model must
  be named, not guessed).

## Tests
- Hermetic (`tests/indexing_hermetic.py`, 18 checks): a **fake vision server** + synthetic session →
  asserts tree shape (2n−1 nodes, parent/child links), vision-only-at-leaves, transcript fusion at
  combines, raw artifacts kept, `8 vision + 7 text` call counts, and **checkpoint resume** (re-run
  recomputes only the missing node).
- Daemon e2e (`/tmp/index_e2e.py`-style): real source daemon + fake endpoint → `index/status`
  available, `can_index`, `POST …/index` 202, `GET …/index` complete (node_count 2n−1, root summary),
  audio-only session → 400.
- Live (manual, pending): point `CAPTURE_INDEX_URL`/the GUI at the LM Studio box, index a real
  imported video, watch the progress, read the root summary.
