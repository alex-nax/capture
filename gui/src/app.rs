//! The GPUI view: a daemon dashboard + a live session detail pane.
//!
//! Lists (health, window picker, sessions) are polled over /v1; the selected
//! session's transcript + screenshot preview are fed LIVE by a background SSE
//! reader on /v1/events into a shared `LiveState` that render() reads. #33 slice 2.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{div, img, prelude::*, px, rgb, App, ClickEvent, Context, SharedString, Timer, Window};

use crate::daemon::{self, Daemon, Health, Session, WindowInfo};

/// Live data for the selected session, written by the SSE thread, read by render.
#[derive(Default)]
struct LiveState {
    tracked: Option<String>,
    transcript: Vec<String>,
    last_shot: Option<String>,
}

pub struct CaptureApp {
    daemon: Option<Daemon>,
    health: Option<Health>,
    sessions: Vec<Session>,
    windows: Vec<WindowInfo>,
    selected: Option<usize>,           // window picker selection
    selected_session: Option<String>,  // session whose detail is shown
    live: Arc<Mutex<LiveState>>,
    message: SharedString,
    out_dir: String,
    polling: bool,
}

fn default_out_dir() -> String {
    dirs::home_dir()
        .map(|h| h.join(".capture").join("runs").to_string_lossy().into_owned())
        .unwrap_or_else(|| "/tmp/capture-runs".into())
}

