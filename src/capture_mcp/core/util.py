"""Small shared helpers: timestamps, filesystem-safe names, command splitting."""

from __future__ import annotations

import os
import shlex
import subprocess
import time
from datetime import datetime, timezone


def now() -> float:
    """Monotonic-ish wall clock as a unix epoch float (seconds)."""
    return time.time()


def descendant_pids(pid: int) -> set[int]:
    """All transitive child pids of ``pid`` (excluding ``pid`` itself).

    Used to find a target process's *real* window when it was launched through a
    wrapper — shell → wine → game, electron, java launchers — where the visible
    window belongs to a CHILD process with a different pid than the one we attached
    to (GitHub #2: a Wine game's window is owned by a wine child, not the launcher,
    so a pid-only lookup finds nothing and falls back to full-screen).

    Best-effort and never raises: returns an empty set if the process table can't be
    read (e.g. a platform without ``ps``). POSIX only for now — Windows returns empty
    (its native windows are owned by their own pid; Wine-style chains are the macOS case).
    """
    if os.name == "nt":
        return set()
    try:
        out = subprocess.run(
            ["ps", "-axo", "pid=,ppid="], capture_output=True, text=True, timeout=4
        ).stdout
    except Exception:
        return set()
    children: dict[int, list[int]] = {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        try:
            child, parent = int(parts[0]), int(parts[1])
        except ValueError:
            continue
        children.setdefault(parent, []).append(child)
    seen: set[int] = set()
    stack = [pid]
    while stack:
        p = stack.pop()
        for c in children.get(p, ()):
            if c not in seen:
                seen.add(c)
                stack.append(c)
    return seen


def split_command(command: str) -> list[str]:
    """Tokenize a command-line string into argv, the way the host OS would.

    POSIX uses ``shlex`` (backslashes escape). Windows uses the OS tokenizer
    ``CommandLineToArgvW`` so backslash-laden paths (e.g. ``C:\\Python\\python.exe``)
    are split per Windows rules rather than mangled by POSIX escaping — this is the
    exact inverse of ``subprocess.list2cmdline``.
    """
    if not command.strip():
        return []  # both platforms agree on empty input (CommandLineToArgvW("") would
        #            otherwise return the current executable path on Windows)
    if os.name != "nt":
        return shlex.split(command)
    import ctypes
    from ctypes import wintypes

    CommandLineToArgvW = ctypes.windll.shell32.CommandLineToArgvW
    CommandLineToArgvW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_int)]
    CommandLineToArgvW.restype = ctypes.POINTER(wintypes.LPWSTR)
    argc = ctypes.c_int(0)
    argv = CommandLineToArgvW(command, ctypes.byref(argc))
    if not argv:
        # Fall back to a non-POSIX shlex pass (keeps backslashes) if the API fails.
        return shlex.split(command, posix=False)
    try:
        return [argv[i] for i in range(argc.value)]
    finally:
        ctypes.windll.kernel32.LocalFree(argv)


def iso(ts: float | None = None) -> str:
    """ISO-8601 UTC timestamp, millisecond precision, e.g. ``2026-06-07T09:47:01.250Z``."""
    dt = datetime.fromtimestamp(ts if ts is not None else now(), tz=timezone.utc)
    return dt.strftime("%Y-%m-%dT%H:%M:%S.") + f"{dt.microsecond // 1000:03d}Z"


def fs_stamp(ts: float | None = None) -> str:
    """Filesystem-safe timestamp for filenames, e.g. ``2026-06-07T09-47-01.250Z``."""
    return iso(ts).replace(":", "-")
