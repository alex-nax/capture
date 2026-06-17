"""Index vision-LLM **providers** — compose endpoint/model-list URLs from a structured
``{provider, host, port}`` config and list a provider's available models.

The GUI/Settings now configures the index endpoint as a provider + host:port (not a raw
chat-completions URL), and the model field is a dropdown populated from the provider's
``/v1/models``. This module is the single source of truth for both the daemon and (via the
``/v1`` contract) the GPUI app. A full ``endpoint`` URL is still honored everywhere for
back-compat (it takes precedence over the structured fields).

OpenAI-compatible providers (LM Studio, Ollama, OpenAI, and most local servers) all expose
``/v1/chat/completions`` + ``/v1/models``; they differ only in host/port defaults and auth.
"""

from __future__ import annotations

import json
import urllib.request
from urllib.parse import urlparse

#: provider id -> metadata. ``fixed`` providers ignore host/port (cloud); ``base`` providers
#: take a full base URL in the ``host`` field instead of host:port.
PROVIDERS: dict[str, dict] = {
    "lmstudio": {"label": "LM Studio", "scheme": "http", "default_port": 1234, "needs_key": False},
    "ollama":   {"label": "Ollama",    "scheme": "http", "default_port": 11434, "needs_key": False},
    "openai":   {"label": "OpenAI",    "fixed_base": "https://api.openai.com/v1", "needs_key": True},
    "custom":   {"label": "Custom (base URL)", "base_url": True, "needs_key": False},
}
DEFAULT_PROVIDER = "lmstudio"


def _base(provider: str, host: str | None, port: int | None) -> str:
    """The ``…/v1`` base URL for a provider config."""
    p = PROVIDERS.get(provider or DEFAULT_PROVIDER, PROVIDERS[DEFAULT_PROVIDER])
    if p.get("fixed_base"):
        return p["fixed_base"]
    if p.get("base_url"):  # custom: host IS the full base (may or may not end in /v1)
        b = (host or "").strip().rstrip("/")
        if not b:
            raise ValueError("custom provider needs a base URL in `host`")
        return b if b.endswith("/v1") or "/v1" in urlparse(b).path else b + "/v1"
    h = (host or "").strip()
    if not h:
        raise ValueError(f"{provider} provider needs a host")
    return f"{p['scheme']}://{h}:{port or p['default_port']}/v1"


def chat_url(provider: str, host: str | None, port: int | None) -> str:
    """The chat-completions endpoint the vision client posts to."""
    return _base(provider, host, port) + "/chat/completions"


def models_url(provider: str, host: str | None, port: int | None) -> str:
    """The model-list endpoint (``GET`` → OpenAI ``{data:[{id}]}``)."""
    return _base(provider, host, port) + "/models"


def list_models(provider: str, host: str | None, port: int | None,
                key: str | None = None, timeout: float = 6.0) -> list[str]:
    """GET the provider's ``/v1/models`` and return the model ids (sorted). ``[]`` on any
    failure (unreachable / unauthenticated) — the caller surfaces a hint, never raises."""
    try:
        req = urllib.request.Request(models_url(provider, host, port))
        if key:
            req.add_header("Authorization", f"Bearer {key}")
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = json.load(r)
        ids = [m.get("id") for m in (data.get("data") or []) if isinstance(m, dict) and m.get("id")]
        return sorted(ids)
    except Exception:
        return []
