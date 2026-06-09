# Spec: process-logs
_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose
Launch a target process and tee its `stdout`/`stderr` to timestamped log files so a capture session has a durable record of process output. Implemented by `ProcessCapture` in `src/capture_mcp/core/proc.py`. This scope is **launch-mode only**: it spawns the child itself so it can attach to the child's pipes. When `capture_mcp` attaches to an already-running pid, log capture is skipped entirely because the kernel gives no handle on a pre-existing process's stdout/stderr (see Known limitations).

## Files
- `src/capture_mcp/core/proc.py` — the entire scope (`ProcessCapture` class).
- Depends on `src/capture_mcp/core/util.py` for `iso` and `now` (timestamp helpers). `util.py` is owned by another scope and is only consumed here.

## Public contract
Class `ProcessCapture` (proc.py:27).

Constructor — `__init__(self, command, out_dir, *, cwd=None)` (proc.py:28):
- `command: str | list[str]` — required. If a `str`, it is tokenized with `util.split_command` at start time (POSIX `shlex.split`; Windows `CommandLineToArgvW`, the OS tokenizer, so backslash paths are not mangled); if a `list[str]`, it is copied verbatim (no shell parsing).
- `out_dir: Path` — required. Directory the four artifacts are written into; created (with parents) at `start()`.
- `cwd: str | None = None` — keyword-only. Working directory passed to `subprocess.Popen`. `None` means inherit the server's cwd.

Attributes / properties:
- `pid -> int | None` (property, proc.py:40): the child pid, or `None` before `start()`.
- `lines: int` (proc.py:38): running count of lines written to the merged `output.log`. Starts at 0. Note this counts only merged-log writes that occurred while the merged file was open (see Behavior step 6). Raw-log lines are not separately counted.
- `command`, `out_dir`, `cwd`: stored as passed.
- Internal (not part of the public contract): `proc`, `_threads`, `_merged_lock`, `_merged`, `_closed`.

Methods:
- `start(self) -> int` (proc.py:44): creates `out_dir`, spawns the child, opens `output.log`, starts the two pump threads, and returns `self.proc.pid`. Raises on failure (after tearing down the child — see Failure modes).
- `poll(self) -> int | None` (proc.py:113): returns the child's exit code, or `None` if still running, or `None` if never started. Thin wrapper over `Popen.poll`.
- `stop(self, timeout: float = 5.0) -> int | None` (proc.py:116): terminates the child (graceful then forced), joins the pumps, closes the merged log, and returns the exit code. Returns `None` if never started.

Return / type notes:
- `start()` returns a non-null `int` pid on success only.
- `stop()`/`poll()` return `None` when `self.proc` is falsy (never started).

This scope writes no MCP/stdout protocol output of its own; all diagnostics go through the module logger `log = logging.getLogger(__name__)` (proc.py:24), consistent with the "stdout is sacred" constraint in docs/architecture.md.


### Event hook (M0b, feature #26)

`ProcessCapture` accepts an optional `emit=None` keyword (an `EventBus.publish`-shaped
callable, normally `CaptureSession.events.publish`). When set, it emits
`log_line` {stream,line} per merged line. Publishing never raises/blocks; with `emit=None` the component is
silent and behaves exactly as before. See [events.md](events.md).

## Behavior
1. **start() — prepare directory and args**: `out_dir.mkdir(parents=True, exist_ok=True)`. `command` is normalized to a list: `util.split_command(command)` if a string (POSIX `shlex.split` / Windows `CommandLineToArgvW`), else `list(command)`.
2. **Spawn child** (proc.py:48-55): `subprocess.Popen(args, cwd=self.cwd, stdout=PIPE, stderr=PIPE, bufsize=1, text=True)`. `text=True` makes pipes yield decoded `str` lines; `bufsize=1` requests line buffering.
3. **Open merged log and pumps under teardown protection** (proc.py:58-68): after the child exists, a `try/except` guards the rest of setup, because any failure here would otherwise leave a child whose pipes nobody is draining (deadlock on a full pipe). Inside the try: open `output.log` for writing (`buffering=1`), then `_spawn_pump` for stdout (`stdout.log`, tag `out`) and stderr (`stderr.log`, tag `err`).
4. **On setup failure** (proc.py:62-68): log the exception, call `_teardown_child()`, close+null `self._merged` if it was opened, then re-raise.
5. **On setup success** (proc.py:69-70): log `launched pid=... : args` at INFO and return the pid.
6. **Pump thread loop** (`_spawn_pump.pump`, proc.py:88-107): each thread opens its raw log (`stdout.log`/`stderr.log`, `buffering=1`). For every line read from the stream it:
   - writes the line verbatim to the raw log;
   - builds a merged line `f"{iso(now())} [{tag}] {line...}"` where a trailing newline is ensured (`line if line.endswith("\n") else line + "\n"`);
   - acquires `_merged_lock`, and if `self._merged` is open and `not self._closed`, writes the stamped line and increments `self.lines`.
   On EOF the `finally` closes the raw log. Threads are `daemon=True`, named `pump-out`/`pump-err`, started immediately, and appended to `self._threads`.
