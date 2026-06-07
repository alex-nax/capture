# Spec: Windows (macOS Quartz window discovery)
_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

> **Note (platform abstraction):** despite the filename, this module is the **macOS** window-discovery
> implementation. It is now reached through the platform abstraction: `platform/macos.py:MacWindowFinder`
> wraps `find_windows`/`primary_window` and maps each `WindowInfo` to a platform-neutral `WindowRef`
> (`window_id`/`pid`/`app_name`/`title`/`width`/`height`). The **Windows-OS** equivalent lives in
> `platform/windows.py:Win32WindowFinder` (`EnumWindows`). See [platform-abstraction.md](platform-abstraction.md).

## Purpose
Map a target process (by pid) or application (by case-insensitive name substring) to its
on-screen `CGWindowID`(s) so that the screenshotter can grab just that window via
`screencapture -l <id>`. This is the bridge between an abstract capture target
(`pid` / `app_name`) and the concrete numeric window id that `screencapture` understands.
The module also resolves a pid from an app name when a session is in attach-by-app mode.

## Files
- `src/capture_mcp/windows.py` — the entire scope (the macOS Quartz discovery).

Consumers (not part of this scope, listed for context) — both now go through the platform finder,
not this module directly:
- `src/capture_mcp/platform/macos.py` — `MacWindowFinder.find` calls `find_windows` and maps to `WindowRef`.
- `src/capture_mcp/screenshots.py` — `Screenshotter._resolve_window_id` calls `self._finder.primary(...)`.
- `src/capture_mcp/session.py` — `CaptureSession` calls `platform.current().window_finder.primary(app_name=...)` to derive a pid/title in attach-by-app mode.

## Public contract
This is an internal Python module (no CLI, no MCP tool surface). Its public symbols:

- `@dataclass WindowInfo` (lines 12-24) with fields:
  - `window_id: int` — the `CGWindowID` (`kCGWindowNumber`).
  - `owner_pid: int` — owning process id (`kCGWindowOwnerPID`).
  - `owner_name: str` — owning app name (`kCGWindowOwnerName`), `""` if absent.
  - `title: str` — window title (`kCGWindowName`), `""` if absent.
  - `width: int`, `height: int` — from `kCGWindowBounds` (`Width`/`Height`), `0` if absent.
  - `layer: int` — window layer (`kCGWindowLayer`), `0` for normal windows.
  - `area` (read-only `@property`, lines 22-24) — `width * height`.

- `find_windows(pid: int | None = None, app_name: str | None = None) -> list[WindowInfo]`
  (lines 69-81). Returns matching windows, **largest area first**. Empty list if none match.
  Both args default to `None`. If both are `None`, every layer-0 on-screen window is returned
  (sorted largest-first), since no pid/name filter is applied.

- `primary_window(pid: int | None = None, app_name: str | None = None) -> WindowInfo | None`
  (lines 84-86). Returns `find_windows(...)[0]` (the largest match) or `None` if there are no matches.

Internal helpers (underscore-prefixed, not part of the stable contract):
- `_list_windows(on_screen_only: bool = True) -> list[WindowInfo]` (lines 27-52).
- `_match(wins, pid, needle) -> list[WindowInfo]` (lines 55-66).

Note on `app_name` matching: callers pass the raw app name; `find_windows` lowercases it once
(`needle = app_name.lower()`) and matches with `needle in w.owner_name.lower()` — a
**case-insensitive substring** test, not an exact match.

## Behavior
For `find_windows(pid, app_name)` (lines 69-81):
1. Compute `needle = app_name.lower()` if `app_name` is truthy, else `None`.
2. Call `_list_windows(on_screen_only=True)` then `_match(...)` to get on-screen, layer-0 matches sorted largest-first.
3. If that list is non-empty, return it immediately (on-screen windows are preferred).
4. Otherwise call `_list_windows(on_screen_only=False)` and `_match(...)` again over the **full** window list, and return that result (possibly empty). This is the cross-Space / fullscreen fallback.

For `_list_windows(on_screen_only)` (lines 27-52):
1. Lazily import Quartz symbols inside the function (so the module imports on non-Quartz hosts).
2. Pick `option = kCGWindowListOptionOnScreenOnly` when `on_screen_only` is `True`, else `kCGWindowListOptionAll`.
3. Call `CGWindowListCopyWindowInfo(option, kCGNullWindowID)`; coerce a falsy/None result to `[]`.
4. For each raw window dict, read `kCGWindowBounds` (defaulting to `{}`) and build a `WindowInfo`, coercing every field with `int(...)`/`str(...)` and defaulting missing values (ids/dimensions -> `0`, names/titles -> `""`).

For `_match(wins, pid, needle)` (lines 55-66):
1. Drop any window with `layer != 0` (non-normal windows: menu bar, Dock, overlays, shadows) or `width < 1` or `height < 1` (zero-size windows).
2. If `pid is not None`, keep only windows whose `owner_pid == pid`.
3. If `needle is not None`, keep only windows where `needle in w.owner_name.lower()`.
4. Sort the survivors by `area` descending (`key=lambda w: w.area, reverse=True`) and return.

For `primary_window` (lines 84-86): call `find_windows`, return `wins[0]` if any, else `None`.

