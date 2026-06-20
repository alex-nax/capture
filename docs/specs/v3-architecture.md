# Spec: v3 — single-language (Rust) architecture

_Status: **PLANNED** (the `v3` branch). Source of truth once code lands = the code; update this spec in the
same change. This doc is the migration plan, not yet the implementation._

## Purpose / why v3
v2 is a **three-language zoo**: ~7,650 lines of **Python** (the `captured` daemon + `/v1` HTTP API, the MCP
server, ASR, the multimodal indexer, session/registry/platform), ~551 lines of **Swift** (`audiocap` —
ScreenCaptureKit audio + AVFoundation mic/extract), a Python **Windows** audio helper, and ~4,840 lines of
**Rust** (the GPUI GUI). Bundling that means PyInstaller (frozen Python + a hand-maintained hidden-imports
list) **plus** an embedded Python runtime **plus** a separately-compiled+signed Swift binary **plus** the
Rust app — and on **Windows** that cross-language packaging is the dominant source of install/bundle pain.

**v3 collapses everything into one Rust cargo workspace** (one toolchain, one build, native single-binary
distribution), *unless* a platform forces native code — and where it does (ScreenCaptureKit, WASAPI), Rust
reaches it via FFI crates (`objc2`, `windows`), so the **build** stays pure-cargo even when it links OS
frameworks. The GPUI GUI is already Rust and is reused unchanged.

## Decision (approved)
- **Rust, one workspace.** Port the Python + Swift into Rust crates; keep the GPUI GUI.
- **Incremental, contract-anchored** migration (not big-bang): the **`/v1` API and the on-disk output
  formats stay byte-identical**, so the existing GUI and all current captures/indexes keep working while we
  port crate-by-crate. The branch is always in a shippable state.
- **ASR stays a pluggable trait**, not a single engine (see below). Default `whisper-rs`.

## Target architecture — the cargo workspace
```
capture/                      (v3: a cargo workspace)
└── crates/                   folder names are prefix-free; package names keep `capture-` (so
    │                         `use capture_core` is unchanged and `core/` doesn't shadow std::core)
    ├── core/                 (pkg capture-core) session lifecycle, registry, frames, events, presets,
    │                         providers, config; the /v1 + on-disk CONTRACT TYPES (serde) — source of truth
    ├── platform/             (pkg capture-platform) trait WindowFinder/ScreenGrabber/AudioSource + backends:
    │     ├─ macos            ScreenCaptureKit (audio+shots) via objc2/`screencapturekit`, AVFoundation
    │     │                   mic + file import — replaces audiocap.swift
    │     └─ windows          windows-rs: Graphics.Capture + WASAPI loopback — replaces audiocap_win.py
    ├── asr/                  (pkg capture-asr) trait AsrBackend + runtime manager + GGML catalog
    ├── asr-whisper/          (pkg capture-asr-whisper) whisper.cpp engine as a dlopen'd cdylib (C ABI)
    ├── index/               (pkg capture-index) vision_client + build_index + live_index (merge-tree) +
    │                         AGENTS.md; reqwest + serde_json + `image` (replaces sips)
    ├── engine/               (pkg capture-engine) capture session orchestration: lifecycle, screenshots
    │                         timer, audio→ASR pump, session.json, launch mode, import, events.jsonl
    ├── daemon/               (pkg capture-daemon) the /v1 HTTP server (axum) + SSE — binary `captured`
    ├── mcp/                  (pkg capture-mcp) MCP stdio server — binary `capture-mcp`
    └── gui/                  (pkg capture-gui) the GPUI app (moved in; points at the Rust daemon)
```
(packaging: cargo-dist / cargo-bundle + platform installers)

## The contract firewall (what stays IDENTICAL — the reason incremental works)
- **`/v1` HTTP API** (routes, request/response JSON shapes, SSE event types). In v3 the **Rust serde types in
  `capture-core` become the contract source of truth**, replacing the v2 pydantic `v1_schema` golden. The
  GUI's daemon client speaks the same `/v1`, so it runs against the v2 *or* v3 daemon unchanged.
- **MCP tool surface** (the 12 tools, their args).
- **On-disk session layout**: `session.json`, `transcript.jsonl`/`.txt`, `screenshots/<iso>.{png,jpg}`,
  `audio.s16le`/`mic.s16le`, `index.json` + `index.prev.json` + `index_summary.txt` + `index_prompts.json` +
  `AGENTS.md`, `events.jsonl`. Existing captures + indexes remain readable; the eval/tuning skills + the 7
  eval corpora keep working as regression fixtures.
- **`~/.capture/daemon.json`** discovery + bearer-token auth.

