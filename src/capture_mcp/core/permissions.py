"""macOS TCC permission status + request (Screen Recording).

The **daemon** is the process that needs Screen Recording — screenshots, window
*titles* (``kCGWindowName`` is redacted without it), and ScreenCaptureKit per-app
audio all depend on it — so the check/request lives here and the grant attributes
to the daemon's responsible process. Exposed over ``/v1/permissions`` for the GUI.

macOS only: apps can *check* and *trigger the prompt*, but can't grant or revoke a
TCC right (only the user can, in System Settings). Note the OS quirk — a freshly
granted Screen Recording right needs the process to **restart** to take effect.

Non-macOS returns ``"not_applicable"``. Attribution/persistence for an ad-hoc
unsigned daemon is subject to the TCC caveats tracked in #31 (Developer ID).
"""

from __future__ import annotations

import sys

GRANTED = "granted"
DENIED = "denied"
NOT_APPLICABLE = "not_applicable"
UNKNOWN = "unknown"


def screen_recording_status() -> str:
    """``granted`` / ``denied`` (macOS), ``not_applicable`` elsewhere."""
    if sys.platform != "darwin":
        return NOT_APPLICABLE
    try:
        import Quartz

        return GRANTED if Quartz.CGPreflightScreenCaptureAccess() else DENIED
    except Exception:
        return UNKNOWN


def request_screen_recording() -> str:
    """Return the Screen Recording status WITHOUT prompting.

    IMPORTANT: do NOT call ``CGRequestScreenCaptureAccess`` here. It must run in a GUI
    app with a window-server connection; from this headless daemon it **aborts the
    process** (SIGABRT). The GUI triggers the prompt itself (CoreGraphics FFI); the
    daemon only ever *checks* status (``CGPreflightScreenCaptureAccess`` is safe).
    """
    return screen_recording_status()


_MIC_STATUS = {0: "undetermined", 1: "denied", 2: "denied", 3: GRANTED}  # AVAuthorizationStatus


def microphone_status() -> str:
    """``granted`` / ``denied`` / ``undetermined`` (macOS), else ``not_applicable``.

    Microphone is only needed for the **mic fallback** (ffmpeg ``avfoundation``);
    per-app audio uses ScreenCaptureKit, which keys off Screen Recording instead.
    """
    if sys.platform != "darwin":
        return NOT_APPLICABLE
    try:
        import AVFoundation as AVF

        st = AVF.AVCaptureDevice.authorizationStatusForMediaType_(AVF.AVMediaTypeAudio)
        return _MIC_STATUS.get(int(st), UNKNOWN)
    except Exception:
        return UNKNOWN


def request_microphone() -> str:
    """Return the Microphone status WITHOUT prompting.

    Like Screen Recording, do NOT prompt from this headless daemon —
    ``requestAccessForMediaType`` aborts a backgroundonly process when it has to show
    the dialog. The mic prompt is triggered elsewhere: macOS shows it automatically the
    first time the ffmpeg mic-fallback opens the device, and the GUI links to Settings.
    """
    return microphone_status()


def request(kind: str) -> dict:
    """Trigger the prompt for ``kind`` (``screen_recording`` | ``microphone``)."""
    if kind == "screen_recording":
        sr = request_screen_recording()
        return {"platform": sys.platform, "screen_recording": sr}
    if kind == "microphone":
        mic = request_microphone()
        return {"platform": sys.platform, "microphone": mic}
    raise ValueError(f"unknown permission {kind!r}")


def status() -> dict:
    """The ``GET /v1/permissions`` payload."""
    return {
        "platform": sys.platform,
        "screen_recording": screen_recording_status(),
        "microphone": microphone_status(),
    }
