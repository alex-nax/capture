# Spec: Screenshots

_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose

Periodically grab timestamped screenshots of a target window (or the whole screen),
on a fixed time grid, into an output directory. Output format and resolution are
configurable. The component runs on its own daemon thread and is one of the capture
components owned by `CaptureSession` (see `docs/architecture.md` module map). It targets
a specific app window when a `pid`/`app_name` is given, re-resolving the window id every
tick so it keeps working across window recreation, Space switches, and fullscreen
transitions.

**This scope owns the platform-neutral parts only**: the capture *schedule*, window-target
resolution with the `_last_wid` fallback, output filenames, and success/error accounting.
The OS-specific pixel capture (encode, resize, window-vs-screen) is delegated to
`platform.current().screen_grabber.capture(...)` and window discovery to
`platform.current().window_finder` — macOS uses `screencapture`/`sips`, Windows uses GDI+
(see [platform-abstraction.md](platform-abstraction.md)). The detailed `screencapture`/`sips`
behavior described below now lives in `platform/macos.py:MacScreenGrabber`.

## Files

- `src/capture_mcp/core/screenshots.py` — `parse_resolution`, `VALID_FORMATS`, `_extension`, and the
  `Screenshotter` class (scheduling + delegation). The `_png_size`/`_fit`(→`base.fit_box`)/
  `_sc_format`/`_sips_format`/`_run_cmd` helpers and the `screencapture`/`sips` command-building
  MOVED to `src/capture_mcp/core/platform/macos.py`; the Windows equivalent is
  `src/capture_mcp/core/platform/windows.py`.
- Dependencies it uses (not owned by this scope): `src/capture_mcp/core/platform/` (the
  `WindowFinder`/`ScreenGrabber` backends), `src/capture_mcp/core/util.py` (`now`, `fs_stamp`).

## Public contract

### `parse_resolution(spec: str | None) -> tuple[int, int, str | None] | None`
(`screenshots.py:30-54`)
- `None` or empty string -> returns `None`.
- `"WxH"` (e.g. `"640x480"`) -> `(w, h, None)`.
- `"WxH/fmt"` (e.g. `"1280x720/jpg"`) -> `(w, h, fmt)` where `fmt` is lowercased and
  must be in `VALID_FORMATS`.
- The `x` separator is case-insensitive and the Unicode `×` (U+00D7) is normalized to
  `x` before splitting. Surrounding whitespace is stripped.
- Raises `ValueError` on: a bad format after `/`; not exactly two `x`-separated parts;
  non-integer dimensions; or any dimension `< 1`.

### `VALID_FORMATS` (`screenshots.py:27`)
The tuple `("png", "jpg", "jpeg", "tiff", "gif", "bmp")`.

### `class Screenshotter` (`screenshots.py:88-251`)
Constructor (`screenshots.py:96-127`):
```
Screenshotter(
    out_dir: Path,
    *,
    pid: int | None = None,
    app_name: str | None = None,
    interval: float = 1.0,
    fmt: str = "png",
    resolution: tuple[int, int] | None = None,
    jpeg_quality: int | None = None,
    whole_screen_fallback: bool = True,
)
```
- `fmt` is coerced to lowercase (with `None` -> `"png"`); raises `ValueError` if not in
  `VALID_FORMATS`.
- `interval` is clamped to a minimum of `0.05` seconds (`max(0.05, float(interval))`).
- `resolution` here is the already-parsed `(w, h)` box (the caller `session.py` calls
  `parse_resolution` and passes the first two elements; the `/fmt` part, if present,
  is applied by `session.py` as the `fmt`, not here).

Public attributes / methods:
- `start() -> None` — creates `out_dir` (parents, ok if exists) and starts a daemon
  thread named `"screenshotter"`.
- `stop() -> None` — sets the stop event and joins the thread with timeout
  `interval + 2.0` seconds.
- `count: int` — number of screenshots successfully written so far.
- `errors: int` — number of failed/timed-out captures or conversions so far.
- Other attributes (`out_dir`, `pid`, `app_name`, `interval`, `fmt`, `resolution`,
  `jpeg_quality`, `whole_screen_fallback`) are stored as given (after the coercions
  above).

