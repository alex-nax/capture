#!/usr/bin/env python3
"""File a capture-mcp bug as a GitHub issue, with auto-collected diagnostics.

SAFETY: by default this only PREVIEWS the issue (prints the title/body and a prefilled
GitHub URL) and posts NOTHING. Pass --create to actually open the issue via `gh` (needs
gh installed + authenticated). Posting publishes to a public repo, so get the user's OK
first and let them review the body. Secrets (env values such as API keys) are never
included — only MCP server *names* and non-sensitive diagnostics.

    python report_issue.py --summary "audio never transcribes" [--session-dir <dir>]
                           [--title "..."] [--create] [--repo alex-nax/capture]
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

REPO_DEFAULT = "alex-nax/capture"
URL_BODY_CAP = 6000  # keep the prefilled GET URL within practical limits


def scrub(text: str) -> str:
    """Redact home-dir paths / usernames from diagnostic text (PII, not secrets).

    Subprocess stderr (ffmpeg / the helper) and ASR exception strings can contain
    absolute paths that reveal the OS username; replace them before the text reaches
    a public issue.
    """
    if not text:
        return text
    text = text.replace(str(Path.home()), "~")
    text = re.sub(r"/Users/[^/\s\"']+", "/Users/<user>", text)
    text = re.sub(r"/home/[^/\s\"']+", "/home/<user>", text)
    text = re.sub(r"([A-Za-z]:\\Users\\)[^\\\s\"']+", r"\1<user>", text)  # Windows
    return text


def daemon_json_path() -> Path:
    """Discovery file: $CAPTURE_DAEMON_JSON (file or dir) else ~/.capture/daemon.json."""
    env = os.environ.get("CAPTURE_DAEMON_JSON")
    if env:
        p = Path(env).expanduser()
        return p / "daemon.json" if p.is_dir() else p
    return Path.home() / ".capture" / "daemon.json"


def capture_version() -> str:
    """Version from the running daemon's GET /v1/health, or 'unknown' if unreachable."""
    try:
        data = json.loads(daemon_json_path().read_text())
        endpoint = data.get("endpoint")
        if not isinstance(endpoint, str) or not endpoint:
            return "unknown"
        req = urllib.request.Request(endpoint.rstrip("/") + "/v1/health")
        with urllib.request.urlopen(req, timeout=2) as resp:
            health = json.loads(resp.read().decode("utf-8", "replace"))
        return str(health.get("version", "unknown"))
    except (OSError, ValueError, urllib.error.URLError):
        return "unknown"


def gh_ready() -> bool:
    if not shutil.which("gh"):
        return False
    return subprocess.run(["gh", "auth", "status"], capture_output=True).returncode == 0


def read_session(session_dir: str | None) -> dict | None:
    if not session_dir:
        return None
    p = Path(session_dir)
    cand = p / "session.json"
    if not cand.exists():
        subs = sorted(p.glob("capture-*/session.json"))
        cand = subs[-1] if subs else None
    if not cand or not cand.exists():
        return None
    try:
        return json.loads(cand.read_text())
    except Exception:
        return None


def mcp_configured(cwd: str = ".") -> str:
    p = Path(cwd) / ".mcp.json"
    if not p.exists():
        return "no .mcp.json in project"
    try:
        d = json.loads(p.read_text() or "{}")
        servers = list((d.get("mcpServers") or {}).keys())
        return f"servers={servers} (env values redacted)"
    except Exception:
        return ".mcp.json present but unparseable"


def build_body(summary: str, session: dict | None, project_dir: str = ".") -> str:
    diag = [
        f"- capture-mcp version: {capture_version()}",
        f"- OS: {platform.platform()}",
        f"- arch: {platform.machine()}",
        f"- python: {platform.python_version()}",
        f"- gh available+authed: {gh_ready()}",
        f"- .mcp.json: {mcp_configured(project_dir)}",
    ]
    parts = [
        f"### What happened\n{summary or '(describe the problem)'}\n",
        "### Diagnostics\n" + "\n".join(diag) + "\n",
    ]
    if session:
        s = session.get("summary", session)
        keep = {
            k: s.get(k)
            for k in ("state", "audio_mode", "audio_status", "screenshots",
                      "screenshot_errors", "transcript_segments", "asr_errors", "notes")
        }
        parts.append("### Session summary\n```json\n" + json.dumps(keep, indent=2) + "\n```\n")
    parts.append(
        "### Steps to reproduce\n1. \n2. \n\n"
        "_Diagnostics auto-collected by the `capture` skill. Env values / API keys are "
        "intentionally omitted — please review before posting and remove anything sensitive._"
    )
    return "\n".join(parts)


def main() -> int:
    ap = argparse.ArgumentParser(description="Preview or file a capture-mcp bug report")
    ap.add_argument("--repo", default=REPO_DEFAULT)
    ap.add_argument("--summary", default="")
    ap.add_argument("--title", default="")
    ap.add_argument("--session-dir", default=None, help="a capture session dir (or its parent)")
    ap.add_argument("--project-dir", default=None,
                    help="dir containing the project's .mcp.json (default: session's cwd, else CWD)")
    ap.add_argument("--labels", default="bug")
    ap.add_argument("--create", action="store_true",
                    help="actually open the issue (publishes publicly; otherwise preview only)")
    a = ap.parse_args()

    session = read_session(a.session_dir)
    # Resolve where the user's .mcp.json lives: explicit flag > the session's recorded
    # cwd > the current directory (the script usually runs from the install dir, so the
    # bare CWD is rarely the user's project).
    project_dir = a.project_dir or (session or {}).get("config", {}).get("cwd") or "."

    title = scrub(a.title or (f"capture: {a.summary[:60]}" if a.summary else "capture: bug report"))
    body = scrub(build_body(a.summary, session, project_dir))

    print("=== Issue title ===\n" + title)
    print("\n=== Issue body ===\n" + body + "\n")

    # Prefilled "new issue" URL fallback the user can open in a browser.
    url_body = body if len(body) <= URL_BODY_CAP else body[:URL_BODY_CAP] + "\n\n_(truncated)_"
    q = urllib.parse.urlencode({"title": title, "body": url_body, "labels": a.labels})
    url = f"https://github.com/{a.repo}/issues/new?{q}"

    if not a.create:
        print("Open this prefilled URL to file the issue:\n" + url)
        print("\n(preview only — re-run with --create to post via gh, or open the URL above.)")
        return 0

    if gh_ready():
        res = subprocess.run(
            ["gh", "issue", "create", "--repo", a.repo, "--title", title,
             "--body", body, "--label", a.labels],
            capture_output=True, text=True,
        )
        if res.returncode == 0:
            print("✓ Created issue:", res.stdout.strip())
            return 0
        print("gh issue create failed:", res.stderr.strip(), file=sys.stderr)
        print("Open this prefilled URL instead:\n" + url, file=sys.stderr)
        return 1

    print("gh not installed/authenticated — open this prefilled URL to file the issue:\n" + url,
          file=sys.stderr)
    print("(or run `gh auth login`, then re-run with --create.)", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
