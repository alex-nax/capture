//! Per-session actions on finished captures: delete, prune, re-transcribe, reveal folder,
//! copy a summary prompt. Relocated verbatim from `app.rs` (#68).

use gpui::{ClipboardItem, Context};

use crate::app::CaptureApp;
use crate::state::short_id;

impl CaptureApp {
    /// Reveal a capture's output folder in the OS file manager (macOS `open` / Windows
    /// `explorer` / else `xdg-open`).
    pub(crate) fn open_folder(&mut self, dir: String, cx: &mut Context<Self>) {
        if dir.is_empty() {
            self.message = "no folder for this capture".into();
            cx.notify();
            return;
        }
        #[cfg(target_os = "macos")]
        let ok = std::process::Command::new("open").arg(&dir).spawn().is_ok();
        #[cfg(target_os = "windows")]
        let ok = std::process::Command::new("explorer").arg(&dir).spawn().is_ok();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let ok = std::process::Command::new("xdg-open").arg(&dir).spawn().is_ok();
        self.message = if ok {
            format!("opened {dir}").into()
        } else {
            "could not open folder".into()
        };
        cx.notify();
    }

    /// Copy a ready-to-paste prompt that asks a coding agent to summarize this
    /// capture (points it at the session dir's transcript + screenshots + logs).
    pub(crate) fn copy_summary_prompt(&mut self, dir: String, cx: &mut Context<Self>) {
        let prompt = format!(
            "Summarize this screen + audio capture for me.\n\n\
             The capture is in this folder:\n  {dir}\n\n\
             It contains:\n\
             - transcript.jsonl — timestamped speech-to-text (one JSON object per line)\n\
             - screenshots/ — timestamped frames of the captured window\n\
             - session.json — metadata (app/window, timing, counts)\n\
             - output.log / stdout.log / stderr.log — process logs (if a launched process)\n\n\
             Read the transcript and skim the screenshots, then give me:\n\
             1. A concise summary of what happened / was discussed.\n\
             2. Key points, decisions, and action items.\n\
             3. Anything notable on screen the transcript misses.\n\
             Cite timestamps where useful."
        );
        cx.write_to_clipboard(ClipboardItem::new_string(prompt));
        self.message = "copied a summarization prompt — paste it into your coding agent".into();
        cx.notify();
    }

    /// Delete a finished capture (its folder + record) via the daemon.
    pub(crate) fn delete_session(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = format!("deleting {}…", short_id(&id)).into();
        if self.selected_session.as_deref() == Some(id.as_str()) {
            self.selected_session = None;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let id = id.clone();
                    async move { d.delete(&id) }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(()) => "deleted capture".into(),
                    Err(e) => format!("delete failed: {e}").into(),
                };
                if v.playback.as_ref().map(|p| p.sid.as_str()) == Some(id.as_str()) {
                    v.playback = None; // close the playback screen for a deleted session
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Prune a finished capture's artifacts (frees disk). Reloads the playback view if
    /// the pruned session is open, so the new state (fewer frames / no audio) shows.
    pub(crate) fn prune(&mut self, id: String, parts: Vec<&'static str>, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = format!("pruning {}…", short_id(&id)).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let r = cx
                .background_executor()
                .spawn(async move { d.prune(&id2, &parts) })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(()) => "pruned".into(),
                    Err(e) => format!("prune failed: {e}").into(),
                };
                if v.playback.as_ref().map(|p| p.sid.as_str()) == Some(id.as_str()) {
                    v.select_session(id.clone(), cx); // reload frames/subs/caps
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-transcribe a finished capture's audio (background on the daemon; progress over
    /// SSE into `LiveState.retranscribe`). The open session reloads when it completes.
    pub(crate) fn retranscribe(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.retranscribing = Some(id.clone());
        self.live.lock().unwrap().retranscribe.insert(id.clone(), 0.0);
        self.message = format!("re-transcribing {}…", short_id(&id)).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let r = cx
                .background_executor()
                .spawn(async move { d.retranscribe(&id2, None) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    v.retranscribing = None;
                    v.live.lock().unwrap().retranscribe.remove(&id);
                    v.message = format!("re-transcribe failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }
}