There is no return value from the capture loop; results are observed via `count` /
`errors` (read by `session.py` for status, e.g. `screenshots` and `screenshot_errors`).

### Logging / stdout-stderr
All diagnostics go through `log = logging.getLogger(__name__)` (warnings/exceptions),
never `print`. This honors the architecture hard constraint that stdout is reserved
for the MCP transport.


### Event hook (M0b, feature #26)

`Screenshotter` accepts an optional `emit=None` keyword (an `EventBus.publish`-shaped
callable, normally `CaptureSession.events.publish`). When set, it emits
`screenshot_taken` {path,count} / `screenshot_error` {errors}. Publishing never raises/blocks; with `emit=None` the component is
silent and behaves exactly as before. See [events.md](events.md).

## Behavior

Per tick (`_capture_once`):
1. Record the timestamp `ts = now()` (used for the filename, so the filename reflects
   tick start, not completion).
2. Resolve the target window id via `_resolve_window_id`:
   - If both `pid` and `app_name` are `None`, return `None` (no specific target).
   - Otherwise call `self._finder.primary(pid, app_name)` (the platform `WindowFinder`). If a
     window is found, cache its `window_id` in `self._last_wid` and return it.
   - If no window is found right now, return the cached `self._last_wid` (which may be
     `None` on the first tick). This keeps targeting a window that has temporarily
     left the on-screen list (e.g. went fullscreen onto its own Space); the grabber
     still targets it by id cross-Space.
3. Skip-tick policy: if `wid is None` AND a `pid`/`app_name` was requested AND
   `whole_screen_fallback` is `False`, return without capturing (a deliberate skip, not an
   error). Otherwise proceed with `wid` (which may be `None` ⇒ whole screen).
4. Build the final path `out_dir/<fs_stamp(ts)>.<ext>` where `ext = _extension(fmt)`
   (jpg/jpeg both -> `jpg`).
5. Delegate the pixel capture to the platform grabber:
   `ok = self._grabber.capture(wid, final, fmt=self.fmt, resolution=self.resolution,
   jpeg_quality=self.jpeg_quality, timeout=max(5.0, interval+5.0))`. `wid is None` captures the
   whole screen; otherwise that window. If `ok`, `count += 1`, else `errors += 1` — exactly one
   of count/errors per non-skipped tick.

The grabber implements the encode/resize/format details per OS (these used to live here):
- **macOS** (`platform/macos.py:MacScreenGrabber`): a fast path
  `screencapture -x -o -t <sc_fmt> <target...> <final>` when no post-processing is needed
  (`needs_post = resolution is not None or (jpg/jpeg and jpeg_quality is not None)`), otherwise a
  capture to a temp `<stamp>.tmp.png` (name deliberately not dot-prefixed; `screencapture` refuses
  hidden paths) followed by `sips` for resize (`-z <height> <width>` from `fit_box`, reading the
  PNG IHDR via `_png_size`) and/or format/quality conversion (`-s format <sips_fmt>`,
  `-s formatOptions <quality>`). `screencapture` exits 0 even on failure, so success is verified by
  the output file existing; a `max(5, interval+5)s` timeout bounds each tool. Returns a bool.
- **Windows** (`platform/windows.py:Win32ScreenGrabber`): GDI `BitBlt` (whole screen) or
  `PrintWindow`/`BitBlt` (a window) into a bitmap, then GDI+ scales (`fit_box`, HighQualityBicubic)
  and encodes to the requested format (JPEG quality via an EncoderParameter). Returns a bool. No
  temp file.

Scheduling loop (`_run`, `screenshots.py:234-251`):
1. `next_t = now()`.
2. While not stopped: run `_capture_once` inside a try/except that catches every
   `Exception`, increments `errors`, and logs via `log.exception` — the loop never dies
   (architecture hard constraint "Capture loops never die").
3. Advance `next_t += interval`; sleep `next_t - now()` so shots land on an absolute
   grid even when a capture takes a non-trivial fraction of the interval.
