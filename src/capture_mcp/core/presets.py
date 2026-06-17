"""Capture PRESETS (#54) — one choice at Start that wires both the CAPTURE config (mic / screenshots)
and the default INDEX intent for the session, so the capture meets the requirements for good indexing.

The chosen preset + its resolved `index_preset` are recorded on the session (session.json), so ANY later
index — from the GUI or from an MCP caller — defaults to it (GUI↔MCP parity). A frontier model driving
the MCP path picks `custom` and supplies its own `leaf_prompt`/`leaf_schema`.

Each entry: `mic` (True force on / False force off / None leave as-is), `screenshots` (force on/off),
`index_preset` (the default `prompt_preset` for the session's index).
"""

from __future__ import annotations

#: preset id -> {label, mic, screenshots, index_preset, hint}. `auto` first — the most common choice.
CAPTURE_PRESETS: dict[str, dict] = {
    "auto": {"label": "Auto", "mic": None, "screenshots": True, "index_preset": "auto",
             "hint": "Let the classifier decide per frame; adapts as the screen changes (used for live indexing)."},
    "meeting": {"label": "Meeting", "mic": True, "screenshots": True, "index_preset": "meeting",
                "hint": "A video call/standup — mic on, captures participants, active speaker, task assignments."},
    "coding": {"label": "Coding / tutorial", "mic": False, "screenshots": True, "index_preset": "coding",
               "hint": "An IDE or a coding video — extracts verbatim code at high resolution."},
    "lecture": {"label": "Lecture / explainer", "mic": False, "screenshots": True, "index_preset": "lecture",
                "hint": "A slide/explainer tutorial — topics, key points, code, and formulas."},
    "general": {"label": "General", "mic": None, "screenshots": True, "index_preset": "auto",
                "hint": "Plain capture with no opinionated wiring; index auto-classifies."},
    "custom": {"label": "Custom", "mic": None, "screenshots": True, "index_preset": "custom",
               "hint": "Tune the indexing parameters yourself in the session's playback view."},
}
DEFAULT_PRESET = "general"


def resolve(preset: str | None) -> dict:
    """The preset's config (falls back to `general` for unknown/None)."""
    return CAPTURE_PRESETS.get(preset or DEFAULT_PRESET, CAPTURE_PRESETS[DEFAULT_PRESET])


def index_preset_for(preset: str | None) -> str:
    """The default index `prompt_preset` a session captured under `preset` should use."""
    return resolve(preset)["index_preset"]
