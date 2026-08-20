#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "soundfile"]
# ///
"""Point-and-click drop labelling in a browser — the GUI counterpart to annotate_drops.py.

    bench/label_drops.py                 # every dumped track that has playable audio
    bench/label_drops.py ~/Music/*.wav   # or name them

Waveform per track. Click to scrub, shift-click to mark a drop. The detector's
own fires are drawn as PREDICTIONS on their own lane, so the job is mostly
accept (Y) / reject (X) rather than hunt — and a rejected prediction becomes a
recorded NEGATIVE, which is exactly the evidence the precision question needs.

Writes the SAME `fosfora-bench-annotation/v1` bundle annotate_drops.py writes, to
the same bench/labels/, so it scores through the untouched score_dump.py path:

    bench/score_dump.py bench/out/dropsweep/music/<track>.signal.jsonl \
                        bench/labels/<track>.json

WHY NEGATIVES, AND WHY CATEGORIES. Finding #2259 established that PRECISION, not
recall, is the open defect: the machine fires on breaks. #2300 then falsified
every discriminator the v2 sidecar can express, and left one hard case — a
break-return at the track's LOUDEST moment with full sub, which no level or
peak-ratio gate can reach. A bundle of drops alone cannot express "this is a
break and firing here is wrong", so negatives carry an optional `label`
(break / buildup / fill / other). Labelled negatives are what let a later round
ask "what separates a break-return from a drop" instead of guessing.

WHY THE BLIND-PASS TOGGLE. Seeding a labeller with the machine's answers anchors
them: if you only ever confirm fires, recall is unmeasurable, because a drop the
machine never predicted is one you were never prompted to look at. #2259 carried
weight precisely because it was free listening. Press H to hide predictions and
label cold; the bundle records `predictions_visible` either way, so a later
reader can tell which kind of evidence they are holding.

Tap latency is not a concern the way it is for the CLI tool: the drop match
window is 1.0 bar (CONVENTIONS["drop"]["match_window_bars"]), ~1.7 s at 140 BPM,
and a click on a waveform lands far inside that.
"""

from __future__ import annotations

import argparse
import http.server
import json
import os
import re
import socketserver
import sys
import threading
import urllib.parse
import webbrowser
from pathlib import Path

import numpy as np
import soundfile as sf

REPO = Path(__file__).resolve().parent.parent
SCHEMA = "fosfora-bench-annotation/v1"
AUDIO_EXT = (".wav", ".mp3", ".flac", ".m4a", ".ogg", ".aiff", ".aif")
PEAK_BUCKETS = 2400
NEG_LABELS = ("break", "buildup", "fill", "other")

MIME = {
    ".wav": "audio/wav", ".mp3": "audio/mpeg", ".flac": "audio/flac",
    ".m4a": "audio/mp4", ".ogg": "audio/ogg", ".aiff": "audio/aiff", ".aif": "audio/aiff",
}


# ---------------------------------------------------------------- sidecar

# Two fires this close are the same moment. The sidecar ticks every ~0.107 s and the
# refractory keeps any one config 16 s from firing twice, so the only thing this ever
# merges is the same tick reached by two different configs.
SAME_MOMENT_S = 0.2


def predictions(stem: str, sidecar_dir: Path, extra: list[float] | None = None) -> list[dict]:
    """Fires to put in front of the labeller, tagged by where each one came from.

    The sidecar's own `d_fired` is what the SHIPPED binary claims. `extra` is any other
    config's fires — which matters because a labelling pass seeded from one config only
    ever adjudicates that config (finding #2365): every fire a wider config makes at an
    unseeded moment is scored as a false alarm on the assumption that the drop list is
    exhaustive there, and nobody ever listened. Seeding both is what makes the second
    config measurable.
    """
    out: list[dict] = []
    p = sidecar_dir / f"{stem}.jsonl"
    if p.exists():
        with p.open(encoding="utf-8") as f:
            for line in f:
                if '"d_fired":1' not in line:
                    continue
                try:
                    out.append({"t": round(float(json.loads(line)["d_t"]), 3),
                                "src": "shipped"})
                except (json.JSONDecodeError, KeyError, ValueError):
                    pass
    for t in extra or []:
        t = round(float(t), 3)
        near = next((d for d in out if abs(d["t"] - t) <= SAME_MOMENT_S), None)
        if near is not None:
            near["src"] = "both"
        else:
            out.append({"t": t, "src": "added"})
    out.sort(key=lambda d: d["t"])
    return out


def load_extra_predictions(path: Path) -> dict[str, list[float]]:
    """`{track_id: [times]}` from another config — `bench/sweep_drop.py --fires` writes it."""
    raw = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        sys.exit(f"{path}: expected a {{track_id: [times]}} object, got {type(raw).__name__}")
    out = {}
    for k, v in raw.items():
        if k.startswith("_"):  # provenance keys the writer left for a human
            continue
        if not isinstance(v, list):
            sys.exit(f"{path}: {k!r} maps to {type(v).__name__}, expected a list of times")
        out[k] = [float(t) for t in v]
    return out


