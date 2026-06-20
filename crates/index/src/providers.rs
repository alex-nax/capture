//! Index vision-LLM **providers** — compose endpoint/model-list URLs from a structured
//! `{provider, host, port}` config and list a provider's available models. Port of
//! `core/providers.py`.
//!
//! The GUI/Settings configures the index endpoint as a provider + host:port (not a raw
//! chat-completions URL), and the model field is a dropdown populated from the provider's
//! `/v1/models`. This module is the single source of truth for both the daemon and (via the
//! `/v1` contract) the GPUI app. A full `endpoint` URL is still honored everywhere and takes
//! precedence over the structured fields.
//!
//! OpenAI-compatible providers (LM Studio, Ollama, OpenAI, and most local servers) all expose
//! `/v1/chat/completions` + `/v1/models`; they differ only in host/port defaults and auth.

use std::time::Duration;

use serde_json::Value;

/// A vision-LLM provider's metadata. `fixed_base` providers ignore host/port (cloud); `base_url`
/// providers take a full base URL in the `host` field instead of host:port.
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    /// Scheme for host:port providers (`None` for fixed-base / base-url providers).
    pub scheme: Option<&'static str>,
    pub default_port: Option<u32>,
    /// A cloud provider's fixed `…/v1` base (host/port ignored).
    pub fixed_base: Option<&'static str>,
    /// `custom`: the `host` field carries the full base URL.
    pub base_url: bool,
    pub needs_key: bool,
}

/// The provider used when none is given / the id is unknown.
pub const DEFAULT_PROVIDER: &str = "lmstudio";

/// The provider catalog (mirrors `providers.PROVIDERS`).
pub const PROVIDERS: &[Provider] = &[
    Provider { id: "lmstudio", label: "LM Studio", scheme: Some("http"), default_port: Some(1234), fixed_base: None, base_url: false, needs_key: false },
    Provider { id: "ollama", label: "Ollama", scheme: Some("http"), default_port: Some(11434), fixed_base: None, base_url: false, needs_key: false },
    Provider { id: "openai", label: "OpenAI", scheme: None, default_port: None, fixed_base: Some("https://api.openai.com/v1"), base_url: false, needs_key: true },
    Provider { id: "custom", label: "Custom (base URL)", scheme: None, default_port: None, fixed_base: None, base_url: true, needs_key: false },
];

/// The provider meta for `id`, falling back to [`DEFAULT_PROVIDER`] for a blank/unknown id (mirrors
/// `PROVIDERS.get(provider or DEFAULT, PROVIDERS[DEFAULT])`).
pub fn provider(id: &str) -> &'static Provider {
    let id = if id.trim().is_empty() { DEFAULT_PROVIDER } else { id.trim() };
    PROVIDERS
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| PROVIDERS.iter().find(|p| p.id == DEFAULT_PROVIDER).unwrap())
}

/// The `…/v1` base URL for a provider config. Mirrors `providers._base`.
pub fn base(provider_id: &str, host: Option<&str>, port: Option<u32>) -> Result<String, String> {
    let p = provider(provider_id);
    if let Some(fb) = p.fixed_base {
        return Ok(fb.to_string());
    }
    if p.base_url {
        // custom: host IS the full base (may or may not end in /v1).
        let b = host.unwrap_or("").trim().trim_end_matches('/').to_string();
        if b.is_empty() {
            return Err("custom provider needs a base URL in `host`".into());
        }
        return Ok(if b.ends_with("/v1") || b.contains("/v1") { b } else { format!("{b}/v1") });
    }
    let h = host.unwrap_or("").trim();
    if h.is_empty() {
        return Err(format!("{} provider needs a host", p.id));
    }
    let scheme = p.scheme.unwrap_or("http");
    let port = port.or(p.default_port).unwrap_or(80);
    Ok(format!("{scheme}://{h}:{port}/v1"))
}

/// The chat-completions endpoint the vision client posts to.
pub fn chat_url(provider_id: &str, host: Option<&str>, port: Option<u32>) -> Result<String, String> {
    Ok(base(provider_id, host, port)? + "/chat/completions")
}

/// The model-list endpoint (`GET` → OpenAI `{data:[{id}]}`).
pub fn models_url(provider_id: &str, host: Option<&str>, port: Option<u32>) -> Result<String, String> {
    Ok(base(provider_id, host, port)? + "/models")
}

/// GET the provider's `/v1/models` and return the model ids (sorted). `[]` on any failure
/// (unreachable / unauthenticated) — never raises, mirroring `providers.list_models`.
pub fn list_models(provider_id: &str, host: Option<&str>, port: Option<u32>, key: Option<&str>) -> Vec<String> {
    let Ok(url) = models_url(provider_id, host, port) else {
        return Vec::new();
    };
    let Ok(client) = reqwest::blocking::Client::builder().timeout(Duration::from_secs(6)).build() else {
        return Vec::new();
    };
    let mut req = client.get(&url);
    if let Some(k) = key.map(str::trim).filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let body: Value = match req.send().and_then(|r| r.error_for_status()).and_then(|r| r.json()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut ids: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_provider_falls_back_to_default() {
        assert_eq!(provider("nope").id, DEFAULT_PROVIDER);
        assert_eq!(provider("").id, DEFAULT_PROVIDER);
        assert_eq!(provider("ollama").id, "ollama");
    }

    #[test]
    fn base_composes_per_provider() {
        // host:port providers use their scheme + default port (overridable).
        assert_eq!(base("lmstudio", Some("1.2.3.4"), None).unwrap(), "http://1.2.3.4:1234/v1");
        assert_eq!(base("lmstudio", Some("1.2.3.4"), Some(9000)).unwrap(), "http://1.2.3.4:9000/v1");
        assert_eq!(base("ollama", Some("h"), None).unwrap(), "http://h:11434/v1");
        // openai is a fixed cloud base (host/port ignored).
        assert_eq!(base("openai", None, None).unwrap(), "https://api.openai.com/v1");
        // custom: host is the full base; /v1 is appended only if absent.
        assert_eq!(base("custom", Some("http://x:5/v1"), None).unwrap(), "http://x:5/v1");
        assert_eq!(base("custom", Some("http://x:5/"), None).unwrap(), "http://x:5/v1");
    }

    #[test]
    fn base_requires_a_host_where_needed() {
        assert!(base("lmstudio", None, None).is_err());
        assert!(base("lmstudio", Some("  "), None).is_err());
        assert!(base("custom", Some(""), None).is_err());
    }

    #[test]
    fn chat_and_models_urls_suffix_the_base() {
        assert_eq!(chat_url("lmstudio", Some("h"), None).unwrap(), "http://h:1234/v1/chat/completions");
        assert_eq!(models_url("lmstudio", Some("h"), None).unwrap(), "http://h:1234/v1/models");
    }

    #[test]
    fn list_models_is_empty_when_unreachable() {
        // A host that won't answer → [] (never panics).
        assert!(list_models("lmstudio", Some("127.0.0.1"), Some(1), None).is_empty());
    }
}
