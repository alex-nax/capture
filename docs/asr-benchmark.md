# ASR Benchmark: faster-whisper (CUDA) vs NVIDIA Nemotron — and the Docker/WSL2 path

_Status: planning + partial. faster-whisper CUDA is **implemented and verified** on the
Windows/NVIDIA box (Session 7). The local-Nemotron path is **documented here for the
benchmark** (feature #23) but not yet stood up. Update this doc as the benchmark lands._

This is the running record for **feature #23** (benchmark local Whisper vs NVIDIA
Nemotron-3.5 ASR on the Windows/NVIDIA box). It also captures the **Docker/WSL2 run path**
for Nemotron so we can benchmark against it later, plus the native-Windows angle.

## Hardware / context
- Box: Windows 11, **RTX 4070 Ti SUPER (16 GB)**, driver 591.86 (CUDA 12.x capable, WSL2 GPU OK).
- Capture pipeline emits the standard contract: `audio.s16le` = **16 kHz mono s16le**. Any ASR
  backend consumes the same clips, so the benchmark is apples-to-apples.

## Backend A — faster-whisper large-v3 (CUDA, native Windows) — IMPLEMENTED
Runs natively on Windows on the 4070 Ti; this is what the capture currently uses.
- Install: `faster-whisper` + `nvidia-cublas-cu12` + `nvidia-cudnn-cu12` (CTranslate2 CUDA 12
  engine; cuDNN 9). On Windows the cuBLAS/cuDNN DLLs ship in the `nvidia/*/bin` pip dirs and
  must be on the DLL search path — `whisper_local._add_nvidia_dll_dirs()` handles that.
- Backend: `capture_mcp.asr.whisper_local.FasterWhisper`, device auto-detected via
  `ctranslate2.get_cuda_device_count()`; `CAPTURE_WHISPER_MODEL` / `CAPTURE_WHISPER_DEVICE` /
  `CAPTURE_WHISPER_COMPUTE` override (default `cuda`+`float16` when a GPU is present, else `cpu`+`int8`).
- Verified: large-v3 loads on CUDA and transcribes real captured speech with segment timestamps.

## Backend B — NVIDIA Nemotron (local) — via Docker/WSL2 (TO RUN FOR THE BENCHMARK)
**Why Docker/WSL2:** NeMo (which runs the Nemotron speech models) is **Linux-primary — it does
not support native Windows** (HF model card lists Linux; LiveKit guide targets macOS/Linux). So
on this Windows box, "Nemotron locally" must run inside Linux under WSL2/Docker with GPU
passthrough to the 4070 Ti.

Model: **`nvidia/nemotron-speech-streaming-en-0.6b`** (~2.4 GB, 600M Cache-Aware
FastConformer-RNNT, English) — or the multilingual **`nvidia/nemotron-3.5-asr-streaming-0.6b`**
(40 locales). Loaded in-process via NeMo (`nemo.collections.asr`).

### Prerequisites (one-time, on this box)
- Docker Desktop running with the **WSL2 backend** + **GPU support** enabled
  (Settings → Resources → WSL integration; NVIDIA GPU support ships with recent Docker Desktop).
  `nvidia-smi` must work inside a CUDA container. (As of Session 7: Docker installed, daemon
  was stopped, only a `docker-desktop` WSL distro existed — start Docker Desktop first.)
- An NGC/NVIDIA account is only needed if pulling NVIDIA's gated NIM/Riva images; the plain
  NeMo + HF model route below needs no NGC key (the HF model auto-downloads).

### Path B1 — NeMo container exposing a tiny localhost ASR service (recommended)
Run NeMo in a CUDA Linux container, load the model once, and expose a minimal HTTP/gRPC ASR
endpoint on `localhost` that takes 16 kHz mono PCM and returns text + word timestamps. A new
`capture_mcp.asr` backend (e.g. `nemo_local`) POSTs each chunk to it.

```bash
# In WSL2 (or as a Dockerfile). Pin a NeMo 25.11+ CUDA base image.
docker run --gpus all -it --rm -p 8088:8088 \
  -v nemo-cache:/root/.cache \
  nvcr.io/nvidia/nemo:25.11  # or build FROM pytorch/cuda + pip install nemo_toolkit[asr]
# inside: pip install fastapi uvicorn soundfile ; then run a ~30-line FastAPI server that does
#   model = nemo_asr.models.ASRModel.from_pretrained("nvidia/nemotron-speech-streaming-en-0.6b")
#   POST /transcribe: 16k mono f32/s16le -> model.transcribe(..., timestamps=True) -> JSON segments
```
Capture side: a `NemoLocal` backend calling `http://localhost:8088/transcribe`. Wire it into
`asr/__init__.py:create` under a new name (e.g. `"nemo"`), mirroring the existing Riva branch.

### Path B2 — NVIDIA Riva / NIM ASR container (fits the EXISTING adapter)
Run a local **Riva** ASR server (Riva Skills quickstart, or an ASR NIM) in Docker/WSL2 GPU,
exposing gRPC on `localhost:50051`. Then the **existing** `capture_mcp.asr.nemotron.NemotronRiva`
adapter works unchanged with `CAPTURE_RIVA_SERVER=localhost:50051` (no API key / function-id for
a local server). Heavier (NGC pulls, multi-GB images, model deployment) but zero new Python code.

## Native-Windows angle (no Docker) — investigation, not yet viable
NeMo does not run on native Windows, so the only no-Docker routes would be:
- Export the Nemotron FastConformer-RNNT to **ONNX** and run via `onnxruntime-gpu` (Windows CUDA) —
  feasible in principle but needs an export pipeline + RNNT decoding reimplemented; non-trivial.
- A `transformers`-based loader if/when NVIDIA ships a Windows-friendly checkpoint.
Until one exists, **WSL2/Docker is the local-Nemotron path on Windows.**

## Benchmark protocol (feature #23)
On identical captured `audio.s16le` clips (e.g. the UE5 playlist videos with narration):
1. Transcribe with Backend A (faster-whisper large-v3 CUDA) and Backend B (Nemotron local).
2. Measure: **WER** vs a reference transcript (hand-corrected or a high-quality reference),
   **real-time factor / latency** (wall time ÷ audio seconds), and **GPU memory** (`nvidia-smi`).
3. Record results in a table here + flip feature #23 when both backends transcribe the same clips
   end-to-end (this also verifies the Riva/Nemotron adapter and closes #13).

## Status / open items
- [x] Backend A (faster-whisper large-v3 CUDA) implemented + verified on Windows.
- [ ] Stand up Backend B locally via Path B1 or B2 (Docker/WSL2 GPU).
- [ ] Add a `nemo_local` ASR backend (Path B1) or reuse the Riva adapter (Path B2).
- [ ] Run the protocol on captured clips; record WER / RTF / GPU-mem; close #23 (and #13).
