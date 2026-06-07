"""Turn a playlist capture run into per-video deliverables.

Reads the run's ``manifest.json`` (per-video nav_epoch + ad_spans) and the single
continuous ``transcript.jsonl`` from the capture dir, then splits the transcript
into one folder per video — ``NN-<title>/transcript.txt`` (+ ``.jsonl``) with ad
spans excluded — copies a few representative screenshots per video, and writes an
index. Per-video + playlist summaries are written separately (by the agent reading
the transcripts).

    python scripts/playlist_deliverables.py --run C:\\...\\capture-runs\\playlist
"""
from __future__ import annotations

import argparse
import json
import re
import shutil
from datetime import datetime, timezone
from pathlib import Path


def iso_to_epoch(s: str) -> float:
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%S.%fZ").replace(tzinfo=timezone.utc).timestamp()


def fsstamp_to_epoch(name: str) -> float | None:
    # "2026-06-07T12-13-52.531Z.jpg" -> epoch. Date uses '-', time uses '-' too; rebuild.
    m = re.match(r"(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})\.(\d+)Z", name)
    if not m:
        return None
    d, hh, mm, ss, ms = m.groups()
    try:
        return iso_to_epoch(f"{d}T{hh}:{mm}:{ss}.{ms}Z")
    except ValueError:
        return None


def safe(s: str, n: int = 60) -> str:
    s = re.sub(r"\s*-\s*YouTube\s*$", "", s or "").strip()
    s = re.sub(r'[<>:"/\\|?*]+', "", s)
    s = re.sub(r"\s+", "_", s)
    return (s[:n] or "video").rstrip("._")


def in_any(t: float, spans: list) -> bool:
    return any(a <= t <= b for a, b in (spans or []))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True, help="playlist run dir (has manifest.json)")
    args = ap.parse_args()

    run = Path(args.run)
    mpath = run / "manifest.json"
    if mpath.exists():
        manifest = json.loads(mpath.read_text(encoding="utf-8"))
    else:
        # Mid-run: fall back to status.json's completed-video list.
        st = json.loads((run / "status.json").read_text(encoding="utf-8"))
        manifest = {"playlist": st.get("playlist", ""), "model": st.get("model", ""),
                    "capture_dir": st["capture_dir"], "videos": st.get("completed", [])}
    videos = sorted(manifest["videos"], key=lambda v: v["index"])
    cap_dir = Path(manifest["capture_dir"])
    tx_path = cap_dir / "transcript.jsonl"
    segs = []
    if tx_path.exists():
        for line in tx_path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                r = json.loads(line)
                r["_epoch"] = iso_to_epoch(r["start"])
                segs.append(r)
            except Exception:
                pass
    segs.sort(key=lambda r: r["_epoch"])

    shots = []
    sdir = cap_dir / "screenshots"
    if sdir.is_dir():
        for p in sorted(sdir.glob("*")):
            e = fsstamp_to_epoch(p.name)
            if e is not None:
                shots.append((e, p))

    out = run / "deliverables"
    out.mkdir(exist_ok=True)
    # Per-video windows: [nav_epoch[i], nav_epoch[i+1]); last video open-ended.
    bounds = [v.get("nav_epoch") for v in videos]
    index_rows = []
    for i, v in enumerate(videos):
        start = bounds[i]
        # Prefer this video's own content_end as the upper bound (robust mid-run and
        # excludes inter-video transitions); fall back to the next video's nav time.
        end = v.get("content_end") or (bounds[i + 1] if i + 1 < len(bounds) else float("inf"))
        ads = v.get("ad_spans", [])
        vseg = [s for s in segs if start is not None and start <= s["_epoch"] < end and not in_any(s["_epoch"], ads)]
        folder = out / f"{v['index']:02d}-{safe(v['title'])}"
        folder.mkdir(parents=True, exist_ok=True)
        # transcript.txt (clean, timestamped) + transcript.jsonl
        with open(folder / "transcript.txt", "w", encoding="utf-8") as f:
            f.write(f"# {v['title']}\n# {v.get('watch_url','')}\n\n")
            for s in vseg:
                f.write(f"[{s['start']}] {s['text']}\n")
        with open(folder / "transcript.jsonl", "w", encoding="utf-8") as f:
            for s in vseg:
                f.write(json.dumps({k: s[k] for k in ("start", "end", "start_offset", "end_offset", "text") if k in s}, ensure_ascii=False) + "\n")
        # a few representative screenshots
        vshots = [p for (e, p) in shots if start is not None and start <= e < end]
        shotdir = folder / "screenshots"
        if vshots:
            shotdir.mkdir(exist_ok=True)
            pick = vshots[:: max(1, len(vshots) // 6)][:6]
            for p in pick:
                try:
                    shutil.copy2(p, shotdir / p.name)
                except Exception:
                    pass
        words = sum(len(s["text"].split()) for s in vseg)
        index_rows.append((v["index"], v["title"], v.get("watch_url", ""), len(vseg), words,
                           len(ads), folder.name))

    # index
    with open(out / "README.md", "w", encoding="utf-8") as f:
        f.write("# Playlist capture — deliverables\n\n")
        f.write(f"Source: {manifest.get('playlist','')}\nModel: {manifest.get('model','')}\n\n")
        f.write("| # | Title | Segments | Words | Ad spans | Folder |\n|---|---|---|---|---|---|\n")
        for idx, title, url, nseg, words, nads, folder in index_rows:
            f.write(f"| {idx} | [{title}]({url}) | {nseg} | {words} | {nads} | `{folder}/` |\n")
        f.write("\nPer-video transcripts in each folder; summaries in `SUMMARY.md` (added by the agent).\n")

    print(json.dumps({"videos": len(videos), "segments_total": len(segs),
                      "per_video": [{"index": r[0], "segments": r[3], "words": r[4]} for r in index_rows]},
                     indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
