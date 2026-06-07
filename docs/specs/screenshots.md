# Spec: Screenshots

_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose

Periodically grab timestamped screenshots of a target window (or the whole screen)
using the macOS `screencapture` CLI, on a fixed time grid, into an output directory.
Output format and resolution are configurable. The component runs on its own daemon
thread and is one of the capture components owned by `CaptureSession`
(see `docs/architecture.md` module map). It targets a specific app window when a
`pid`/`app_name` is given, re-resolving the window id every tick so it keeps working
across window recreation, Space switches, and fullscreen transitions.

## Files

- `src/capture_mcp/screenshots.py` — the entire scope: `parse_resolution`, format
  helpers, PNG size/fit helpers, and the `Screenshotter` class.
- Dependencies it uses (not owned by this scope): `src/capture_mcp/windows.py`
  (`primary_window`), `src/capture_mcp/util.py` (`now`, `fs_stamp`).

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

## Behavior

Per tick (`_capture_once`, `screenshots.py:189-230`):
1. Record the timestamp `ts = now()` (used for the filename, so the filename reflects
   tick start, not completion).
2. Resolve the target window id via `_resolve_window_id` (`:143-152`):
   - If both `pid` and `app_name` are `None`, return `None` (no specific target).
   - Otherwise call `windows.primary_window(pid, app_name)`. If a window is found,
     cache its `window_id` in `self._last_wid` and return it.
   - If no window is found right now, return the cached `self._last_wid` (which may be
     `None` on the first tick). This keeps targeting a window that has temporarily
     left the on-screen list (e.g. went fullscreen onto its own Space); `screencapture
     -l` still grabs it cross-Space.
3. Compute target flags via `_target_args(wid)` (`:154-161`):
   - `wid is not None` -> `["-l", str(wid)]` (window capture).
   - `wid is None` and a `pid`/`app_name` was requested:
     - if `whole_screen_fallback` is `False` -> return `None` => the tick is skipped
       (no capture this tick).
     - else fall through to whole-screen.
   - Otherwise -> `[]` (whole-screen capture).
4. Build the final path `out_dir/<fs_stamp(ts)>.<ext>` where `ext = _extension(fmt)`
   (jpg/jpeg both -> `jpg`).
5. Decide if post-processing is needed (`needs_post`): true if `resolution is not None`,
   or if `fmt` is jpg/jpeg AND `jpeg_quality is not None`.
6. Fast path (`not needs_post`): run
   `screencapture -x -o -t <sc_fmt> <target...> <final>` and, if it produces the file,
   increment `count`. `-x` mutes the shutter sound; `-o` omits the window shadow;
   `_sc_format` maps jpg/jpeg -> `jpg`.
7. Post-process path (`needs_post`):
   a. Capture to a temp PNG `out_dir/<fs_stamp(ts)>.tmp.png` via
      `screencapture -x -o -t png <target...> <tmp>`. (The temp name deliberately does
      not start with a dot; `screencapture` refuses hidden destination paths.)
   b. If the temp capture fails, unlink the temp file and return.
   c. Build a `sips` command:
      - If `resolution` is set, read the PNG pixel size from the IHDR header
        (`_png_size`); if readable, compute the fitted target via `_fit` and append
        `-z <height> <width>` (sips takes height then width).
      - Append `-s format <sips_fmt>` (`_sips_format` maps jpg/jpeg -> `jpeg`).
      - If jpg/jpeg and `jpeg_quality` is set, append `-s formatOptions <quality>`.
      - Append `<tmp> --out <final>`.
   d. Run sips; always unlink the temp PNG afterward; increment `count` only if the
      final file was produced.

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
- **macOS-only.** Relies on `screencapture`, `sips`, and Quartz (via `windows.py`)
  (architecture "Platform").
- **Filenames use `fs_stamp`.** Per architecture naming convention, filenames use
  `util.fs_stamp()` (`:` replaced by `-`).

## Failure modes & handling

All handled in `_run_cmd` (`screenshots.py:163-187`) and `_capture_once`:
- **Tool timeout.** Each `screencapture`/`sips` invocation runs with
  `timeout = max(5.0, interval + 5.0)`. On `TimeoutExpired`: `errors += 1`, log a
  warning, return `False` (skip this tick). This keeps a hung tool from wedging the
  loop and blocking shutdown.
- **Non-zero exit or missing output.** If `returncode != 0` OR the expected file does
  not exist: `errors += 1`, log a warning including rc, the `wrote=<bool>` flag, and
  stderr (or stdout if stderr empty). Returns `False`.
- **screencapture rc=0 but no file (the quirk).** Treated as failure because the file
  existence check (`not expected.exists()`) fails, even with rc 0.
