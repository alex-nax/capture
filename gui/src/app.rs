//! The GPUI view: a daemon dashboard — health, a window picker, start/stop, and
//! a live-polled session list. Slice 1 of #33 (macOS, gpui 0.2.2).

use std::time::Duration;

use gpui::{div, prelude::*, rgb, App, ClickEvent, Context, SharedString, Timer, Window};

use crate::daemon::{self, Daemon, Health, Session, WindowInfo};

pub struct CaptureApp {
    daemon: Option<Daemon>,
    health: Option<Health>,
    sessions: Vec<Session>,
    windows: Vec<WindowInfo>,
    selected: Option<usize>,
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
            message: "".into(),
            out_dir: default_out_dir(),
            polling: false,
        };
        app.refresh_blocking(); // brief initial load against the local daemon
        app.start_poll(cx);
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

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_millis(1500)).await;
            let Some(d) = daemon.clone() else { continue };
            let result = cx
                .background_executor()
                .spawn(async move { (d.health().ok(), d.sessions().unwrap_or_default()) })
                .await;
            if this
                .update(cx, |v, cx| {
                    v.health = result.0;
                    v.sessions = result.1;
                    cx.notify();
                })
                .is_err()
            {
                break; // view dropped
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
                v.message = match r {
                    Ok(s) => format!("started {}", short_id(&s.session_id)).into(),
                    Err(e) => format!("start failed: {e}").into(),
                };
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
            Some(h) if h.ok => format!(
                "daemon v{} (api {}) · pid {}",
                h.version, h.api_version, h.pid
            ),
            _ => "no daemon — run: capture daemon start".to_string(),
        };

        let window_rows: Vec<_> = self
            .windows
            .iter()
            .enumerate()
            .take(7)
            .map(|(ix, w)| {
                let selected = self.selected == Some(ix);
                let label = format!("{} — {}", w.app_name, truncate(&w.title, 64));
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
                let id = s.session_id.clone();
                let line = format!(
                    "{} · {} · shots {} · segs {} · {}",
                    short_id(&s.session_id),
                    s.state,
                    s.screenshots,
                    s.transcript_segments,
                    s.audio_status,
                );
                let mut row = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .child(div().child(line));
                if running {
                    row = row.child(
                        div()
                            .id(("stop", ix))
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
        session_rows.reverse(); // newest first
        session_rows.truncate(12);

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
            .child(div().text_color(rgb(0x9aa0a6)).child("Windows"))
            .child(div().flex().flex_col().gap_1().children(window_rows))
            .child(div().text_color(rgb(0x9aa0a6)).child("Sessions"))
            .child(div().flex().flex_col().gap_1().children(session_rows))
    }
}
