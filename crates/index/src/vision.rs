//! Remote multimodal (vision) chat client for session indexing — feature #44/#62.
//!
//! A faithful Rust port of `core/vision_client.py`: an OpenAI-compatible
//! `/v1/chat/completions` client pointed at an **LM Studio** server on the LAN running a Qwen
//! vision model. `reqwest` (blocking) replaces urllib; the `image` crate replaces sips/Pillow.
//! Two call shapes:
//!
//! - [`VisionClient::caption_image`] — a vision call: the screenshot is downscaled +
//!   JPEG-encoded (via the `image` crate, falling back to the raw PNG) and sent as a base64
//!   `image_url` data URI alongside the text prompt.
//! - [`VisionClient::combine`] — a text-only call used to fuse child summaries up the tree.
//!
//! Disabled unless `CAPTURE_INDEX_URL` is set; the indexer only runs when [`VisionClient::available`]
//! (a `/v1/models` preflight) also succeeds.
//!
//! Configuration (env; each is overridable per call site):
//!   CAPTURE_INDEX_URL          full chat endpoint, e.g.
//!                              `http://192.168.31.217:1234/v1/chat/completions` (required)
//!   CAPTURE_INDEX_MODEL        model id (e.g. `qwen3.5-9b`)
//!   CAPTURE_INDEX_KEY          bearer token (optional; LM Studio usually needs none)
//!   CAPTURE_INDEX_TIMEOUT      per-call seconds (default 120)
//!   CAPTURE_INDEX_MAX_IMAGE_PX longest-edge downscale before upload (default 1024)
//!   CAPTURE_INDEX_MAX_TOKENS   completion budget (default 2048)

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

/// Errors surface as `RuntimeError`-style strings (Python raises `RuntimeError`); the daemon maps later.
pub type Result<T> = std::result::Result<T, String>;

pub const ENV_URL: &str = "CAPTURE_INDEX_URL";
pub const ENV_MODEL: &str = "CAPTURE_INDEX_MODEL";
pub const ENV_KEY: &str = "CAPTURE_INDEX_KEY";
pub const ENV_TIMEOUT: &str = "CAPTURE_INDEX_TIMEOUT";
pub const ENV_MAX_PX: &str = "CAPTURE_INDEX_MAX_IMAGE_PX";
pub const ENV_MAX_TOKENS: &str = "CAPTURE_INDEX_MAX_TOKENS";

/// The chat endpoint to use: an explicit override else the env. `""` if unset.
pub fn configured_url(override_: Option<&str>) -> String {
    override_
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var(ENV_URL).ok())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Build a client from explicit args (GUI/request overrides) falling back to env.
/// Returns `Err` if no URL is configured (the route guards this earlier).
pub fn load(url: Option<&str>, model: Option<&str>) -> Result<VisionClient> {
    let u = configured_url(url);
    if u.is_empty() {
        return Err(format!(
            "{ENV_URL} is not set (e.g. http://192.168.31.217:1234/v1/chat/completions)"
        ));
    }
    let model = model
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var(ENV_MODEL).ok().filter(|s| !s.is_empty()));
    let api_key = std::env::var(ENV_KEY).ok().filter(|s| !s.is_empty());
    let timeout = std::env::var(ENV_TIMEOUT)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(120.0);
    let max_px = std::env::var(ENV_MAX_PX)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1024);
    let max_tokens = std::env::var(ENV_MAX_TOKENS)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(2048);
    VisionClient::new(u, model, api_key, timeout, max_px, max_tokens)
}

/// `(base64, mime)` for a screenshot: downscale + JPEG (longest edge ≤ `max_px`) to shrink the
/// payload via the `image` crate; on ANY failure send the raw PNG bytes unchanged. The raw-PNG
/// fallback keeps indexing working even if the encode fails (just with a heavier payload).
fn encode_image(path: &Path, max_px: u32) -> Result<(String, &'static str)> {
    if max_px > 0 {
        if let Some(jpg) = downscale_jpeg(path, max_px) {
            return Ok((
                base64::engine::general_purpose::STANDARD.encode(&jpg),
                "image/jpeg",
            ));
        }
    }
    let raw = std::fs::read(path).map_err(|e| format!("read image {}: {e}", path.display()))?;
    Ok((
        base64::engine::general_purpose::STANDARD.encode(&raw),
        "image/png",
    ))
}

