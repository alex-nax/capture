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
├── crates/
│   ├── capture-core/         session lifecycle, registry, frames, events, presets, providers, config;
│   │                         the /v1 + on-disk CONTRACT TYPES (serde) — the source of truth
│   ├── capture-platform/     trait WindowFinder/ScreenGrabber/AudioSource + backends:
│   │     ├─ macos            ScreenCaptureKit (audio+shots) via objc2/`screencapturekit`/`cidre`,
│   │     │                   AVFoundation mic — replaces audiocap.swift
│   │     └─ windows          windows-rs: Graphics.Capture + WASAPI loopback — replaces audiocap_win.py
│   ├── capture-asr/          trait AsrBackend + manager + backends (whisper-rs default; HTTP/Riva; …)
│   ├── capture-index/        vision_client + build_index + live_index (merge-tree) + AGENTS.md;
│   │                         reqwest + serde_json + `image` (replaces sips)
│   ├── capture-daemon/       the /v1 HTTP server (axum) + SSE — binary `captured`
│   ├── capture-mcp/          MCP stdio server (`rmcp` SDK or JSON-RPC) — binary `capture-mcp`
│   └── capture-gui/          the existing GPUI app (moved in; points at the Rust daemon)
└── (packaging: cargo-dist / cargo-bundle + platform installers)
```

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
| selenium (browser-capture flow) | `chromiumoxide` (CDP) or keep optional | medium | peripheral; defer |
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
   keep the v1 golden green.
2. **`capture-index`** — pure logic + HTTP; **fully testable against the 7 existing eval corpora** (no
   capture/permissions needed). High-value, low-risk first port; proves the toolchain.
3. **`capture-daemon` + `capture-mcp`** — serve the same `/v1` + MCP; GUI flips to the Rust daemon.
4. **`capture-asr`** (`whisper-rs`) — benchmark vs mlx-whisper using the existing `docs/asr-benchmark.md`
   harness before deleting the Python ASR.
5. **`capture-platform` macOS** (ScreenCaptureKit) — the hard part; gated on the spike (below).
6. **`capture-platform` windows** + the unified single-binary bundling.
7. Retire the Python package + the Swift/Python helpers; drop PyInstaller.

## First two tickets — DE-RISK SPIKES (do before committing to the full port)
- **Spike A — ScreenCaptureKit from Rust**: a throwaway binary that captures one app's audio (s16le) + a
  window screenshot via `screencapturekit`/`cidre`/`objc2`. Answers the biggest unknown (bindings maturity,
  entitlements, the `-3805` reconnect behavior) before the platform crate is scheduled.
- **Spike B — whisper-rs vs mlx-whisper bench**: transcribe a fixed clip with `whisper-rs` (Metal + CoreML)
  vs the current mlx path; measure latency/accuracy. Decides whether MLX-as-a-backend is ever worth it.

## Open questions / risks
- ScreenCaptureKit Rust-bindings maturity (Spike A) — may need hand `objc2` FFI; the highest schedule risk.
- whisper.cpp/Metal perf vs MLX on Apple silicon (Spike B).
- GPUI maturity for any new surfaces (known quantity — already shipping).
- selenium → `chromiumoxide` parity for the YouTube-capture flow (peripheral; can ship without it).
- Effort: ~8k LOC of logic port + the capture-FFI work — multi-week, but mostly mechanical behind the
  stable contract.

## Preserved / reused
The GPUI GUI, the `/v1` + on-disk contracts, the Whisper models, the MCP tool surface, the `capture-*`
skills (eval/tuning — they target the index, which is unchanged), and the 7 eval corpora (now regression
fixtures for `capture-index`).
