//! The session playback screen (redesign): the framed screenshot at the playhead, the
//! time-synced subtitle / live transcript, the scrubber + transport (saved) or the live
//! control bar + mic/language switchers (live), the Manage card, and the index summary.
//! Matches `design/screens/playback.html` + `playback-live.html`.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{div, img, prelude::*, px, relative, rgb, rgba, App, ClickEvent, Context, MouseButton, MouseDownEvent, Pixels, Timer, Window};

use crate::app::CaptureApp;
use crate::components::{card, chip, eyebrow, icon, icon_button, ButtonVariant};
use crate::state::{fmt_dur, short_id, truncate, ConfirmKind};
use crate::theme;

impl CaptureApp {
    /// The full playback screen: the screenshot at the playhead (or live latest), the active
    /// subtitle / live transcript, and (saved) a scrubber + transport + Manage, or (live) the
    /// REC frame + Stop + mic/language switchers.
    pub(crate) fn render_playback(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let asr_lang_focused = self.asr_language_focus.is_focused(window);
        let Some(pb) = self.playback.as_ref() else {
            return div().into_any_element();
        };
        let finished = pb.finished;

        // Resolve the frame at the playhead + the active subtitle text(s) / live transcript.
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

        let sb = gpui::FontWeight(theme::FW_SEMIBOLD as f32);
        let mut root = div().flex().flex_col().gap(px(18.0)).flex_shrink_0();

        // ── Session line: mono id · dot · state ──────────────────────────────
        let (dot, state_label) = if finished { (theme::SUCCESS, "saved capture") } else { (theme::LIVE, "live") };
        root = root.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(theme::TS_BODY))
                        .font_weight(sb)
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(short_id(&pb.sid).to_string()),
                )
                .child(div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::TEXT_MUTED)).child("·"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(dot))
                        .child(div().flex_none().size(px(if finished { 6.0 } else { 7.0 })).rounded_full().bg(rgb(dot)))
                        .child(state_label),
                ),
        );

        // ── Frame: bordered 380px; the shot if present, else a placeholder. REC badge when live. ──
        let mut frame = div()
            .relative()
            .w_full()
            .h(px(380.0))
            .rounded(px(theme::RADIUS_LG))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(theme::CARD_BORDER))
            .bg(rgb(theme::BG))
            .flex()
            .items_center()
            .justify_center();
        match &shot {
            Some(p) => frame = frame.child(img(PathBuf::from(p.clone())).w_full().h(px(380.0))),
            None => {
                frame = frame.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_color(rgb(theme::TEXT_DISABLED))
                        .child(icon("image", 16.0, theme::TEXT_DISABLED))
                        .child(div().text_size(px(theme::TS_SMALL)).child(if finished {
                            "no screenshots"
                        } else {
                            "waiting for first frame…"
                        })),
                )
            }
        }
        if !finished {
            frame = frame.child(
                div()
                    .absolute()
                    .top(px(12.0))
                    .left(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(theme::RADIUS_SM))
                    .bg(rgb(theme::ERROR_SUBTLE))
                    .border_1()
                    .border_color(rgb(theme::ERROR_BORDER))
                    .child(div().flex_none().size(px(7.0)).rounded_full().bg(rgb(theme::LIVE)))
                    .child(div().text_size(px(theme::TS_EYEBROW)).font_weight(sb).text_color(rgb(theme::LIVE)).child("REC")),
            );
        }
        root = root.child(frame);

        // ── Subtitle (saved) / live transcript with cursor ──────────────────
        if subs.is_empty() {
            root = root.child(
                div()
                    .text_size(px(15.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(if finished { "—" } else { "…" }),
            );
        } else {
            let mut tx = div().flex().flex_col().gap(px(4.0));
            let n = subs.len();
            for (i, (txt, is_mic)) in subs.into_iter().enumerate() {
                let color = if is_mic { theme::SUCCESS } else { theme::TEXT_PRIMARY };
                let mut line = div().flex().items_center().gap(px(6.0)).text_size(px(15.0)).line_height(px(23.0)).text_color(rgb(color));
                if is_mic {
                    line = line.child(icon("mic", 13.0, theme::SUCCESS));
                }
                line = line.child(div().child(txt));
                // Live caret after the last line.
                if !finished && i + 1 == n {
                    line = line.child(div().flex_none().w(px(2.0)).h(px(15.0)).bg(rgb(theme::ACCENT)));
                }
                tx = tx.child(line);
            }
            root = root.child(tx);
        }

        // ── Saved: scrubber + transport ─────────────────────────────────────
        if finished && pb.loaded && pb.t1 > pb.t0 {
            let dur = pb.t1 - pb.t0;
            let frac = (((pb.pos - pb.t0) / dur) as f32).clamp(0.0, 1.0);
            let track = div()
                .id("pb-track")
                .relative()
                .w_full()
                .h(px(6.0))
                .rounded(px(3.0))
                .bg(rgb(theme::ELEVATED))
                .overflow_hidden()
                .cursor_pointer()
                .child(div().absolute().left_0().top_0().h(px(6.0)).w(relative(frac)).rounded(px(3.0)).bg(rgb(theme::ACCENT)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        this.pb_dragging = true;
                        this.pb_seek_x(ev.position.x, window, cx);
                    }),
                );
            let playing = pb.playing;
            let play = div()
                .id("pb-play")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(38.0))
                .rounded_full()
                .bg(rgb(theme::ACCENT))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::ACCENT_HOVER)))
                .child(icon(if playing { "pause" } else { "play" }, 16.0, theme::ON_ACCENT))
                .on_click(cx.listener(|this, _, _, cx| this.pb_toggle_play(cx)));
            let transport = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(self.pb_ghost("pb-start", "skip-back", cx.listener(|this, _, _, cx| this.pb_step(f64::NEG_INFINITY, cx))))
                .child(self.pb_ghost("pb-rew", "rewind", cx.listener(|this, _, _, cx| this.pb_step(-5.0, cx))))
                .child(play)
                .child(self.pb_ghost("pb-ff", "fast-forward", cx.listener(|this, _, _, cx| this.pb_step(5.0, cx))))
                .child(self.pb_ghost("pb-end", "skip-forward", cx.listener(|this, _, _, cx| this.pb_step(f64::INFINITY, cx))))
                .child(div().ml(px(6.0)).flex().child(icon("volume", 16.0, theme::TEXT_MUTED)))
                .child(div().flex_1())
                .child(div().text_size(px(theme::TS_BODY)).text_color(rgb(theme::TEXT_SECONDARY)).child(format!("{} / {}", fmt_dur(pb.pos - pb.t0), fmt_dur(dur))));
            root = root.child(div().flex().flex_col().gap(px(14.0)).child(track).child(transport));
        } else if finished && !pb.loaded {
            root = root.child(div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::TEXT_MUTED)).child("loading…"));
        }

        // ── Live: control bar (Stop + elapsed) + mic + language ─────────────
        if !finished {
            let sid = pb.sid.clone();
            // Real elapsed from the session's started_at (re-derived each render/poll — no fake ticker).
            let elapsed = self
                .sessions
                .iter()
                .find(|s| s.session_id == sid)
                .and_then(|s| s.started_at.as_deref())
                .and_then(crate::state::parse_iso_epoch)
                .map(|t0| (capture_core::time::now() - t0).max(0.0));
            let stop_sid = sid.clone();
            let stop = div()
                .id("pb-stop")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.0))
                .h(px(34.0))
                .px(px(15.0))
                .rounded(px(theme::RADIUS_SM))
                .bg(rgb(theme::ERROR))
                .text_color(rgb(theme::ON_ACCENT))
                .font_weight(sb)
                .text_size(px(theme::TS_BODY))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::LIVE)))
                .child(icon("stop", 14.0, theme::ON_ACCENT))
                .child("Stop capture")
                .on_click(cx.listener(move |this, _, _, cx| this.stop_capture(stop_sid.clone(), cx)));
            let bar = div()
                .flex()
                .items_center()
                .gap(px(theme::SP_3))
                .child(stop)
                .child(div().flex_1())
                .when_some(elapsed, |d, e| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().flex_none().size(px(8.0)).rounded_full().bg(rgb(theme::LIVE)))
                            .child(div().text_size(px(theme::TS_BODY)).text_color(rgb(theme::TEXT_PRIMARY)).child(fmt_dur(e))),
                    )
                });
            root = root.child(bar);

            // Mic switcher (#46): live-switch the input device (or off) without restarting.
            let active = self.sessions.iter().find(|s| s.session_id == sid).and_then(|s| s.mic_device.clone());
            let mut mics = div().flex().gap(px(theme::SP_2)).items_center().flex_wrap();
            let s_off = sid.clone();
            mics = mics.child(chip("live-mic-off", "Off", active.is_none(), cx.listener(move |this, _, _, cx| this.switch_mic(s_off.clone(), None, cx))));
            for dev in &self.mics {
                let id = dev.id.clone();
                let s = sid.clone();
                mics = mics.child(chip(
                    &format!("live-mic-{}", dev.id),
                    &truncate(&dev.name, 22),
                    active.as_deref() == Some(dev.id.as_str()),
                    cx.listener(move |this, _, _, cx| this.switch_mic(s.clone(), Some(id.clone()), cx)),
                ));
            }
            if self.mics.is_empty() {
                mics = mics.child(div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::TEXT_MUTED)).child("(Refresh windows to load devices)"));
            }
            root = root.child(div().flex().items_center().gap(px(14.0)).child(self.pb_label("Mic")).child(mics));
            // Live language switch (the next chunk transcribes in it). Opens UPWARD here (low on screen).
            root = root.child(self.language_field(asr_lang_focused, true, cx));
        }

        // ── Saved: Manage card + index summary card ─────────────────────────
        if finished {
            let sess = self.sessions.iter().find(|s| s.session_id == pb.sid);
            let has_shots = sess.map_or(true, |s| s.has_screenshots);
            let has_audio = sess.map_or(true, |s| s.has_audio);
            let can_retr = sess.map_or(true, |s| s.can_retranscribe);
            let can_index = sess.map_or(false, |s| s.can_index);
            let retr_frac = self.live.lock().unwrap().retranscribe.get(&pb.sid).copied();
            let idx_prog = self.live.lock().unwrap().index_progress.get(&pb.sid).cloned();
            let sid = pb.sid.clone();

            let status = div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .flex_wrap()
                .child(self.pb_status_chip("image", if has_shots { "screenshots" } else { "screenshots pruned" }, has_shots))
                .child(self.pb_status_chip(if has_audio { "volume" } else { "volume-x" }, if has_audio { "audio" } else { "audio removed" }, has_audio));

            let mut actions = div().flex().items_center().gap(px(10.0)).flex_wrap();
            // Re-transcribe (or its in-flight progress).
            if let Some(frac) = retr_frac {
                actions = actions.child(self.pb_progress("refresh", &format!("re-transcribing {:.0}%", (frac * 100.0).clamp(0.0, 100.0)), theme::SUCCESS));
            } else {
                let s = sid.clone();
                actions = actions.child(self.pb_action("refresh", "Re-transcribe", ButtonVariant::Secondary, can_retr, cx.listener(move |this, _, _, cx| this.retranscribe(s.clone(), cx))));
            }
            if has_shots {
                let s = sid.clone();
                actions = actions.child(self.pb_action("scissors", "Halve frames", ButtonVariant::Secondary, true, cx.listener(move |this, _, _, cx| this.prune(s.clone(), vec!["screenshots_halve"], cx))));
                let s = sid.clone();
                actions = actions.child(self.pb_action(
                    "trash",
                    "Delete frames",
                    ButtonVariant::Destructive,
                    true,
                    cx.listener(move |this, _, _, cx| {
                        this.confirm = Some(ConfirmKind::Prune(s.clone(), vec!["screenshots"], "Delete all screenshots? The transcript and audio stay.".into()));
                        cx.notify();
                    }),
                ));
            }
            if has_audio {
                let s = sid.clone();
                actions = actions.child(self.pb_action(
                    "volume-x",
                    "Remove audio",
                    ButtonVariant::Destructive,
                    true,
                    cx.listener(move |this, _, _, cx| {
                        this.confirm = Some(ConfirmKind::Prune(
                            s.clone(),
                            vec!["audio"],
                            "Remove the audio stream? Frees the most disk but disables re-transcribe (the transcript stays).".into(),
                        ));
                        cx.notify();
                    }),
                ));
            }
            // Build index (#44): off unless frames present AND the configured endpoint is reachable.
            if let Some((phase, frac)) = idx_prog {
                actions = actions.child(self.pb_progress("list-tree", &format!("indexing {} {:.0}%", phase, (frac * 100.0).clamp(0.0, 100.0)), theme::ACCENT_TEXT));
            } else {
                let s = sid.clone();
                let enabled = can_index && self.index_status.available;
                actions = actions.child(self.pb_action("list-tree", "Build index", ButtonVariant::Primary, enabled, cx.listener(move |this, _, _, cx| this.index_session(s.clone(), cx))));
            }

            let manage = card(
                div()
                    .flex()
                    .flex_col()
                    .child(div().mb(px(14.0)).child(eyebrow("Manage")))
                    .child(status)
                    // Change the language on the fly, then Re-transcribe to fix what was done wrong.
                    .child(div().mt(px(14.0)).child(self.language_field(asr_lang_focused, true, cx)))
                    .child(div().mt(px(16.0)).child(actions)),
            );
            root = root.child(manage);

            // Index root summary, once built (#44).
            if let Some(summary) = pb.index_summary.clone() {
                let nodes = pb.index_nodes.unwrap_or(0);
                root = root.child(card(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .mb(px(12.0))
                                .child(icon("list-tree", 14.0, theme::ACCENT_TEXT))
                                .child(div().text_size(px(theme::TS_BODY)).font_weight(sb).text_color(rgb(theme::TEXT_PRIMARY)).child("Index summary"))
                                .child(div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::TEXT_MUTED)).child(format!("· {nodes} nodes"))),
                        )
                        .child(div().text_size(px(theme::TS_BODY)).line_height(px(22.0)).text_color(rgb(theme::TEXT_SECONDARY)).child(summary)),
                ));
            }
        }

        root.into_any_element()
    }

    /// An 80–118px left-hand label for a playback row (Mic / Language), matching the field label.
    fn pb_label(&self, text: &'static str) -> impl IntoElement {
        div()
            .w(px(118.0))
            .flex_none()
            .text_size(px(theme::TS_BODY))
            .text_color(rgb(theme::TEXT_MUTED))
            .child(text)
    }

    /// A Manage action button (design `.btnS`/`.btnD`/`.btnP` with a leading icon). When disabled it
    /// renders dimmed + non-interactive (the kit's `icon_button` has no disabled state).
    fn pb_action(
        &self,
        icon_name: &'static str,
        label: &'static str,
        variant: ButtonVariant,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> gpui::AnyElement {
        if enabled {
            icon_button(icon_name, label, variant, on_click).into_any_element()
        } else {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(7.0))
                .h(px(32.0))
                .px(px(14.0))
                .rounded(px(theme::RADIUS_SM))
                .bg(rgb(theme::ELEVATED))
                .text_size(px(theme::TS_BODY))
                .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                .text_color(rgb(theme::TEXT_DISABLED))
                .child(icon(icon_name, 15.0, theme::TEXT_DISABLED))
                .child(label.to_string())
                .into_any_element()
        }
    }

    /// A non-interactive in-flight indicator (re-transcribe / index), tinted to `color`.
    fn pb_progress(&self, icon_name: &'static str, label: &str, color: u32) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(32.0))
            .px(px(11.0))
            .text_size(px(theme::TS_SMALL))
            .text_color(rgb(color))
            .child(icon(icon_name, 14.0, color))
            .child(label.to_string())
    }

    /// A Manage status chip (screenshots / audio) — bordered, dimmed when the artifact is gone.
    fn pb_status_chip(&self, icon_name: &'static str, label: &'static str, ok: bool) -> impl IntoElement {
        let fg = if ok { theme::TEXT_SECONDARY } else { theme::TEXT_DISABLED };
        div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .px(px(11.0))
            .py(px(6.0))
            .rounded(px(theme::RADIUS_SM))
            .bg(rgb(if ok { theme::CHIP_IDLE } else { theme::ELEVATED }))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .text_size(px(theme::TS_BODY))
            .text_color(rgb(fg))
            .child(icon(icon_name, 13.0, if ok { theme::SUCCESS } else { theme::TEXT_DISABLED }))
            .child(label)
    }

    /// A ghost transport-control icon button (skip / rewind / fast-forward).
    fn pb_ghost(&self, id: &'static str, name: &'static str, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .p(px(6.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(theme::GHOST_HOVER)))
            .child(icon(name, 17.0, theme::TEXT_SECONDARY))
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
