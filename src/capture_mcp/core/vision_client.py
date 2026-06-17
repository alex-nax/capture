"""Remote multimodal (vision) chat client for session indexing — feature #44.

A **stdlib-only** OpenAI-compatible `/v1/chat/completions` client (urllib + base64 +
json), pointed at an **LM Studio** server on the LAN running a Qwen vision model. Two
call shapes:

- ``caption_image(path, prompt)`` — a vision call: the screenshot is downscaled +
  JPEG-encoded (via ``sips``, falling back to the raw PNG) and sent as a base64
  ``image_url`` data URI alongside the text prompt.
- ``combine(prompt)`` — a text-only call used to fuse child summaries up the tree.

Mirrors ``asr/openai_compat.py`` (same stdlib discipline, same env-config pattern), so
indexing adds no install weight. Disabled unless ``CAPTURE_INDEX_URL`` is set; the
``indexer`` only runs when ``available()`` (a ``/v1/models`` preflight) also succeeds.

Configuration (env; each is overridable per call site):
  CAPTURE_INDEX_URL          full chat endpoint, e.g.
                             ``http://192.168.31.217:1234/v1/chat/completions`` (required)
  CAPTURE_INDEX_MODEL        model id (e.g. ``qwen3.5-9b``)
  CAPTURE_INDEX_KEY          bearer token (optional; LM Studio usually needs none)
  CAPTURE_INDEX_TIMEOUT      per-call seconds (default 120)
  CAPTURE_INDEX_MAX_IMAGE_PX longest-edge downscale before upload (default 1024)
"""

from __future__ import annotations

import base64
import json
import logging
import os
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

log = logging.getLogger(__name__)

ENV_URL = "CAPTURE_INDEX_URL"
ENV_MODEL = "CAPTURE_INDEX_MODEL"
ENV_KEY = "CAPTURE_INDEX_KEY"
ENV_TIMEOUT = "CAPTURE_INDEX_TIMEOUT"
ENV_MAX_PX = "CAPTURE_INDEX_MAX_IMAGE_PX"
ENV_MAX_TOKENS = "CAPTURE_INDEX_MAX_TOKENS"


def configured_url(override: str | None = None) -> str:
    """The chat endpoint to use: an explicit override else the env. ``""`` if unset."""
    return (override or os.environ.get(ENV_URL, "")).strip()


def load(url: str | None = None, model: str | None = None) -> "VisionClient":
    """Build a client from explicit args (GUI/request overrides) falling back to env.
    Raises ``RuntimeError`` if no URL is configured (the route guards this earlier)."""
    u = configured_url(url)
    if not u:
        raise RuntimeError(f"{ENV_URL} is not set (e.g. http://192.168.31.217:1234/v1/chat/completions)")
    return VisionClient(
        u,
        model=(model or os.environ.get(ENV_MODEL) or None),
        api_key=os.environ.get(ENV_KEY) or None,
        timeout=float(os.environ.get(ENV_TIMEOUT, "120")),
        max_px=int(os.environ.get(ENV_MAX_PX, "1024")),
        max_tokens=int(os.environ.get(ENV_MAX_TOKENS, "2048")),
    )


def _downscale_sips(p: str, max_px: int) -> "bytes | None":
    """Downscale to JPEG (longest edge ≤ ``max_px``) via macOS ``sips``; ``None`` on any
    failure (e.g. ``sips`` absent on non-macOS)."""
    tmp = None
    try:
        fd, tmp = tempfile.mkstemp(suffix=".jpg")
        os.close(fd)
        r = subprocess.run(
            ["sips", "-Z", str(max_px), "-s", "format", "jpeg", p, "--out", tmp],
            capture_output=True,
        )
        if r.returncode == 0 and os.path.getsize(tmp) > 0:
            with open(tmp, "rb") as f:
                return f.read()
    except Exception:
        return None
    finally:
        if tmp and os.path.exists(tmp):
            try:
                os.unlink(tmp)
            except OSError:
                pass
    return None


