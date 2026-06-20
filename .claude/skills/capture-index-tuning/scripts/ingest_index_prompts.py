#!/usr/bin/env python3
"""Ingest the index-prompt corpus across sessions → a report for tuning the defaults.

Every `capture_index` run writes `<session>/index_prompts.json` (the prompts/schemas it used +
the per-type content distribution + any frontier-model-crafted custom prompts) alongside
`index.json` (the structured results). This script aggregates that corpus so an agent can decide
how to improve the built-in classifier (`classify_prompt` / content types) and per-type extractors
in the Rust index defaults (`crates/index/src/prompts.toml`, executed by `crates/index/src/prompts.rs`).

It surfaces:
  • the content-type distribution (which types actually occur — are any missing/over-broad?),
  • every CUSTOM leaf_prompt / leaf_schema / classify_prompt used (deduped, with frequency),
  • custom schema FIELDS not present in the matching default extractor (candidates to add),
  • sample structured extractions per type (so you can judge extractor quality).

Run from the repo root:  python .claude/skills/capture-index-tuning/scripts/ingest_index_prompts.py
Options: --runs <dir> (default $CAPTURE_RUNS_DIR or ~/.capture/runs), --samples N, --json <path>.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections import Counter, defaultdict
from pathlib import Path


def _repo_root() -> Path:
    # .claude/skills/capture-index-tuning/scripts/this.py → repo root is parents[4]
    return Path(__file__).resolve().parents[4]


def _load_defaults():
    """Read the built-in extractor defaults from the Rust index's `prompts.toml` — the v3 home of
    what used to be `capture_mcp.core.indexer.CONTENT_PROMPTS`. Returns (defaults, types) where
    `defaults[type] = {"schema": {"properties": {field: {}}}}` (so the field-diff below is unchanged);
    an empty pair if the toml can't be read (the report still works, just without the field-diff)."""
    toml_path = _repo_root() / "crates" / "index" / "src" / "prompts.toml"
    try:
        import tomllib  # stdlib (py3.11+)

        data = tomllib.loads(toml_path.read_text())
    except Exception as e:  # the report still works without the defaults
        print(f"(note: could not read index defaults from {toml_path}: {e})", file=sys.stderr)
        return {}, []
    defaults = {}
    for c in data.get("content", []):
        key = c.get("key")
        if not key:
            continue
        fields = {f[0] for f in c.get("fields", []) if f} | {"summary"}
        defaults[key] = {"schema": {"properties": {f: {} for f in fields}}}
    return defaults, list(defaults)


def _schema_fields(schema: dict) -> set[str]:
    try:
        return set((schema or {}).get("properties", {}))
    except Exception:
        return set()


def main() -> int:
    ap = argparse.ArgumentParser(description="Aggregate the index-prompt corpus for tuning.")
    ap.add_argument("--runs", default=os.environ.get("CAPTURE_RUNS_DIR") or str(Path.home() / ".capture" / "runs"))
    ap.add_argument("--samples", type=int, default=2, help="Sample N leaf extractions per content type")
    ap.add_argument("--json", help="Also write the aggregate as JSON to this path")
    args = ap.parse_args()

    runs = Path(args.runs).expanduser()
    records = sorted(runs.glob("capture-*/index_prompts.json"))
    if not records:
        print(f"No index_prompts.json found under {runs}. Run capture_index on a few sessions first.")
        return 0

    defaults, default_types = _load_defaults()
    type_counts: Counter = Counter()
    custom_leaf_prompts: Counter = Counter()
    custom_classify_prompts: Counter = Counter()
    custom_schemas: list[dict] = []
    new_fields: dict[str, set] = defaultdict(set)  # content_type -> fields seen in customs but not defaults
    samples: dict[str, list] = defaultdict(list)
    sessions = []

    for rp in records:
        try:
            rec = json.loads(rp.read_text())
        except Exception:
            continue
        sess = rp.parent.name
        sessions.append({"session": sess, "preset": rec.get("preset"), "types": rec.get("type_counts", {})})
        type_counts.update(rec.get("type_counts", {}))
        custom = rec.get("custom") or {}
        if custom.get("leaf_prompt"):
            custom_leaf_prompts[custom["leaf_prompt"].strip()] += 1
        if custom.get("classify_prompt"):
            custom_classify_prompts[custom["classify_prompt"].strip()] += 1
        if custom.get("leaf_schema"):
            custom_schemas.append({"session": sess, "schema": custom["leaf_schema"]})
        # Sample extractions from index.json
        idx_path = rp.parent / "index.json"
        if idx_path.is_file() and sum(len(v) for v in samples.values()) < 200:
            try:
                idx = json.loads(idx_path.read_text())
                for nd in idx.get("nodes", []):
                    if nd.get("children") or not nd.get("data"):
                        continue
                    ct = nd.get("content_type") or "other"
                    if len(samples[ct]) < args.samples:
                        samples[ct].append(nd["data"])
                    # default extractor for this type
                    dflt = defaults.get(ct, {})
                    dfields = _schema_fields(dflt.get("schema", {}))
                    if dfields:
                        extra = set(nd["data"]) - dfields - {"summary"}
                        new_fields[ct] |= extra
            except Exception:
                pass

    # -- report --
    print(f"# Index-prompt corpus — {len(records)} indexed session(s) under {runs}\n")
    print("## Content-type distribution (leaves)")
    for t, c in type_counts.most_common():
        marker = "" if t in default_types or t in ("general", "custom") else "  ⚠ NOT a known type"
        print(f"  {t:12} {c}{marker}")
    print()

    if custom_leaf_prompts:
        print("## Custom leaf_prompts used (frontier-model-crafted) — candidates to fold into defaults")
        for p, c in custom_leaf_prompts.most_common():
            print(f"  ×{c}: {p}")
        print()
    if custom_classify_prompts:
        print("## Custom classify_prompts used")
        for p, c in custom_classify_prompts.most_common():
            print(f"  ×{c}: {p}")
        print()
    if custom_schemas:
        print(f"## Custom leaf_schemas used ({len(custom_schemas)}) — new fields to consider adding")
        for cs in custom_schemas[:10]:
            print(f"  [{cs['session']}] fields: {sorted(_schema_fields(cs['schema']))}")
        print()
    extra_fields = {t: sorted(f) for t, f in new_fields.items() if f}
    if extra_fields:
        print("## Fields seen in extractions but NOT in the default schema (extend the extractor?)")
        for t, fs in extra_fields.items():
            print(f"  {t}: {fs}")
        print()

    print("## Sample extractions per type (judge extractor quality)")
    for t, exs in samples.items():
        print(f"  {t}:")
        for ex in exs:
            print(f"    {json.dumps(ex, ensure_ascii=False)[:200]}")
    print()
    print("## Next: edit crates/index/src/prompts.toml (executed by crates/index/src/prompts.rs)")
    print("  • Add/refine the [[content]] entry (prompt + fields) where customs or new fields show a better extractor.")
    print("  • Add any ⚠ unknown content types as a new [[content]] key, or fold over-broad ones together.")
    print("  • Re-verify with tools/index_prompt_eval.py --compare and `cargo test -p capture-index`.")

    if args.json:
        Path(args.json).write_text(json.dumps({
            "sessions": sessions, "type_counts": dict(type_counts),
            "custom_leaf_prompts": dict(custom_leaf_prompts),
            "custom_classify_prompts": dict(custom_classify_prompts),
            "custom_schemas": custom_schemas, "new_fields": extra_fields, "samples": samples,
        }, indent=2, ensure_ascii=False))
        print(f"\n(aggregate JSON → {args.json})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
