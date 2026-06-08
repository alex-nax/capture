# Distributable skills

## `capture` — load-and-go capture setup + operation

A single skill that lets anyone install, wire up, and operate **capture-mcp** from any project.
It installs capture-mcp + dependencies if missing, creates/merges the project's `.mcp.json`, and
runs quick capture actions (capture a browser video, launch & capture a process, change/download
the ASR model, edit per-project config).

### How to load it into your Claude

Copy the skill folder into your skills directory:

```bash
# user-wide (all projects):
cp -R skills/capture ~/.claude/skills/

# or per-project:
mkdir -p .claude/skills && cp -R skills/capture .claude/skills/
```

Then just ask Claude to set up capture or to "capture the browser video" / "record this app" —
the skill triggers on capture/record/screen-capture/transcribe-app-audio requests.

### Package it for sharing (optional)

Using the `skill-creator` skill's packager (run with its root on `PYTHONPATH`):

```bash
SC=~/.claude/skills/skill-creator
PYTHONPATH="$SC" python "$SC/scripts/package_skill.py" "$PWD/skills/capture" ./dist
```

produces `dist/capture.skill` (a zip bundle) to share; the recipient unzips it into their
skills directory.

### What it sets up

- Clones capture-mcp to `~/.capture-mcp` (override with `CAPTURE_HOME`), makes a venv, installs
  the package + an ASR backend (mlx-whisper on Apple Silicon, faster-whisper elsewhere), and on
  macOS builds **and stably code-signs** the ScreenCaptureKit per-app audio helper so the Screen
  Recording grant is approved once and persists. Windows uses `install.ps1` (no helper needed).
- Adds a `capture` server to the project's `.mcp.json` (preserving any existing servers).

Platform: **macOS** (screenshots + per-app audio) and **Windows** (screenshots/window-discovery/logs
via GDI+/EnumWindows; per-app audio loopback pending → mic fallback) are supported
(see `../docs/specs/platform-abstraction.md`).
