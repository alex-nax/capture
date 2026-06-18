//! App-local state TYPES and shared constants/helpers used across the GUI screens.
//!
//! Relocated verbatim from `app.rs` (#68 refactor — pure code relocation). These are the
//! enums/structs that aren't `CaptureApp` itself, plus the small free helper fns + the
//! Settings/picker constants that several screens reference.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::update;

/// Live data written by the SSE thread, read by render: the selected session's
/// transcript + screenshot, plus in-flight ASR model download progress (repo →
/// fraction 0..1; entries are removed on done/error).
#[derive(Default)]
pub(crate) struct LiveState {
    pub(crate) tracked: Option<String>,
    pub(crate) transcript: Vec<String>,
    pub(crate) last_shot: Option<String>,
    pub(crate) asr_progress: HashMap<String, f32>,
    pub(crate) retranscribe: HashMap<String, f32>, // session id -> re-transcribe fraction (0..1)
    pub(crate) retranscribe_done: Vec<String>,     // session ids that just finished (drained by poll)
    pub(crate) import_progress: Option<(String, f32)>, // active import: (phase, fraction), one at a time
    pub(crate) import_result: Option<Result<String, String>>, // Ok(session_id) / Err(msg), drained by poll
    pub(crate) index_progress: HashMap<String, (String, f32)>, // session id -> (phase, fraction)
    pub(crate) runtime_install: HashMap<String, f32>, // ASR runtime id -> install fraction (0..1)
    pub(crate) index_done: Vec<(String, Option<String>)>, // (session id, error?) — Some=failed; drained by poll
}

/// A destructive action awaiting confirmation in the modal.
#[derive(Clone)]
pub(crate) enum ConfirmKind {
    DeleteSession(String),                    // session id
    Prune(String, Vec<&'static str>, String), // session id, prune parts, body text
    Update(update::UpdateInfo),               // a newer GitHub release to install (#48)
}

/// Which index-endpoint text field a key event targets (#52), for the shared key handler.
#[derive(Clone, Copy)]
pub(crate) enum IndexField {
    Host,
    Port,
    Key,
}

/// The Settings screen's left-nav sections (#71). Selecting one switches the content pane;
/// each maps to one or two of the relocated panels. `settings_section` on `CaptureApp` drives it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    CaptureQuality,
    Transcription,
    Voice,
    IndexEndpoint,
    Skills,
    Permissions,
    Updates,
}
impl SettingsSection {
    pub(crate) const ALL: [SettingsSection; 7] = [
        SettingsSection::CaptureQuality,
        SettingsSection::Transcription,
        SettingsSection::Voice,
        SettingsSection::IndexEndpoint,
        SettingsSection::Skills,
        SettingsSection::Permissions,
        SettingsSection::Updates,
    ];
    pub(crate) fn label(&self) -> &'static str {
        match self {
            SettingsSection::CaptureQuality => "Capture quality",
            SettingsSection::Transcription => "Transcription",
            SettingsSection::Voice => "Voice recognition",
            SettingsSection::IndexEndpoint => "Index endpoint",
            SettingsSection::Skills => "Skills",
            SettingsSection::Permissions => "Permissions",
            SettingsSection::Updates => "App & updates",
        }
    }
    pub(crate) fn icon(&self) -> &'static str {
        match self {
            SettingsSection::CaptureQuality => "image",
            SettingsSection::Transcription => "mic",
            SettingsSection::Voice => "waveform",
            SettingsSection::IndexEndpoint => "list-tree",
            SettingsSection::Skills => "clipboard",
            SettingsSection::Permissions => "shield",
            SettingsSection::Updates => "refresh",
        }
    }
}

/// The session "playback" screen state (loaded from the session's on-disk artifacts).
#[derive(Default)]
pub(crate) struct PlaybackState {
    pub(crate) sid: String,
    pub(crate) finished: bool,
    pub(crate) loaded: bool,                          // disk read finished (finished sessions only)
    pub(crate) frames: Vec<(f64, String)>,            // (epoch_secs, screenshot path), time-sorted
    pub(crate) subs: Vec<(f64, f64, String, bool)>,   // (start, end, text, is_mic), start-sorted
    pub(crate) pos: f64,                              // playhead, epoch seconds
    pub(crate) t0: f64,                               // timeline start (first frame/segment)
    pub(crate) t1: f64,                               // timeline end
    pub(crate) playing: bool,                         // auto-advancing
    pub(crate) index_summary: Option<String>,         // root summary of the built multimodal index (#44)
    pub(crate) index_nodes: Option<usize>,            // node count of the built index
}

