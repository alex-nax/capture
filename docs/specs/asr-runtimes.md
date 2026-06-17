# Spec: ASR runtimes (user-chosen, installable runtime packs)

_Status: **partially implemented**, 2026-06-17 — the **keystone is validated** (a lean frozen daemon
imports an external runtime pack); the activation mechanism, runtime-aware catalog, the **daemon routes**
(`GET /v1/asr/runtimes`, `POST /install`, `POST /v1/asr/runtime`, `GET /v1/asr/backend`), and the
**no-silent-fallback** wiring, the **pack-build tooling** (`build_runtime_packs.ps1`), the
**lean-by-default** Windows freeze, and the **GUI runtime picker** have all landed. What remains:
**hosting** the packs as release assets, and the **AMD/Intel** runtimes. Source of truth = the code;
sections mark **[done]** vs
**[planned]**. Tracked in `features.json` (#58)._

## Purpose

Voice recognition is **off by default** — the packaged daemon bundles **no ASR engine**. The user
**chooses a runtime that matches their hardware**, the app **installs** it (downloads a prebuilt
**pack**), then the user **picks a compatible model and downloads it**. Nothing is auto-selected and
**nothing falls back silently** (owner's directives, 2026-06-17): if the chosen runtime/device can't
load, the daemon says so — it does not quietly degrade to CPU.

This replaces the earlier "bundle faster-whisper + auto-detect" approach. Decisions (owner):
- **Frozen daemon + downloadable runtime packs** (not an embeddable-Python venv): keep the frozen
  daemon; a runtime is a prebuilt, ABI-matched pack added to the daemon's import path.
- **Multiple GPU runtimes by design ("option 3 track"), AMD deferred**: ship **CPU**, **NVIDIA
  (CUDA)**, and **remote** now; AMD/Intel (whisper.cpp Vulkan / ONNX DirectML) are registry slots added
  later with no mechanism change.

## Files

**[done]**
- `src/capture_mcp/core/asr/runtimes.py` — the runtime **registry** (`REGISTRY`) + the pack
  **activation** mechanism: `base_dir()`/`runtime_dir()`, `active_runtime()` (config `asr_runtime`),
  `is_installed()`, and `activate()` — inserts the active pack dir on `sys.path` and adds its DLL dirs
  (`os.add_dll_directory`) so the **frozen** daemon can import the engine. `CAPTURE_ASR_RUNTIME_DIR`
  overrides (dev/test). Idempotent.
- `packaging/captured_main.py` — calls `runtimes.activate()` before the daemon starts (and in the
  runtime-aware `--asr-selftest`), so an installed engine is importable.
- `src/capture_mcp/core/asr/manager.py` — **runtime-aware** catalog (`catalog()`/`default_repo()`/
  `runtime_available()`): faster-whisper `Systran/*` repos when faster-whisper is importable, mlx repos
  on Apple Silicon (see [asr.md](asr.md)).

- Daemon routes (`daemon/server.py` + `daemon/client.py`): `GET /v1/asr/runtimes`,
  `POST /v1/asr/runtimes/install` (background + SSE), `POST /v1/asr/runtime`, `GET /v1/asr/backend`.
- `whisper_local.FasterWhisper` — device from the active runtime; **no silent fallback** (raises +
  records `runtimes.last_error()`).
- `packaging/build_runtime_packs.ps1` — builds each local runtime's pack (`pip install --target` for the
  daemon's Python tag, zipped to `dist/runtime-<id>-<pytag>.zip`).
- `packaging/build_windows.ps1` — freezes the daemon **lean** by default (no engine bundled).
- `gui/` — the runtime picker (Settings → Voice recognition): lists runtimes (GPU hint) → Install
  (`POST /install`, SSE progress) → Use (`POST /v1/asr/runtime`) → the runtime-aware model picker.

**[planned]**
- **Hosting** the packs: publish `runtime-*-<pytag>.zip` as **GitHub release assets** (the release
  flow); `runtimes.pack_url()` already defaults to `…/releases/download/v<version>/…`.
- AMD/Intel runtimes (whisper.cpp Vulkan / ONNX DirectML) as new registry entries + packs.

## Public contract

- **Runtime registry** (`runtimes.REGISTRY`), each: `id`, `label`, `kind` (`local`/`remote`),
  `engine`, `device` (explicit for local), `requires` (hardware note), `pip` (pack contents). Today:
  `faster-cpu`, `faster-cuda`, `remote`. (Deferred: `whispercpp-vulkan`, `onnx-directml`.)
- **Pack** = a directory of the engine's wheels (e.g. `faster_whisper` + `ctranslate2` + `tokenizers` +
  `onnxruntime` + `av` + deps) installed for the daemon's exact Python tag (`cp312-win_amd64`), hosted
  as a release asset `runtime-<id>-<version>-<pytag>.zip`. Installed under
  `%LOCALAPPDATA%\Capture\runtimes\<id>` (Windows) / `~/.capture/runtimes/<id>`.
- **Activation contract:** `runtimes.activate()` runs before any engine import; it prepends the active
  pack dir to `sys.path` and adds its DLL dirs. The engine modules (`faster_whisper`, `ctranslate2`)
  then import normally in the frozen interpreter (**validated** — see Tests).
- **Config:** `asr_runtime` (the active runtime id) and `whisper_model` (the active model, validated
  against the runtime's catalog) in `~/.capture/config.json`.
- No change to `/v1` capture routes, the helper PCM contract, or the session layout.

## Behavior

1. **No runtime → ASR off (reported).** Lean daemon: `runtime_available()` is False, the model picker
   shows "no runtime — choose one", capture proceeds with **screenshots + logs only**; the session's
   `audio_status` reflects "no ASR runtime" rather than silently producing nothing.
2. **Choose + install a runtime.** The GUI lists runtimes (with `requires` notes + a detected-hardware
   hint — e.g. NVIDIA present). The user picks one; for a **local** runtime the daemon downloads the
   matching pack (by `id`+version+pytag) and extracts it to `runtime_dir(id)`, then sets `asr_runtime`.
   `remote` needs no download (configure the endpoint URL). The daemon **restarts** (or re-activates) so
   `activate()` puts the pack on `sys.path`.
3. **Pick + download a model.** With the runtime active, `runtime_available()` is True and `catalog()`
   returns that engine's models; the user downloads one (existing `/v1/asr/models/download`).
4. **Transcribe — explicit device, no silent fallback.** `FasterWhisper` uses the runtime's `device`
   (cuda for `faster-cuda`, cpu for `faster-cpu`). If it can't load (e.g. CUDA libs missing), it
   **raises with a clear message** and the daemon reports it (`audio_status` / `GET /v1/asr/backend`) —
   it does **not** silently switch to CPU.

## Invariants & constraints

- **No engine bundled in the default build** (the installer is lean; runtimes are opt-in downloads).
- **No silent fallback** — device is explicit per runtime; load failures surface, never degrade quietly.
- **Packs are ABI-matched** to the frozen daemon's Python (built with the same interpreter/tag);
  `activate()` prepends them so they win over anything the lean daemon still bundles.
- **Frozen daemon stays the model** (owner's pick over an embeddable venv); the daemon never runs `pip`
  itself — packs are prebuilt + hosted, downloaded + extracted.
- **Mechanism is engine-agnostic** — adding AMD (whisper.cpp/ONNX) is a new `REGISTRY` entry + a pack +
  a model catalog, no change to `activate()`.

## Failure modes & handling

- **No/again-missing runtime:** import fails → reported (`runtime_available()` False, clear
  `audio_status`); capture still records screenshots + logs.
- **Pack download fails / partial:** install route reports an error over `/v1/events`; the runtime stays
  un-installed (no half-activated state — `is_installed()` checks the dir is non-empty).
- **Wrong/old pack (ABI mismatch):** the engine import raises; surfaced, not hidden. The pack carries
  the pytag so the daemon only offers a matching pack.
- **Chosen device unavailable (e.g. `faster-cuda` without a GPU/driver):** load raises with the reason;
  reported. The user can switch to `faster-cpu` or `remote`.

## Configuration

- `asr_runtime` (config) — active runtime id. `whisper_model` (config) — active model.
- `CAPTURE_ASR_RUNTIME_DIR` — override the active pack dir (dev/test/spike).
- Existing engine env still wins at load time (`CAPTURE_WHISPER_MODEL`/`_DEVICE`/`_COMPUTE`,
  `CAPTURE_OPENAI_ASR_URL`, `CAPTURE_RIVA_*`).

## Known limitations / open items

- **Pack build — done 2026-06-17:** `packaging/build_runtime_packs.ps1` builds each local runtime's pack
  (`pip install --target` per pytag) → `dist/runtime-<id>-<pytag>.zip` (verified: faster-cpu = 86 MB
  zip). The daemon's download/extract install route is in (`POST /v1/asr/runtimes/install`).
  **[planned] Hosting:** publish those zips as **GitHub release assets** (`pack_url()` defaults to
  `…/releases/download/v<version>/…`). The CUDA pack is large (~1–2 GB) — release asset, on-demand.
- **Dependency overlap:** a pack ships its own `numpy`/`huggingface_hub`; prepended on `sys.path` they
  shadow the lean daemon's copies. Build packs pinned compatible with the daemon's code (or keep the
  daemon's overlap minimal). Validate per pack.
- **No-silent-fallback — done 2026-06-17:** `FasterWhisper` takes its device from the active runtime
  (`runtimes.active_device()`); a load failure **raises** (recording `runtimes.last_error()`, surfaced
  via `GET /v1/asr/backend`) instead of silently switching to CPU. `_auto_device` remains only as the
  no-runtime dev default.
- **GUI runtime picker — done 2026-06-17** — Settings → Voice recognition: lists runtimes
  (`GET /v1/asr/runtimes`, with the GPU hint) → Install (`POST /install`, SSE progress) → Use
  (`POST /v1/asr/runtime`) → the
  (runtime-aware) model picker → download.
- **AMD/Intel GPU deferred:** `whispercpp-vulkan` / `onnx-directml` registry slots + packs + model
  catalogs, later.
- **macOS** keeps its bundled mlx for now (the pack model is the Windows/Linux answer); unifying later.

## Tests

- **[done, spike 2026-06-17]** Keystone validated on the Windows box: a **lean** PyInstaller freeze
  (faster-whisper/ctranslate2 **excluded**) reports `ModuleNotFoundError: ctranslate2` on
  `--asr-selftest` (rc=1) with no runtime; pointed at an external CPU pack
  (`pip install --target faster-whisper`, 286 MB) via `CAPTURE_ASR_RUNTIME_DIR`, the same frozen
  `captured.exe --asr-selftest` prints `faster-whisper OK (ctranslate2=4.8.0, cuda_devices=1)` (rc=0) —
  i.e. the frozen daemon imported the external CTranslate2 C-extension + DLLs from `sys.path`.
- **[done, 2026-06-17]** Engine/daemon routes verified on the box: `GET /v1/asr/runtimes` returns the
  registry + `gpu:{nvidia:true}` + active; `install("faster-cpu", source=<local pack>)` → `set_active` →
  `GET /v1/asr/backend` reports `runtime=faster-cpu, device=cpu, available=true, error=null`; `GET
  /v1/asr/runtimes` + `/v1/asr/backend` dispatch live over HTTP. smoke 67/67, contracts 4/4. (Config +
  pack dir restored after the test.)
- **[planned]** full round-trip from a **hosted** pack (download → activate → download a model →
  transcribe a clip) once the pack-build/hosting tooling exists; a `faster-cuda`-without-GPU run surfaces
  a clear error (no silent CPU).