def hints(stem: str, sidecar_dir: Path) -> list[dict]:
    """Q4 section-boundary announcements — navigation aids, not claims about drops."""
    out: list[tuple[float, str]] = []
    p = sidecar_dir / f"{stem}.signal.jsonl"
    if not p.exists():
        return []
    with p.open(encoding="utf-8") as f:
        for line in f:
            if "section/boundary" not in line:
                continue
            try:
                r = json.loads(line)
                a = r.get("args") or []
                conf = float(a[0]["f"])
                age = float(a[1]["f"]) if len(a) > 1 else 0.0
                if conf >= 0.5:
                    # Events are announced late by a latency they state themselves.
                    out.append((max(0.0, float(r["ts"]) - age), f"boundary {conf:.2f}"))
            except (json.JSONDecodeError, KeyError, ValueError, IndexError):
                pass
    out.sort(key=lambda p_: p_[0])
    merged: list[dict] = []
    for t, why in out:
        if merged and t - merged[-1]["t"] < 2.0:
            continue
        merged.append({"t": round(t, 3), "why": why})
    return merged


# ---------------------------------------------------------------- waveform

def peaks(path: Path, buckets: int = PEAK_BUCKETS) -> list[float]:
    """Per-bucket peak amplitude 0..1, read in blocks so a long track never lands
    in memory whole."""
    info = sf.info(str(path))
    total = info.frames
    if total <= 0:
        return [0.0] * buckets
    per = max(1, total // buckets)
    vals: list[float] = []
    with sf.SoundFile(str(path)) as f:
        while len(vals) < buckets:
            block = f.read(per, dtype="float32", always_2d=True)
            if block.shape[0] == 0:
                break
            vals.append(float(np.abs(block).max()))
    if not vals:
        return [0.0] * buckets
    hi = max(vals) or 1.0
    vals = [round(v / hi, 4) for v in vals]
    return (vals + [0.0] * buckets)[:buckets]


def duration_of(path: Path) -> float:
    info = sf.info(str(path))
    return info.frames / float(info.samplerate)


# ---------------------------------------------------------------- bundle io

def bundle_path(stem: str, out_dir: Path) -> Path:
    return out_dir / f"{stem}.json"


def load_marks(stem: str, out_dir: Path) -> dict:
    """Existing bundle -> the mark lists, so a session can be resumed or revised."""
    empty = {"drops": [], "not_drops": [], "predictions_visible": True}
    p = bundle_path(stem, out_dir)
    if not p.exists():
        return empty
    try:
        raw = json.loads(p.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return empty
    grab = lambda k: [
        {"t": float(d["time"]), "label": d.get("label"), "source": d.get("source", ""),
         # Carried through a resume, or re-saving the track would drop it.
         "pred_t": d.get("pred_time")}
        for d in (raw.get(k) or []) if "time" in d
    ]
    return {
        "drops": grab("drops"),
        "not_drops": grab("not_drops"),
        "predictions_visible": bool(
            (raw.get("provenance") or {}).get("predictions_visible", True)
        ),
    }


def write_bundle(track: dict, body: dict, out_dir: Path) -> Path:
    audio = Path(track["path"])

    def marks(items: list[dict], with_label: bool) -> list[dict]:
        out = []
        for m in sorted(items, key=lambda d: float(d.get("t", 0.0))):
            # How the mark came about: a fresh click, or a verdict on a
            # prediction. Lets a later reader separate free listening from
            # confirm/reject, which bias differently.
            #
            # Take the verb off the end rather than prefixing what arrives. The browser
            # sends a bare verb ("click" / "confirm" / "reject") but `load_marks` hands
            # back the STORED form, "kevin:click" — so prefixing unconditionally turned
            # every resume into "kevin:kevin:click", compounding once per save. Times and
            # labels survived it, which is why nothing caught it: the reader is tolerant
            # (`d.get("source", "")`) and only the writer was wrong. Splitting on the last
            # colon is idempotent, so this also repairs an already-corrupted bundle the
            # next time it is saved.
            verb = str(m.get("source") or "click").rsplit(":", 1)[-1] or "click"
            d = {
                "time": round(max(0.0, float(m["t"])), 3),
                "kind": "local_manual",
                "source": f"kevin:{verb}",
            }
            # The prediction this mark was a verdict on, when it was one. A
            # confirm has no independent time — pressing Y stamps the machine's
            # own instant — so `time == pred_time` means this mark cannot
            # measure the detector's timing error, only its existence. Recording
            # it makes that visible in the data instead of requiring a reader to
            # know what `source` implies.
            if m.get("pred_t") is not None:
                d["pred_time"] = round(max(0.0, float(m["pred_t"])), 3)
            if with_label and m.get("label"):
                d["label"] = m["label"]
            out.append(d)
        return out

    payload = {
        "schema": SCHEMA,
        "track_id": audio.stem,
        "dataset": "local_drops",
        "audio": {"path": os.path.relpath(audio, out_dir), "duration_s": track["duration"]},
        "drops": marks(body.get("drops", []), with_label=False),
        # Additive: v1 readers ignore both the list and the per-mark label.
        "not_drops": marks(body.get("not_drops", []), with_label=True),
        "provenance": {
            "tool": "bench/label_drops.py",
            "method": "listen + click (waveform)",
            "tap_latency_s": 0.0,
            "_latency_note": "clicks are placed on a waveform, not tapped in real time, so "
                             "there is no systematic reaction-time offset to subtract; the "
                             "drop match window is 1.0 bar regardless",
            "predictions_seeded": len(track["predictions"]),
            # How many of those came from a config other than the shipped binary. A reader
            # scoring a non-shipped config needs to know whether its fires were ever put in
            # front of a listener — without this, an unjudged moment and a rejected one are
            # indistinguishable in the bundle (#2365).
            "predictions_added": sum(1 for p in track["predictions"]
                                     if p.get("src") in ("added", "both")),
            "predictions_source": track.get("pred_source") or "sidecar d_fired (shipped)",
            # False = labelled cold. True = the detector's fires were on screen, so
            # these labels are confirm/reject evidence and recall is anchored.
            "predictions_visible": bool(body.get("predictions_visible", True)),
            "negative_labels": list(NEG_LABELS),
        },
    }
    out = bundle_path(audio.stem, out_dir)
    out.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return out


# ---------------------------------------------------------------- server

class Handler(http.server.BaseHTTPRequestHandler):
    tracks: list[dict] = []
    out_dir: Path = REPO / "bench" / "labels"

    def log_message(self, *a):  # quiet; the page is the interface
        pass

    def _send(self, code: int, body: bytes, ctype: str):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _json(self, obj, code: int = 200):
        self._send(code, json.dumps(obj).encode(), "application/json")

    def do_GET(self):
        route = urllib.parse.urlparse(self.path).path
        if route == "/":
            return self._send(200, PAGE.encode(), "text/html; charset=utf-8")
        if route == "/api/tracks":
            keys = ("id", "name", "duration", "predictions", "hints", "marks", "saved")
            return self._json([{k: t[k] for k in keys} for t in self.tracks])
        if m := re.fullmatch(r"/audio/(\d+)", route):
            return self._audio(int(m.group(1)))
        if m := re.fullmatch(r"/api/peaks/(\d+)", route):
            i = int(m.group(1))
            if not 0 <= i < len(self.tracks):
                return self._json({"error": "no such track"}, 404)
            t = self.tracks[i]
            if t["peaks"] is None:
                t["peaks"] = peaks(Path(t["path"]))
            return self._json({"peaks": t["peaks"]})
        self._json({"error": "not found"}, 404)

    def _audio(self, i: int):
        """Range-capable so the <audio> element can seek without refetching."""
        if not 0 <= i < len(self.tracks):
            return self._json({"error": "no such track"}, 404)
        path = Path(self.tracks[i]["path"])
        size = path.stat().st_size
        rng = self.headers.get("Range")
        start, end = 0, size - 1
        if rng and (m := re.match(r"bytes=(\d*)-(\d*)", rng)):
            if m.group(1):
                start = int(m.group(1))
            if m.group(2):
                end = int(m.group(2))
        start = max(0, min(start, size - 1))
        end = max(start, min(end, size - 1))
        length = end - start + 1

        self.send_response(206 if rng else 200)
        self.send_header("Content-Type", MIME.get(path.suffix.lower(), "application/octet-stream"))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(length))
        if rng:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        with path.open("rb") as f:
            f.seek(start)
            remaining = length
            while remaining > 0:
                chunk = f.read(min(65536, remaining))
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return  # the browser seeked away mid-stream; normal
                remaining -= len(chunk)

    def do_POST(self):
        if urllib.parse.urlparse(self.path).path != "/api/save":
            return self._json({"error": "not found"}, 404)
        n = int(self.headers.get("Content-Length", 0))
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
            t = self.tracks[int(body["track"])]
            out = write_bundle(t, body, self.out_dir)
        except (json.JSONDecodeError, KeyError, IndexError, ValueError, OSError) as e:
            return self._json({"error": str(e)}, 400)
        t["marks"] = {
            "drops": body.get("drops", []),
            "not_drops": body.get("not_drops", []),
            "predictions_visible": bool(body.get("predictions_visible", True)),
        }
        t["saved"] = True
        blind = "" if body.get("predictions_visible", True) else "  [blind]"
        print(f"  wrote {out.relative_to(REPO)}  ({len(t['marks']['drops'])} drop, "
              f"{len(t['marks']['not_drops'])} not-a-drop){blind}")
        return self._json({"ok": True, "path": str(out.relative_to(REPO))})


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


# ---------------------------------------------------------------- page

PAGE = r"""<!doctype html>
<meta charset="utf-8">
<title>Drop labelling</title>
<style>
  :root { --bg:#14161a; --panel:#1c1f26; --line:#2c313b; --fg:#e8eaed; --dim:#9aa3af;
          --head:#ffd166; }
  * { box-sizing:border-box; }
  body { margin:0; background:var(--bg); color:var(--fg);
         font:14px/1.5 ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;
         display:flex; height:100vh; overflow:hidden; }
  #list { width:280px; flex:none; border-right:1px solid var(--line); overflow-y:auto;
          background:var(--panel); }
  #list h2 { font-size:11px; text-transform:uppercase; letter-spacing:.08em;
             color:var(--dim); margin:14px 14px 8px; }
  .trk { padding:9px 14px; cursor:pointer; border-left:3px solid transparent;
         display:flex; justify-content:space-between; gap:8px; align-items:baseline; }
  .trk:hover { background:#232733; }
  .trk.on { background:#2a3040; border-left-color:var(--head); }
  .trk .nm { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis;
             white-space:nowrap; }
  .trk .ct { font-size:11px; color:var(--dim); font-variant-numeric:tabular-nums; flex:none; }
  main { flex:1; display:flex; flex-direction:column; min-width:0; }
  header { padding:14px 20px 10px; border-bottom:1px solid var(--line); }
  h1 { font-size:16px; margin:0 0 3px; font-weight:600; }
  #sub { color:var(--dim); font-size:12px; font-variant-numeric:tabular-nums; }
  #stage { flex:1; padding:16px 20px; overflow-y:auto; }
  canvas { width:100%; display:block; cursor:crosshair; background:#0f1115;
           border:1px solid var(--line); border-radius:5px; }
  #wave { height:230px; }
  #zoom { height:150px; margin-top:10px; }
  #bar { padding:10px 20px; border-top:1px solid var(--line); display:flex; gap:8px;
         align-items:center; flex-wrap:wrap; }
  button { background:#252a34; color:var(--fg); border:1px solid var(--line);
           padding:6px 12px; border-radius:5px; cursor:pointer; font-size:13px; }
  button:hover { background:#2f3542; }
  button.pri { background:#3a4a63; border-color:#4b5f7d; }
  button.on { background:#5a4a2a; border-color:#8a6d3b; }
  kbd { background:#0f1115; border:1px solid var(--line); border-bottom-width:2px;
        border-radius:3px; padding:1px 5px; font-size:11px;
        font-family:ui-monospace,monospace; }
  #marks { margin-left:auto; color:var(--dim); font-size:12px;
           font-variant-numeric:tabular-nums; }
  #help { padding:0 20px 12px; color:var(--dim); font-size:12px; }
  #toast { position:fixed; bottom:18px; left:50%; transform:translateX(-50%);
           background:#2a3040; border:1px solid var(--line); padding:9px 16px;
           border-radius:6px; opacity:0; transition:opacity .2s; pointer-events:none; }
  #toast.on { opacity:1; }
</style>

<div id="list"><h2>Tracks</h2><div id="trks"></div></div>
<main>
  <header><h1 id="title">—</h1><div id="sub"></div></header>
  <div id="stage"><canvas id="wave"></canvas><canvas id="zoom"></canvas></div>
  <div id="help">
    <kbd>space</kbd> play · <kbd>←</kbd><kbd>→</kbd> 5s · <kbd>J</kbd><kbd>K</kbd> 15s ·
    <kbd>,</kbd><kbd>.</kbd> next prediction · <kbd>Y</kbd> accept · <kbd>X</kbd> reject ·
    <kbd>D</kbd> drop here · <kbd>N</kbd> not-a-drop here · <kbd>⌫</kbd> delete nearest ·
    <kbd>1</kbd>-<kbd>4</kbd> label negative (break/buildup/fill/other) ·
    <kbd>H</kbd> hide predictions · <kbd>S</kbd> save.
    Click to scrub; shift-click marks a drop, alt-click a not-a-drop.
  </div>
  <div id="bar">
    <button id="play" class="pri">Play</button>
    <button id="acc">Accept <kbd>Y</kbd></button>
    <button id="rej">Reject <kbd>X</kbd></button>
    <button id="mkD">Drop here <kbd>D</kbd></button>
    <button id="mkN">Not a drop <kbd>N</kbd></button>
    <button id="del">Delete <kbd>⌫</kbd></button>
    <button id="blind">Predictions: shown <kbd>H</kbd></button>
    <button id="save" class="pri">Save <kbd>S</kbd></button>
    <span id="marks"></span>
  </div>
</main>
<div id="toast"></div>

<script>
const $ = s => document.querySelector(s);
const wave = $("#wave"), zoom = $("#zoom");
const audio = new Audio(); audio.preload = "auto";
const LABELS = ["break", "buildup", "fill", "other"];
const ZOOM_S = 12;                    // seconds either side in the detail strip
const PRED_LANE = 26;                 // px reserved at the top for the prediction lane

let tracks = [], cur = -1, peaks = [], dur = 0, dirty = false;
let drops = [], notDrops = [], showPred = true;

const mmss = t => { if (!isFinite(t)) return "—";
  const m = Math.floor(t / 60); return `${m}:${(t - m * 60).toFixed(1).padStart(4, "0")}`; };
const toast = m => { const e = $("#toast"); e.textContent = m; e.classList.add("on");
  clearTimeout(e._t); e._t = setTimeout(() => e.classList.remove("on"), 1700); };

async function boot() {
  tracks = await (await fetch("/api/tracks")).json();
  renderList();
  if (tracks.length) load(0);
}

function renderList() {
  $("#trks").innerHTML = "";
  tracks.forEach((t, i) => {
    const d = document.createElement("div");
    d.className = "trk" + (i === cur ? " on" : "");
    const n = t.marks.drops.length, x = t.marks.not_drops.length;
    d.innerHTML = `<span class="nm">${t.name}</span><span class="ct">${
      t.saved || n || x ? `${n}D ${x}N` : `${t.predictions.length}?`}</span>`;
    d.onclick = () => load(i);
    $("#trks").appendChild(d);
  });
}

async function load(i) {
  if (dirty && !confirm("Unsaved marks on this track. Discard them?")) return;
  cur = i; dirty = false;
  const t = tracks[i];
  drops = t.marks.drops.map(m => ({...m}));
  notDrops = t.marks.not_drops.map(m => ({...m}));
  showPred = t.marks.predictions_visible !== false;
  dur = t.duration;
  $("#title").textContent = t.name;
  audio.src = "/audio/" + i;
  peaks = []; renderList(); syncBlind(); draw();
  peaks = (await (await fetch("/api/peaks/" + i)).json()).peaks;
  draw();
}

function sub() {
  const t = tracks[cur];
  const done = t.predictions.filter(p => verdictAt(p.t)).length;
  // The count that matters on a second pass is the UNJUDGED new ones — that is the whole
  // remaining job, and it is not the same as "predictions minus judged".
  const todo = t.predictions.filter(p => p.src === "added" && !verdictAt(p.t)).length;
  $("#sub").textContent = `${mmss(audio.currentTime)} / ${mmss(dur)}   ·   `
    + `${t.predictions.length} prediction(s), ${done} judged`
    + (todo ? `, ${todo} NEW unjudged` : "")
    + `   ·   ${t.hints.length} boundary hint(s)`;
  $("#marks").textContent = `${drops.length} drop · ${notDrops.length} not-a-drop`
    + (dirty ? "  (unsaved)" : "");
}

// A prediction counts as judged when a mark of either kind sits within half a
// bar of it. 0.9 s is under the 1.0-bar match window at any tempo we care about,
// so this never claims a verdict the scorer would disagree with.
const JUDGED_S = 0.9;
function verdictAt(t) {
  if (drops.some(m => Math.abs(m.t - t) < JUDGED_S)) return "drop";
  if (notDrops.some(m => Math.abs(m.t - t) < JUDGED_S)) return "not";
  return null;
}

// ---- drawing. Everything is told apart by SHAPE and LETTER, never hue alone.
//  drop          solid line + filled triangle
//  not-a-drop    dashed line + crossed square (+ label text)
//  prediction    hollow circle on its own lane; filled when accepted,
//                struck through when rejected
function markGlyph(ctx, x, top, bottom, kind, label) {
  ctx.save();
  ctx.strokeStyle = "#f5f5f5"; ctx.fillStyle = "#f5f5f5"; ctx.lineWidth = 2;
  ctx.setLineDash(kind === "drop" ? [] : [5, 4]);
  ctx.beginPath(); ctx.moveTo(x, top + 14); ctx.lineTo(x, bottom); ctx.stroke();
  ctx.setLineDash([]);
  if (kind === "drop") {
    ctx.beginPath(); ctx.moveTo(x - 6, top + 2); ctx.lineTo(x + 6, top + 2);
    ctx.lineTo(x, top + 13); ctx.closePath(); ctx.fill();
  } else {
    ctx.lineWidth = 1.6;
    ctx.strokeRect(x - 5, top + 2, 10, 10);
    ctx.beginPath();
    ctx.moveTo(x - 5, top + 2); ctx.lineTo(x + 5, top + 12);
    ctx.moveTo(x + 5, top + 2); ctx.lineTo(x - 5, top + 12); ctx.stroke();
    if (label) {
      ctx.font = "10px ui-monospace,monospace"; ctx.fillText(label, x + 8, top + 11);
    }
  }
  ctx.restore();
}

function predGlyph(ctx, x, y, verdict, src) {
  ctx.save();
  ctx.strokeStyle = "#cfd6e0"; ctx.fillStyle = "#cfd6e0"; ctx.lineWidth = 1.8;
  ctx.beginPath(); ctx.arc(x, y, 6, 0, Math.PI * 2);
  if (verdict === "drop") ctx.fill(); else ctx.stroke();
  if (verdict === "not") {
    ctx.beginPath(); ctx.moveTo(x - 8, y - 8); ctx.lineTo(x + 8, y + 8); ctx.stroke();
  }
  // A fire only a WIDER config makes gets a stub beneath it, so "this is new work" is
  // legible by shape. Never by hue: these judgments have to survive a colorblind reader.
  if (src === "added") {
    ctx.lineWidth = 1.4;
    ctx.beginPath(); ctx.moveTo(x, y + 7); ctx.lineTo(x, y + 12); ctx.stroke();
  }
  ctx.restore();
}

function paint(cv, t0, t1) {
  const dpr = devicePixelRatio || 1, w = cv.clientWidth, h = cv.clientHeight;
  cv.width = w * dpr; cv.height = h * dpr;
  const ctx = cv.getContext("2d"); ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, w, h);
  const span = t1 - t0 || 1, X = t => (t - t0) / span * w;
  const top = PRED_LANE, mid = top + (h - top) / 2, amp = (h - top - 34) / 2;
  const t = tracks[cur];

  if (peaks.length && dur > 0) {
    ctx.fillStyle = "#5d7fa8";
    for (let px = 0; px < w; px++) {
      const j = Math.floor((t0 + px / w * span) / dur * peaks.length);
      const v = peaks[Math.max(0, Math.min(peaks.length - 1, j))] || 0;
      ctx.fillRect(px, mid - v * amp, 1, Math.max(1, v * amp * 2));
    }
  } else { ctx.fillStyle = "#3a4250"; ctx.fillRect(0, mid - 1, w, 2); }

  // boundary hints along the floor
  ctx.strokeStyle = "#6b7683"; ctx.fillStyle = "#6b7683"; ctx.lineWidth = 1;
  ctx.font = "10px ui-monospace,monospace";
  t.hints.forEach(c => {
    if (c.t < t0 || c.t > t1) return;
    const x = X(c.t);
    ctx.beginPath(); ctx.moveTo(x, h - 13); ctx.lineTo(x, h - 2); ctx.stroke();
    if (span <= 60) ctx.fillText(c.why, x + 3, h - 4);
  });

  if (showPred) {
    ctx.strokeStyle = "#3d4552"; ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(0, PRED_LANE - 1); ctx.lineTo(w, PRED_LANE - 1); ctx.stroke();
    t.predictions.forEach(p => {
      if (p.t < t0 || p.t > t1) return;
      predGlyph(ctx, X(p.t), PRED_LANE / 2, verdictAt(p.t), p.src);
    });
  }

  drops.forEach(m => { if (m.t >= t0 && m.t <= t1) markGlyph(ctx, X(m.t), top, h - 15, "drop"); });
  notDrops.forEach(m => {
    if (m.t >= t0 && m.t <= t1) markGlyph(ctx, X(m.t), top, h - 15, "not", m.label);
  });

  const p = audio.currentTime;
  if (p >= t0 && p <= t1) {
    ctx.strokeStyle = "#ffd166"; ctx.lineWidth = 2;
    ctx.beginPath(); ctx.moveTo(X(p), 0); ctx.lineTo(X(p), h); ctx.stroke();
  }
}

function draw() {
  if (cur < 0) return;
  paint(wave, 0, dur || 1);
  const a = Math.max(0, audio.currentTime - ZOOM_S);
  paint(zoom, a, a + ZOOM_S * 2);
}
function tick() { if (cur >= 0) { draw(); sub(); } requestAnimationFrame(tick); }

// ---- marking
function add(kind, t, source, label, predT) {
  t = Math.max(0, Math.min(dur, t == null ? audio.currentTime : t));
  const m = {t, source: source || "click", label: label || null,
             pred_t: predT == null ? null : predT};
  (kind === "drop" ? drops : notDrops).push(m);
  dirty = true;
  toast(`${kind === "drop" ? "Drop" : "Not-a-drop"} at ${mmss(t)}`
        + (label ? ` (${label})` : ""));
  draw(); sub();
}

function nearestPrediction() {
  const p = tracks[cur].predictions;
  if (!p.length) return null;
  const now = audio.currentTime;
  return p.reduce((b, v) => Math.abs(v.t - now) < Math.abs(b.t - now) ? v : b, p[0]);
}

function judge(kind) {
  const p = nearestPrediction();
  if (!p) return toast("no predictions on this track — use D / N");
  if (Math.abs(p.t - audio.currentTime) > 8) {
    return toast(`nearest prediction is at ${mmss(p.t)} — press . to go there first`);
  }
  // Replace any existing verdict on this prediction.
  drops = drops.filter(m => Math.abs(m.t - p.t) >= JUDGED_S);
  notDrops = notDrops.filter(m => Math.abs(m.t - p.t) >= JUDGED_S);
  add(kind, p.t, kind === "drop" ? "confirm" : "reject",
      kind === "not" ? "break" : null, p.t);
}

function delNearest() {
  const now = audio.currentTime;
  const all = [...drops.map(m => ["drop", m]), ...notDrops.map(m => ["not", m])];
  if (!all.length) return toast("no marks");
  const [kind, m] = all.reduce(
    (b, v) => Math.abs(v[1].t - now) < Math.abs(b[1].t - now) ? v : b, all[0]);
  const arr = kind === "drop" ? drops : notDrops;
  arr.splice(arr.indexOf(m), 1);
  dirty = true; toast(`deleted ${kind === "drop" ? "drop" : "not-a-drop"} at ${mmss(m.t)}`);
  draw(); sub();
}

function setLabel(i) {
  if (!notDrops.length) return toast("no not-a-drop marks to label");
  const now = audio.currentTime;
  const m = notDrops.reduce((b, v) => Math.abs(v.t - now) < Math.abs(b.t - now) ? v : b,
                            notDrops[0]);
  m.label = LABELS[i]; dirty = true;
  toast(`${mmss(m.t)} labelled "${LABELS[i]}"`); draw(); sub();
}

function jump(dir) {
  const p = tracks[cur].predictions;
  if (!p.length) return toast("no predictions seeded");
  const now = audio.currentTime;
  const nxt = dir > 0 ? p.find(x => x.t > now + 1.0)
                      : [...p].reverse().find(x => x.t < now - 1.0);
  const target = nxt || (dir > 0 ? p[p.length - 1] : p[0]);
  // Land a few seconds early so the run-up is audible, not just the hit.
  audio.currentTime = Math.max(0, target.t - 6.0);
  const v = verdictAt(target.t);
  toast(`prediction ${p.indexOf(target) + 1}/${p.length} at ${mmss(target.t)}`
        + (v ? ` — already ${v === "drop" ? "accepted" : "rejected"}` : ""));
  draw(); sub();
}

function syncBlind() {
  $("#blind").textContent = showPred ? "Predictions: shown" : "Predictions: HIDDEN";
  $("#blind").classList.toggle("on", !showPred);
}

async function save() {
  const r = await fetch("/api/save", {
    method: "POST", headers: {"Content-Type": "application/json"},
    body: JSON.stringify({track: cur, drops, not_drops: notDrops,
                          predictions_visible: showPred}),
  });
  const j = await r.json();
  if (!r.ok) return toast("save failed: " + (j.error || r.status));
  tracks[cur].marks = {drops: drops.map(m => ({...m})),
                       not_drops: notDrops.map(m => ({...m})),
                       predictions_visible: showPred};
  tracks[cur].saved = true; dirty = false;
  renderList(); sub(); toast("saved " + j.path);
}

function at(cv, ev, t0, t1) {
  const r = cv.getBoundingClientRect();
  return Math.max(0, Math.min(dur, t0 + (ev.clientX - r.left) / r.width * (t1 - t0)));
}
function wire(cv, span) {
  cv.addEventListener("mousedown", ev => {
    const [t0, t1] = span(), t = at(cv, ev, t0, t1);
    if (ev.shiftKey) return add("drop", t);
    if (ev.altKey) return add("not", t);
    audio.currentTime = t; draw(); sub();
    const mv = e => { audio.currentTime = at(cv, e, t0, t1); draw(); sub(); };
    const up = () => { removeEventListener("mousemove", mv); removeEventListener("mouseup", up); };
    addEventListener("mousemove", mv); addEventListener("mouseup", up);
  });
}
wire(wave, () => [0, dur || 1]);
wire(zoom, () => { const a = Math.max(0, audio.currentTime - ZOOM_S); return [a, a + ZOOM_S * 2]; });

$("#play").onclick = () => audio.paused ? audio.play() : audio.pause();
$("#acc").onclick = () => judge("drop");
$("#rej").onclick = () => judge("not");
$("#mkD").onclick = () => add("drop");
$("#mkN").onclick = () => add("not");
$("#del").onclick = delNearest;
$("#save").onclick = save;
$("#blind").onclick = () => { showPred = !showPred; dirty = true; syncBlind(); draw(); };
audio.onplay = () => $("#play").textContent = "Pause";
audio.onpause = () => $("#play").textContent = "Play";

addEventListener("keydown", e => {
  if (e.target.tagName === "INPUT") return;
  const k = e.key === " " ? " " : e.key.toLowerCase();
  const map = {
    " ": () => audio.paused ? audio.play() : audio.pause(),
    "arrowright": () => audio.currentTime += 5, "arrowleft": () => audio.currentTime -= 5,
    "k": () => audio.currentTime += 15, "j": () => audio.currentTime -= 15,
    ".": () => jump(1), ",": () => jump(-1),
    "y": () => judge("drop"), "x": () => judge("not"),
    "d": () => add("drop"), "n": () => add("not"),
    "backspace": delNearest, "delete": delNearest,
    "1": () => setLabel(0), "2": () => setLabel(1), "3": () => setLabel(2), "4": () => setLabel(3),
    "h": () => { showPred = !showPred; dirty = true; syncBlind(); },
    "s": save,
  };
  if (map[k]) { e.preventDefault(); map[k](); draw(); sub(); }
});
addEventListener("beforeunload", e => { if (dirty) { e.preventDefault(); e.returnValue = ""; } });
addEventListener("resize", draw);

boot(); tick();
</script>
"""


# ---------------------------------------------------------------- main

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("files", nargs="*", type=Path,
                    help="audio files; default = every dumped track with playable audio")
    ap.add_argument("--out-dir", type=Path, default=REPO / "bench" / "labels")
    ap.add_argument("--sidecar-dir", type=Path,
                    default=REPO / "bench" / "out" / "dropsweep" / "music")
    ap.add_argument("--music-dir", type=Path, default=Path.home() / "Music",
                    help="where to look for audio when discovering from sidecars")
    ap.add_argument("--predictions", type=Path, default=None,
                    help="seed another config's fires alongside the shipped ones: a "
                         "{track_id: [times]} JSON, as written by sweep_drop.py --fires. "
                         "Existing marks are kept, so a second pass only adds work for the "
                         "moments nobody has judged yet (#2365).")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--no-open", action="store_true")
    args = ap.parse_args()

    extra = load_extra_predictions(args.predictions) if args.predictions else {}

    files = list(args.files)
    if not files:
        # Strip the known sidecar suffixes rather than cutting at the first dot:
        # real track names contain dots ("(feat. Kadhal Nathi)", "(SixLoaded.com)")
        # and splitting on them silently drops those tracks from the session.
        def stem_of(name: str) -> str:
            for suf in (".new.signal.jsonl", ".signal.jsonl", ".jsonl"):
                if name.endswith(suf):
                    return name[: -len(suf)]
            return name

        stems = sorted({stem_of(p.name) for p in args.sidecar_dir.glob("*.jsonl")})
        for stem in stems:
            hit = next((c for ext in AUDIO_EXT
                        for c in [args.music_dir / f"{stem}{ext}"] if c.exists()), None)
            if hit:
                files.append(hit)
            else:
                print(f"  no audio for dumped track: {stem}", file=sys.stderr)
        if not files:
            sys.exit(f"no dumped tracks with audio under {args.music_dir}; name files explicitly")

    args.out_dir.mkdir(parents=True, exist_ok=True)
    tracks = []
    for i, f in enumerate(files):
        if not f.exists():
            sys.exit(f"no such file: {f}")
        try:
            dur = duration_of(f)
        except Exception as e:  # noqa: BLE001 — an unreadable track is fatal, loudly
            sys.exit(f"cannot read {f}: {e}")
        tracks.append({
            "id": i, "name": f.name, "path": str(f), "duration": dur,
            "predictions": predictions(f.stem, args.sidecar_dir, extra.get(f.stem)),
            "hints": hints(f.stem, args.sidecar_dir),
            "marks": load_marks(f.stem, args.out_dir),
            "saved": bundle_path(f.stem, args.out_dir).exists(),
            "peaks": None,
        })

    if args.predictions:
        missing = sorted(set(extra) - {Path(t["path"]).stem for t in tracks})
        if missing:
            # Silence here would read as "that config fires nowhere new on those tracks".
            print(f"  WARNING: --predictions names {len(missing)} track(s) not in this "
                  f"session: {', '.join(missing[:4])}", file=sys.stderr)

    Handler.tracks = tracks
    Handler.out_dir = args.out_dir

    if args.predictions:
        for t in tracks:
            t["pred_source"] = f"sidecar d_fired (shipped) + {args.predictions.name}"

    url = f"http://127.0.0.1:{args.port}/"
    n_pred = sum(len(t["predictions"]) for t in tracks)
    n_add = sum(1 for t in tracks for p in t["predictions"] if p["src"] == "added")
    print(f"\n  {len(tracks)} track(s), {n_pred} detector prediction(s) to judge"
          + (f", {n_add} of them NEW (seeded from {args.predictions.name})"
             if args.predictions else ""))
    for t in tracks:
        new = sum(1 for p in t["predictions"] if p["src"] == "added")
        print(f"    {'*' if t['saved'] else ' '} {t['name']}  "
              f"({len(t['predictions'])} pred{f', {new} new' if new else ''}, "
              f"{len(t['hints'])} hint)")
    print(f"\n  -> {url}    (ctrl-c when done)\n")

    if not args.no_open:
        threading.Timer(0.5, lambda: webbrowser.open(url)).start()
    with Server(("127.0.0.1", args.port), Handler) as srv:
        try:
            srv.serve_forever()
        except KeyboardInterrupt:
            print("\n  stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