/// Cross-platform downscale to JPEG (longest edge ≤ `max_px`, aspect preserved, RGB8, quality 85)
/// via the `image` crate — like Pillow `thumbnail`. `None` on any failure (caller falls back to raw PNG).
fn downscale_jpeg(path: &Path, max_px: u32) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    if w == 0 || h == 0 {
        return None;
    }
    // Pillow thumbnail only ever shrinks (longest edge ≤ max_px) and preserves aspect ratio.
    let longest = w.max(h);
    let resized = if longest > max_px {
        let scale = max_px as f64 / longest as f64;
        let nw = ((w as f64 * scale).round() as u32).max(1);
        let nh = ((h as f64 * scale).round() as u32).max(1);
        image::imageops::thumbnail(&rgb, nw, nh)
    } else {
        rgb
    };
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    enc.encode(
        resized.as_raw(),
        resized.width(),
        resized.height(),
        image::ExtendedColorType::Rgb8,
    )
    .ok()?;
    Some(buf)
}

pub struct VisionClient {
    pub url: String,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub timeout: f64,
    pub max_px: u32,
    pub max_tokens: u32,
    http: reqwest::blocking::Client,
}

impl VisionClient {
    fn new(
        url: String,
        model: Option<String>,
        api_key: Option<String>,
        timeout: f64,
        max_px: u32,
        max_tokens: u32,
    ) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs_f64(timeout))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        Ok(VisionClient {
            url,
            model,
            api_key,
            timeout,
            max_px,
            max_tokens,
            http,
        })
    }

    // -- availability ---------------------------------------------------------

    fn models_url(&self) -> String {
        let u = &self.url;
        for suffix in ["/chat/completions", "/completions"] {
            if let Some(stripped) = u.strip_suffix(suffix) {
                return format!("{stripped}/models");
            }
        }
        u.clone()
    }

    /// True iff the endpoint answers a `GET /v1/models` (the preflight that gates indexing).
    /// Never panics/errors — returns false on any failure.
    pub fn available(&self) -> bool {
        let mut req = self
            .http
            .get(self.models_url())
            .timeout(Duration::from_secs(5));
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        match req.send() {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    // -- calls ----------------------------------------------------------------

    /// Vision call: describe `image_path` per `prompt`. Returns the model text.
    /// `max_px` overrides the client default downscale for this call (e.g. higher res for code).
    pub fn caption_image(
        &self,
        image_path: &Path,
        prompt: &str,
        max_px: Option<u32>,
    ) -> Result<String> {
        let content = self.image_content(image_path, prompt, max_px)?;
        self.chat(json!([{ "role": "user", "content": content }]), None)
    }

    /// STRUCTURED vision call: returns a JSON object validated against `schema` (an OpenAI
    /// `json_schema` response_format). Returns `{}` (an empty JSON object) on a non-JSON reply.
    ///
    /// KEY (researched against LM Studio): structured output uses llama.cpp grammar-constrained
    /// sampling, which forbids a reasoning model's `<think>` block and yields EMPTY content. The
    /// fix is `reasoning_effort: "none"` — sent verbatim in the request extra.
    pub fn structured_image(
        &self,
        image_path: &Path,
        prompt: &str,
        schema: &Value,
        max_px: Option<u32>,
    ) -> Result<Value> {
        let extra = json!({
            "response_format": {
                "type": "json_schema",
                "json_schema": { "name": "frame", "strict": true, "schema": schema },
            },
            "reasoning_effort": "none",
        });
        let content = self.image_content(image_path, prompt, max_px)?;
        let text = self.chat(
            json!([{ "role": "user", "content": content }]),
            Some(extra),
        )?;
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => Ok(v),
            _ => Ok(json!({})),
        }
    }

    /// Text-only call: fuse child summaries (+ transcript) into a range summary. Reasoning off —
    /// it's summarization (faster, and avoids the reasoning-model empty-content issue).
    pub fn combine(&self, prompt: &str) -> Result<String> {
        self.chat(
            json!([{ "role": "user", "content": prompt }]),
            Some(json!({ "reasoning_effort": "none" })),
        )
    }

    fn image_content(&self, image_path: &Path, prompt: &str, max_px: Option<u32>) -> Result<Value> {
        let (b64, mime) = encode_image(image_path, max_px.unwrap_or(self.max_px))?;
        Ok(json!([
            { "type": "text", "text": prompt },
            { "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{b64}") } },
        ]))
    }

    fn chat(&self, messages: Value, extra: Option<Value>) -> Result<String> {
        const RETRIES: u32 = 3;
        // max_tokens is load-bearing for REASONING models (e.g. Qwen3.5): they spend most of the
        // completion budget on `reasoning_content`, so a small/absent cap leaves the actual
        // `content` empty (finish_reason=length). A generous cap keeps room for both.
        let mut payload = json!({
            "model": self.model.clone().unwrap_or_else(|| "local".to_string()),
            "messages": messages,
            "temperature": 0.2,
            "stream": false,
            "max_tokens": self.max_tokens,
        });
        if let Some(extra) = extra {
            if let (Some(obj), Some(ex)) = (payload.as_object_mut(), extra.as_object()) {
                for (k, v) in ex {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let mut last: Option<String> = None;
        let mut empties: u32 = 0;
        for attempt in 0..RETRIES {
            let mut req = self
                .http
                .post(&self.url)
                .header("Content-Type", "application/json");
            if let Some(key) = &self.api_key {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
            match req.json(&payload).send() {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        match resp.json::<Value>() {
                            Ok(body) => {
                                let text = extract_text(&body);
                                if !text.is_empty() {
                                    return Ok(text);
                                }
                                // Empty content (transient, or reasoning exhausted budget) — retry.
                                empties += 1;
                                last = Some("model returned empty content".to_string());
                            }
                            Err(e) => last = Some(format!("parse index response: {e}")),
                        }
                    } else {
                        let code = status.as_u16();
                        let detail: String = resp
                            .text()
                            .unwrap_or_default()
                            .chars()
                            .take(300)
                            .collect();
                        last = Some(format!("HTTP {code} from index endpoint: {detail}"));
                        // 4xx (bad request/model) won't fix on retry — fail fast.
                        if (400..500).contains(&code) {
                            break;
                        }
                    }
                }
                // connection refused / timeout — retry with backoff.
                Err(e) => last = Some(e.to_string()),
            }
            if attempt < RETRIES - 1 {
                std::thread::sleep(Duration::from_secs_f64(0.5 * (attempt as f64 + 1.0)));
            }
        }
        // If every attempt merely returned EMPTY content (no real error), degrade gracefully to ""
        // so one bad node doesn't abort a whole index build; a real error still raises.
        if empties == RETRIES {
            log_warn(&format!(
                "index: model returned empty content after {RETRIES} attempts"
            ));
            return Ok(String::new());
        }
        Err(format!(
            "vision call failed after {RETRIES} attempt(s): {}",
            last.unwrap_or_else(|| "unknown error".to_string())
        ))
    }
}

/// Best-effort warn log without pulling in a logging framework (the Python uses `log.warning`).
fn log_warn(msg: &str) {
    eprintln!("WARN capture-index::vision: {msg}");
}

/// Pull the assistant text out of an OpenAI chat-completions response.
/// `content` may be a STRING or a LIST of parts (`[{type,text}]`); join the parts' `text`.
/// Returns `""` on any shape error.
pub fn extract_text(payload: &Value) -> String {
    let content = match payload
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
    {
        Some(c) => c,
        None => return String::new(),
    };
    let text = if let Some(s) = content.as_str() {
        s.to_string()
    } else if let Some(parts) = content.as_array() {
        // some servers return content parts: [{type, text}, ...]
        parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<String>()
    } else {
        return String::new();
    };
    text.trim().to_string()
}

/// Pull a classification label from a free-text reply. Prefers a value following a
/// `content_type`/`type` marker, else the first enum value that appears as a word; falls back to
/// `"other"` (or the last enum entry).
pub fn extract_label(text: &str, enum_: &[&str]) -> String {
    let low = text.to_lowercase();
    let fallback = if enum_.contains(&"other") {
        "other".to_string()
    } else {
        enum_.last().map(|s| s.to_string()).unwrap_or_else(|| "other".to_string())
    };
    if low.is_empty() {
        return fallback;
    }
    // r"content[_ ]?type[\"'*\s:]+([a-z_]+)" — a content_type/type marker followed by a label.
    if let Some(label) = capture_marker(&low) {
        if enum_.contains(&label.as_str()) {
            return label;
        }
    }
    // First enum value (excluding "other") that appears as a whole word.
    for v in enum_ {
        if *v != "other" && word_match(&low, v) {
            return (*v).to_string();
        }
    }
    fallback
}

/// Port of `re.search(r"content[_ ]?type[\"'*\s:]+([a-z_]+)", low)` — returns the captured label.
fn capture_marker(low: &str) -> Option<String> {
    // Find any occurrence of "content_type"/"content type"/"contenttype" or bare "type".
    // The Python regex requires the literal "content" then optional "_"/" " then "type".
    for marker in ["content_type", "content type", "contenttype", "type"] {
        let mut from = 0;
        while let Some(idx) = low[from..].find(marker) {
            let after = from + idx + marker.len();
            // Require at least one separator char from [\"'*\s:] before the label, like the regex.
            let bytes = low.as_bytes();
            let mut i = after;
            let mut saw_sep = false;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if c == '"' || c == '\'' || c == '*' || c == ':' || c.is_whitespace() {
                    saw_sep = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if saw_sep {
                // Capture [a-z_]+.
                let start = i;
                while i < bytes.len() {
                    let c = bytes[i] as char;
                    if c.is_ascii_lowercase() || c == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                if i > start {
                    return Some(low[start..i].to_string());
                }
            }
            from = after;
        }
    }
    None
}

/// Port of `re.search(rf"\b{re.escape(v)}\b", low)` — a whole-word match for `v` in `low`.
fn word_match(low: &str, v: &str) -> bool {
    if v.is_empty() {
        return false;
    }
    let bytes = low.as_bytes();
    let vlen = v.len();
    let mut from = 0;
    while let Some(idx) = low[from..].find(v) {
        let pos = from + idx;
        let before_ok = pos == 0 || !is_word_char(bytes[pos - 1] as char);
        let after = pos + vlen;
        let after_ok = after >= bytes.len() || !is_word_char(bytes[after] as char);
        if before_ok && after_ok {
            return true;
        }
        from = pos + 1;
    }
    false
}

/// `\w` per Python's `re`: ASCII alphanumerics + underscore (the labels are lowercased ASCII).
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_text_string_content() {
        let payload = json!({
            "choices": [{ "message": { "content": "  hello world  " } }]
        });
        assert_eq!(extract_text(&payload), "hello world");
    }

    #[test]
    fn extract_text_list_of_parts() {
        let payload = json!({
            "choices": [{ "message": { "content": [
                { "type": "text", "text": "foo " },
                { "type": "text", "text": "bar" },
                { "type": "image_url" }
            ] } }]
        });
        assert_eq!(extract_text(&payload), "foo bar");
    }

    #[test]
    fn extract_text_malformed_returns_empty() {
        assert_eq!(extract_text(&json!({})), "");
        assert_eq!(extract_text(&json!({ "choices": [] })), "");
        assert_eq!(extract_text(&json!({ "choices": [{ "message": {} }] })), "");
        assert_eq!(
            extract_text(&json!({ "choices": [{ "message": { "content": 42 } }] })),
            ""
        );
    }

    #[test]
    fn extract_label_marker() {
        let enum_ = ["meeting", "code", "slides", "other"];
        assert_eq!(
            extract_label("The content_type: meeting here.", &enum_),
            "meeting"
        );
        // Marker label not in enum → falls through to word match / fallback.
        assert_eq!(
            extract_label("content_type: webpage", &enum_),
            "other"
        );
    }

    #[test]
    fn extract_label_bare_enum_word() {
        let enum_ = ["meeting", "code", "slides", "other"];
        assert_eq!(
            extract_label("This looks like some code on screen", &enum_),
            "code"
        );
    }

    #[test]
    fn extract_label_no_match_fallback() {
        let enum_ = ["meeting", "code", "slides", "other"];
        assert_eq!(extract_label("nothing relevant here", &enum_), "other");
        assert_eq!(extract_label("", &enum_), "other");
        // No "other" in enum → fall back to last entry.
        let enum2 = ["meeting", "code", "misc"];
        assert_eq!(extract_label("nothing relevant", &enum2), "misc");
    }

    #[test]
    fn encode_image_downscales_preserving_aspect() {
        // 4000×10 RGB → longest edge must drop to ≤ 1024, aspect preserved.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("capture_index_vision_test_{}.png", std::process::id()));
        let img = image::RgbImage::from_fn(4000, 10, |x, _| {
            image::Rgb([(x % 256) as u8, 0, 128])
        });
        img.save(&path).expect("save test png");

        let (b64, mime) = encode_image(&path, 1024).expect("encode");
        assert_eq!(mime, "image/jpeg");

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("valid base64");
        // Decode the JPEG and check the downscale happened with aspect preserved.
        let decoded = image::load_from_memory(&bytes).expect("valid jpeg");
        let (w, h) = (decoded.width(), decoded.height());
        assert!(w.max(h) <= 1024, "longest edge {} should be ≤ 1024", w.max(h));
        // 4000×10 scaled by 1024/4000 → ~1024×3 (rounds, min 1); aspect preserved (much wider than tall).
        assert!(w > h, "aspect preserved: {w}×{h}");
        assert_eq!(w, 1024);

        let _ = std::fs::remove_file(&path);
    }
}