## Invariants & constraints
- **Layer-0 only.** Only normal application windows (`layer == 0`) are ever returned; system/UI layers are filtered in `_match`.
- **Largest-first ordering.** Results are always sorted by pixel area, descending. `primary_window` therefore returns the largest matching window.
- **No zero-size windows.** Windows with `width < 1` or `height < 1` are excluded.
- **On-screen preferred, all-windows as fallback.** `find_windows` only consults the full (all-Spaces) window list when the on-screen query yields nothing, so it can still target a window on another Space/Desktop or in fullscreen — `screencapture -l` captures by id regardless of Space.
- **Lazy Quartz import.** Quartz is imported inside `_list_windows`, not at module top, so importing `capture_mcp.windows` succeeds even where Quartz is unavailable; the dependency is only required at call time.
- **No I/O / no blocking side effects beyond the Quartz query.** The module reads window metadata only; it does not capture, write files, or spawn processes. Per `docs/architecture.md`, components do not know about each other or the MCP layer — this module exposes pure helpers consumed by `screenshots.py`/`session.py`.
- **macOS-only.** Per the Platform section of `docs/architecture.md`, this is a macOS Quartz backend; cross-platform support would mean a new `windows` backend behind the same `find_windows`/`primary_window` interface.

## Failure modes & handling
- **Quartz unavailable / not importable at call time:** the lazy `from Quartz import (...)` inside `_list_windows` raises `ImportError` (or platform error) and propagates to the caller. The module does not catch this. Callers (`screenshots.py`, `session.py`) run on background threads whose loops absorb exceptions per the architecture's "capture loops never die" constraint, but `windows.py` itself does no catching. (Note: module-level *import* does not fail, only the call.)
- **`CGWindowListCopyWindowInfo` returns `None`/empty:** coerced to `[]` (line 37); `find_windows` returns `[]` and `primary_window` returns `None`. No exception.
- **Missing keys in a window dict:** every field uses `.get(..., default)` plus `int()`/`str()` coercion, so absent `kCGWindowName`, `kCGWindowBounds`, etc. yield safe defaults (`""`, `0`) rather than `KeyError`.
- **No match for the given pid/app_name:** both the on-screen and all-windows passes return empty; `find_windows` -> `[]`, `primary_window` -> `None`. Consumer behavior on `None`:
  - `screenshots.py` keeps using the last known window id (`_last_wid`) so `screencapture -l` can still grab a window that has moved off the current Space; it does not silently fall back to whole-screen.
  - `session.py` (attach-by-app) records the note `"no on-screen window found for app <name>"` and proceeds without a resolved pid.
- **Non-integer Quartz values:** `int(...)`/`str(...)` coercion would raise `ValueError`/`TypeError` if Quartz ever returned a non-numeric/non-coercible value; this is not defended against, but in practice Quartz returns numeric `CFNumber`s for these keys.

## Outputs / artifacts
None. This scope writes no files and produces no on-disk artifacts. It returns in-memory `WindowInfo` objects only. (Artifacts such as screenshots are written by `screenshots.py`, which is out of scope.)

## Configuration
No environment variables and no module-level configuration. All behavior is driven by call arguments:
- `find_windows` / `primary_window`: `pid: int | None = None`, `app_name: str | None = None`.
- `_list_windows`: `on_screen_only: bool = True`.

There is no tunable for sort order, layer filter, or minimum size — these are hard-coded in `_match`.

## Known limitations / open items
- **`app_name` is a substring match**, so an unspecific name (e.g. `"Code"`) may match multiple apps; `primary_window` then returns whichever has the largest window, which may not be the intended app. Documented behavior, not a bug, but worth noting for callers.
- **Layer-0 assumption.** Apps whose target surface is not a layer-0 window (e.g. certain overlay-only or borderless utilities) will not be discovered.
- **Fullscreen/Space handling is best-effort.** The all-windows fallback finds the window, but the consumer must keep using a previously resolved id when a window leaves the current Space (handled in `screenshots.py`, not here).
- **No defense against malformed Quartz return types** beyond `int()`/`str()` coercion (see Failure modes).
- **No multi-display semantics** in this module — it does not reason about which display a window is on; selection is purely by area.
- **Untested directly** (see Tests).

## Tests
- There is currently **no direct test** of `windows.py`. `tests/smoke.py` deliberately exercises only paths that work without special permissions and does not import or call `windows` (confirmed: `smoke.py` imports `audio`, `screenshots.parse_resolution`, `asr.base`, and `server`, but not `windows`). Window discovery depends on a live windowing session and Quartz, which the hermetic smoke test avoids.
- Recommended verification (not yet implemented):
  - Unit test `_match` with synthetic `WindowInfo` lists to assert: layer filtering, zero-size exclusion, pid filter, case-insensitive substring name filter, and largest-first ordering.
  - Unit test `find_windows`' fallback by monkeypatching `_list_windows` to return matches only for `on_screen_only=False`, asserting the all-windows list is used when the on-screen pass is empty.
  - Optional integration/manual check on a real macOS session: `primary_window(app_name="Finder")` returns a non-`None` `WindowInfo` with `layer == 0` and positive `area`.