## Per-component port plan + risk
| v2 (today) | v3 crate / approach | Risk | Notes |
|---|---|---|---|
| GPUI GUI (Rust) | `capture-gui` | none | reused as-is; only its build moves into the workspace |
| daemon `/v1` + SSE (Py) | `capture-daemon` (axum/hyper) | low | mechanical; SSE is a streaming response |
| MCP server (FastMCP) | `capture-mcp` (`rmcp` / JSON-RPC stdio) | low | small protocol; tools wrap `capture-core` |
| indexer/live-index/vision (Py) | `capture-index` (reqwest+serde+`image`) | low | merge-tree, classify→extract, AGENTS.md, #49/#51 all port; `image` replaces `sips` downscale/encode |
| session/registry/frames/events/presets/providers (Py) | `capture-core` | low | also defines the contract types |
| ASR ×3 (mlx, faster-whisper, Nemotron) | `capture-asr` (trait + `whisper-rs` + HTTP) | **medium** | a *simplification* — see ASR below |
| `audiocap.swift` (ScreenCaptureKit/AVFoundation) | `capture-platform` macOS (objc2) | **high** | the main unknown — spike first |
| `audiocap_win.py` + Win32 grabber (Py) | `capture-platform` windows (`windows` crate) | medium | mature crate; single `.exe`, no Python |
| selenium (browser-capture flow) | **dropped** — not ported | n/a | selenium was never actually used; if browser-driving is ever wanted, do it via the **chrome-devtools MCP**, not a bundled in-process driver |
| numpy PCM buffers | `Vec<i16>`/`Vec<f32>` (+ `ndarray` if needed) | low | chunking + silence-gate port directly |