7. **poll()** (proc.py:113-114): returns `self.proc.poll()` if `self.proc` else `None`.
8. **stop() — terminate child** (proc.py:116-129): if no `self.proc`, return `None`. If still running, `terminate()` then `wait(timeout=timeout)` (default 5.0s); on `TimeoutExpired`, `kill()` then `wait(timeout=2.0)` (a second `TimeoutExpired` is swallowed). Capture `rc = self.proc.poll()`.
9. **stop() — drive pumps to EOF** (proc.py:130-136): close `self.proc.stdout`/`stderr` (if open) so the pump `for line in stream` loops reach EOF and return. Per-stream close errors are swallowed.
10. **stop() — join pumps** (proc.py:137-138): join each pump thread with `timeout=2.0`.
11. **stop() — close merged log** (proc.py:139-144): under `_merged_lock`, set `self._closed = True`, then flush + close + null `self._merged`. Setting `_closed` before/while holding the lock prevents a still-running pump from writing into a closed file.
12. **stop() returns** the captured exit code `rc` (proc.py:145).

## Invariants & constraints
- **Launch-mode only.** This scope owns the child (it calls `Popen`), which is the only way to get the pipes. Attach mode does not instantiate log capture. (proc.py module docstring, lines 9-11.)
- **Never leave an undrained child.** From the moment `Popen` returns, any failure path must tear the child down; otherwise the child can deadlock writing into a full pipe with no reader. Enforced by the `try/except` in `start()` (proc.py:56-68) and the "drain without raw log" fallback in the pump (proc.py:92-97). Matches docs/architecture.md "Roll back on partial start."
- **Capture loops never die.** Pump threads catch their own file-open failure and still drain the stream; they must not let one bad write abort the session. Matches docs/architecture.md "Capture loops never die." (Note: a write failure mid-loop is NOT individually caught — see Failure modes.)
- **Reader-before-files on shutdown.** `stop()` joins the pump threads before flushing/closing the merged log, and uses `_merged_lock` + `_closed` to avoid a close-vs-write race — the same ordering principle documented for `audio.py` in docs/architecture.md ("Reader-before-files on shutdown").
- **No stdout pollution.** All output from this scope goes to the logger (stderr), never `print` to stdout. Matches docs/architecture.md "stdout is sacred in server.py."
- **Timestamp source.** Merged-line timestamps use `util.iso(util.now())` for content, per docs/architecture.md naming conventions (`iso` for content). Filenames in this scope are fixed strings, so `fs_stamp` is not used here.
- **Thread safety of the merged file.** Only `self.lines` and writes to `self._merged` are guarded by `_merged_lock`; the two raw logs are each touched by a single owning thread and need no lock.

## Failure modes & handling
- **`Popen` fails (bad command, missing executable, bad cwd):** `start()` raises before the `try` block; no child exists, no files opened, nothing to roll back. Caller (`session.py`) sees the exception.
- **`util.split_command` fails (e.g. unbalanced quotes in a POSIX `str` command):** the underlying `shlex.split` raises `ValueError` from `start()` before `Popen`; same as above. (On Windows, `CommandLineToArgvW` does not raise on unbalanced quotes — it tokenizes per Windows rules.)
- **Opening `output.log` or spawning a pump fails after launch:** caught at proc.py:62; logs `ProcessCapture.start failed after launch; tearing down child`, calls `_teardown_child()` (kill + wait(2.0) + close pipe fds, errors swallowed — proc.py:72-86), closes `self._merged` if opened, and re-raises.
- **A pump cannot open its raw log:** caught at proc.py:92; logs `could not open <path>; draining <tag> without raw log`, then drains the stream (`for _ in stream: pass`) and returns. The merged log loses this stream's lines, but the child never blocks.
- **A write to the raw or merged log fails mid-loop (e.g., disk full):** NOT explicitly caught inside the per-line body. The exception propagates out of the `for` loop; the `finally` closes the raw log; the daemon thread then dies. This means a mid-stream I/O error can silently stop pumping one stream while the child continues. Uncertain whether this is intended; flagged as an open item.
- **Child ignores SIGTERM:** `stop()` escalates to `kill()` after `timeout` (proc.py:122-128); a hung child after `kill()` is given 2.0s, and a remaining timeout is swallowed (the method still returns).
- **Pump thread does not exit within join timeout:** `t.join(timeout=2.0)` returns anyway (proc.py:138). Because threads are daemons, a stuck pump will not block interpreter exit, but `stop()` may then close the merged log while that thread is still alive — the `_closed` flag + lock ensure such a late write is dropped rather than hitting a closed file.
- **Pipe close errors during `stop()`/teardown:** swallowed per-stream (proc.py:131-136, proc.py:81-86).
- **`stop()`/`poll()` before `start()`:** return `None` safely.

