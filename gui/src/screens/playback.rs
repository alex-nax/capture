//! The session playback screen: the screenshot at the playhead, time-synced subtitles,
//! the scrubber + transport, the live mic/language switchers, and the Manage panel.
//! Relocated verbatim from `app.rs` (#68). `render_playback` is dispatched from `render()`.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{div, img, prelude::*, px, relative, rgb, App, ClickEvent, Context, MouseButton, MouseDownEvent, Pixels, Timer, Window};

use crate::app::CaptureApp;
use crate::components::{chip, icon};
use crate::state::{fmt_dur, short_id, truncate, ConfirmKind};

impl CaptureApp {
    /// The full playback screen: the screenshot at the playhead (or live latest),
    /// time-synced subtitles, and (for finished captures) a scrubber + transport.
    pub(crate) fn render_playback(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let asr_lang_focused = self.asr_language_focus.is_focused(window);
        let Some(pb) = self.playback.as_ref() else {
            return div().into_any_element();
        };
        let finished = pb.finished;
        let (shot, subs): (Option<String>, Vec<(String, bool)>) = if finished {
            let frame = pb
                .frames
                .iter()
                .rev()
                .find(|(t, _)| *t <= pb.pos)
                .or_else(|| pb.frames.first())
                .map(|(_, p)| p.clone());
            let mut active: Vec<(String, bool)> = pb
                .subs
                .iter()
                .filter(|(s, e, _, _)| *s <= pb.pos && pb.pos <= *e)
                .map(|(_, _, t, m)| (t.clone(), *m))
                .collect();
            if active.is_empty() {
                if let Some((_, _, t, m)) = pb.subs.iter().rev().find(|(s, _, _, _)| *s <= pb.pos) {
                    active.push((t.clone(), *m));
                }
            }
            (frame, active)
        } else {
            let st = self.live.lock().unwrap();
            let lines = st.transcript.iter().rev().take(8).rev().map(|l| (l.clone(), false)).collect();
            (st.last_shot.clone(), lines)
        };

        let mut root = div().flex().flex_col().gap_2().flex_shrink_0();
        root = root.child(div().text_color(rgb(0x9aa0a6)).child(format!(
            "{} · {}",
            short_id(&pb.sid),
            if finished { "saved capture" } else { "● live" }
        )));
        root = match shot {
            Some(p) => root.child(img(PathBuf::from(p)).w_full().h(px(360.0)).rounded_md()),
            None => root.child(
                div()
                    .w_full()
                    .h(px(360.0))
                    .rounded_md()
                    .bg(rgb(0x0e1216))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_color(rgb(0x6a6a6a)).child(if finished {
                        "no screenshots"
                    } else {
                        "waiting for first frame…"
                    })),
            ),
        };
        let mut subbox = div().flex().flex_col().gap_1().p_2().rounded_md().bg(rgb(0x0e1216));
        if subs.is_empty() {
            subbox = subbox.child(div().text_color(rgb(0x6a6a6a)).child("…"));
        } else {
            for (txt, is_mic) in subs {
                subbox = subbox.child(if is_mic {
                    div()
                        .flex()
                        .gap_1()
                        .items_center()
                        .child(icon("mic", 12.0, 0x88c0a0))
                        .child(div().text_color(rgb(0x88c0a0)).child(txt))
                } else {
                    div().child(div().text_color(rgb(0xe6e6e6)).child(txt))
                });
            }
        }
        root = root.child(subbox);

