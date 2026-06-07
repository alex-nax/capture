"""Capture a YouTube playlist end-to-end with capture-mcp.

Drives Chrome (Selenium) through each video in the interactive desktop session,
autoplaying with sound and **muting/skipping ads** (so ad audio is silence in the
transcript), while ONE continuous capture-mcp ``CaptureSession`` records the screen
+ system audio (WASAPI loopback) and transcribes it with the local ASR backend
(faster-whisper CUDA). The model loads once; per-video content windows + ad spans
are recorded to ``manifest.json`` so the transcript can be split per video.

Must run in the INTERACTIVE session (WinSta0) via ``scripts/run_interactive.ps1``,
ideally with ``pythonw.exe`` so no console window steals the foreground.

    pythonw scripts/capture_youtube_playlist.py --playlist <url> --out <dir> --model large-v3
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import traceback
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "src"))

_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0

# Mute/skip ads and keep main content playing, via the YouTube player API.
# PlayerState: -1 unstarted, 0 ended, 1 playing, 2 paused, 3 buffering, 5 cued.
_TICK_JS = r"""
var p=document.querySelector('#movie_player');
var v=document.querySelector('video');
var ad = p ? (p.classList.contains('ad-showing')||p.classList.contains('ad-interrupting')) : false;
var st = (p&&p.getPlayerState)?p.getPlayerState():-99;
var ct = (p&&p.getCurrentTime)?p.getCurrentTime():(v?v.currentTime:null);
var dur = (p&&p.getDuration)?p.getDuration():(v?v.duration:null);
if(ad){
  if(p&&p.mute)p.mute(); else if(v)v.muted=true;
  var b=document.querySelector('.ytp-ad-skip-button-modern,.ytp-ad-skip-button,.ytp-skip-ad-button,.ytp-ad-skip-button-container button');
  if(b){try{b.click();}catch(e){}}
} else {
  if(p&&p.unMute)p.unMute(); else if(v&&v.muted)v.muted=false;
  if(st===2||st===-1||st===5){ if(p&&p.playVideo)p.playVideo(); else if(v)v.play(); }
}
return {st:st, ct:ct, dur:dur, ad:ad, title:document.title};
"""


class Log:
    def __init__(self, path: Path):
        self._f = open(path, "a", encoding="utf-8", buffering=1)

    def __call__(self, msg: str):
        line = "%s %s" % (time.strftime("%H:%M:%S"), msg)
        try:
            self._f.write(line + "\n")
        except Exception:
            pass
        if sys.stdout:  # pythonw has no console; guard
            try:
                print(line, flush=True)
            except Exception:
                pass


def enumerate_playlist(url: str) -> list[dict]:
    out = subprocess.run(
        [sys.executable, "-m", "yt_dlp", "--flat-playlist", "--no-warnings",
         "--print", "%(playlist_index)s\t%(id)s\t%(duration)s\t%(title)s", url],
        capture_output=True, text=True, timeout=180, creationflags=_NO_WINDOW,
    )
    vids = []
    for ln in out.stdout.splitlines():
        parts = ln.split("\t", 3)
        if len(parts) == 4 and parts[1]:
            idx, vid, dur, title = parts
            vids.append({"index": int(idx), "id": vid,
                         "duration": int(dur) if dur.isdigit() else None, "title": title})
    return vids


def make_driver(chrome_binary: str, profile_dir: str, attach: str | None = None):
    from selenium import webdriver
    from selenium.webdriver.chrome.options import Options

    opt = Options()
    if attach:
        # Attach to an already-running Chrome (started with --remote-debugging-port).
        opt.add_experimental_option("debuggerAddress", attach)
        return webdriver.Chrome(options=opt)
    opt.binary_location = chrome_binary
    opt.add_argument("--user-data-dir=" + profile_dir)
    opt.add_argument("--autoplay-policy=no-user-gesture-required")
    opt.add_argument("--start-maximized")
    opt.add_argument("--no-first-run")
    opt.add_argument("--no-default-browser-check")
    opt.add_argument("--disable-session-crashed-bubble")
    opt.add_experimental_option("excludeSwitches", ["enable-automation"])
    d = webdriver.Chrome(options=opt)
    try:
        d.maximize_window()
    except Exception:
        pass
    return d


def write_status(path: Path, status: dict):
    try:
        path.write_text(json.dumps(status, indent=2, ensure_ascii=False), encoding="utf-8")
    except Exception:
        pass


def play_one(driver, video, sess, status, status_path, log) -> dict:
    vid, title, dur_hint = video["id"], video["title"], video.get("duration")
    log("=== video %d: %s (%s) ===" % (video["index"], title, vid))
    nav_epoch = round(time.time(), 2)  # robust per-video window boundary for splitting
    driver.get("https://www.youtube.com/watch?v=" + vid)

    # Wait for the player + duration, clearing any pre-roll ad.
    dur = None
    t0 = time.time()
    while time.time() - t0 < 90:
        st = driver.execute_script(_TICK_JS)
        if st.get("dur") and st["dur"] > 0 and not st.get("ad") and (st.get("ct") or 0) < 8 and st.get("st") in (1, 3):
            dur = st["dur"]
            break
        time.sleep(0.5)
    if dur is None:
        dur = dur_hint or 600
        log("WARN: content start not detected cleanly; using dur=%s" % dur)

    content_start = time.time()
    real_title = st.get("title", title)
    log("content start (dur=%.0fs) title=%r" % (dur, real_title))

    ad_spans, last_ad = [], None
    deadline = content_start + dur * 1.8 + 90
    ended = False
    while time.time() < deadline:
        try:
            st = driver.execute_script(_TICK_JS)
        except Exception as e:
            log("tick error: %r" % e)
            time.sleep(1.0)
            continue
        now = time.time()
        if st.get("ad"):
            if last_ad is None:
                last_ad = now
        elif last_ad is not None:
            ad_spans.append([round(last_ad, 2), round(now, 2)])
            last_ad = None
        status["current"] = {"index": video["index"], "id": vid, "title": real_title,
                             "st": st.get("st"), "ct": round(st.get("ct") or 0, 1),
                             "dur": round(dur, 1), "ad": st.get("ad"),
                             "segments": sess.summary().get("transcript_segments")}
        write_status(status_path, status)
        if st.get("st") == 0 or (not st.get("ad") and (st.get("ct") or 0) >= dur - 1.5):
            ended = True
            break
        time.sleep(1.0)

    if last_ad is not None:
        ad_spans.append([round(last_ad, 2), round(time.time(), 2)])
    content_end = time.time()
    log("video done: ended=%s ads=%d wall=%.0fs" % (ended, len(ad_spans), content_end - content_start))
    return {"index": video["index"], "id": vid, "title": real_title, "duration": dur,
            "nav_epoch": nav_epoch,
            "content_start": round(content_start, 2), "content_end": round(content_end, 2),
            "ad_spans": ad_spans, "ended": ended,
            "watch_url": "https://www.youtube.com/watch?v=" + vid}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--playlist")
    ap.add_argument("--ids", help="comma-separated ids (overrides enumeration)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--model", default="large-v3")
    ap.add_argument("--max", type=int, default=0)
    ap.add_argument("--screenshot-interval", type=float, default=6.0)
    ap.add_argument("--screenshot-resolution", default="1280x720/jpg")
    ap.add_argument("--chrome", default=r"C:\Program Files\Google\Chrome\Application\chrome.exe")
    ap.add_argument("--attach", help="host:port of a Chrome started with --remote-debugging-port")
    ap.add_argument("--target-pid", type=int, default=None,
                    help="pid of the Chrome browser window to screenshot via PrintWindow "
                         "(occlusion-proof; lets you keep working with the video in the background)")
    args = ap.parse_args()

    os.environ["CAPTURE_WHISPER_MODEL"] = args.model
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    status_path = out_dir / "status.json"
    log = Log(out_dir / "driver.log")

    status = {"state": "starting", "started_at": time.strftime("%Y-%m-%d %H:%M:%S"),
              "started_epoch": round(time.time(), 2), "model": args.model,
              "out": str(out_dir), "current": None, "completed": [], "errors": []}
    write_status(status_path, status)

    from capture_mcp.session import CaptureSession
    sess = None
    driver = None
    try:
        if args.ids:
            vids = [{"index": i + 1, "id": v.strip(), "duration": None, "title": v.strip()}
                    for i, v in enumerate(args.ids.split(",")) if v.strip()]
        else:
            log("enumerating playlist...")
            vids = enumerate_playlist(args.playlist)
        if args.max:
            vids = vids[: args.max]
        status["playlist"] = vids
        log("videos: %d" % len(vids))

        driver = make_driver(args.chrome, str(out_dir / "_chrome_profile"), attach=args.attach)
        if args.attach:
            driver.switch_to.new_window("tab")  # fresh tab; don't disturb the user's tabs
            log("attached to Chrome at %s (new tab)" % args.attach)
        else:
            log("chrome launched")

        # ONE continuous capture session (model loads once, before video 1).
        sess = CaptureSession(
            str(out_dir), capture_screenshots=True, capture_audio=True,
            pid=args.target_pid,            # screenshot this window via PrintWindow (occlusion-proof)
            audio_source="auto", asr_backend="local",
            screenshot_interval=args.screenshot_interval,
            screenshot_resolution=args.screenshot_resolution, audio_chunk_seconds=8.0)
        summ = sess.start()
        status["capture_dir"] = summ["dir"]
        status["capture_epoch"] = round(time.time(), 2)
        status["state"] = "running"
        write_status(status_path, status)
        log("capture session: %s (audio=%s)" % (summ["dir"], summ.get("audio_status")))

        for v in vids:
            try:
                rec = play_one(driver, v, sess, status, status_path, log)
                status["completed"].append(rec)
            except Exception as e:
                log("VIDEO ERROR %s: %r\n%s" % (v["id"], e, traceback.format_exc()))
                status["errors"].append({"id": v["id"], "error": repr(e)})
            write_status(status_path, status)

        fin = sess.stop()
        log("capture stopped: segments=%s audio=%s" % (fin.get("transcript_segments"), fin.get("audio_status")))
        try:
            if args.attach:
                driver.close()  # close only our tab; leave the user's Chrome running
            else:
                driver.quit()
        except Exception:
            pass

        status["state"] = "done"
        status["finished_at"] = time.strftime("%Y-%m-%d %H:%M:%S")
        status["final"] = fin
        write_status(status_path, status)
        (out_dir / "manifest.json").write_text(json.dumps(
            {"playlist": args.playlist, "model": args.model, "capture_dir": summ["dir"],
             "videos": status["completed"], "errors": status["errors"]},
            indent=2, ensure_ascii=False), encoding="utf-8")
        log("DONE: %d videos, %d errors, segments=%s" %
            (len(status["completed"]), len(status["errors"]), fin.get("transcript_segments")))
        return 0
    except Exception as e:
        log("FATAL: %r\n%s" % (e, traceback.format_exc()))
        status["state"] = "error"
        status["fatal"] = repr(e)
        write_status(status_path, status)
        try:
            if sess:
                sess.stop()
        except Exception:
            pass
        try:
            if driver:
                driver.close() if args.attach else driver.quit()
        except Exception:
            pass
        return 1


if __name__ == "__main__":
    sys.exit(main())