def _downscale_pillow(p: str, max_px: int) -> "bytes | None":
    """Cross-platform downscale to JPEG via Pillow (longest edge ≤ ``max_px``); ``None`` if
    Pillow is unavailable or the encode fails. Lazy import so it stays an optional dep."""
    try:
        import io

        from PIL import Image  # optional; only used when sips isn't available

        with Image.open(p) as im:
            im = im.convert("RGB")
            im.thumbnail((max_px, max_px))
            buf = io.BytesIO()
            im.save(buf, format="JPEG", quality=85)
            return buf.getvalue()
    except Exception:
        return None


def _encode_image(path: "str | os.PathLike", max_px: int) -> tuple[str, str]:
    """``(base64, mime)`` for a screenshot: downscale + JPEG (longest edge ≤ ``max_px``) to
    shrink the payload, tried in order ``sips`` (macOS) → Pillow (cross-platform); on any
    failure send the raw PNG unchanged. The raw-PNG fallback keeps indexing working on a box
    with neither downscaler (just with a heavier payload)."""
    p = str(path)
    if max_px > 0:
        jpg = _downscale_sips(p, max_px) or _downscale_pillow(p, max_px)
        if jpg is not None:
            return base64.b64encode(jpg).decode("ascii"), "image/jpeg"
    with open(p, "rb") as f:
        return base64.b64encode(f.read()).decode("ascii"), "image/png"


