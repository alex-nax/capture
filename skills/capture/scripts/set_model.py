#!/usr/bin/env python3
"""Set the capture daemon's active ASR model (and optionally pre-download it) over /v1.

This talks to the RUNNING capture daemon instead of editing any config file. It reads
the daemon's discovery file (`$CAPTURE_DAEMON_JSON` else `~/.capture/daemon.json`) for
`{endpoint, token}`, then:
  - POST {endpoint}/v1/asr/model            {"repo": <model>}  -> set the active model
  - POST {endpoint}/v1/asr/models/download  {"repo": <model>}  -> prefetch (with --prefetch)

    python set_model.py --model ggml-large-v3-turbo [--prefetch]

If no daemon is running this prints a clear message and exits non-zero — start a capture
(or the Capture app) first so the daemon is up, then re-run.

Pure stdlib: this skill is installed standalone and can NOT import repo modules.
"""
from __future__ import annotations

import argparse
import json
import os
import urllib.error
import urllib.request
from pathlib import Path


def daemon_json_path() -> Path:
    """Discovery file: $CAPTURE_DAEMON_JSON (file or dir) else ~/.capture/daemon.json."""
    env = os.environ.get("CAPTURE_DAEMON_JSON")
    if env:
        p = Path(env).expanduser()
        return p / "daemon.json" if p.is_dir() else p
    return Path.home() / ".capture" / "daemon.json"


def read_daemon() -> tuple[str, str] | None:
    """Return (endpoint, token) from the discovery file, or None if absent/unreadable."""
    path = daemon_json_path()
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        return None
    endpoint = data.get("endpoint")
    token = data.get("token")
    if not isinstance(endpoint, str) or not endpoint:
        return None
    return endpoint, token if isinstance(token, str) else ""


def post(endpoint: str, token: str, route: str, payload: dict) -> tuple[int, str]:
    """POST JSON to {endpoint}{route} with a bearer token. Returns (status, body_text)."""
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        endpoint.rstrip("/") + route,
        data=body,
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")


def main() -> int:
    ap = argparse.ArgumentParser(description="Set + optionally prefetch the capture daemon's ASR model")
    ap.add_argument("--model", required=True, help="model repo to make active (e.g. ggml-large-v3-turbo)")
    ap.add_argument("--prefetch", action="store_true", help="also download the model now")
    a = ap.parse_args()

    daemon = read_daemon()
    if daemon is None:
        print(
            "No running capture daemon found "
            f"({daemon_json_path()}). Start a capture (or the Capture app) so the "
            "daemon is up, then re-run."
        )
        return 1
    endpoint, token = daemon

    status, text = post(endpoint, token, "/v1/asr/model", {"repo": a.model})
    if status >= 400:
        print(f"failed to set model (HTTP {status}): {text.strip()}")
        return 1
    try:
        active = json.loads(text).get("active", a.model)
    except ValueError:
        active = a.model
    print(f"active ASR model -> {active}")

    if a.prefetch:
        print(f"  prefetching {a.model} (first download can take a while)...")
        status, text = post(endpoint, token, "/v1/asr/models/download", {"repo": a.model})
        if status >= 400:
            print(f"  prefetch failed (HTTP {status}): {text.strip()}")
            return 1
        print("  download started (runs in the background on the daemon).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