## ASR — a pluggable trait, not one engine (per the runtime-flexibility requirement)
v2 already abstracts ASR (`asr/base.py` + `asr/manager.py` + 3 backends). v3 keeps a Rust **`AsrBackend`
trait + manager**, selectable per platform/config — the same model as the Windows whisper-vs-Nemotron choice
(#23), extended to macOS:
| platform | runtimes |
|---|---|
| macOS | **whisper-rs/Metal** (default) · **whisper-rs/CoreML** · *MLX (optional, future — via `mlx-c` FFI or a sidecar; only if its Apple-silicon edge beats whisper.cpp on the bench)* |
| Windows | **whisper-rs/CUDA** (default) · **Nemotron/Riva** (HTTP, #23) |
| any | **OpenAI-compatible** remote (HTTP) |
`whisper-rs` (whisper.cpp) gives **two** macOS acceleration paths (Metal + CoreML) with no MLX needed; MLX
stays a clean future backend behind the trait. **MLX note:** MLX has C++/Swift bindings + a `mlx-c` C API
(FFI-able from Rust) but no turnkey Rust binding, and `mlx-whisper` (weights + mel/tokenizer/decode pipeline)
is Python — so an MLX backend is reimplementation/sidecar work, deferred until justified.

**Unify the two ASR systems — one source of truth (#77).** v2 ended up with the runtime *registry*
(`core/asr/runtimes.py`, #58 — faster-cpu/faster-cuda/remote, `remote` active by default) **disconnected**
from the *backend* that actually serves+runs models (the mlx model manager / `GET /v1/asr/models`). They can
disagree — the registry reporting `remote` active while mlx is the engine actually transcribing — which made
the GUI's runtime flag and the model list contradict each other (the Voice section had to stop gating the
model list on the active runtime to avoid silently hiding local models). v3's `AsrBackend` trait + manager
**is** the unification: the **active runtime owns its model catalog + active model**, exposed as one coherent
`/v1/asr` surface (runtime ⇒ its compatible models ⇒ the active model). Fold mlx in as a *first-class
runtime* (not an out-of-band default), make the model catalog runtime-derived, and keep `active runtime`
always equal to the engine actually in use — so a client can gate the model list on the runtime without ever
hiding the real local models. Tracked as **#77** (depends on #58 + #64).

## Bundling target (the payoff)
- One `cargo build` → daemon + MCP + GUI. No PyInstaller, no embedded Python runtime, no Swift compile/sign
  step, no hidden-imports list.
- **macOS**: one signed Rust binary + the app bundle; ScreenCaptureKit/AVFoundation entitlements + notarize
  (reuse the existing inline-notary flow). Helper binary disappears (capture is in-process via FFI).
- **Windows**: a single signed `.exe` + installer (`cargo-dist`/WiX/NSIS) — **no Python, no DLL zoo** (the
  whole motivation).
- whisper.cpp models stay downloadable `.gguf`/CoreML data (same as today's Whisper model manager), not a
  bundled runtime.

## Migration order (incremental, always-shippable)
1. **Workspace skeleton** + `capture-core` contract types (serde) → regenerate the GUI's types from these;
   keep the v1 golden green. **DONE (#61):** root `Cargo.toml` workspace (members `gui` + `crates/core`,
   shared `./target`); `capture-core::v1` holds all 14 `/v1` types — the proven response structs moved out of
   `gui/src/daemon.rs` (now `Serialize+Deserialize`, lenient) + the 4 request types ported from `models.py`
   (`deny_unknown_fields`, serde defaults); the GUI re-exports them (`pub use capture_core::v1::*;`), its HTTP
   client stays in `daemon.rs`; round-trip tests in `capture-core`. The Python `models.py`/`v1_schema` golden
   stays the v2 daemon's contract until cutover (#67).
2. **`capture-index`** — pure logic + HTTP; **fully testable against the 7 existing eval corpora** (no
   capture/permissions needed). High-value, low-risk first port; proves the toolchain. **DONE (#62):** the
   whole indexer ported — `vision` (OpenAI-compatible client, `reasoning_effort:"none"`/json_schema/retry,
   `image` downscale), `prompts` (the classify/extract prompts in an editable embedded `prompts.toml`),
   `build` (the classify→extract→combine merge-tree + checkpoint-resume + `#49` code-res + `#51` confab
   flagging + content-aware `AGENTS.md`), `live` (the `#55` binary-counter incremental tree); the
   frames/time/transcript foundation lives in `capture-core`. Validated with hermetic tests + a **golden-replay**
   harness (`examples/golden_replay.rs`): replaying 4 corpora's leaf data through the Rust `build_index`
   reproduces the current Python's deterministic output (tree, `content_type`, `#51` flags) byte-for-byte —
   the only diffs were *stale* corpus fixtures predating the current `#51` logic (confirmed by recomputing
   with today's Python, which matches the Rust). The live end-to-end against LM Studio is blocked only by the
   macOS Local-Network gate on shell binaries — it runs once the granted v3 daemon (#63) drives it.
3. **`capture-daemon` + `capture-mcp`** — serve the same `/v1` + MCP; GUI flips to the Rust daemon.
   **IN PROGRESS (#63):**
   - *Read-layer (piece 1):* `capture-core::sessions` ports the read half of `core/registry.py` +
     `session.session_capabilities` — `recover_session` (one dir → a `v1::Session`, re-deriving capability
     flags from disk, rewriting a gone-live state to `interrupted`), `list_sessions`/`find_session` (fold the
     `sessions.jsonl` index + scan `capture-*`, oldest-first, bounded 100). `v1::Session` was expanded from the
     GUI's field subset to the **full `SessionSummary`** (pid/started/stopped/errors/audio_mode/mic_*/presets/
     notes) with serde defaults, so the daemon serializes the complete wire shape while the GUI's lenient read
     is unaffected.
   - *The daemon (piece 2):* `crates/daemon` — an **axum** `/v1` server (binary `captured`). Bearer
     auth on every route except `/v1/health`; `~/.capture/daemon.json` discovery (0600, single-instance guard
     via a `/v1/health` preflight); ephemeral `127.0.0.1` port; SSE `/v1/events` over a `broadcast` bus;
     graceful shutdown (SIGINT/SIGTERM/`admin/shutdown`). **Ready routes** are served now: `health`,
     `GET /v1/sessions[/{id}]` (read-layer), `GET …/transcript` (raw `transcript.jsonl` passthrough),
     `GET …/index` (read `index.json`), `GET /v1/index/status`, and `POST …/index` — the **real index build**
     via `capture-index` on a `spawn_blocking` task, streaming `index`/`index_done`/`index_error` over SSE
     (same payloads as `start_index`). The ten **`asr/*` routes**, the **platform read routes** (windows /
     permissions / audio mics), and the **session routes** (capture start [attach + launch] / stop, mic,
     delete, prune, retranscribe) are all live now (#64/#65 — see below), as are `import` (#43) and the
     index provider catalog/model listing `index/{models,providers}` (#67, `capture-index::providers`). The
     **only** remaining `501` is `/v1/schema` (a v2 pydantic-introspection artifact with no Rust equivalent —
     the `capture-core` serde types are the contract). Validated against
     the live v2 daemon: identical
     `/v1/sessions` (42≡42, **zero differing shared fields**), correct 401/404/501/503/400 gating, SSE-ready.
   - *The MCP (piece 3):* `crates/mcp` (binary `capture-mcp`) — a port of `capture_mcp/server.py`'s
     **12 tools** as a synchronous line-delimited JSON-RPC stdio server (`initialize`/`tools/list`/
     `tools/call`/`ping` + notifications), **daemon-first with no embedded engine**: every tool proxies to the
     daemon at `daemon.json` (re-checked per call), and an unported engine route surfaces the daemon's `501`
     as an `isError` tool result. Request-shape validation (exactly-one-target, prune parts) runs before the
     daemon call, mirroring the Python. Tool descriptions are ported verbatim (they're the agent's contract).
     Validated end-to-end against BOTH daemons: against the live v2 Python daemon (12 tools, `capture_status`→42
     sessions, `list_windows`→22 windows) and the v3 Rust daemon (`capture_status`→42, `list_windows`→the 501
     message, `capture_index` unknown→404, `capture_start` no-target→client validation). The MCP is daemon-
     agnostic — it speaks to whatever `captured` (v2 or v3) is at `daemon.json`.
   - *GUI flip (done, #67):* the GPUI app now drives the **Rust** daemon. Its `daemon::resolve_daemon()`
     auto-spawns the bundled `captured` (packaged app) **or, in dev, the `captured` built beside `capture-gui`
     in the shared workspace target** — so `cargo run -p capture-gui` is self-sufficient against the Rust
     daemon. The HTTP client needed no change (it and the daemon both derive their wire types from
     `capture-core::v1`). The last gap — the GUI's indexing surface — closed with the providers port: the
     **structured `provider/host/port`** config now composes the endpoint server-side (explicit `endpoint`
     still wins), and `GET /v1/index/models|providers` populate the model dropdown + provider list.
     Live-validated against an isolated Rust daemon: every GUI endpoint 200 (read layer, ASR/runtimes,
     permissions, mics, index status/providers/models), a launch-mode **Start→Stop** cycle (artifacts +
     `events.jsonl` lifecycle + captured stdout), and the `/v1/events` SSE stream the GUI tails.
4. **`capture-asr`** (`whisper-rs`) — benchmark vs mlx-whisper using the existing `docs/asr-benchmark.md`
   harness before deleting the Python ASR.
   **IN PROGRESS (#64).** Engines are **dynamically-loaded cdylibs**, NOT statically compiled in (owner's
   call, 2026-06-18): the lean daemon links no engine and `dlopen`s an engine dylib at runtime when its
   runtime is selected. This keeps the daemon small and the engine an optional, shippable, swappable
   component — the native equivalent of the Python's "lean daemon + installable runtime packs", minus the
   wheel/`sys.path` machinery. So the **runtime-management abstraction STAYS** (it is the #77 unification done
   right): one `AsrBackend` trait; a runtime registry/selector that loads the active engine. "Models" are
   per-engine (GGML files for whisper.cpp; HF repos for a future mlx engine).
   - *The engine C ABI:* a small, **engine-agnostic** `capture_asr_engine_*` C ABI (`abi_version`/`name`/
     `load`/`transcribe`→JSON segments/`set_language`/`free*`/`last_error`). One loader
     (`capture-asr::DynamicEngine`) serves every engine; each engine ships as a cdylib. whisper.cpp is
     `capture-asr-whisper` → `libcapture_asr_whisper.dylib` (whisper-rs/whisper.cpp linked statically INSIDE
     the dylib, so no fragile hand-FFI against `whisper_full_params`). A future **`capture-asr-mlx`** cdylib
     ([mlx-rs](https://github.com/oxiglade/mlx-rs)) and Windows engines implement the same ABI and load through
     the same seam; remote engines (openai-compat/riva) stay in-process HTTP.
   - *Pieces 1 + 1b (done):* `crates/asr` — the `AsrBackend` trait + `Segment` + the `is_silent` gate
     (ports of `core/asr/base.py` + `__init__.py`) + a resampler + the `DynamicEngine` `dlopen` loader (lean:
     no whisper dep). `crates/asr-whisper` (cdylib) — the whisper.cpp engine behind the C ABI, with
     the **#45 hallucination guards** (`no_context`/`no_speech_thold 0.6`/`logprob_thold -1.0`/`entropy_thold
     2.4`), live language pinning, centisecond→second timestamps. Validated end-to-end through the FULL
     `dlopen`→C-ABI→whisper.cpp path: a 90 s slice of a real session's `audio.s16le` with `ggml-base.en`
     matched the ground truth nearly verbatim at **90–135× realtime** on an M3 Max (Metal). Tests: 8.
   - *Piece 2 — the runtime manager (done):* `capture-asr::runtime::AsrRuntimeManager` — the abstraction that
     manages runtimes (the v3 unification of the Python `runtimes.py` + `manager.py`). A platform registry
     ({`whisper-metal`, `remote`} on macOS; `whisper-cpu`/Windows + mlx later), per-runtime availability
     (local = engine dylib present + a GGML model downloaded; remote = endpoint configured), active selection
     persisted to `~/.capture/config.json` (`asr_runtime`/`whisper_model`/`whisper_language`, via
     `capture-asr::config`, preserving other keys; a stale cross-engine `mlx-community/...` model is ignored),
     and `backend()` → loads the active runtime's engine through the `DynamicEngine` seam (whisper → the GGML
     model + the cdylib; remote stubbed until piece 3) with clear errors (no runtime / engine missing / model
     not downloaded). 11 capture-asr tests.
   - *Piece 2b — the GGML model catalog + the daemon routes (done):* `capture-asr::models` — the curated
     GGML catalog (model **ids** like `base.en`/`large-v3-turbo`, each `ggml-<id>.bin` from
     `ggerganov/whisper.cpp`; `CAPTURE_GGML_BASE_URL` overrides the host), validation, and the persisted
     `whisper_model`/`whisper_language`/`audio_chunk_seconds` settings — **no HTTP in this crate** so the
     engine cdylib that links it stays lean (reqwest lives only in the daemon; verified by `cargo tree`). The
     manager binds the catalog to the models dir: `catalog_status` (downloaded/active/downloading flags +
     language/chunk), `delete_model`, `backend_report`, `runtimes_payload`. The daemon un-`501`s all ten
     `/v1/asr/*` routes (`routes::asr_*`): list runtimes / select runtime / install (a no-op on macOS — the
     whisper engine ships in the app bundle; downloadable engine *packs* are the Windows story #66) / backend
     report / model catalog / **streamed download** (a single GET of the `.bin` into a `.part` sidecar,
     atomically renamed; throttled `asr_download`→`asr_download_done`/`_error` SSE + an `asr_downloading`
     in-flight guard) / delete / select model / language / chunk. The wire `repo` field carries a GGML id, so
     the GUI's model picker flips to the v3 daemon unchanged. Live-validated end-to-end against a running
     `captured` (catalog, a real local-server download → `downloaded:true`, settings persisted to
     `config.json` as the exact keys the Python reads, delete, 400 on an unknown model). 16 capture-asr + 8
     capture-daemon tests.
   - *Remaining:* [3] the remote backends (openai-compat, riva — `backend()` still stubs remote); [4] wire the
     capture loop to the manager's `backend()` (with #65). Then the mlx-vs-whisper.cpp benchmark.

   *Live indexer note (2026-06-18): the v3 `capture-index` was validated end-to-end on 3 real sessions vs the
   LAN LM Studio (qwen/qwen3.5-9b) — classify→extract→combine→root, #51 flags, transcript fusion all correct.
   The macOS Local-Network gate (which silently denies unsigned shell binaries) was worked around with a
   curl-backed localhost proxy; see the `v3-index-port-validation` memory.*
5. **`capture-platform` macOS** (ScreenCaptureKit) — the hard part; spike A proved it turnkey. **Core DONE
   (#65)** — attach + launch capture, the mic track, and live transcription work end-to-end (pieces A–D
   below); all follow-ups (launch, mic track, mic TCC, import, events.jsonl) are now done. No Swift helper — the `screencapturekit` crate covers
   windows/screenshots/audio.
   - *Piece A — the platform read layer (done):* `crates/platform` stood up with the spike's
     packaging fixes baked in: `apple-metal` pinned to `0.6.0` (its 0.8.x references macOS-26 SDK symbols
     that don't build on the 15.x SDK — forced in the lock since 0.6/0.8 are incompatible 0.x ranges) and
     the Swift-runtime rpath embedded via `build.rs` (`-rpath /usr/lib/swift`, the OS Swift runtime resolved
     from the dyld shared cache — the shippable fix vs the spike's dev-only `DYLD_LIBRARY_PATH`; re-emitted
     in the daemon's own `build.rs` since link args don't propagate to the dependent binary). Read-only
     capabilities: `list_windows` (SCShareableContent → the `core.list_windows` wire shape: normal-layer
     windows only, pid / case-insensitive app-name filter, area-sorted, on-screen with an all-windows
     fallback), `screen_recording_status` (CoreGraphics `CGPreflightScreenCaptureAccess` — a pure check that
     never prompts), and `audio_input_devices` (`AudioInputDevice::list`, AVFoundation inside the crate so no
     extra dep). The daemon un-501s the four read routes: `GET /v1/windows`, `GET /v1/permissions`,
     `POST /v1/permissions/request` (reports status, never prompts — the prompt aborts a headless daemon),
     `GET /v1/audio/mics`. Live-validated against a running `captured`: 228 real windows (titles, pids, dims,
     area-sorted) + an app-name filter, 2 mics (default flagged), `screen_recording:granted` — proving the
     Swift rpath loads in the binary and `SCShareableContent::get()` runs off a tokio blocking thread. 3
     capture-platform + 10 capture-daemon tests. Microphone TCC still reports `unknown` (the AVFoundation
     check lands with the audio slice).
   - *Piece B — screenshots (done):* `capture_platform::capture_screenshot(window_id, opts)` →
     `SCScreenshotManager::capture_image` (a desktop-independent-**window** filter == `screencapture -l`,
     or the whole main display) → BGRA→RGBA → the cross-platform `encode_image` (the `image` crate: PNG /
     JPEG + an aspect-preserving `fit_box` downscale — the Rust replacement for `screencapture` + `sips`).
     `ScreenshotOptions{format,resolution,jpeg_quality}` mirrors the `screenshot_*` capture settings. The
     per-window filter trips `CGS_REQUIRE_INIT` in a non-GUI process (the daemon), so capture first forces
     the window-server connection up via `CGMainDisplayID`. Live-validated: a whole-display PNG (1512×982)
     and a per-window JPEG (1512×917, occlusion-correct). 8 capture-platform tests +
     `examples/screenshot.rs`. (No daemon route — screenshots are driven by the capture loop, piece D.)
   - *Piece C — audio streaming (done):* `capture_platform::start_audio_capture(target, on_samples)` →
     an `SCStream` (the spike's `AudioSink` delegate, generalized) converting each Float32 buffer to mono
     s16le and handing `(&[i16], source_rate)` to the callback until `AudioCapture::stop`/drop.
     `AudioTarget::App{pid,bundle_id}` filters to one application (`with_including_applications`);
     `AudioTarget::Mic{device_id}` uses macOS-15 SCK mic capture (`with_captures_microphone`). **Key
     finding: `with_sample_rate(16000)` resamples app/system audio to 16 kHz, but the microphone arrives
     at its native hardware rate (48 kHz here)** — so the callback carries the buffer's *actual* rate
     (derived from `num_samples`/`duration`) and the session loop resamples each chunk to 16 kHz (via
     `capture-asr::resample_linear`) before ASR + `audio.s16le`. Live-validated: app audio 32000 samples
     @ 16 kHz (silent → RMS 0, buffers flow), mic 95744 samples @ 48 kHz with real audio (RMS 83).
     `examples/audio.rs`. (Mic TCC status now reads `granted`/`denied`/`undetermined` via
     `+[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]` — raw Obj-C runtime FFI, a pure
     status read that never prompts; `/v1/permissions` matches the v2 Python.)
   - *Piece D — the capture session loop (done — the capstone):* `crates/engine`
     (`CaptureSession`, deps capture-core + capture-platform + capture-asr) orchestrates the lifecycle
     (created→starting→running→stopping→stopped/error), the on-disk session dir (byte-compatible with the
     Python: `session.json` `{config, summary}` built as a `v1::Session` so it can't drift from the read
     layer, `screenshots/<fs_stamp>.<ext>`, `audio.s16le`, `transcript.{jsonl,txt}`), a screenshot timer
     thread (absolute grid, re-resolving the window each tick via `list_windows`, whole-screen fallback),
     and the audio→ASR pump: `start_audio_capture` → a worker thread accumulates samples → chunks by
     `chunk_seconds × source_rate` → **resamples to 16 kHz** (`resample_linear`, identity for 16 kHz app
     audio) → writes `audio.s16le` → **#45 silence gate** (offset still advances) →
     `AsrRuntimeManager::backend().transcribe()` → appends `transcript.{jsonl,txt}` + emits SSE. The daemon
     gained a **live-session registry** (`AppState.sessions`) that's authoritative for a running session's
     state (the read layer rewrites a recovered "running"→"interrupted"); `list`/`get` serve live entries
     from it. Un-501'd `POST /v1/sessions` (start, attach mode) + `…/stop`. **Live-validated end-to-end**:
     a 4 s attach capture (5 screenshots, `audio.s16le` 130 KB ≈ 4 s @ 16 kHz, correct `session.json`),
     and a real mic transcription — base.en downloaded via the daemon, two spoken pangrams transcribed
     **verbatim** (offsets 0–4 / 4–7 s) through the dlopen'd whisper.cpp on Metal. 4 capture-engine + 10
     capture-daemon tests.
   - *Session-management routes (done):* un-501'd `delete` / `prune` / `retranscribe`. The shared disk
     helpers live in `capture_core::sessions` (`prune_session_dir`, `capabilities`, `remove_from_index`,
     `rewrite_session_summary`); `capture_engine::retranscribe_session` re-runs ASR over `audio.s16le`
     (16 kHz — no resample), preserves the prior as `transcript.prev.*`, and **anchors to the recovered audio
     epoch** so subtitles still line up. `delete`/`prune` refuse a live session (registry-authoritative).
     Live-validated: prune freed 1.3 MB (halve→2 frames, audio dropped, caps refreshed), delete (dir + index),
     retranscribe (prev kept, identical epoch-anchored timestamps, `retranscribe`/`_done` SSE, summary
     updated, 400 on pruned audio).
   - *Mic track + `set_mic` (done):* un-501'd `POST …/mic` + `CaptureSession::set_mic_device` (live
     on/off/switch) writing `mic.s16le`/`mic_transcript.*` (appends across switches) + the
     `mic_status`/`mic_segments`/`mic_device` summary fields. **The single-stream fix:** macOS won't run
     two concurrent audio `SCStream`s in a process, so app audio + mic come from ONE
     `start_audio_capture_dual` stream emitting both the `Audio` (→ `main_buf`) and `Microphone` (→
     `mic_buf`) output types, **each on its OWN serial dispatch queue** (a shared queue starves the second
     handler — that was the real bug, not a hard limit). Two persistent worker buffers + a
     track-parameterized worker drain them; a switch rebuilds the dual stream (the app worker keeps draining
     `main_buf` across a sub-second gap). Live-validated: a running app capture + `set_mic default` records
     **both** `audio.s16le` (~198 KB) **and** `mic.s16le` (~195 KB) concurrently, mic transcribed **verbatim**
     ("the quick brown fox…"), then `set_mic` off.
   - *Launch mode (done):* `POST /v1/sessions` now accepts a `command` (+ optional `cwd`) — `CaptureSession`
     spawns the child (`capture_engine::ProcessCapture`, a port of `core/proc.py`), uses ITS pid as the
     capture target (window + audio), and tees its stdout/stderr to `stdout.log`/`stderr.log` (raw) +
     `output.log` (merged, `<iso> [out|err] ` per line). `log_lines` + `process_running` populate the summary;
     `config.command`/`config.cwd` land in `session.json`. SIGTERM → 5 s → SIGKILL on stop (exit code noted;
     a launch that never spawns → state `error`, 400). The v3 MCP `capture_start` already forwarded
     `command`/`cwd`, so the MCP path works once the daemon honors it. Live-validated: a 5 s shell command
     (capture off) tee'd 8 log lines to all three files byte-correct, `process_running` true→false, exit-code
     note, `session.json` config; a bogus command → 400 + `error` (no phantom-running session). Hermetic
     `process_capture_tees_streams_and_reaps` test (self-exit code 7 captured).
   - *Mic TCC check (done):* `microphone_status()` now reads `+[AVCaptureDevice
     authorizationStatusForMediaType:AVMediaTypeAudio]` via raw Obj-C runtime FFI (`objc_getClass` /
     `sel_registerName` / `objc_msgSend` transmuted to the method prototype) + the `AVMediaTypeAudio`
     AVFoundation constant — `0→undetermined, 1|2→denied, 3→granted`. A pure read that never prompts (a
     prompt would `SIGABRT` the daemon). `/v1/permissions` reports `granted` here, matching the v2 Python.
   - *File import (#43, done):* `POST /v1/sessions/import {path, output_dir?}` turns an existing audio/video
     file into a session. Extraction is **in-process AVFoundation** (`capture_platform::extract_audio_s16le`
     via `AVAssetReader` → LinearPCM 16 kHz mono s16le; `extract_frames` via `AVAssetImageGenerator` → PNG
     through ImageIO `CGImageDestination`) — the objc2-av-foundation binding (version-aligned with the objc2
     0.6 screencapturekit pulls; no Swift helper, no ffmpeg). The engine orchestration is its OWN module
     (`capture-engine/src/import.rs`, a port of `core/import_media.py`): audio → `audio.s16le`, frames →
     `screenshots/<fs_stamp(base+offset)>.png` on the SAME epoch as the audio, `session.json`
     (`audio_source="import"`), then ASR via `retranscribe_session` (epoch from `started_at`, so subtitles
     align with frames; a backend failure leaves a valid session + note). The v3 MCP `capture_import` already
     posts here. Live-validated: a 3 s `.mov` → 3 frames at 0/1/2 s (no audio); a `say` m4a → `audio.s16le`
     transcribed verbatim via whisper-metal; both list in `/v1/sessions`; missing file → 400.
   - *`events.jsonl` (#26, done):* a per-session lifecycle log a client can tail
     (`capture-engine/src/events.rs`, port of `core/events.py::EventsFileWriter`). `CaptureSession::start`
     (now `&Arc<Self>`) opens it + spawns a snapshot thread holding a `Weak<Self>`; `publish()` persists
     every `state` transition; the thread writes a counter `snapshot` line every
     `CAPTURE_EVENTS_SNAPSHOT_SECONDS` (default 5 s) and `stop()` writes a final one. High-volume events
     (log_line/screenshot_taken/transcript_segment) are NOT persisted — they live in their own files.
     Live-validated: `starting→running→[snapshots]→stopping→stopped→final snapshot`, snapshots carrying the
     live summary. (Imports get no events.jsonl — they don't run a live session, matching v2.)
   - *#65/#43 follow-ups: all done. GUI flipped onto the Rust daemon (#67 — see piece 3 above).* The macOS
     surface (read · capture · import · index · ASR · lifecycle · GUI) is end-to-end Rust. Next: the workspace
     was also reorganized — every crate lives under `crates/` with a prefix-free folder name (package names
     keep `capture-`).
6. **`capture-platform` windows** (#66) — **DEFERRED (dev is on macOS).** The non-macOS path already ships
   as a compiling **stub** (every backend returns "not supported on this platform yet"; the macOS-only deps
   are `cfg(target_os = "macos")`-gated), so the workspace builds Windows-side without the real backends.
   The windows-rs Graphics.Capture + WASAPI-loopback impl + the `.exe`/WiX installer land on a Windows box.
7. **v3 cutover** (#67) — **macOS-first, in progress.** Retire the Python package + the Swift/Python helpers;
   drop PyInstaller; bundle the Rust `captured` + `capture-mcp` in `build_macos_dmg.sh`; make v3 the default
   branch. **Windows packaging deferred with #66.** **Selenium dropped** — it was never used; browser-driving,
   if ever needed, goes through the **chrome-devtools MCP**, not a bundled driver. (The GUI flip is done.)

## De-risk spikes — DONE, both GREEN (`spikes/`)
Both spikes were run as throwaway cargo binaries that compiled and *ran* on this Apple-silicon Mac. The two
biggest unknowns are resolved positively — the Rust rewrite is feasible.

- **Spike A — ScreenCaptureKit from Rust** (`spikes/sck-capture/`): **VIABLE, roughly turnkey.** The safe
  **`screencapturekit` v7** crate mirrors `audiocap.swift` ~1:1 (`SCShareableContent`, the per-app
  `SCContentFilter::with_including_applications`, `SCStreamConfiguration` audio, `SCScreenshotManager`,
  `CMSampleBuffer::audio_buffer_list`) with **zero unsafe**. All three capabilities ran: listed 309 windows /
  31 apps, produced a real PNG screenshot, and captured per-app audio → 16 kHz mono s16le. **No Swift
  needed.** Two one-time packaging notes: pin `apple-metal = "=0.6.0"` (its 0.8.8 references macOS-26 SDK
  symbols that fail on the 15.7 SDK), and a shipped binary must embed the Swift-runtime rpath
  (`libswift_Concurrency.dylib`) at link time. **Risk: was HIGH → now LOW.**
- **Spike B — whisper-rs vs mlx-whisper** (`spikes/whisper-bench/`): **ADOPT.** `whisper-rs 0.16`
  (`features=["metal"]`) built clean (whisper.cpp compiles via cc/cmake, no system deps), Metal active on the
  M3 Max, correct transcription of a real 60 s clip at **~73× realtime** (vs mlx-turbo ~33×; base.en is
  smaller, so a parity bench would ship `ggml-large-v3-turbo`). One Rust crate covers macOS-Metal /
  Windows-CUDA / Linux — kills the Python+mlx macOS-only constraint. Follow-ups: ship large-v3-turbo, warm
  the context at daemon start, chunk audio ourselves (whisper.cpp has no built-in streaming), add a ggml
  model fetch/cache.

## Open questions / risks (post-spike)
- ~~ScreenCaptureKit Rust-bindings maturity~~ — **resolved** (Spike A): `screencapturekit` v7 is turnkey.
  Residual: the helper's non-SCK AVFoundation paths (`--mic`, file `--extract-audio/-frames`) need
  `objc2-avfoundation` or a tiny shim; the `apple-metal` pin + Swift-rpath packaging notes above.
- ~~whisper.cpp/Metal perf vs MLX~~ — **resolved** (Spike B): fast + correct; MLX is unnecessary (optional
  later only if a specific model's Apple-silicon edge ever matters).
- GPUI maturity for any new surfaces (known quantity — already shipping).
- ~~selenium → `chromiumoxide` parity~~ — **dropped (2026-06-19).** Selenium was never actually used; the
  v3 daemon ships no in-process browser driver. If browser-driving is ever wanted, drive it via the
  **chrome-devtools MCP** (an external tool the agent already speaks), not a bundled dependency.
- Effort (revised, post-spike): the capture path is ~1–2 days (SCK) + ~0.5–1 day packaging; ASR ~1–2 days;
  the bulk is the ~8k-LOC logic port behind the stable contract — multi-week but mechanical and low-risk.

## Preserved / reused
The GPUI GUI, the `/v1` + on-disk contracts, the Whisper models, the MCP tool surface, the `capture-*`
skills (eval/tuning — they target the index, which is unchanged), and the 7 eval corpora (now regression
fixtures for `capture-index`).