        if finished && pb.loaded && pb.t1 > pb.t0 {
            let dur = pb.t1 - pb.t0;
            let frac = (((pb.pos - pb.t0) / dur) as f32).clamp(0.0, 1.0);
            let track = div()
                .id("pb-track")
                .relative()
                .w_full()
                .h(px(10.0))
                .rounded_full()
                .bg(rgb(0x2a2a2a))
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h(px(10.0))
                        .w(relative(frac))
                        .rounded_full()
                        .bg(rgb(0x2d7f67)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        this.pb_dragging = true;
                        this.pb_seek_x(ev.position.x, window, cx);
                    }),
                );
            let playing = pb.playing;
            let controls = div()
                .flex()
                .items_center()
                .gap_2()
                .child(self.pb_ctrl("pb-start", "skip-back", cx.listener(|this, _, _, cx| this.pb_step(f64::NEG_INFINITY, cx))))
                .child(self.pb_ctrl("pb-rew", "rewind", cx.listener(|this, _, _, cx| this.pb_step(-5.0, cx))))
                .child(self.pb_ctrl("pb-play", if playing { "pause" } else { "play" }, cx.listener(|this, _, _, cx| this.pb_toggle_play(cx))))
                .child(self.pb_ctrl("pb-ff", "fast-forward", cx.listener(|this, _, _, cx| this.pb_step(5.0, cx))))
                .child(self.pb_ctrl("pb-end", "skip-forward", cx.listener(|this, _, _, cx| this.pb_step(f64::INFINITY, cx))))
                .child(div().text_color(rgb(0x9aa0a6)).child(format!("{} / {}", fmt_dur(pb.pos - pb.t0), fmt_dur(dur))));
            root = root.child(div().flex().flex_col().gap_2().child(track).child(controls));
        } else if finished && !pb.loaded {
            root = root.child(div().text_color(rgb(0x6a6a6a)).child("loading…"));
        }

        // Live mic switcher (#46): on a running capture, change the input device (or turn
        // it off) without restarting — appends to the mic track.
        if !finished {
            let sid = pb.sid.clone();
            let active = self.sessions.iter().find(|s| s.session_id == sid).and_then(|s| s.mic_device.clone());
            let mut row = div()
                .flex()
                .gap_2()
                .items_center()
                .flex_wrap()
                .child(div().min_w(px(40.0)).text_color(rgb(0x9aa0a6)).child("Mic"));
            let s_off = sid.clone();
            row = row.child(chip(
                "live-mic-off",
                "Off",
                active.is_none(),
                cx.listener(move |this, _, _, cx| this.switch_mic(s_off.clone(), None, cx)),
            ));
            for dev in &self.mics {
                let label = truncate(&dev.name, 18);
                let id = dev.id.clone();
                let s = sid.clone();
                row = row.child(chip(
                    &format!("live-mic-{}", dev.id),
                    &label,
                    active.as_deref() == Some(dev.id.as_str()),
                    cx.listener(move |this, _, _, cx| this.switch_mic(s.clone(), Some(id.clone()), cx)),
                ));
            }
            if self.mics.is_empty() {
                row = row.child(div().text_color(rgb(0x6a6a6a)).child("(Refresh windows to load devices)"));
            }
            root = root.child(row);
            // Live transcription-language toggle: the same searchable dropdown as Settings,
            // surfaced here so the language can be switched DURING a live capture (especially
            // meetings). Picking applies it immediately via daemon `asr_set_language` (the
            // next chunk transcribes in it), the way the Mic row above live-switches devices.
            root = root.child(self.language_field(asr_lang_focused, cx));
        }

        // Manage: capability status + prune + re-transcribe (finished sessions only).
        if finished {
            let sess = self.sessions.iter().find(|s| s.session_id == pb.sid);
            let has_shots = sess.map_or(true, |s| s.has_screenshots);
            let has_audio = sess.map_or(true, |s| s.has_audio);
            let can_retr = sess.map_or(true, |s| s.can_retranscribe);
            let retr_frac = self.live.lock().unwrap().retranscribe.get(&pb.sid).copied();
            let sid = pb.sid.clone();

            let status = div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icon("image", 13.0, if has_shots { 0x88c0a0 } else { 0x5a5a5a }))
                        .child(div().text_xs().text_color(rgb(if has_shots { 0x9aa0a6 } else { 0x5a5a5a })).child(
                            if has_shots { "screenshots" } else { "screenshots pruned" },
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icon(if has_audio { "volume" } else { "volume-x" }, 13.0, if has_audio { 0x88c0a0 } else { 0x5a5a5a }))
                        .child(div().text_xs().text_color(rgb(if has_audio { 0x9aa0a6 } else { 0x5a5a5a })).child(
                            if has_audio { "audio" } else { "audio removed" },
                        )),
                );

            let mut actions = div().flex().items_center().gap_2().flex_wrap();
            if let Some(frac) = retr_frac {
                actions = actions.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .child(icon("refresh", 13.0, 0x66d9a0))
                        .child(div().text_xs().text_color(rgb(0x66d9a0)).child(format!(
                            "re-transcribing {:.0}%",
                            (frac * 100.0).clamp(0.0, 100.0)
                        ))),
                );
            } else if can_retr {
                let s = sid.clone();
                actions = actions.child(self.mng_btn(
                    "mng-retr", "refresh", "Re-transcribe", 0xcfd3d6, 0x2a2a2a,
                    cx.listener(move |this, _, _, cx| this.retranscribe(s.clone(), cx)),
                ));
            } else {
                actions = actions.child(self.mng_btn("mng-retr", "refresh", "Re-transcribe", 0x5a5a5a, 0x222222, |_, _, _| {}));
            }
            if has_shots {
                let s = sid.clone();
                actions = actions.child(self.mng_btn(
                    "mng-halve", "scissors", "Halve frames", 0xcfd3d6, 0x2a2a2a,
                    cx.listener(move |this, _, _, cx| this.prune(s.clone(), vec!["screenshots_halve"], cx)),
                ));
                let s = sid.clone();
                actions = actions.child(self.mng_btn(
                    "mng-delshots", "image", "Delete frames", 0xe6c0c0, 0x3a2a2a,
                    cx.listener(move |this, _, _, cx| {
                        this.confirm = Some(ConfirmKind::Prune(
                            s.clone(),
                            vec!["screenshots"],
                            "Delete all screenshots? The transcript and audio stay.".into(),
                        ));
                        cx.notify();
                    }),
                ));
            }
            if has_audio {
                let s = sid.clone();
                actions = actions.child(self.mng_btn(
                    "mng-delaudio", "volume-x", "Remove audio", 0xe6c0c0, 0x3a2a2a,
                    cx.listener(move |this, _, _, cx| {
                        this.confirm = Some(ConfirmKind::Prune(
                            s.clone(),
                            vec!["audio"],
                            "Remove the audio stream? Frees the most disk but disables re-transcribe (the transcript stays)."
                                .into(),
                        ));
                        cx.notify();
                    }),
                ));
            }
            // Build index (#44): caption frames with the remote vision LLM → a tree summary.
            // Off unless the session has frames AND the configured endpoint is reachable.
            let can_index = sess.map_or(false, |s| s.can_index);
            let idx_prog = self.live.lock().unwrap().index_progress.get(&pb.sid).cloned();
            if let Some((phase, frac)) = idx_prog {
                actions = actions.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .child(icon("list-tree", 13.0, 0x8ab4f8))
                        .child(div().text_xs().text_color(rgb(0x8ab4f8)).child(format!(
                            "indexing {} {:.0}%",
                            phase,
                            (frac * 100.0).clamp(0.0, 100.0)
                        ))),
                );
            } else if can_index && self.index_status.available {
                let s = sid.clone();
                actions = actions.child(self.mng_btn(
                    "mng-index", "list-tree", "Build index", 0xcfd3d6, 0x2a2a2a,
                    cx.listener(move |this, _, _, cx| this.index_session(s.clone(), cx)),
                ));
            } else {
                // Disabled: dim it; the Settings → Index endpoint dot says why.
                actions = actions.child(self.mng_btn(
                    "mng-index", "list-tree", "Build index", 0x5a5a5a, 0x222222, |_, _, _| {},
                ));
            }
            let mut manage = div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_2()
                .child(div().text_color(rgb(0x9aa0a6)).child("Manage"))
                .child(status)
                // Change the language on the fly (a running capture's next chunk uses it);
                // then Re-transcribe to fix the part already done with the wrong language.
                .child(self.language_field(asr_lang_focused, cx))
                .child(actions);
            // Show the index's root summary once built (#44).
            if let Some(summary) = pb.index_summary.clone() {
                let nodes = pb.index_nodes.unwrap_or(0);
                manage = manage.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0x16181c))
                        .border_1()
                        .border_color(rgb(0x2a2a2a))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(icon("list-tree", 13.0, 0x8ab4f8))
                                .child(div().text_xs().text_color(rgb(0x8ab4f8)).child(format!("Index summary · {nodes} nodes"))),
                        )
                        .child(div().text_sm().text_color(rgb(0xc8ccd0)).child(summary)),
                );
            }
            root = root.child(manage);
        }
        root.into_any_element()
    }

    /// A labeled icon button for the playback "Manage" actions (prune / re-transcribe).
    pub(crate) fn mng_btn(
        &self,
        id: &'static str,
        name: &'static str,
        label: &'static str,
        tint: u32,
        bg: u32,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(bg))
            .child(icon(name, 13.0, tint))
            .child(div().text_xs().text_color(rgb(tint)).child(label))
            .on_click(on_click)
    }

    /// A small transport-control icon button.
    pub(crate) fn pb_ctrl(
        &self,
        id: &'static str,
        name: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(36.0))
            .h(px(28.0))
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(0x2a2a2a))
            .child(icon(name, 14.0, 0xcfd3d6))
            .on_click(on_click)
    }

    /// Seek the playhead to a scrubber-track mouse-x (the track spans the content
    /// width: left = root padding 16px, width = viewport − 32).
    pub(crate) fn pb_seek_x(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let tw = window.viewport_size().width - px(32.0);
        if tw <= px(0.0) {
            return;
        }
        let frac = ((x - px(16.0)) / tw).clamp(0.0, 1.0);
        if let Some(pb) = self.playback.as_mut() {
            if pb.t1 > pb.t0 {
                pb.pos = pb.t0 + frac as f64 * (pb.t1 - pb.t0);
                pb.playing = false;
                cx.notify();
            }
        }
    }

    pub(crate) fn pb_step(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(pb) = self.playback.as_mut() {
            pb.pos = (pb.pos + delta).clamp(pb.t0, pb.t1);
            pb.playing = false;
            cx.notify();
        }
    }

    pub(crate) fn pb_toggle_play(&mut self, cx: &mut Context<Self>) {
        let mut now_playing = false;
        if let Some(pb) = self.playback.as_mut() {
            if pb.pos >= pb.t1 {
                pb.pos = pb.t0; // replay from the start if parked at the end
            }
            pb.playing = !pb.playing;
            now_playing = pb.playing;
        }
        cx.notify();
        if now_playing {
            self.pb_start_ticker(cx);
        }
    }

    /// Advance the playhead in ~real time while `playing`; exits when paused/closed.
    pub(crate) fn pb_start_ticker(&mut self, cx: &mut Context<Self>) {
        if self.pb_ticker {
            return;
        }
        self.pb_ticker = true;
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(200)).await;
                let go = this
                    .update(cx, |v, cx| {
                        let go = matches!(v.playback.as_ref(), Some(pb) if pb.playing);
                        if go {
                            if let Some(pb) = v.playback.as_mut() {
                                pb.pos = (pb.pos + 0.2).min(pb.t1);
                                if pb.pos >= pb.t1 {
                                    pb.playing = false;
                                }
                            }
                            cx.notify();
                        } else {
                            v.pb_ticker = false;
                        }
                        go
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
        })
        .detach();
    }
}
