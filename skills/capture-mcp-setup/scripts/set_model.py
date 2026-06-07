#!/usr/bin/env python3
"""Change the default ASR model in .mcp.json (capture server env) and optionally
pre-download it so the first capture doesn't stall on a model download.

    python set_model.py --model mlx-community/whisper-large-v3-turbo \
                        [--prefetch --python /path/to/.capture-mcp/.venv/bin/python] \
                        [--project-dir .] [--name capture]

Valid examples: mlx-community/whisper-tiny, mlx-community/whisper-large-v3-turbo.
NOTE: mlx-community/whisper-base does NOT exist (404).
"""
from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser(description="Set + optionally prefetch the capture ASR model")
    ap.add_argument("--model", required=True)
    ap.add_argument("--project-dir", default=".")
    ap.add_argument("--name", default="capture")
    ap.add_argument("--prefetch", action="store_true", help="download the model now")
    ap.add_argument("--python", default=None, help="capture-mcp venv python (needed for --prefetch)")
    a = ap.parse_args()

    path = Path(a.project_dir).expanduser().resolve() / ".mcp.json"
    data = json.loads(path.read_text() or "{}") if path.exists() else {}
    if not isinstance(data, dict):
        print(f"refusing to edit {path}: top level is not an object")
        return 1
    entry = data.setdefault("mcpServers", {}).setdefault(a.name, {})
    entry.setdefault("env", {})["CAPTURE_WHISPER_MODEL"] = a.model
    path.write_text(json.dumps(data, indent=2) + "\n")
    print(f"set CAPTURE_WHISPER_MODEL={a.model} in {path}")

    if a.prefetch:
        if not a.python:
            print("  --prefetch needs --python <venv python>; skipping download "
                  "(model will download on first capture).")
            return 0
        # Load the model once through the installed backend to trigger the download.
        code = (
            "import numpy as np;"
            "from capture_mcp.asr.whisper_local import MlxWhisper, FasterWhisper;"
            f"m={a.model!r};"
            "be=None\n"
            "try:\n"
            "    be=MlxWhisper(model=m)\n"
            "except Exception:\n"
            "    be=FasterWhisper(model=m)\n"
            "be.transcribe(np.zeros(16000, dtype='float32'), 16000)\n"
            "print('prefetched', m)"
        )
        print(f"  prefetching {a.model} (first download can take a while)...")
        rc = subprocess.run([a.python, "-c", code]).returncode
        if rc != 0:
            print("  prefetch failed — check the model name is a real HF repo.")
            return rc
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