/// Transcription languages (Whisper) for the searchable dropdown: `(ISO code, English name)`.
/// `""` = auto-detect. Filtered by code or name as the user types.
pub(crate) const LANGUAGES: &[(&str, &str)] = &[
    ("", "Auto-detect"), ("en", "English"), ("zh", "Chinese"), ("de", "German"), ("es", "Spanish"),
    ("ru", "Russian"), ("ko", "Korean"), ("fr", "French"), ("ja", "Japanese"), ("pt", "Portuguese"),
    ("tr", "Turkish"), ("pl", "Polish"), ("ca", "Catalan"), ("nl", "Dutch"), ("ar", "Arabic"),
    ("sv", "Swedish"), ("it", "Italian"), ("id", "Indonesian"), ("hi", "Hindi"), ("fi", "Finnish"),
    ("vi", "Vietnamese"), ("he", "Hebrew"), ("uk", "Ukrainian"), ("el", "Greek"), ("ms", "Malay"),
    ("cs", "Czech"), ("ro", "Romanian"), ("da", "Danish"), ("hu", "Hungarian"), ("ta", "Tamil"),
    ("no", "Norwegian"), ("th", "Thai"), ("ur", "Urdu"), ("hr", "Croatian"), ("bg", "Bulgarian"),
    ("lt", "Lithuanian"), ("la", "Latin"), ("ml", "Malayalam"), ("cy", "Welsh"), ("sk", "Slovak"),
    ("te", "Telugu"), ("fa", "Persian"), ("lv", "Latvian"), ("bn", "Bengali"), ("sr", "Serbian"),
    ("az", "Azerbaijani"), ("sl", "Slovenian"), ("kn", "Kannada"), ("et", "Estonian"), ("mk", "Macedonian"),
    ("eu", "Basque"), ("is", "Icelandic"), ("hy", "Armenian"), ("ne", "Nepali"), ("mn", "Mongolian"),
    ("bs", "Bosnian"), ("kk", "Kazakh"), ("sq", "Albanian"), ("sw", "Swahili"), ("gl", "Galician"),
    ("mr", "Marathi"), ("pa", "Punjabi"), ("si", "Sinhala"), ("km", "Khmer"), ("af", "Afrikaans"),
    ("be", "Belarusian"), ("gu", "Gujarati"), ("am", "Amharic"), ("yi", "Yiddish"), ("lo", "Lao"),
    ("uz", "Uzbek"), ("fo", "Faroese"), ("ps", "Pashto"), ("tg", "Tajik"), ("my", "Burmese"),
];

/// Screenshot resolution presets for the Settings panel (label, "WxH" or None = native).
pub(crate) const RES_PRESETS: [(&str, Option<&str>); 4] = [
    ("Native", None),
    ("1440p", Some("2560x1440")),
    ("1080p", Some("1920x1080")),
    ("720p", Some("1280x720")),
];

/// Start-capture presets for the picker popup: `(id, label, hint)`. The `id` is sent to
/// the daemon (which records it + defaults a later index to it); see `start_with_preset`
/// for how each maps to the mic / screenshots toggles. Mirrors the backend contract.
pub(crate) const CAPTURE_PRESETS: &[(&str, &str, &str)] = &[
    ("auto", "Auto", "Classify per frame; adapts as the screen changes."),
    ("meeting", "Meeting", "Video call/standup — mic on; captures participants, active speaker, task assignments."),
    ("coding", "Coding / tutorial", "An IDE or coding video — extracts verbatim code at high resolution."),
    ("lecture", "Lecture / explainer", "A slide/explainer tutorial — topics, key points, formulas."),
    ("general", "General", "Plain capture; index auto-classifies."),
    ("custom", "Custom", "Use your current capture settings — set resolution, mic, and language in Settings."),
];

/// Index-endpoint providers for the Settings selector (#52): `(id, label, default_port, needs_key, is_base_url)`.
/// Mirrors the daemon's GET /v1/index/providers list — hardcoded here (simpler than a fetch + the
/// daemon composes the URL from provider+host either way). `is_base_url` providers (custom) carry a
/// full base URL in the host field and hide the port.
pub(crate) const INDEX_PROVIDERS: &[(&str, &str, &str, bool, bool)] = &[
    ("lmstudio", "LM Studio", "1234", false, false),
    ("ollama", "Ollama", "11434", false, false),
    ("openai", "OpenAI", "", true, false),
    ("custom", "Custom (base URL)", "", false, true),
];

