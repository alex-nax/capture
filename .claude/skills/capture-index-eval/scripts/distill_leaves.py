#!/usr/bin/env python3
"""Distill a built index.json into a lean leaves.json — the text-only reconstruction
agent's cheap input (no images).

A leaf is a node with no `children`. From each leaf we keep only what a reader needs to
reconstruct the target content WITHOUT looking at pixels:
  - i  : leaf ordinal (sort by lo_idx so leaves are in capture order)
  - t  : start time (t_lo, rounded)
  - the structured `data` fields the extractor produced (surface + content fields)
  - transcript: the narration slice aligned to this leaf (often the load-bearing signal)

Output shape (matches what the reconstruction agent expects):
  {"root_summary": "...", "leaves": [ {i, t, <data fields...>, transcript}, ... ]}

Usage:
  distill_leaves.py --index custom/index.json --out custom/leaves.json
  distill_leaves.py --index custom/index.json --out custom/leaves.json --drop-empty
"""
from __future__ import annotations
import argparse, json
from pathlib import Path


def is_leaf(node: dict) -> bool:
    return not node.get("children")


def distill(index: dict, drop_empty: bool = False) -> dict:
    nodes = index.get("nodes", [])
    leaves = [n for n in nodes if is_leaf(n)]
    leaves.sort(key=lambda n: n.get("lo_idx", 0))
    out = []
    for i, n in enumerate(leaves):
        row = {"i": i, "t": round(n.get("t_lo", 0.0), 1)}
        data = n.get("data") or {}
        for k, v in data.items():
            # collapse boring empties so the scaffold stays cheap to read
            if drop_empty and (v == "" or v == [] or v is None):
                continue
            row[k] = v
        if "summary" not in row and n.get("summary"):
            row["summary"] = n["summary"]
        ts = n.get("transcript_slice")
        if ts:
            row["transcript"] = ts
        out.append(row)
    return {"root_summary": index.get("root_summary", ""), "leaves": out}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--index", required=True, help="built index.json")
    ap.add_argument("--out", required=True, help="lean leaves.json to write")
    ap.add_argument("--drop-empty", action="store_true",
                    help="omit fields that are empty string / [] / null (smaller scaffold)")
    args = ap.parse_args()

    index = json.loads(Path(args.index).read_text())
    result = distill(index, drop_empty=args.drop_empty)
    outp = Path(args.out)
    outp.parent.mkdir(parents=True, exist_ok=True)
    outp.write_text(json.dumps(result, indent=1))
    n = len(result["leaves"])
    chars = len(json.dumps(result))
    print(f"wrote {n} leaves -> {args.out}  (~{chars} chars / ~{chars//4} tokens)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