fn short_id(id: &str) -> &str {
    id.rsplit('-').next().unwrap_or(id)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

impl CaptureApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut app = Self {
            daemon: daemon::discover(),
            health: None,
            sessions: Vec::new(),
            windows: Vec::new(),
            selected: None,
            selected_session: None,
            live: Arc::new(Mutex::new(LiveState::default())),
            message: "".into(),
            out_dir: default_out_dir(),
            polling: false,
        };
        app.refresh_blocking();
        app.start_poll(cx);
        app.spawn_sse();
        app
    }

    fn refresh_blocking(&mut self) {
        if let Some(d) = &self.daemon {
            self.health = d.health().ok();
            self.sessions = d.sessions().unwrap_or_default();
            if self.windows.is_empty() {
                self.windows = d.windows().unwrap_or_default();
            }
        }
    }

    /// Background thread: read /v1/events forever and accumulate the tracked
    /// session's transcript + latest screenshot into the shared LiveState.
    fn spawn_sse(&self) {
        let Some(daemon) = self.daemon.clone() else { return };
        let live = self.live.clone();
        std::thread::spawn(move || loop {
            if let Ok(reader) = daemon.open_events() {
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let Some(json) = line.strip_prefix("data: ") else { continue };
                    let Ok(ev) = serde_json::from_str::<serde_json::Value>(json) else { continue };
                    let sid = ev.get("session_id").and_then(|v| v.as_str());
                    let mut st = live.lock().unwrap();
                    if st.tracked.is_none() || st.tracked.as_deref() != sid {
                        continue;
                    }
                    match ev.get("type").and_then(|v| v.as_str()) {
                        Some("transcript_segment") => {
                            if let Some(t) = ev.get("text").and_then(|v| v.as_str()) {
                                st.transcript.push(t.trim().to_string());
                            }
                        }
                        Some("screenshot_taken") => {
                            if let Some(p) = ev.get("path").and_then(|v| v.as_str()) {
                                st.last_shot = Some(p.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(1)); // reconnect backoff
        });
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_millis(1000)).await;
            let Some(d) = daemon.clone() else { continue };
            let result = cx
                .background_executor()
                .spawn(async move { (d.health().ok(), d.sessions().unwrap_or_default()) })
                .await;
            if this
                .update(cx, |v, cx| {
                    v.health = result.0;
                    v.sessions = result.1;
                    // Default the live pane to the newest running capture.
                    if v.selected_session.is_none() {
                        if let Some(s) = v.sessions.iter().rev().find(|s| s.state == "running") {
                            let id = s.session_id.clone();
                            v.select_session(id, cx);
                        }
                    }
                    cx.notify(); // also repaints the live SSE-fed detail pane
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    fn refresh_windows(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        cx.spawn(async move |this, cx| {
            let ws = cx
                .background_executor()
                .spawn(async move { d.windows().unwrap_or_default() })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.windows = ws;
                cx.notify();
            });
        })
        .detach();
    }

    /// Select a session: track it for SSE, clear the pane, and backfill the
    /// existing transcript over REST (SSE then appends new segments).
    fn select_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected_session = Some(id.clone());
        {
            let mut st = self.live.lock().unwrap();
            st.tracked = Some(id.clone());
            st.transcript.clear();
            st.last_shot = None;
        }
        let Some(d) = self.daemon.clone() else {
            cx.notify();
            return;
        };
        let live = self.live.clone();
        cx.spawn(async move |_this, cx| {
            let id2 = id.clone();
            let segs = cx
                .background_executor()
                .spawn(async move { d.transcript(&id2, 200).unwrap_or_default() })
                .await;
            let mut st = live.lock().unwrap();
            if st.tracked.as_deref() == Some(id.as_str()) {
                st.transcript = segs.into_iter().map(|s| s.text.trim().to_string()).collect();
            }
        })
        .detach();
        cx.notify();
    }

    fn start_capture(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else {
            self.message = "no daemon — run: capture daemon start".into();
            return;
        };
        let Some(ix) = self.selected else {
            self.message = "select a window first".into();
            cx.notify();
            return;
        };
        let Some(w) = self.windows.get(ix) else { return };
        let pid = w.pid;
        let out = self.out_dir.clone();
        self.message = format!("starting capture on pid {pid}…").into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let body = serde_json::json!({
                "output_dir": out, "pid": pid,
                "audio_source": "app", "screenshot_interval": 2.0,
            });
            let r = cx
                .background_executor()
                .spawn(async move { d.start(body) })
                .await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(s) => {
                        v.message = format!("started {}", short_id(&s.session_id)).into();
                        v.select_session(s.session_id, cx); // auto-open its live pane
                    }
                    Err(e) => v.message = format!("start failed: {e}").into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_capture(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = format!("stopping {}…", short_id(&id)).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { d.stop(&id) })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(s) => format!("stopped {}", short_id(&s.session_id)).into(),
                    Err(e) => format!("stop failed: {e}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

fn button(
    label: &str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(label.to_string()))
        .px_3()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(0x2d4f67))
        .child(label.to_string())
        .on_click(on_click)
}

impl Render for CaptureApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = match &self.health {
            Some(h) if h.ok => {
                format!("daemon v{} (api {}) · pid {}", h.version, h.api_version, h.pid)
            }
            _ => "no daemon — run: capture daemon start".to_string(),
        };

        let window_rows: Vec<_> = self
            .windows
            .iter()
            .enumerate()
            .take(6)
            .map(|(ix, w)| {
                let selected = self.selected == Some(ix);
                let label = format!("{} — {}", w.app_name, truncate(&w.title, 40));
                div()
                    .id(("win", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected { rgb(0x2d4f67) } else { rgb(0x1e1e1e) })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(ix);
                        cx.notify();
                    }))
            })
            .collect();

        let mut session_rows: Vec<_> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(ix, s)| {
                let running = s.state == "running";
                let open = self.selected_session.as_deref() == Some(s.session_id.as_str());
                let id = s.session_id.clone();
                let line = format!(
                    "{} · {} · {}s · {}seg",
                    short_id(&s.session_id),
                    s.state,
                    s.screenshots,
                    s.transcript_segments
                );
                let id_sel = id.clone();
                let mut row = div().flex().items_center().justify_between().child(
                    div()
                        .id(("sel", ix))
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if open { rgb(0x24323b) } else { rgb(0x1a1a1a) })
                        .child(line)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_session(id_sel.clone(), cx);
                        })),
                );
                if running {
                    row = row.child(
                        div()
                            .id(("stop", ix))
                            .ml_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(0x7a2d2d))
                            .child("Stop")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stop_capture(id.clone(), cx);
                            })),
                    );
                }
                row
            })
            .collect();
        session_rows.reverse();
        session_rows.truncate(6);

        // Live detail pane for the selected session (SSE-fed).
        let detail = self.selected_session.clone().map(|sid| {
            let (shot, lines) = {
                let st = self.live.lock().unwrap();
                let lines: Vec<String> =
                    st.transcript.iter().rev().take(12).rev().cloned().collect();
                (st.last_shot.clone(), lines)
            };
            let mut pane = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .flex_1()
                .rounded_md()
                .bg(rgb(0x0e1216))
                .child(
                    div()
                        .text_color(rgb(0x66d9a0))
                        .child(format!("▶ live · {}", short_id(&sid))),
                );
            if let Some(p) = shot {
                pane = pane.child(img(PathBuf::from(p)).w_full().h(px(190.0)));
            }
            pane = pane.child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_color(rgb(0x9aa0a6)).child("transcript (live)"))
                    .children(
                        lines
                            .into_iter()
                            .map(|l| div().text_color(rgb(0xcfd3d6)).child(l)),
                    ),
            );
            pane
        });

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .size_full()
            .bg(rgb(0x141414))
            .text_color(rgb(0xe0e0e0))
            .text_sm()
            .child(div().text_xl().child("capture"))
            .child(div().text_color(rgb(0x9aa0a6)).child(header))
            .child(div().text_color(rgb(0xffcc66)).child(self.message.clone()))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        "Refresh windows",
                        cx.listener(|this, _, _, cx| this.refresh_windows(cx)),
                    ))
                    .child(button(
                        "Start capture",
                        cx.listener(|this, _, _, cx| this.start_capture(cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_color(rgb(0x9aa0a6)).child("Windows"))
                            .children(window_rows),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_color(rgb(0x9aa0a6)).child("Sessions"))
                            .children(session_rows),
                    ),
            )
            .children(detail)
    }
}
