#!/usr/bin/env python3
"""Bump (or set) the capture version across the THREE places it lives, atomically.

Default bump is a PATCH/revision (x.y.Z+1). `minor` → x.(Y+1).0, `major` → (X+1).0.0.
`--set X.Y.Z` writes an explicit version. Prints `OLD -> NEW`. Does NOT build/tag/commit —
that's the rest of the capture-release flow. Run from the repo root.

  python .claude/skills/capture-release/scripts/bump_version.py            # patch
  python .claude/skills/capture-release/scripts/bump_version.py minor
  python .claude/skills/capture-release/scripts/bump_version.py --set 0.3.0
  python .claude/skills/capture-release/scripts/bump_version.py --current  # print, no change
"""
from __future__ import annotations
import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve()
while REPO != REPO.parent and not (REPO / "crates" / "gui" / "Cargo.toml").exists():
    REPO = REPO.parent

# (path, regex with a single capture group around the version, template to rewrite the line).
# v3: the Python package + pyproject are retired; the version now lives in the GUI crate manifest
# (the bundle version) + the two platform packaging scripts.
TARGETS = [
    (REPO / "crates/gui/Cargo.toml",       r'^version = "([0-9]+\.[0-9]+\.[0-9]+)"', 'version = "{v}"'),
    (REPO / "packaging/build_macos_dmg.sh", r'CAPTURE_GUI_VERSION:-([0-9]+\.[0-9]+\.[0-9]+)', 'CAPTURE_GUI_VERSION:-{v}'),
    (REPO / "packaging/build_windows.ps1", r'else \{ "([0-9]+\.[0-9]+\.[0-9]+)" \}', 'else {{ "{v}" }}'),
]


def current() -> str:
    txt = TARGETS[0][0].read_text()
    m = re.search(TARGETS[0][1], txt, re.M)
    if not m:
        sys.exit("could not read current version from crates/gui/Cargo.toml")
    return m.group(1)


def nextv(cur: str, level: str) -> str:
    x, y, z = (int(n) for n in cur.split("."))
    return {"patch": f"{x}.{y}.{z + 1}", "minor": f"{x}.{y + 1}.0", "major": f"{x + 1}.0.0"}[level]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("level", nargs="?", default="patch", choices=["patch", "minor", "major"])
    ap.add_argument("--set", dest="explicit")
    ap.add_argument("--current", action="store_true", help="print current version and exit")
    args = ap.parse_args()

    cur = current()
    if args.current:
        print(cur)
        return 0
    new = args.explicit or nextv(cur, args.level)
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", new):
        sys.exit(f"bad version {new!r}")

    missing = []
    for path, pat, tmpl in TARGETS:
        txt = path.read_text()
        if not re.search(pat, txt, re.M):
            missing.append(str(path))
            continue
        path.write_text(re.sub(pat, tmpl.format(v=new), txt, count=1, flags=re.M))
    if missing:
        sys.exit(f"version pattern not found in: {missing} (aborted partway — check git diff)")
    print(f"{cur} -> {new}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