4. If the loop fell behind (`sleep_for < 0`), skip the missed ticks
   (`missed = int((-sleep_for) // interval) + 1`), advance `next_t` by `missed *
   interval`, and fire on the next on-grid tick.
5. Wait using `self._stop.wait(sleep_for)` so a stop request interrupts the sleep
   promptly.

## Invariants & constraints

- **Loop never dies.** `_run` catches all exceptions per tick (architecture: "Capture
  loops never die"). One bad tick increments `errors` and continues.
- **stdout is sacred.** No `print`; all output is via the `logging` module to stderr
  (architecture hard constraint).
- **Grid scheduling.** Shots are scheduled on absolute ticks (`next_t += interval`),
  not `sleep(interval)` after each capture, so timestamps stay on a regular grid and
  drift does not accumulate. Missed ticks are skipped rather than queued.
- **rc=0 is not success.** `screencapture` exits 0 even when it fails to write a file;
  success is verified by the expected output file actually existing (`_run_cmd`,
  `:178`).
- **Never upscale.** `_fit` clamps `scale` to `min(..., 1.0)`, so the resize fits inside
  the bounding box and never enlarges the source (and each dimension is `max(1, ...)`).
- **Aspect ratio preserved.** `_fit` uses a single uniform `scale = min(bw/sw, bh/sh,
  1.0)` for both axes.
- **Temp file naming.** The intermediate file must not begin with a dot, because
  `screencapture` refuses hidden destination paths. The temp file is always removed
  (`unlink(missing_ok=True)`) on both success and failure of the post-process path.
- **Window targeting is per-tick.** The window id is re-resolved every tick so the
  shooter survives window recreation; `_last_wid` is the cross-Space/fullscreen
  fallback.
- **Interval floor.** `interval` is never below `0.05s`.
- **Cross-platform via delegation.** This scope is OS-neutral; the pixel capture is the platform
  grabber's job (macOS `screencapture`/`sips`; Windows GDI+). The rc=0-is-not-success check, the
  never-upscale (`fit_box`) and temp-file invariants above describe the **macOS backend**
  (`platform/macos.py`); the Windows backend (`platform/windows.py`) satisfies the same
  `capture(...) -> bool` contract via GDI+. See [platform-abstraction.md](platform-abstraction.md).
- **Filenames use `fs_stamp`.** Per architecture naming convention, filenames use
  `util.fs_stamp()` (`:` replaced by `-`).

## Failure modes & handling

This scope's accounting (in `_capture_once`/`_run`):
- **Per tick, exactly one of `count`/`errors`.** The grabber returns a `bool`;
  `_capture_once` does `count += 1` on `True`, else `errors += 1`.
- **Target requested but not resolvable and fallback disabled.** `_capture_once` returns
  early (a deliberate skip), counting neither `count` nor `errors`.
- **Any unexpected exception in a tick.** Caught in `_run`: `errors += 1`,
  `log.exception("screenshot tick failed")`, the loop continues (never dies).

The grabber-internal failure handling (which returns the `False` this scope counts as one
error) differs per backend and is owned by `platform/`:
- **macOS** (`platform/macos.py:MacScreenGrabber`): each `screencapture`/`sips` runs with
  `timeout = max(5.0, interval+5.0)` (on `TimeoutExpired` → `False`); a non-zero exit OR a
  missing output file → `False` (the rc=0-but-no-file quirk is caught by the file-existence
  check); a failed temp capture unlinks the temp and returns `False`; an unreadable PNG
  header (`_png_size`) omits the resize `-z` flags but still runs sips for format/quality.
- **Windows** (`platform/windows.py:Win32ScreenGrabber`): an unsupported `fmt`, no device
  context, a GDI+ status ≠ Ok, a failed scale step, or any exception → `False` (logged);
  all GDI/GDI+ resources are freed in a `finally` so there is no leak on failure.

## Outputs / artifacts

- One image file per successful tick in `out_dir` (which is
  `<session-dir>/screenshots/` when driven by `session.py`).
- Filename: `<fs_stamp(ts)>.<ext>`, e.g. `2026-06-07T09-47-01.250Z.png`. `ext` is the
  format with jpg/jpeg collapsed to `jpg` (`_extension`).
- Formats: png (default), jpg/jpeg (written as `.jpg`), tiff, gif, bmp.
- Transient artifact (macOS resize/convert path only): `<fs_stamp(ts)>.tmp.png`, always
  removed before the next tick. The Windows GDI+ backend writes the final image directly
  with no temp file.

## Configuration

Constructor parameters (no environment variables are read by this scope):
- `out_dir: Path` — output directory (created on `start`).
- `pid: int | None = None`, `app_name: str | None = None` — window target selector
  (app_name is a case-insensitive substring match, handled by the platform `WindowFinder`).
  If both are `None`, capture is whole-screen.
- `interval: float = 1.0` — seconds between shots; floored at `0.05`.
- `fmt: str = "png"` — one of `VALID_FORMATS`; lowercased.
- `resolution: tuple[int, int] | None = None` — bounding box `(w, h)`.
- `jpeg_quality: int | None = None` — forwarded to the backend (macOS `sips -s
  formatOptions`; Windows GDI+ JPEG quality EncoderParameter, clamped 0–100); only applied
  when `fmt` is jpg/jpeg. This scope does not range-check it (the `server.py` docstring
  documents 0-100).
- `whole_screen_fallback: bool = True` — whether to capture the whole screen when a
  target was requested but no window id is available.

Upstream surface (for reference; owned by `server.py`/`session.py`): the MCP tool
exposes `screenshot_interval`, `screenshot_format`, `screenshot_resolution`
(the `"WxH"` / `"WxH/fmt"` string parsed via `parse_resolution`), `screenshot_jpeg_quality`,
and `capture_screenshots`. A `/fmt` suffix in `screenshot_resolution` overrides
`screenshot_format` (applied in `session.py`, not here).

## Known limitations / open items

- This scope is now cross-platform (delegates to `platform/`); the items below about
  `screencapture`/`sips`/`_png_size`/Screen Recording are **macOS-backend** specifics.
- `jpeg_quality` is not validated in this scope; out-of-range values are forwarded to the
  backend (macOS `sips` as-is; Windows GDI+ clamps to 0–100).
- macOS `_png_size` only understands PNG IHDR; the post-process path always captures to PNG
  first, so this is fine in practice, but a non-PNG temp would silently skip resizing.
- macOS requires Screen Recording permission for the host process; denial surfaces as
  `screencapture` producing no file (counted as an error), not a distinct message. On
  Windows, capturing real app-window content requires the interactive desktop (`WinSta0`);
  see [platform-abstraction.md](platform-abstraction.md).
- `_last_wid` is never invalidated; if the cached window id becomes stale and the app is
  gone, ticks would keep targeting a dead id (the grabber would fail and be counted as
  errors) until a new window is resolved. Whether this is a concern in practice is uncertain.
- Whole-screen fallback captures the entire main display; multi-display selection is not
  configurable here.

## Tests

`tests/smoke.py` covers this scope:
- `test_parse_resolution` (`smoke.py:119-128`): asserts `parse_resolution("1280x720/jpg")
  == (1280, 720, "jpg")`, `"640x480" -> (640, 480, None)`, `None -> None`, and that
  `"bad"`, `"10x"`, `"1x2x3"`, `"axb"`, `"0x0"` all raise `ValueError`.
- `test_launch_mode`: runs a launch-mode session with `screenshot_interval=0.4` and
  `screenshot_resolution="640x480/jpg"`, then checks the final status reports
  `screenshots >= 2` and that the number of `*.jpg` files in the `screenshots/` dir equals
  the reported screenshot count (verifying the jpg format and the resize+encode path end to
  end — on the running OS's grabber; this passes 20/20 on Windows via GDI+).

Gaps / suggested additions: the whole-screen fallback path, `whole_screen_fallback=False`
skip behavior, the `_last_wid` cross-Space caching, the grid-catch-up logic in `_run`,
and the rc=0-but-no-file quirk are not directly unit-tested; they are only exercised
indirectly (or not at all) by the smoke test.