/// Look up a provider's `(default_port, needs_key, is_base_url)` (defaults for an unknown id).
pub(crate) fn index_provider_meta(id: &str) -> (&'static str, bool, bool) {
    INDEX_PROVIDERS
        .iter()
        .find(|(pid, _, _, _, _)| *pid == id)
        .map(|(_, _, port, needs_key, is_base)| (*port, *needs_key, *is_base))
        .unwrap_or(("", false, false))
}

pub(crate) fn default_out_dir() -> String {
    dirs::home_dir()
        .map(|h| h.join(".capture").join("runs").to_string_lossy().into_owned())
        .unwrap_or_else(|| "/tmp/capture-runs".into())
}

/// Where the GUI persists its capture-quality preferences (sibling of `daemon.json`).
pub(crate) fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".capture").join("gui-settings.json"))
}

/// File picker for the "Import…" action: returns the chosen media file's path, or None if
/// the user cancelled / the dialog failed. macOS uses `osascript`; Windows a PowerShell
/// `OpenFileDialog` (no extra crate — `powershell.exe` is signed, so Smart App Control
/// doesn't block it); other platforms try `zenity`. Blocking — call it off the UI thread.
pub(crate) fn pick_media_file() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"POSIX path of (choose file with prompt "Import audio or video as a session" of type {"public.audio","public.movie","public.mpeg-4","com.apple.quicktime-movie"})"#;
        let out = std::process::Command::new("osascript").arg("-e").arg(script).output().ok()?;
        if !out.status.success() {
            return None; // user cancelled (AppleScript error -128) or osascript unavailable
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() { None } else { Some(path) }
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.OpenFileDialog; $f.Title = 'Import audio or video as a session'; $f.Filter = 'Media|*.mp4;*.mov;*.m4a;*.mp3;*.wav;*.mkv;*.webm;*.aac;*.flac|All files|*.*'; if ($f.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Out.Write($f.FileName) }"#;
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", script])
            .output()
            .ok()?;
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() { None } else { Some(path) }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let out = std::process::Command::new("zenity")
            .args(["--file-selection", "--title=Import audio or video as a session"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() { None } else { Some(path) }
    }
}

/// The persisted index-endpoint config (#52): `(provider, host, port, key)`. Migrated from the
/// legacy free-text `index_url` (`http://HOST:PORT/…`) when the structured keys are absent.
pub(crate) struct IndexCfg {
    pub(crate) provider: String,
    pub(crate) host: String,
    pub(crate) port: String,
    pub(crate) key: String,
}

/// Parse a legacy `http://HOST:PORT/…` index URL into `(host, port)` (lmstudio-shaped). Returns
/// None if it doesn't look like an `http(s)://host:port` URL (then the URL is just ignored).
pub(crate) fn migrate_index_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split('/').next().unwrap_or(rest); // host[:port]
    let (host, port) = authority.split_once(':')?;
    if host.is_empty() || port.parse::<u32>().is_err() {
        return None;
    }
    Some((host.to_string(), port.to_string()))
}