## Outputs / artifacts
All written under `out_dir`:
- `stdout.log` — raw child stdout, one child line per line, verbatim (no timestamp). Line-buffered.
- `stderr.log` — raw child stderr, verbatim. Line-buffered.
- `output.log` — merged view of both streams in arrival order, each line prefixed `"<ISO-Z timestamp> [<tag>] "` where `<tag>` is `out` or `err`. Example (from the docstring): `2026-06-07T09:47:01.250Z [out] hello`. A trailing newline is always appended to each merged line. Line-buffered.
- The `out_dir` directory itself is created if absent.

Naming is fixed (literal filenames); there is no per-stream timestamp/token in the filenames. Per docs/architecture.md, `out_dir` itself is typically the session directory `<output_dir>/capture-<fs_stamp>-<token>/` chosen by `session.py`, not by this scope.

## Configuration
This scope has no environment variables. All configuration is via constructor parameters:
- `command: str | list[str]` — required; tokenized via `util.split_command` only when a `str`.
- `out_dir: Path` — required.
- `cwd: str | None` — default `None` (inherit).
- `stop(timeout=5.0)` — graceful-termination wait in seconds; default 5.0. The forced-kill wait (2.0s) and pump-join timeout (2.0s) are hard-coded constants, not configurable.
- Subprocess buffering is fixed: `bufsize=1`, `text=True`; log files opened with `buffering=1`.

## Known limitations / open items
- **Attach mode cannot capture stdio.** When `capture_mcp` attaches to an existing pid rather than launching it, the OS exposes no handle to that process's already-open stdout/stderr file descriptors, so there is no stream to tee. Log capture is therefore unavailable in attach mode (screenshots and audio still work). Documented in the module docstring (proc.py:9-11) and the Purpose above. This is a kernel-level constraint, not a code gap.
- **Mid-loop write errors are uncaught** (see Failure modes): a disk-full or I/O error during pumping can silently kill one pump thread. Consider wrapping the per-line write body in a try/except that counts errors (to honor "capture loops never die" fully). Uncertain if intentional.
- **`self.lines` only counts merged-log writes** while the merged file is open and `not _closed`; it does not count raw-log lines and undercounts if `output.log` failed to open. There is no separate per-stream counter and no exposed error counter for this scope.
- **No partial-flush guarantee on crash:** files are line-buffered, so a hard crash may lose the final unterminated partial line; normal `stop()` flushes the merged log explicitly (proc.py:142) but relies on `finally` close for the raw logs.
- **`bufsize=1` with `text=True`** gives line-buffered reads, but a child that does not flush its own stdout (e.g., block-buffered when not a TTY) will deliver lines late regardless of settings here — outside this scope's control.

## Tests
- I could not confirm the contents of `tests/smoke.py` in this task (not read); referenced here per the spec template. A smoke check for this scope should: launch a short-lived command (e.g. one that prints to both stdout and stderr and exits), call `start()`, let it finish, call `stop()`, then assert: (a) `out_dir/stdout.log`, `stderr.log`, and `output.log` exist; (b) raw logs contain the expected verbatim lines; (c) every `output.log` line matches `^<ISO>Z \[(out|err)\] `; (d) `self.lines` equals the merged line count; (e) `stop()` returns the child exit code; (f) `pid` is non-null after `start()` and `poll()` reflects exit.
- Additional cases worth covering: rollback on a forced post-launch failure (assert the child is killed and `start()` re-raises); raw-log-open failure path (assert the stream is still drained and the child does not hang); `command` as both `str` (shlex-split) and `list[str]`; `stop()` escalation when the child ignores SIGTERM; `stop()`/`poll()` before `start()` return `None`.
- Verify the doc-promised merged format string exactly, including the trailing-newline normalization (proc.py:101).
