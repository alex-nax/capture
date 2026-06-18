#!/usr/bin/env python3
"""Decode the Claude-design "standalone" export into navigable per-screen files.

`design/Capture Screens (standalone).html` is a self-unpacking *bundled page*: the real
DOM lives gzip+base64 inside a `<script type="__bundler/manifest">` (uuid -> asset) and a
`<script type="__bundler/template">` (the HTML, with {{ }} dc-runtime bindings + <sc-if>/
<sc-for>). Tools can't read the frames until it's decoded.

This script:
  1. decodes every manifest asset into design/unpacked/<uuid8>.<ext> (the template +
     the Inter/JetBrains-Mono woff2 fonts; design/unpacked/_template.html is the real markup),
  2. splits the template's `<!-- ==== NAME ==== -->`-delimited artboards into standalone
     viewable files design/screens/<slug>.html (shared <style>; sc-if/sc-for -> display:contents
     so every branch shows — handy as a static reference).

The decoded `_template.html` (and the per-screen files) are the visual SOURCE OF TRUTH for the
GPUI redesign — implement against them, not the condensed design/CAPTURE-HANDOFF.md (which drifts).
Re-run after dropping a new export in design/.
"""
import re, json, base64, gzip, pathlib

ROOT = pathlib.Path(__file__).resolve().parent
SRC = ROOT / "Capture Screens (standalone).html"
EXT = {"text/javascript": "js", "application/javascript": "js", "text/css": "css",
       "text/html": "html", "image/svg+xml": "svg", "application/json": "json"}


def grab(html: str, kind: str):
    m = re.search(r'<script type="__bundler/%s">(.*?)</script>' % re.escape(kind), html, re.DOTALL)
    return m.group(1) if m else None


def main():
    html = SRC.read_text()
    unpacked = ROOT / "unpacked"; unpacked.mkdir(exist_ok=True)

    manifest = json.loads(grab(html, "manifest"))
    for uuid, e in manifest.items():
        data = base64.b64decode(e["data"])
        if e.get("compressed"):
            data = gzip.decompress(data)
        (unpacked / f"{uuid[:8]}.{EXT.get(e['mime'], 'bin')}").write_bytes(data)

    template = json.loads(grab(html, "template"))
    (unpacked / "_template.html").write_text(template)

    style_m = re.search(r"<style>.*?</style>", template, re.DOTALL)
    style = style_m.group(0) if style_m else ""
    screens = ROOT / "screens"; screens.mkdir(exist_ok=True)
    parts = re.split(r"(<!-- =+ [^=]+ =+ -->)", template)
    for i in range(1, len(parts) - 1, 2):
        name = re.search(r"=+\s*([^=]+?)\s*=+", parts[i]).group(1).strip()
        slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
        page = (f'<!DOCTYPE html><html><head><meta charset="utf-8"><title>{name}</title>\n{style}\n'
                '<style>body{background:#d4d4d7;padding:40px;display:block} sc-if,sc-for{display:contents}</style>\n'
                f'</head><body>{parts[i]}{parts[i + 1].rstrip()}</body></html>')
        (screens / f"{slug}.html").write_text(page)
        print(f"  design/screens/{slug}.html  ({name})")


if __name__ == "__main__":
    main()