- **Temp capture fails (post-process path).** Temp file unlinked, tick aborted, `count`
  not incremented (errors already counted in `_run_cmd`).
- **PNG header unreadable.** `_png_size` returns `None` on a non-PNG signature, short
  read, or `OSError`; the resize `-z` flags are then omitted and sips still runs for
  format/quality conversion at the captured (native) resolution. (No explicit error is
  raised for this case.)
- **Target requested but not on screen and fallback disabled.** `_target_args` returns
  `None`; `_capture_once` returns early — this is a deliberate skip, not counted as an
  error.
- **Any unexpected exception in a tick.** Caught in `_run`, `errors += 1`,
  `log.exception("screenshot tick failed")`, loop continues.

## Outputs / artifacts

- One image file per successful tick in `out_dir` (which is
  `<session-dir>/screenshots/` when driven by `session.py`).
- Filename: `<fs_stamp(ts)>.<ext>`, e.g. `2026-06-07T09-47-01.250Z.png`. `ext` is the
  format with jpg/jpeg collapsed to `jpg` (`_extension`).
- Formats: png (default), jpg/jpeg (written as `.jpg`), tiff, gif, bmp.
- Transient artifact during the post-process path: `<fs_stamp(ts)>.tmp.png`, always
  removed before the next tick (on success and failure).

## Configuration

Constructor parameters (no environment variables are read by this scope):
- `out_dir: Path` — output directory (created on `start`).
- `pid: int | None = None`, `app_name: str | None = None` — window target selector
  (app_name is a case-insensitive substring match, handled in `windows.py`). If both
  are `None`, capture is whole-screen.
- `interval: float = 1.0` — seconds between shots; floored at `0.05`.
- `fmt: str = "png"` — one of `VALID_FORMATS`; lowercased.
- `resolution: tuple[int, int] | None = None` — bounding box `(w, h)`.
- `jpeg_quality: int | None = None` — passed verbatim to `sips -s formatOptions`; only
  applied when `fmt` is jpg/jpeg. The code does not range-check it (the docstring in
  `server.py` documents 0-100).
- `whole_screen_fallback: bool = True` — whether to capture the whole screen when a
  target was requested but no window id is available.

Upstream surface (for reference; owned by `server.py`/`session.py`): the MCP tool
exposes `screenshot_interval`, `screenshot_format`, `screenshot_resolution`
(the `"WxH"` / `"WxH/fmt"` string parsed via `parse_resolution`), `screenshot_jpeg_quality`,
and `capture_screenshots`. A `/fmt` suffix in `screenshot_resolution` overrides
`screenshot_format` (applied in `session.py`, not here).

## Known limitations / open items

- macOS-only; cross-platform would require a new backend behind the same interface
  (architecture "Platform").
- `jpeg_quality` is not validated in this scope; an out-of-range value is forwarded to
  `sips` as-is.
- `_png_size` only understands PNG IHDR; the post-process path always captures to PNG
  first, so this is fine in practice, but a non-PNG temp would silently skip resizing.
- Requires Screen Recording permission for the host process; permission denial would
  surface as `screencapture` producing no file (counted as an error) rather than a
  distinct message — not specifically detected here.
- `_last_wid` is never invalidated; if the cached window id becomes stale and the app is
  gone, ticks would keep targeting a dead id (`screencapture -l` would fail and be
  counted as errors) until a new window is resolved. Whether this is a concern in
  practice is uncertain.
- Whole-screen fallback captures the entire main display; multi-display selection is not
  configurable here.

## Tests

`tests/smoke.py` covers this scope:
- `test_parse_resolution` (`smoke.py:119-128`): asserts `parse_resolution("1280x720/jpg")
  == (1280, 720, "jpg")`, `"640x480" -> (640, 480, None)`, `None -> None`, and that
  `"bad"`, `"10x"`, `"1x2x3"`, `"axb"`, `"0x0"` all raise `ValueError`.
- `test_launch_mode` (`smoke.py`, around `:48-59`): runs a launch-mode session with
  `screenshot_interval=0.4` and `screenshot_resolution="640x480/jpg"`, then checks the
  final status reports `screenshots >= 2` and that the number of `*.jpg` files in the
  `screenshots/` dir equals the reported screenshot count (verifying the jpg format and
  the resize/convert post-process path end to end).

Gaps / suggested additions: the whole-screen fallback path, `whole_screen_fallback=False`
skip behavior, the `_last_wid` cross-Space caching, the grid-catch-up logic in `_run`,
and the rc=0-but-no-file quirk are not directly unit-tested; they are only exercised
indirectly (or not at all) by the smoke test.