/// `(shot_format, shot_res_ix, jpeg_quality, …, index_cfg, model, sample_rate, preset)` loaded from
/// `gui-settings.json`, or the defaults if the file is missing/unreadable. So a chosen quality
/// survives a GUI relaunch (the settings live in the window process, not the daemon).
pub(crate) fn load_settings() -> (String, usize, u32, Option<String>, bool, IndexCfg, String, f64, String) {
    let def = ("png".to_string(), 0usize, 80u32);
    let def_cfg = || IndexCfg {
        provider: "lmstudio".into(),
        host: String::new(),
        port: index_provider_meta("lmstudio").0.to_string(),
        key: String::new(),
    };
    let Some(v) = settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return (def.0, def.1, def.2, None, true, def_cfg(), String::new(), 0.25, "auto".into());
    };
    let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    // Structured config if present, else migrate the legacy `index_url` (host:port → lmstudio).
    let cfg = if v.get("index_provider").is_some() || v.get("index_host").is_some() {
        let provider = {
            let p = str_of("index_provider");
            if p.is_empty() { "lmstudio".to_string() } else { p }
        };
        let port = {
            let p = str_of("index_port");
            if p.is_empty() { index_provider_meta(&provider).0.to_string() } else { p }
        };
        IndexCfg { provider, host: str_of("index_host"), port, key: str_of("index_key") }
    } else if let Some((host, port)) = migrate_index_url(&str_of("index_url")) {
        IndexCfg { provider: "lmstudio".into(), host, port, key: String::new() }
    } else {
        def_cfg()
    };
    (
        v.get("shot_format").and_then(|x| x.as_str()).unwrap_or(&def.0).to_string(),
        v.get("shot_res_ix").and_then(|x| x.as_u64()).map_or(def.1, |n| n as usize),
        v.get("jpeg_quality").and_then(|x| x.as_u64()).map_or(def.2, |n| n as u32),
        v.get("mic_device").and_then(|x| x.as_str()).map(String::from),
        v.get("capture_screenshots").and_then(|x| x.as_bool()).unwrap_or(true),
        cfg,
        v.get("index_model").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        v.get("index_sample_rate").and_then(|x| x.as_f64()).unwrap_or(0.25),
        v.get("index_preset").and_then(|x| x.as_str()).unwrap_or("auto").to_string(),
    )
}

pub(crate) fn short_id(id: &str) -> &str {
    id.rsplit('-').next().unwrap_or(id)
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Seconds → `m:ss` for the playback time read-out.
pub(crate) fn fmt_dur(s: f64) -> String {
    let s = s.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Parse an ISO-8601 stamp to epoch seconds. Handles both transcript form
/// (`2026-06-16T11:00:34.937Z`) and the screenshot-filename form
/// (`2026-06-16T11-00-34.937Z`) — the separators sit at fixed indices, so we read
/// by position regardless of `:`/`-`. No `chrono` dep (civil-days arithmetic).
pub(crate) fn parse_iso_epoch(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let ms = if b.len() >= 23 && b[19] == b'.' {
        s.get(20..23).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0)
    } else {
        0
    };
    // days_from_civil (Howard Hinnant): civil date -> days since 1970-01-01.
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = (if y2 >= 0 { y2 } else { y2 - 399 }) / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as f64 * 86400.0 + h as f64 * 3600.0 + mi as f64 * 60.0 + se as f64 + ms as f64 / 1000.0)
}

/// Read a finished session's playback data off disk: screenshots (time-sorted) +
/// transcript/mic-transcript segments, and the timeline bounds. Runs off the main thread.
pub(crate) fn load_playback_data(
    dir: &str,
) -> (Vec<(f64, String)>, Vec<(f64, f64, String, bool)>, f64, f64) {
    let base = std::path::Path::new(dir);
    let mut frames: Vec<(f64, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(base.join("screenshots")) {
        for e in rd.flatten() {
            let p = e.path();
            let is_img = p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                matches!(x, "png" | "jpg" | "jpeg" | "tiff" | "gif" | "bmp")
            });
            if !is_img {
                continue;
            }
            if let Some(t) = p.file_stem().and_then(|s| s.to_str()).and_then(parse_iso_epoch) {
                frames.push((t, p.to_string_lossy().into_owned()));
            }
        }
    }
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut subs: Vec<(f64, f64, String, bool)> = Vec::new();
    for (fname, is_mic) in [("transcript.jsonl", false), ("mic_transcript.jsonl", true)] {
        if let Ok(text) = std::fs::read_to_string(base.join(fname)) {
            for ln in text.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(ln) else { continue };
                let Some(start) = v.get("start").and_then(|x| x.as_str()).and_then(parse_iso_epoch)
                else {
                    continue;
                };
                let end = v
                    .get("end")
                    .and_then(|x| x.as_str())
                    .and_then(parse_iso_epoch)
                    .unwrap_or(start + 2.0);
                let txt = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
                if !txt.is_empty() {
                    subs.push((start, end, txt, is_mic));
                }
            }
        }
    }
    subs.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for (t, _) in &frames {
        lo = lo.min(*t);
        hi = hi.max(*t);
    }
    for (s, e, _, _) in &subs {
        lo = lo.min(*s);
        hi = hi.max(*e);
    }
    if !lo.is_finite() {
        lo = 0.0;
        hi = 0.0;
    }
    (frames, subs, lo, hi)
}
