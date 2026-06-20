//! Launched-process capture (launch mode): spawn a target command and tee its stdout/stderr to disk.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::json;

use capture_core::time::{iso, now};

use crate::helpers::expand;
use crate::EventSink;

/// Launches a target process and tees its stdout/stderr to disk. Port of `core/proc.py`:
///   * `stdout.log` / `stderr.log` — raw streams, one line per source line.
///   * `output.log` — merged, each line prefixed `<iso> [out|err] `.
///
/// Only used in launch mode; attach mode gets no handle on an existing process's streams. Interior-
/// mutable so the session can keep it in `Inner` and still read `lines()`/`is_running()` after `stop()`.
pub(crate) struct ProcessCapture {
    pid: u32,
    child: Mutex<Option<Child>>,
    lines: Arc<AtomicI64>,
    merged: Arc<Mutex<std::io::BufWriter<std::fs::File>>>,
    pumps: Mutex<Vec<JoinHandle<()>>>,
    // None = not yet stopped; Some(code) once stopped (code itself None ⇒ killed by signal).
    exit_code: Mutex<Option<Option<i32>>>,
}

impl ProcessCapture {
    /// Spawn `command` (shell-tokenized), piping its stdout/stderr into pump threads. Fails if the
    /// command is empty/unparseable or the program can't be spawned.
    pub(crate) fn start(
        command: &str,
        dir: &Path,
        cwd: Option<&str>,
        emit: EventSink,
        session_id: String,
    ) -> Result<ProcessCapture, String> {
        let args: Vec<String> = shlex::split(command)
            .ok_or_else(|| format!("could not parse command: {command:?}"))?;
        let (prog, rest) = args.split_first().ok_or_else(|| "empty command".to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

        let mut cmd = Command::new(prog);
        cmd.args(rest).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(c) = cwd {
            cmd.current_dir(expand(c));
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn {prog:?}: {e}"))?;
        let pid = child.id();

        let merged_file = std::fs::File::create(dir.join("output.log"))
            .map_err(|e| format!("open output.log: {e}"))?;
        let merged = Arc::new(Mutex::new(std::io::BufWriter::new(merged_file)));
        let lines = Arc::new(AtomicI64::new(0));

        let mut pumps = Vec::new();
        let streams: [(Option<Box<dyn Read + Send>>, &str, &str); 2] = [
            (child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), "stdout.log", "out"),
            (child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>), "stderr.log", "err"),
        ];
        for (stream, raw_name, tag) in streams {
            if let Some(stream) = stream {
                let raw_path = dir.join(raw_name);
                let (merged, lines, emit, id, tag) =
                    (merged.clone(), lines.clone(), emit.clone(), session_id.clone(), tag.to_string());
                let h = std::thread::Builder::new()
                    .name(format!("pump-{tag}"))
                    .spawn(move || pump(stream, &raw_path, &tag, &merged, &lines, &emit, &id))
                    .expect("spawn pump");
                pumps.push(h);
            }
        }

        Ok(ProcessCapture {
            pid,
            child: Mutex::new(Some(child)),
            lines,
            merged,
            pumps: Mutex::new(pumps),
            exit_code: Mutex::new(None),
        })
    }

    pub(crate) fn pid(&self) -> Option<i64> {
        Some(self.pid as i64)
    }

    pub(crate) fn lines(&self) -> i64 {
        self.lines.load(Ordering::Relaxed)
    }

    /// True while the child is still running (false once `stop()` has reaped it, or if it self-exited).
    pub(crate) fn is_running(&self) -> bool {
        if self.exit_code.lock().unwrap().is_some() {
            return false;
        }
        match self.child.lock().unwrap().as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Terminate the child (SIGTERM → 5 s grace → SIGKILL), join the pump threads, flush `output.log`.
    /// Returns the exit code (`None` ⇒ killed by signal / unknown). Idempotent.
    pub(crate) fn stop(&self) -> Option<i32> {
        if let Some(code) = *self.exit_code.lock().unwrap() {
            return code;
        }
        let code = match self.child.lock().unwrap().take() {
            Some(mut child) => terminate_child(&mut child),
            None => None,
        };
        // The child's exit closes its pipe write-ends → the pumps hit EOF and return.
        for h in self.pumps.lock().unwrap().drain(..) {
            let _ = h.join();
        }
        if let Ok(mut m) = self.merged.lock() {
            let _ = m.flush();
        }
        *self.exit_code.lock().unwrap() = Some(code);
        code
    }
}

/// Read `stream` line-by-line: append raw bytes to `raw_path`, append a timestamped `<iso> [tag] `
/// line to the shared `merged` writer (incrementing `lines`), and emit a `log_line` event.
fn pump(
    stream: Box<dyn Read + Send>,
    raw_path: &Path,
    tag: &str,
    merged: &Arc<Mutex<std::io::BufWriter<std::fs::File>>>,
    lines: &Arc<AtomicI64>,
    emit: &EventSink,
    session_id: &str,
) {
    let mut reader = BufReader::new(stream);
    let mut raw = match std::fs::File::create(raw_path) {
        Ok(f) => std::io::BufWriter::new(f),
        Err(_) => {
            // Still drain the stream so the child never blocks on a full pipe.
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
            return;
        }
    };
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break, // EOF or a read error
            Ok(_) => {
                let _ = raw.write_all(&buf);
                let _ = raw.flush();
                let text = String::from_utf8_lossy(&buf);
                let nl = if text.ends_with('\n') { "" } else { "\n" };
                let stamp = format!("{} [{tag}] {text}{nl}", iso(Some(now())));
                if let Ok(mut m) = merged.lock() {
                    let _ = m.write_all(stamp.as_bytes());
                    let _ = m.flush();
                    lines.fetch_add(1, Ordering::Relaxed);
                }
                emit(json!({ "type": "log_line", "session_id": session_id,
                    "stream": tag, "line": text.trim_end_matches('\n') }));
            }
        }
    }
    let _ = raw.flush();
}

/// SIGTERM the child, poll up to 5 s, then SIGKILL. Returns its exit code — `status.code()` for a
/// normal exit, the negated signal for a signal death (mirrors Python's `subprocess.returncode`).
fn terminate_child(child: &mut Child) -> Option<i32> {
    fn code_of(status: std::process::ExitStatus) -> Option<i32> {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            return status.code().or_else(|| status.signal().map(|s| -s));
        }
        #[cfg(not(unix))]
        {
            status.code()
        }
    }
    if let Ok(Some(status)) = child.try_wait() {
        return code_of(status); // already exited
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = child.kill();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return code_of(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    let _ = child.kill();
    child.wait().ok().and_then(code_of)
}
