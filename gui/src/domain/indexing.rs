//! Multimodal-index endpoint config + index-build actions. Relocated verbatim from `app.rs` (#68).

use std::time::Duration;

use gpui::{prelude::*, Context, KeyDownEvent, Timer};

use crate::app::CaptureApp;
use crate::daemon;
use crate::state::{index_provider_meta, short_id, IndexField};

impl CaptureApp {
    /// Poll the multimodal-index endpoint availability on a slow, separate cadence — its
    /// `/v1/models` preflight can take seconds (or time out when offline), so it must NOT
    /// share the 1 s session loop. Drives the Index button's enabled/disabled gate.
    pub(crate) fn start_index_status_poll(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_millis(8000)).await;
            let Ok(url) = this.update(cx, |v, _| v.index_chat_url()) else { break };
            let status = cx
                .background_executor()
                .spawn(async move { daemon::discover().and_then(|d| d.index_status(&url).ok()) })
                .await;
            if this
                .update(cx, |v, cx| {
                    if let Some(s) = status {
                        v.index_status = s;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    /// The index model picker (#53): a clickable field showing the chosen `index_model` that
    /// expands the fetched `index_models` as selectable rows, plus a Refresh affordance that
    /// re-fetches from the provider. Reuses the language-dropdown layout/idioms.
    pub(crate) fn index_model_field(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::components::{button, icon, ButtonVariant};
        use crate::theme;
        use gpui::{deferred, div, px, relative, rgb, rgba};
        let field_text = if self.index_model.is_empty() {
            "server default".to_string()
        } else {
            self.index_model.clone()
        };
        let dim = self.index_model.is_empty();
        let open = self.model_dropdown_open;

        // Label (118px `.lbl`) + the `.fld` dropdown field + a secondary Refresh button.
        // `relative` so the open menu can float as an absolute popover (see below).
        let mut col = div().flex().flex_col().gap_1().relative().child(
            div()
                .flex()
                .items_center()
                .gap(px(14.0))
                .child(
                    div()
                        .w(px(118.0))
                        .flex_none()
                        .text_size(px(theme::TS_BODY))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child("Model"),
                )
                .child(
                    div()
                        .id("index-model-dropdown")
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(theme::SP_3))
                        .py(px(theme::SP_2))
                        .rounded(px(theme::RADIUS_MD))
                        .border_1()
                        .border_color(if open { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                        .bg(rgb(theme::BG))
                        .cursor_pointer()
                        .text_size(px(theme::TS_BODY))
                        .text_color(if dim { rgb(theme::TEXT_MUTED) } else { rgb(theme::TEXT_PRIMARY) })
                        .child(field_text)
                        .child(icon("chevron-down", 15.0, theme::TEXT_MUTED))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model_dropdown_open = !this.model_dropdown_open;
                            // Lazily refresh on first open if we have nothing yet.
                            if this.model_dropdown_open && this.index_models.is_empty() {
                                this.fetch_index_models(cx);
                            }
                            cx.notify();
                        })),
                )
                .child(button(
                    "Refresh",
                    ButtonVariant::Secondary,
                    cx.listener(|this, _, _, cx| this.fetch_index_models(cx)),
                )),
        );

        if self.model_dropdown_open {
            // §4 dropdown menu surface: ELEVATED / 1px BORDER, radius 6, pad 4. Floated as an
            // absolute popover below the field (top:100% of the relative col, left = label width)
            // so it overlays the rows below instead of pushing them down.
            let mut list = div()
                .absolute()
                .top(relative(1.0))
                .left(px(132.0))
                .flex()
                .flex_col()
                .w(px(280.0))
                .p(px(theme::SP_1))
                .rounded(px(theme::RADIUS_MD))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .bg(rgb(theme::ELEVATED));
            // A "server default" row (blank model) plus each fetched model.
            let default_active = self.index_model.is_empty();
            list = list.child(
                div()
                    .id("idx-model-row-default")
                    .flex()
                    .py(px(7.0))
                    .px(px(10.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_size(px(theme::TS_BODY))
                    .when(default_active, |s| s.bg(rgb(theme::ACCENT_SUBTLE)).text_color(rgb(theme::ACCENT_TEXT)))
                    .when(!default_active, |s| s.text_color(rgb(theme::TEXT_MUTED)).hover(|h| h.bg(rgba(theme::GHOST_HOVER))))
                    .child("server default")
                    .on_click(cx.listener(|this, _, _, cx| this.set_index_model(String::new(), cx))),
            );
            if self.index_models.is_empty() {
                list = list.child(
                    div()
                        .py(px(7.0))
                        .px(px(10.0))
                        .text_size(px(theme::TS_BODY))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child("no models — set host/port, then Refresh"),
                );
            } else {
                for (i, model) in self.index_models.iter().take(40).enumerate() {
                    let m = model.clone();
                    let is_active = *model == self.index_model;
                    list = list.child(
                        div()
                            .id(("idx-model-row", i))
                            .flex()
                            .py(px(7.0))
                            .px(px(10.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(theme::TS_BODY))
                            .when(is_active, |s| s.bg(rgb(theme::ACCENT_SUBTLE)).text_color(rgb(theme::ACCENT_TEXT)))
                            .when(!is_active, |s| s.text_color(rgb(theme::TEXT_SECONDARY)).hover(|h| h.bg(rgba(theme::GHOST_HOVER))))
                            .child(model.clone())
                            .on_click(cx.listener(move |this, _, _, cx| this.set_index_model(m.clone(), cx))),
                    );
                }
            }
            // `deferred` paints the popover after (on top of) the rows below it.
            col = col.child(deferred(list));
        }
        col
    }

    /// Compose the index chat-completions URL from the structured provider config (#52), for the
    /// `/v1/index/status?url=` availability probe. openai is fixed; custom carries a full base URL.
    pub(crate) fn index_chat_url(&self) -> String {
        let host = self.index_host.trim();
        let port = self.index_port.trim();
        match self.index_provider.as_str() {
            "openai" => "https://api.openai.com/v1/chat/completions".to_string(),
            "custom" => {
                if host.is_empty() {
                    String::new()
                } else {
                    format!("{}/chat/completions", host.trim_end_matches('/'))
                }
            }
            _ => {
                // lmstudio / ollama (and any future host:port provider).
                if host.is_empty() {
                    String::new()
                } else if port.is_empty() {
                    format!("http://{host}/v1/chat/completions")
                } else {
                    format!("http://{host}:{port}/v1/chat/completions")
                }
            }
        }
    }

    /// Whether the selected provider needs an API key (only `openai`), to gate the key field.
    pub(crate) fn index_needs_key(&self) -> bool {
        index_provider_meta(&self.index_provider).1
    }

    /// Whether the selected provider carries a full base URL (custom): host field is the base, no port.
    pub(crate) fn index_is_base_url(&self) -> bool {
        index_provider_meta(&self.index_provider).2
    }

    /// Pick a provider (#52): set it, prefill the default port when empty, clear the stale model
    /// list, persist, and re-fetch this provider's models.
    pub(crate) fn set_index_provider(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.index_provider == id {
            return;
        }
        self.index_provider = id.to_string();
        let (default_port, _needs_key, _is_base) = index_provider_meta(id);
        if self.index_port.trim().is_empty() {
            self.index_port = default_port.to_string();
        }
        self.index_models.clear();
        self.model_dropdown_open = false;
        self.save_settings();
        cx.notify();
        self.fetch_index_models(cx);
    }

    /// Choose a model from the dropdown (#53): set it, close the dropdown, persist.
    pub(crate) fn set_index_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.index_model = model;
        self.model_dropdown_open = false;
        self.save_settings();
        cx.notify();
    }

    /// Generic key handling for a focusable index text field (host / port / key), mirroring the
    /// launch field: printable chars (`key_char`), backspace, ⌘V paste. Enter persists + acts.
    pub(crate) fn on_index_field_key(
        &mut self,
        field: IndexField,
        ev: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let buf = match field {
            IndexField::Host => &mut self.index_host,
            IndexField::Port => &mut self.index_port,
            IndexField::Key => &mut self.index_key,
        };
        if m.platform && ks.key == "v" {
            if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                buf.push_str(t.trim());
                cx.notify();
            }
            return;
        }
        if m.platform || m.control || m.function {
            return;
        }
        match ks.key.as_str() {
            "backspace" => {
                buf.pop();
            }
            "enter" => {
                // Persist, re-probe reachability, and refresh the model list for the new endpoint.
                self.save_settings();
                self.probe_index_status(cx);
                self.fetch_index_models(cx);
                return;
            }
            _ => {
                if let Some(c) = ks.key_char.as_deref() {
                    if !c.is_empty() && !c.chars().any(char::is_control) {
                        // The port field is digits-only.
                        if matches!(field, IndexField::Port) && !c.chars().all(|ch| ch.is_ascii_digit()) {
                            return;
                        }
                        buf.push_str(c);
                    }
                }
            }
        }
        cx.notify();
    }

    /// Fetch the current provider's model list (#53) off the UI thread; fills `index_models` and
    /// flips the status dot via `reachable`. Triggered on launch, provider/host/port edits, and Refresh.
    pub(crate) fn fetch_index_models(&mut self, cx: &mut Context<Self>) {
        let provider = self.index_provider.clone();
        let host = self.index_host.clone();
        let port = self.index_port.clone();
        let key = self.index_key.clone();
        cx.spawn(async move |this, cx| {
            let models = cx
                .background_executor()
                .spawn(async move {
                    daemon::discover().and_then(|d| d.index_models(&provider, &host, &port, &key).ok())
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Some(models) = models {
                    v.index_models = models;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-probe index-endpoint availability now (after editing the config), off the UI thread.
    pub(crate) fn probe_index_status(&mut self, cx: &mut Context<Self>) {
        self.save_settings();
        let url = self.index_chat_url();
        self.message = "checking index endpoint…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move { daemon::discover().and_then(|d| d.index_status(&url).ok()) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Some(s) = status {
                    v.message = if s.available {
                        "index endpoint reachable".into()
                    } else if s.configured {
                        "index endpoint not reachable".into()
                    } else {
                        "index endpoint not set".into()
                    };
                    v.index_status = s;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Build a finished capture's multimodal index (background on the daemon; progress over
    /// SSE into `LiveState.index_progress`). Uses the GUI-configured LM Studio endpoint.
    pub(crate) fn index_session(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        let provider = self.index_provider.clone();
        let host = self.index_host.clone();
        let port = self.index_port.clone();
        let model = self.index_model.clone();
        let rate = self.index_sample_rate;
        let preset = self.index_preset.clone();
        self.indexing.insert(id.clone());
        self.live.lock().unwrap().index_progress.insert(id.clone(), ("starting".into(), 0.0));
        self.message = format!("indexing {} ({preset})…", short_id(&id)).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let r = cx
                .background_executor()
                .spawn(async move { d.index(&id2, &provider, &host, &port, &model, rate, &preset) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    v.indexing.remove(&id);
                    v.live.lock().unwrap().index_progress.remove(&id);
                    v.message = format!("index failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }
}