class VisionClient:
    def __init__(self, url: str, *, model: str | None, api_key: str | None,
                 timeout: float, max_px: int, max_tokens: int = 2048) -> None:
        self.url = url
        self.model = model
        self.api_key = api_key
        self.timeout = timeout
        self.max_px = max_px
        self.max_tokens = max_tokens

    # -- availability ---------------------------------------------------------

    def _models_url(self) -> str:
        u = self.url
        for suffix in ("/chat/completions", "/completions"):
            if u.endswith(suffix):
                return u[: -len(suffix)] + "/models"
        return u

    def available(self, timeout: float = 5.0) -> bool:
        """True iff the endpoint answers a ``GET /v1/models`` (the preflight that gates
        indexing — `index_available`). Never raises."""
        try:
            req = urllib.request.Request(self._models_url(), headers=self._headers(json_body=False))
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return 200 <= resp.status < 300
        except Exception as e:
            log.debug("vision endpoint not available: %s", e)
            return False

    # -- calls ----------------------------------------------------------------

    def caption_image(self, image_path: "str | os.PathLike", prompt: str, *, max_px: int | None = None) -> str:
        """Vision call: describe ``image_path`` per ``prompt``. Returns the model text.
        ``max_px`` overrides the client default downscale for this call (e.g. higher res for code)."""
        return self._chat([{"role": "user", "content": self._image_content(image_path, prompt, max_px)}])

    def structured_image(self, image_path: "str | os.PathLike", prompt: str, schema: dict,
                         *, max_px: int | None = None) -> dict:
        """STRUCTURED vision call: returns a dict validated against ``schema`` (an OpenAI
        ``json_schema`` response_format). Returns ``{}`` on a non-JSON reply.

        KEY (researched against LM Studio): structured output uses llama.cpp grammar-constrained
        sampling, which forbids a reasoning model's ``<think>`` block and yields EMPTY content. The
        fix is **`reasoning_effort: "none"`** (the OpenAI-standard param LM Studio honors) to disable
        thinking — then the grammar applies cleanly. `/no_think` and `chat_template_kwargs` are NOT
        honored by Qwen3.5 here. Bonus: with reasoning off these calls are ~8× faster."""
        extra = {
            "response_format": {"type": "json_schema",
                                "json_schema": {"name": "frame", "strict": True, "schema": schema}},
            "reasoning_effort": "none",
        }
        text = self._chat([{"role": "user", "content": self._image_content(image_path, prompt, max_px)}], extra=extra)
        try:
            obj = json.loads(text)
            return obj if isinstance(obj, dict) else {}
        except Exception:
            return {}

    def combine(self, prompt: str) -> str:
        """Text-only call: fuse child summaries (+ transcript) into a range summary. Reasoning
        off — it's summarization (faster, and avoids the reasoning-model empty-content issue)."""
        return self._chat([{"role": "user", "content": prompt}], extra={"reasoning_effort": "none"})

    def _image_content(self, image_path: "str | os.PathLike", prompt: str, max_px: int | None = None) -> list:
        b64, mime = _encode_image(image_path, max_px if max_px is not None else self.max_px)
        return [
            {"type": "text", "text": prompt},
            {"type": "image_url", "image_url": {"url": f"data:{mime};base64,{b64}"}},
        ]

    def _headers(self, json_body: bool = True) -> dict:
        h = {}
        if json_body:
            h["Content-Type"] = "application/json"
        if self.api_key:
            h["Authorization"] = f"Bearer {self.api_key}"
        return h

    def _chat(self, messages: list, *, retries: int = 3, extra: dict | None = None) -> str:
        # max_tokens is load-bearing for REASONING models (e.g. Qwen3.5): they spend most
        # of the completion budget on `reasoning_content`, so a small/absent cap leaves the
        # actual `content` empty (finish_reason=length). A generous cap keeps room for both.
        payload = {"model": self.model or "local", "messages": messages,
                   "temperature": 0.2, "stream": False, "max_tokens": self.max_tokens}
        if extra:
            payload.update(extra)
        body = json.dumps(payload).encode("utf-8")
        last: Exception | None = None
        empties = 0
        for attempt in range(retries):
            try:
                req = urllib.request.Request(self.url, data=body, headers=self._headers(), method="POST")
                with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                    payload = json.loads(resp.read().decode("utf-8"))
                text = _extract_text(payload)
                if text:
                    return text
                # Empty content (transient, or reasoning exhausted the budget) — retry.
                empties += 1
                last = RuntimeError("model returned empty content")
            except urllib.error.HTTPError as e:
                # 4xx (bad request/model) won't fix on retry — fail fast.
                detail = e.read().decode("utf-8", "replace")[:300] if e.fp else ""
                last = RuntimeError(f"HTTP {e.code} from index endpoint: {detail}")
                if 400 <= e.code < 500:
                    break
            except Exception as e:  # connection refused / timeout / parse — retry
                last = e
            if attempt < retries - 1:
                time.sleep(0.5 * (attempt + 1))
        # If every attempt merely returned EMPTY content (no real error), degrade gracefully
        # to "" so one bad node doesn't abort a whole index build; a real error still raises.
        if empties == retries:
            log.warning("index: model returned empty content after %d attempts", retries)
            return ""
        raise RuntimeError(f"vision call failed after {retries} attempt(s): {last}")


def _extract_label(text: str, enum: list[str]) -> str:
    """Pull a classification label from a free-text reply. Prefers a value following a
    ``content_type``/``type`` marker, else the first enum value that appears as a word; falls
    back to ``"other"`` (or the last enum entry)."""
    import re

    low = (text or "").lower()
    fallback = "other" if "other" in enum else (enum[-1] if enum else "other")
    if not low:
        return fallback
    m = re.search(r"content[_ ]?type[\"'*\s:]+([a-z_]+)", low)
    if m and m.group(1) in enum:
        return m.group(1)
    for v in enum:
        if v != "other" and re.search(rf"\b{re.escape(v)}\b", low):
            return v
    return fallback


def _extract_text(payload: dict) -> str:
    """Pull the assistant text out of an OpenAI chat-completions response."""
    try:
        msg = payload["choices"][0]["message"]
        content = msg.get("content", "")
        if isinstance(content, list):  # some servers return content parts
            content = "".join(p.get("text", "") for p in content if isinstance(p, dict))
        return (content or "").strip()
    except Exception:
        return ""
