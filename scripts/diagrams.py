# /// script
# requires-python = ">=3.11"
# ///
"""Draw the architecture diagrams as SVG pairs, one per GitHub theme.

    uv run scripts/diagrams.py            # writes docs/images/<name>-{light,dark}.svg
    uv run scripts/diagrams.py tiers beat # only these

Four perspectives on the whole system (tiers, runtime, dataflow, builds) and one diagram per
flow (hop, beat, frame, signal, round, model, offline). Same grammar as giro's pipeline
diagram: white boxes for stages, orange pills for what goes in and what comes out, blue
diamonds for gates and blue edges for their outcomes, dashed boxes for the optional and the
dev-only, a ring for a loop. docs/DIAGRAMS.md shows each pair through a <picture> element.
"""

import math
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parents[1] / "docs" / "images"
FONT = "ui-monospace, 'JetBrains Mono', SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace"

# The same two accents as the app and giro: orange (#ff6040) for in / out, blue for gates.
THEMES = {
    "light": dict(
        node="#ffffff", node_stroke="#8c959f", text="#1f2328", muted="#59636e",
        io="#ffece6", io_stroke="#ff6040", gate="#e3edfa", gate_stroke="#3a70c0",
        retry="#eef4fc", retry_stroke="#5090e0", ring="#b6bec7", arrow="#6e7781", bg="#ffffff",
        band="#f6f8fa", band_stroke="#d0d7de",
    ),
    "dark": dict(
        node="#0d1117", node_stroke="#6e7681", text="#e6edf3", muted="#9198a1",
        io="#4a2317", io_stroke="#ff6040", gate="#14305c", gate_stroke="#5090e0",
        retry="#1a2740", retry_stroke="#5090e0", ring="#484f58", arrow="#8b949e", bg="#0d1117",
        band="#161b22", band_stroke="#30363d",
    ),
}

NODE_W, NODE_H = 112, 46


def f(v: float) -> str:
    return f"{v:.1f}".rstrip("0").rstrip(".")


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


TITLE_PX, SUB_PX = 8.3, 6.4   # per character at 13 px bold and 9.5 px letter-spaced, monospace


def fit(w, title, *subs):
    """The box width the text needs, if wider than the one asked for."""
    need = max([len(title) * TITLE_PX + 16] + [len(s) * SUB_PX + 14 for s in subs if s])
    return max(w, math.ceil(need / 2) * 2)


def box(cx, cy, title, sub="", kind="node", w=NODE_W, h=NODE_H, dashed=False, pill=False):
    w = fit(w, title, sub)
    x, y = cx - w / 2, cy - h / 2
    dash = ' stroke-dasharray="4 3"' if dashed else ""
    r = h / 2 if pill else 2
    return (
        f'<rect x="{f(x)}" y="{f(y)}" width="{w}" height="{h}" rx="{f(r)}" class="{kind}"{dash}/>'
        f'<text x="{f(cx)}" y="{f(cy - 3 if sub else cy + 4.5)}" class="title">{esc(title)}</text>'
        + (f'<text x="{f(cx)}" y="{f(cy + 13)}" class="sub">{esc(sub)}</text>' if sub else "")
    )


def box3(cx, cy, title, sub, sub2, kind="node", w=NODE_W, h=58, dashed=False):
    """A box with two sub lines."""
    w = fit(w, title, sub, sub2)
    x, y = cx - w / 2, cy - h / 2
    dash = ' stroke-dasharray="4 3"' if dashed else ""
    return (
        f'<rect x="{f(x)}" y="{f(y)}" width="{w}" height="{h}" rx="2" class="{kind}"{dash}/>'
        f'<text x="{f(cx)}" y="{f(cy - 9)}" class="title">{esc(title)}</text>'
        f'<text x="{f(cx)}" y="{f(cy + 7)}" class="sub">{esc(sub)}</text>'
        f'<text x="{f(cx)}" y="{f(cy + 20)}" class="sub">{esc(sub2)}</text>'
    )


def diamond(cx, cy, r, title, sub):
    pts = f"{f(cx)},{f(cy - r)} {f(cx + r)},{f(cy)} {f(cx)},{f(cy + r)} {f(cx - r)},{f(cy)}"
    return (
        f'<polygon points="{pts}" class="gate"/>'
        f'<text x="{f(cx)}" y="{f(cy - 2)}" class="title">{esc(title)}</text>'
        f'<text x="{f(cx)}" y="{f(cy + 13)}" class="sub">{esc(sub)}</text>'
    )


def line(d: str, cls="edge", dashed=False, arrow=True) -> str:
    dash = ' stroke-dasharray="4 3"' if dashed else ""
    m = ' marker-end="url(#arrow)"' if arrow else ""
    return f'<path d="{d}" class="{cls}"{dash}{m}/>'


def label(x, y, text, anchor="start", cls="edge-label"):
    return f'<text x="{f(x)}" y="{f(y)}" class="{cls}" style="text-anchor: {anchor}">{esc(text)}</text>'


def eyebrow(x, y, text, anchor="start"):
    return f'<text x="{f(x)}" y="{f(y)}" class="eyebrow" style="text-anchor: {anchor}">{esc(text)}</text>'


def band(x, y, w, h, title=""):
    """A tinted region with an eyebrow in its top-left corner."""
    s = f'<rect x="{f(x)}" y="{f(y)}" width="{w}" height="{h}" rx="6" class="band"/>'
    if title:
        s += eyebrow(x + 14, y + 20, title)
    return s


def frame_box(x, y, w, h, title=""):
    """A dashed outline (a loop body or a grouping) with an eyebrow."""
    s = f'<rect x="{f(x)}" y="{f(y)}" width="{w}" height="{h}" rx="6" class="group"/>'
    if title:
        s += eyebrow(x + 14, y + 20, title)
    return s


def vline(x, y0, y1, cls="edge", dashed=False):
    return line(f"M{f(x)} {f(y0)} V{f(y1)}", cls, dashed)


def hline(y, x0, x1, cls="edge", dashed=False):
    return line(f"M{f(x0)} {f(y)} H{f(x1)}", cls, dashed)


def elbow(x0, y0, x1, y1, cls="edge", dashed=False, first="h"):
    """Two-segment connector: horizontal then vertical (first='h') or vertical then horizontal."""
    if first == "h":
        return line(f"M{f(x0)} {f(y0)} H{f(x1)} V{f(y1)}", cls, dashed)
    return line(f"M{f(x0)} {f(y0)} V{f(y1)} H{f(x1)}", cls, dashed)


def ring_pt(cx, cy, r, deg):
    a = math.radians(deg)
    return cx + r * math.cos(a), cy - r * math.sin(a)


def ring_arc(cx, cy, r, a0, a1, cls="edge", dashed=False):
    x0, y0 = ring_pt(cx, cy, r, a0)
    x1, y1 = ring_pt(cx, cy, r, a1)
    ccw = (a1 - a0) % 360 < 180
    sweep = 0 if ccw else 1
    dash = ' stroke-dasharray="4 3"' if dashed else ""
    return (f'<path d="M{f(x0)} {f(y0)} A{r} {r} 0 0 {sweep} {f(x1)} {f(y1)}" '
            f'class="{cls}"{dash} marker-end="url(#arrow)"/>')


def ring_text(cx, cy, r, a0, a1, text, pid):
    x0, y0 = ring_pt(cx, cy, r, a0)
    x1, y1 = ring_pt(cx, cy, r, a1)
    return (f'<path id="{pid}" d="M{f(x0)} {f(y0)} A{r} {r} 0 0 1 {f(x1)} {f(y1)}" fill="none"/>'
            f'<text class="eyebrow"><textPath href="#{pid}" startOffset="50%" style="text-anchor: middle">{esc(text)}</textPath></text>')


def beads(cx, cy, r, step=6):
    return "".join(f'<circle cx="{f(x)}" cy="{f(y)}" r="2.4" class="cam"/>'
                   for x, y in (ring_pt(cx, cy, r, d) for d in range(0, 360, step)))


def document(parts, t: dict, w: int, h: int, aria: str, y0: int = 0) -> str:
    style = f"""
      text {{ font-family: {FONT}; text-anchor: middle; }}
      .title {{ font-size: 13px; font-weight: 600; fill: {t['text']}; }}
      .sub {{ font-size: 9.5px; letter-spacing: 0.06em; fill: {t['muted']}; }}
      .eyebrow {{ font-size: 10.5px; font-weight: 600; letter-spacing: 0.18em; fill: {t['muted']}; }}
      .node {{ fill: {t['node']}; stroke: {t['node_stroke']}; stroke-width: 1.3; }}
      .io {{ fill: {t['io']}; stroke: {t['io_stroke']}; stroke-width: 1.6; }}
      .gate {{ fill: {t['gate']}; stroke: {t['gate_stroke']}; stroke-width: 1.6; }}
      .retry {{ fill: {t['retry']}; stroke: {t['retry_stroke']}; stroke-width: 1.3; }}
      .band {{ fill: {t['band']}; stroke: {t['band_stroke']}; stroke-width: 1; }}
      .group {{ fill: none; stroke: {t['ring']}; stroke-width: 1.2; stroke-dasharray: 5 4; }}
      .cam {{ fill: {t['bg']}; stroke: {t['arrow']}; stroke-width: 1.1; }}
      .edge {{ fill: none; stroke: {t['arrow']}; stroke-width: 1.5; stroke-linejoin: round; }}
      .edge.blue {{ stroke: {t['retry_stroke']}; marker-end: url(#arrow-blue); }}
      .edge.orange {{ stroke: {t['io_stroke']}; marker-end: url(#arrow-orange); }}
      .edge-label {{ font-size: 11px; font-style: italic; fill: {t['muted']}; }}
      .edge-label.blue {{ fill: {t['retry_stroke']}; font-style: normal; font-weight: 600; letter-spacing: 0.04em; font-size: 10.5px; }}
      .edge-label.orange {{ fill: {t['io_stroke']}; font-style: normal; font-weight: 600; letter-spacing: 0.04em; font-size: 10.5px; }}
      .note {{ font-size: 10.5px; fill: {t['muted']}; }}
    """
    marker = ('<marker id="{id}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" '
              'orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="{fill}"/></marker>')
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 {y0} {w} {h}" width="{w}" height="{h}" '
        f'role="img" aria-label="{esc(aria)}">'
        f'<defs>{marker.format(id="arrow", fill=t["arrow"])}{marker.format(id="arrow-blue", fill=t["retry_stroke"])}'
        f'{marker.format(id="arrow-orange", fill=t["io_stroke"])}</defs>'
        f"<style>{style}</style>" + "".join(parts) + "</svg>\n"
    )


# ---------------------------------------------------------------------------------------------
# Perspectives
# ---------------------------------------------------------------------------------------------

def tiers():
    """What ships and decides, what ships but only records, what never ships."""
    p = []
    p.append(band(30, 20, 875, 270, "PRODUCTION · EVERY RELEASE BUILD · THE SAME CHAIN LIVE, HEADLESS AND OFFLINE"))
    y = 150
    p.append(box(80, y, "Audio in", "DEVICE · FILE", "io", w=100, pill=True))
    p.append(box(200, y, "Capture", "CPAL · WASAPI", w=110))
    p.append(box(360, y, "Per-hop chain", "FFT · BEAT · KEY · MORE", w=170))
    p.append(box(525, y, "Frame", "85 FEATURES · EVENTS", w=144))
    p.append(box(680, 100, "Signal", "OSC · --signal", w=130))
    p.append(box(680, 200, "Visuals", "BINDINGS · LAYERS", w=130))
    p.append(box(680, 255, "Controls", "MIDI · OSC · WEB", w=130))
    p.append(box(830, 100, "Rigs", "TD · RESOLUME · MAX", "io", w=140, pill=True))
    p.append(box3(830, 200, "Outputs", "WINDOW · NDI · SPOUT", "SYPHON · VCAM · MP4", "io", w=140))
    p.append(hline(y, 130, 143)); p.append(hline(y, 255, 273)); p.append(hline(y, 445, 451))
    p.append(line("M597 142 H605 V100 H613"))
    p.append(line("M597 158 H605 V200 H613"))
    p.append(vline(680, 232, 225))
    p.append(hline(100, 745, 758)); p.append(hline(200, 745, 758))
    p.append(band(30, 310, 875, 120, "INSTRUMENTATION · IN EVERY BUILD, INERT UNTIL AN ENV VAR NAMES A FILE · RECORDS, NEVER DECIDES"))
    iy = 375
    p.append(box(200, iy, "Hop sidecar", "FOSFORA_HOP_SIDECAR", w=150, dashed=True))
    p.append(box(360, iy, "Beat sidecar", "FOSFORA_BEAT_SIDECAR", w=150, dashed=True))
    p.append(box(525, iy, "Key · structure", "FOSFORA_*_SIDECAR", w=150, dashed=True))
    p.append(box(760, iy, "Frame log", "FOSFORA_FRAME_LOG", w=150, dashed=True))
    p.append(line("M300 173 V240 H200 V351", dashed=True))
    p.append(vline(360, 173, 351, dashed=True))
    p.append(line("M420 173 V240 H525 V351", dashed=True))
    p.append(line("M745 215 H752 V351", dashed=True))
    p.append(band(30, 450, 875, 210, "DEV TOOLING · --features analyze · NEVER IN A DOWNLOAD"))
    dy = 510
    p.append(box3(200, dy, "Model training", "scripts/beat_model", "scripts/kick_model", w=150))
    p.append(box3(410, dy, "Offline CLI", "--signal-dump · --analyze", "--render-scene · --validate", w=190))
    p.append(box3(615, dy, "Bench", "DATASETS · MANIFESTS", "ARMS · VERDICT · HOLDOUT", w=184))
    p.append(box(800, dy, "Replicas", "REPLAYS · FORECASTS", w=150))
    p.append(box(200, 605, "beat_model.json", "TREES · LAG · PICKER", "io", w=150, pill=True))
    p.append(vline(200, 399, 480, dashed=True))
    p.append(line("M360 399 V440 H800 V480", dashed=True))
    p.append(line("M525 399 V440", dashed=True, arrow=False))
    p.append(hline(dy, 505, 521))
    p.append(hline(dy, 707, 723))
    p.append(vline(200, 539, 581))
    p.append(line("M122 605 H15 V60 H360 V126", cls="edge orange"))
    p.append(label(190, 55, "the trained model ships inside the chain", "middle", "edge-label orange"))
    return p, 920, 680, ("Fosfora by tier: a production chain (audio in, capture, the 86 Hz per-hop chain, the frame, "
                         "visuals to window and sinks, Signal to rigs) in every build; sidecars and logs that ship but "
                         "only record when an env var names a file; dev tooling behind the analyze feature (offline CLI, "
                         "bench, replicas, model training) whose trained model ships back into the chain.")


def runtime():
    """Threads and the channels between them, in the windowed app."""
    p = []
    lx, mx, rx = 120, 450, 780
    p.append(frame_box(325, 30, 250, 560, "MAIN THREAD · EVERY FRAME"))
    main = [
        (90, "Drain inputs", "MIDI → OSC → WEB"),
        (160, "Bindings → params", "LATEST FRAME · TRANSFORMS"),
        (230, "Per-layer passes", "COMPUTE · EFFECT · PARTICLES"),
        (300, "Compositor", "BLEND · DISPLACE · HDR"),
        (370, "Post-process", "BLOOM · TONEMAP"),
        (440, "egui overlay", "PANELS · SHELL"),
        (510, "Present + readback", "SURFACE · FRAME CAPTURE"),
    ]
    for y, t, s in main:
        p.append(box(mx, y, t, s, w=210))
    for (y0, _, _), (y1, _, _) in zip(main, main[1:]):
        p.append(vline(mx, y0 + 23, y1 - 25))
    p.append(box(lx, 50, "MIDI thread", "MIDIR · BOUNDED 64", w=150))
    p.append(box(lx, 110, "OSC thread", "ROSC UDP · BOUNDED 64", w=150))
    p.append(box3(lx, 175, "Web threads", "ONE PER CLIENT", "HTTP · WS · JSON", w=150))
    p.append(box(lx, 260, "Capture", "CPAL · WASAPI · PULSE", w=150))
    p.append(box3(lx, 335, "Audio thread", "HOP CHAIN · 86 HZ", "AudioFrame CHANNEL", w=150))
    p.append(box(lx, 420, "File watcher", "NOTIFY · DEBOUNCED", w=150))
    p.append(box(lx, 490, "Depth thread", "DEPTH ANYTHING", w=150, dashed=True))
    for y in (50, 110, 175):
        p.append(line(f"M201 {y} H290 V90", arrow=False))
    p.append(hline(90, 290, 343))
    p.append(vline(lx, 273, 304))
    p.append(line("M195 335 H310 V160 H343"))
    p.append(line("M195 420 H320", arrow=False))
    p.append(line("M195 490 H320 V230 H343"))
    p.append(label(338, 82, "last write wins", "end"))
    p.append(label(338, 152, "one frame per hop", "end"))
    p.append(label(338, 222, "shader reload · depth maps", "end"))
    p.append(box(rx, 90, "Web clients", "SNAPSHOT · AUDIO 10 HZ", w=190))
    p.append(box(rx, 160, "OSC out", "/fosfora/audio/*", w=190))
    p.append(box3(rx, 440, "Sink threads", "NDI · SPOUT · SYPHON · V4L2", "FRAME CHANNEL · HEALTH", w=204))
    p.append(box3(rx, 520, "Recording", "ENCODER + AUDIO THREADS", "FFMPEG · MP4", w=190))
    p.append(line("M555 90 H683"))
    p.append(line("M555 160 H683"))
    p.append(line("M555 510 H640 V440 H676"))
    p.append(line("M640 510 V520 H683"))
    p.append(label(565, 82, "broadcast", "start"))
    p.append(label(565, 152, "features + pulses", "start"))
    p.append(label(565, 502, "readback", "start"))
    p.append(label(450, 606, "no mutex in the hot path: every edge is a channel, drained in order each frame", "middle", "note"))
    return p, 900, 620, ("Fosfora at runtime: input threads (MIDI, OSC, web, capture into the audio thread's hop chain, "
                         "a file watcher, an optional depth thread) feed the main thread through channels; the main "
                         "thread drains inputs, applies bindings, renders per-layer passes, composites, post-processes, "
                         "draws egui and presents; readback feeds sink threads (NDI, Spout, Syphon, v4l2) and recording; "
                         "web clients and OSC out are broadcast from the main thread.")


def dataflow():
    """What comes in, what goes out, and the two consumers of the analysis."""
    p = []
    p.append(box(100, 150, "Audio", "DEVICE · MONITOR · FILE", "io", w=140, pill=True))
    p.append(box(100, 260, "Controls", "MIDI · OSC · WEB · LINK", "io", w=140, pill=True))
    p.append(box(100, 370, "Presets · scenes", "JSON · TRAMA GRAPHS", "io", w=140, pill=True))
    p.append(box(100, 460, "Media", "VIDEO · WEBCAM · SPLATS", "io", w=140, pill=True))
    p.append(box(320, 150, "Analysis", "85 FEATURES · EVENTS", w=170))
    p.append(box(540, 150, "Signal", "FOUR CADENCES · /fosfora/v1", w=200))
    p.append(box(790, 150, "Rigs", "TD · RESOLUME · MAX", "io", w=150, pill=True))
    p.append(box(540, 60, "OSC out · web", "/fosfora/audio/* · 10 HZ", w=182, dashed=True))
    p.append(box(790, 60, "Listeners", "THE APP'S OWN TELEMETRY", "io", w=150, pill=True, dashed=True))
    p.append(box(320, 260, "Bindings", "SOURCES → TARGETS", w=170))
    p.append(box(540, 260, "Params", "ONE STORE PER LAYER", w=150))
    p.append(box(790, 260, "Layers", "EFFECTS · MEDIA · TRAMA", w=150))
    p.append(box(790, 370, "Compositor + post", "13 BLENDS · BLOOM", w=158))
    p.append(box3(790, 480, "Outputs", "WINDOW · NDI · SPOUT", "SYPHON · VCAM · MP4", "io", w=150))
    p.append(hline(150, 190, 233)); p.append(hline(150, 405, 438)); p.append(hline(150, 640, 713))
    p.append(line("M320 127 V60 H447", dashed=True)); p.append(hline(60, 631, 713, dashed=True))
    p.append(vline(320, 173, 235))
    p.append(hline(260, 190, 233)); p.append(hline(260, 405, 463)); p.append(hline(260, 615, 713))
    p.append(line("M190 370 H540 V285"))
    p.append(line("M190 460 H690 V272 H713"))
    p.append(vline(790, 283, 345)); p.append(vline(790, 393, 449))
    p.append(label(212, 142, "hop chain", "middle")); p.append(label(422, 142, "same chain", "middle"))
    p.append(label(330, 210, "every hop", "start"))
    p.append(label(380, 52, "features + pulses, per frame", "middle"))
    return p, 900, 540, ("Fosfora's data flow: audio, controls, presets and media come in; the analysis feeds Signal "
                         "(to rigs over OSC) and the bindings that drive per-layer params; layers composite and post-"
                         "process into the window and the sinks; the app also broadcasts its own telemetry over OSC and "
                         "to web clients.")


def builds():
    """One crate, the feature sets, and what each build is for."""
    p = []
    p.append(box3(130, 200, "crates/fosfora-app", "ONE CRATE · LIB + BINARY", "cargo features choose the rest", w=200))
    p.append(band(290, 30, 320, 190, "release = WHAT A DOWNLOAD SHIPS"))
    p.append(box(450, 80, "desktop", "WINDOW · EGUI · MIDI · WEB · WATCH", w=260))
    p.append(box(450, 130, "video · webcam · depth", "MEDIA LAYERS · DEPTH ANYTHING", w=260))
    p.append(box(450, 180, "ndi · spout · syphon · v4l2", "SINKS · INERT OFF THEIR OS", w=260))
    p.append(box(450, 260, "link", "ABLETON LINK · MAKES THE BINARY GPL", w=260, dashed=True))
    p.append(box(450, 320, "analyze = desktop + decode", "OFFLINE CLI · BENCH · DEV ONLY", w=260, dashed=True))
    p.append(box(450, 380, "profiling", "WGPU PROFILER", w=260, dashed=True))
    p.append(box(450, 440, "no default features", "THE LIB ALONE · NO DESKTOP CRATES", w=260, dashed=True))
    p.append(line("M233 190 H270 V80 H318")); p.append(line("M270 130 H318")); p.append(line("M270 180 H318"))
    p.append(line("M270 180 V440 H318")); p.append(line("M270 260 H318")); p.append(line("M270 320 H318")); p.append(line("M270 380 H318"))
    p.append(box3(770, 110, "Download", "MACOS · WINDOWS · LINUX", "app · --signal · --render-loop", "io", w=200))
    p.append(box3(770, 260, "Opt-in build", "cargo build --features link", "GPL DOWNSTREAM", w=200, dashed=True))
    p.append(box3(770, 350, "Dev build", "target/analyze · --signal-dump", "--analyze · BENCH · CI FIXTURE", w=200, dashed=True))
    p.append(box3(770, 440, "fosfora-xr", "QUEST · ANDROID · xr BRANCH", "ITS OWN FRONTEND", w=200, dashed=True))
    p.append(line("M610 130 H640 V110 H665"))
    p.append(line("M580 260 H668")); p.append(line("M580 320 H640 V350 H665")); p.append(line("M580 440 H668"))
    p.append(label(652, 102, "release.yml", "start"))
    return p, 900, 500, ("Fosfora's builds: one crate whose release feature set (desktop, video, webcam, depth, NDI, "
                         "Spout, Syphon, v4l2) is the download, which runs as the app, as headless --signal and as "
                         "--render-loop; link is opt-in and GPL; analyze adds the offline CLI for the bench and CI; "
                         "profiling is dev; the lib with no default features is the XR frontend's base.")


# ---------------------------------------------------------------------------------------------
# Flows
# ---------------------------------------------------------------------------------------------

def hop():
    """The per-hop analysis chain, in the order the code runs it."""
    p = []
    p.append(eyebrow(450, 32, "EVERY HOP · 86 TIMES A SECOND · THE SAME CODE LIVE, HEADLESS AND OFFLINE", "middle"))
    W = 124
    r1 = [(85, "Audio hop", "512 SAMPLES", "io"), (235, "DC strip", "MONO + L/R"),
          (385, "Multi-res FFT", "4096 · 1024 · 512"), (535, "Loudness", "BS.1770 · SILENCE"),
          (685, "Stereo field", "WIDTH · BAND PAN"), (835, "HPSS", "H / P SPLIT")]
    r2 = [(835, "Pitch", "YIN F0"), (685, "Contrast", "PEAK VS VALLEY"),
          (535, "Timbre", "ΔMFCC · TIMBRE FLUX"), (385, "Kick envelope", "P95 · SILENCE-GATED"),
          (235, "Key", "ENERGY CHROMA"), (85, "Snapshot", "PRE-NORM · TRACKERS")]
    r3 = [(85, "Normalize", "4 POLICIES · GATED"), (235, "Beat detector", "ONSET · TEMPO · GRID"),
          (385, "Downbeat", "BAR · METER"), (535, "Structure", "SECTION · DROP"),
          (685, "Smooth", "ASYMMETRIC EMA"), (835, "Frame", "85 FEATURES · EVENTS", "io")]
    for y, row, back in ((100, r1, False), (220, r2, True), (340, r3, False)):
        for it in row:
            x, t, s = it[0], it[1], it[2]
            kind = it[3] if len(it) > 3 else "node"
            p.append(box(x, y, t, s, kind, w=W, pill=(kind == "io")))
        xs = [it[0] for it in row]
        for a, b in zip(xs, xs[1:]):
            if back:
                p.append(hline(y, a - W / 2 - 1, b + W / 2 + 3))
            else:
                p.append(hline(y, a + W / 2 + 1, b - W / 2 - 3))
    p.append(vline(835, 123, 195)); p.append(vline(85, 243, 315))
    p.append(box(235, 160, "Key sidecar", "FOSFORA_KEY_SIDECAR", w=W, h=38, dashed=True))
    p.append(box(400, 440, "Beat + hop sidecars", "FOSFORA_BEAT/HOP_SIDECAR", w=170, h=38, dashed=True))
    p.append(box(640, 440, "Structure sidecar", "FOSFORA_STRUCTURE_SIDECAR", w=170, h=38, dashed=True))
    p.append(vline(235, 197, 181, dashed=True))
    p.append(line("M300 363 V400 H400 V419", dashed=True))
    p.append(line("M560 363 V400 H640 V419", dashed=True))
    p.append(label(450, 480, "dashed: records for the bench and the model, only when the env var names a file", "middle", "note"))
    return p, 900, 500, ("The per-hop chain: DC strip, multi-resolution FFT, loudness and the silence flag, stereo field, "
                         "HPSS, pitch, contrast, timbre, kick envelope, key, a pre-normalization snapshot, normalize, "
                         "the beat detector, downbeat, structure, smoothing, and the frame of 85 features and events; "
                         "sidecars tap key, beat, hop evidence and structure when an env var names a file.")


def beat():
    """The beat path: the estimator earns a tempo, the grid runs as a loop, the arbiter can move it."""
    p = []
    CX, CY, R = 560, 320, 150
    ST = {"emit": 90, "settle": 162, "pll": 234, "fold": 306, "gate": 18}
    gap = 17
    p.append(ring_text(CX, CY, R + 40, 160, 80, "FOR EACH GRID INSTANT", "gridpath"))
    order = ["emit", "settle", "pll", "fold", "gate"]
    for a, b in zip(order, order[1:]):
        p.append(ring_arc(CX, CY, R, ST[a] + gap, ST[b] - gap))
    p.append(ring_arc(CX, CY, R, ST["gate"] + gap, ST["emit"] - gap))
    lx, ly = ring_pt(CX, CY, R + 14, 54)
    p.append(label(lx + 2, ly, "no shift", "start"))
    p.append(beads(CX, CY, R))
    p.append(box(*ring_pt(CX, CY, R, ST["emit"]), "Emit", "10 MS EARLY"))
    p.append(box(*ring_pt(CX, CY, R, ST["settle"]), "Settle", "TRACKERS' CLOCK"))
    p.append(box(*ring_pt(CX, CY, R, ST["pll"]), "PLL", "ONSET → PHASE"))
    p.append(box(*ring_pt(CX, CY, R, ST["fold"]), "Fold", "8 + 32 BEATS"))
    gx, gy = ring_pt(CX, CY, R, ST["gate"])
    p.append(diamond(gx, gy, 46, "Vote?", "2:1 · 1.5:1"))
    p.append(box(CX, 300, "Grid", "PERIOD · PHASE · TRIM", w=150))
    p.append(box(CX, 366, "Shift", "½ · ¼ · ¾ LATER", "retry", w=110, h=38))
    p.append(line(f"M{f(gx)} {f(gy + 48)} Q{f(gx - 30)} 366 {f(CX + 57)} 366", cls="edge blue", dashed=True))
    p.append(vline(CX, 347, 325, cls="edge blue"))
    p.append(label(gx - 10, 406, "three votes in a row", "end", "edge-label blue"))
    p.append(box(110, 120, "Onset flux", "SUPERFLUX · 3 BANDS", w=136))
    p.append(diamond(110, 230, 46, "Seed?", "FLUX CONF"))
    p.append(box3(110, 340, "Tempo", "KALMAN · SUPPORT LOCK", "ANCHOR · PRIOR 150", w=162))
    p.append(vline(110, 143, 182)); p.append(vline(110, 276, 309))
    p.append(label(124, 170, "flux ACF", "start")); p.append(label(124, 300, "≥ .35 once, or ≥ .2 twice 2 s apart", "start"))
    p.append(line(f"M191 340 H480 V300 H{f(CX - 77)}"))
    p.append(label(330, 332, "period", "middle"))
    p.append(box(300, 120, "Activation", "TREES · EVERY HOP", w=150))
    p.append(line("M300 143 V320 H193"))
    p.append(label(292, 212, "chooses the level", "end")); p.append(label(292, 226, "once seeded", "end"))
    p.append(line("M375 110 H395 V24 H880 V418"))
    p.append(label(640, 18, "peaks ≥ .70 as events", "middle"))
    p.append(box(830, 441, "Window", "KICKS ∪ PEAKS", w=130))
    p.append(box(830, 540, "Kick model", "KICK EVENTS", w=130))
    p.append(vline(830, 517, 466))
    p.append(hline(441, 765, 708))
    p.append(box(CX, 60, "Beat", "beat · beat_phase · bpm · locked", "io", w=234, pill=True))
    p.append(vline(CX, 147, 84))
    p.append(box(780, 60, "Downbeat · structure", "ON THE SETTLED INSTANT", w=182))
    p.append(hline(60, 679, 687))
    return p, 900, 590, ("The beat path: the onset flux's autocorrelation earns the tempo through the seed gate, a "
                         "Kalman filter with a support lock and a metrical anchor tracks it, and the beat activation's "
                         "autocorrelation chooses the level once seeded. The grid runs as a loop per instant: emit 10 ms "
                         "early, settle, correct the phase on in-window onsets, fold the kick and activation events, "
                         "vote; three votes in a row shift the grid later by a half, quarter or three quarters.")


def frame():
    """One rendered frame on the main thread, with the per-layer loop."""
    p = []
    CX, CY, R = 430, 320, 150
    p.append(eyebrow(450, 32, "EVERY FRAME · THE MAIN THREAD · NO MUTEX IN THE HOT PATH", "middle"))
    p.append(box(140, 100, "Drain inputs", "MIDI → OSC → WEB", w=190))
    p.append(box(140, 180, "Bindings → params", "LATEST FRAME → PARAMS", w=190))
    p.append(vline(140, 123, 155))
    p.append(line(f"M235 180 H{CX - R - 50} V320 H{f(CX - R - 2)}"))
    p.append(ring_text(CX, CY, R + 40, 160, 80, "FOR EACH ENABLED LAYER", "layerpath"))
    ST = {"compute": 90, "effect": 210, "particles": 330}
    gap = 22
    p.append(ring_arc(CX, CY, R, ST["compute"] + gap, ST["effect"] - gap))
    p.append(ring_arc(CX, CY, R, ST["effect"] + gap, ST["particles"] - gap))
    p.append(ring_arc(CX, CY, R, ST["particles"] + gap, ST["compute"] - gap))
    p.append(beads(CX, CY, R, 8))
    p.append(box(*ring_pt(CX, CY, R, 90), "Compute", "PARTICLE SIM", w=120))
    p.append(box(*ring_pt(CX, CY, R, 210), "Effect passes", "PING-PONG HDR", w=120))
    p.append(box(*ring_pt(CX, CY, R, 330), "Particle render", "INSTANCED · ADDITIVE", w=142))
    p.append(box(CX, CY, "Layer", "EFFECT · MEDIA", w=104, h=40))
    rx = 740
    col = [(160, "Compositor", "13 BLENDS · SKIPPED IF ONE"), (240, "Post-process", "BLOOM · TONEMAP"),
           (320, "egui overlay", "PANELS · SHELL"), (400, "Present", "SURFACE · SRGB")]
    for y, t, s in col:
        p.append(box(rx, y, t, s, w=194))
    for (y0, _, _), (y1, _, _) in zip(col, col[1:]):
        p.append(vline(rx, y0 + 23, y1 - 25))
    p.append(line(f"M{f(CX + R + 2)} 320 H610 V160 H641"))
    p.append(box3(rx, 490, "Window + sinks", "NDI · SPOUT · SYPHON", "V4L2 · RECORDING", "io", w=194))
    p.append(vline(rx, 423, 459))
    p.append(label(400, 520, "u.time · u.beat · u.drop: rates per second, impulses per event", "middle", "note"))
    return p, 900, 540, ("One frame: drain MIDI, OSC and web inputs, apply bindings to the per-layer params from the "
                         "latest audio frame, then for each enabled layer run compute, effect passes and particle "
                         "render; composite, post-process, draw egui, present, and read back for the sinks.")


def signal():
    """From a hop to an OSC packet, in headless --signal."""
    p = []
    p.append(eyebrow(450, 32, "ONE CLOCK · THE AUDIO SAMPLE CLOCK · NO WALL TIME IN THE PIPELINE", "middle"))
    p.append(box3(110, 220, "Hop chain", "--signal · HEADLESS", "THE SAME 86 HZ CHAIN", w=160))
    p.append(box3(110, 360, "--signal-dump", "FILE IN · JSONL OUT", "WHAT THE BENCH SCORES", w=162, dashed=True))
    p.append(vline(110, 331, 251, dashed=True)); p.append(label(122, 300, "same emitter", "start"))
    lanes = [(90, "Events", "beat · downbeat · drop · onset · drums · boundary", "IMMEDIATE · RUNNING COUNTS"),
             (180, "Continuous", "bpm · locked · bar_phase · build · energy · stems", "phrase · predict/drop · DECIMATED TO 30 HZ"),
             (270, "On change", "section · key · phrase/len", "ON CHANGE + 1 HZ RE-BROADCAST"),
             (360, "Status", "online · uptime · device · hop_hz · tier", "1 HZ HEARTBEAT · ONLINE 0 ON EXIT")]
    for y, t, s, s2 in lanes:
        p.append(box3(450, y, t, s, s2, w=330))
    p.append(box(450, 430, "link/* · feat/<name>", "OPT-IN: --features link · --feat-bus (85)", w=330, h=36, dashed=True))
    p.append(line("M191 220 H250", arrow=False))
    for y, _, _, _ in lanes:
        p.append(line(f"M250 220 V{y} H283" if y != 220 else "M250 220 H283"))
    p.append(line("M250 360 V430 H283", dashed=True))
    for y, _, _, _ in lanes:
        p.append(line(f"M617 {y} H650 V220", arrow=False))
    p.append(line("M617 430 H640 V220", dashed=True, arrow=False))
    p.append(line("M650 220 H675"))
    p.append(box(780, 220, "Rig", "TOUCHDESIGNER · RESOLUME · MAX", "io", w=206, pill=True))
    p.append(label(662, 212, "UDP", "middle"))
    return p, 900, 470, ("Signal: in headless --signal the same hop chain feeds an emitter with four cadences, events "
                         "the hop they are detected with running counts, continuous values decimated to the TX rate, "
                         "on-change addresses re-broadcast at 1 Hz, and a 1 Hz status heartbeat, over UDP to any OSC "
                         "receiver; --signal-dump is the same emitter on a file, which is what the bench scores.")


def round_():
    """A pre-registered bench round, the dev loop that ships a constant."""
    p = []
    CX, CY, R = 400, 380, 130
    p.append(box(90, 80, "A measured gap", "ITS MECHANISM NAMED", "io", w=150, pill=True))
    p.append(box3(90, 170, "Freeze the manifest", "TARGETS · GUARDS", "SPLIT · RULE · ON MISS", w=150))
    p.append(box3(90, 270, "Replica forecast", "EXACT REPLAY", "EVERY GUARD", w=150))
    p.append(diamond(90, 380, 46, "Forecast", "≥ TARGET?"))
    p.append(box(90, 480, "Close the round", "RECORD THE FINDING", "retry", w=150, h=38))
    p.append(vline(90, 103, 139)); p.append(vline(90, 199, 239)); p.append(vline(90, 299, 332))
    p.append(line("M90 426 V459", cls="edge blue", dashed=True)); p.append(label(100, 448, "no", "start", "edge-label blue"))
    p.append(line(f"M136 380 H{f(CX - R - 2)}")); p.append(label(146, 372, "yes", "start"))
    p.append(ring_text(CX, CY, R + 44, 160, 80, "FOR EACH ARM", "armpath"))
    ST = {"build": 90, "dump": 210, "verdict": 330}
    gap = 26
    p.append(ring_arc(CX, CY, R, ST["build"] + gap, ST["dump"] - gap))
    p.append(ring_arc(CX, CY, R, ST["dump"] + gap, ST["verdict"] - gap))
    p.append(ring_arc(CX, CY, R, ST["verdict"] + gap, ST["build"] - gap))
    p.append(beads(CX, CY, R, 8))
    p.append(box3(*ring_pt(CX, CY, R, 90), "Build the arm", "analyze · OWN TARGET", "ENV KNOBS · 8 JOBS", w=160))
    p.append(box3(*ring_pt(CX, CY, R, 210), "Dump the tune halves", "+ BATIDA RENDERS", "+ 155 BEATLESS CLIPS", w=150))
    p.append(box3(*ring_pt(CX, CY, R, 330), "Tune verdict", "TARGETS · GUARDS", "LOST / GAINED LISTS", w=150))
    p.append(line(f"M{f(CX + R + 2)} 380 H600 V180 H663"))
    rx = 740
    p.append(box(rx, 180, "Select", "THE FROZEN RULE", w=150))
    p.append(box(rx, 270, "Holdout, once", "THE SINGLE READ", w=150))
    p.append(diamond(rx, 380, 46, "Holds?", "GUARDS · TARGETS"))
    p.append(box3(rx, 490, "Ship", "KNOBS → CONSTANTS", "SAME DUMPS · TESTS", w=150))
    p.append(box(rx, 610, "Commit", "BENCHMARKS · CHANGELOG", "io", w=170, pill=True))
    p.append(vline(rx, 203, 245)); p.append(vline(rx, 293, 332)); p.append(vline(rx, 426, 459)); p.append(vline(rx, 519, 585))
    p.append(label(rx + 12, 448, "yes", "start"))
    p.append(box(540, 560, "On miss", "THE RULE DECIDES · RECORDED", "retry", w=150, h=38))
    p.append(line(f"M{f(rx - 46)} 380 H660 V560 H637", cls="edge blue", dashed=True))
    p.append(label(668, 372, "no", "start", "edge-label blue"))
    return p, 900, 660, ("A bench round: a measured gap with its mechanism named, a frozen manifest (targets, guards, "
                         "split, selection rule), an exact replica forecast over every guard population, and only "
                         "then arms: build with the analyze feature in its own target dir, dump the tune halves plus "
                         "the batida renders and the beatless clips, verdict on tune; select by the frozen rule, read "
                         "the holdout once, ship constants with byte-identical dumps and tests, commit.")


def model():
    """Training the beat activation that ships inside the chain."""
    p = []
    p.append(eyebrow(450, 32, "TRAINED ON THE RAW EVIDENCE THE DETECTORS READ, NOT THE PUBLISHED BUS", "middle"))
    top = [(100, "Corpus", "CC BY · ANNOTATED", "io"), (280, "Hop dumps", "FOSFORA_HOP_SIDECAR"),
           (460, "Labels", "BEATS AT THE LAG"), (640, "Dataset", "build_dataset.py"), (820, "Train", "TREES · train.py")]
    for x, t, s, *k in top:
        kind = k[0] if k else "node"
        p.append(box(x, 120, t, s, kind, w=150, pill=(kind == "io")))
    for (a, *_), (b, *_) in zip(top, top[1:]):
        p.append(hline(120, a + 77, b - 77))
    bot = [(820, "Export", "beat_model.json"), (640, "Golden test", "WITHIN AN ULP · CI"),
           (460, "Runtime", "beat_model.rs"), (280, "Activation", "PEAKS · ACF LEVEL", "io")]
    for x, t, s, *k in bot:
        kind = k[0] if k else "node"
        p.append(box(x, 230, t, s, kind, w=150, pill=(kind == "io")))
    for (a, *_), (b, *_) in zip(bot, bot[1:]):
        p.append(hline(230, a - 77, b + 77))
    p.append(vline(820, 143, 205))
    p.append(box(100, 230, "Negatives", "155 BEATLESS CLIPS", "io", w=150, pill=True, dashed=True))
    p.append(line("M100 207 V160 H190 V130", dashed=True))
    p.append(label(450, 300, "a better activation needs its readers re-tuned with it: the arbiter threshold, the level choice", "middle", "note"))
    p.append(label(450, 320, "the kick model follows the same shape: scripts/kick_model → kick_model.json → kick_model.rs", "middle", "note"))
    return p, 900, 340, ("Training the beat activation: a CC BY corpus with annotated beats is dumped per hop through "
                         "the hop sidecar, labeled at the target lag, built into a dataset and trained as a tree "
                         "ensemble; the export is golden-tested in CI and evaluated by the runtime every hop, whose "
                         "peaks feed the arbiter and whose autocorrelation chooses the level; beatless clips guard it.")


def offline():
    """The same chain driven by a file: what ships and what is dev."""
    p = []
    p.append(eyebrow(450, 32, "THE SAME CODE AS LIVE, DRIVEN BY A FILE, FASTER THAN REALTIME", "middle"))
    p.append(box(90, 200, "File", "WAV · MP3 · FLAC", "io", w=120, pill=True))
    p.append(box(250, 200, "Decode", "SYMPHONIA", w=140))
    p.append(box3(440, 200, "The hop chain", "BYTE-DETERMINISTIC", "SIDECARS IF ASKED", w=160))
    p.append(hline(200, 150, 178)); p.append(hline(200, 320, 358))
    outs = [(90, "--analyze", "TEMPO · KEY · SECTIONS", True), (165, "--signal-dump", "WHAT --signal WOULD SEND", True),
            (240, "--render-scene", "FRAMES · NEEDS A GPU", True), (315, "--render-loop", "BEAT-LOCKED VIDEO · FFMPEG", False)]
    for y, t, s, dev in outs:
        p.append(box(660, y, t, s, w=194, dashed=dev))
        p.append(line(f"M520 200 H540 V{y} H561" if y != 200 else "M520 200 H561"))
    p.append(box(830, 90, "Report", "JSON", "io", w=120, h=40, pill=True))
    p.append(box(830, 165, "Bench", "TD COMPARE TOO", "io", w=120, h=40, pill=True))
    p.append(box(830, 240, "Scene check", "", "io", w=120, h=36, pill=True))
    p.append(box(830, 315, "Loop video", "", "io", w=120, h=36, pill=True))
    p.append(hline(90, 757, 768)); p.append(hline(165, 757, 768)); p.append(hline(240, 757, 768)); p.append(hline(315, 757, 768))
    p.append(label(450, 380, "dashed: behind --features analyze · solid: in every download", "middle", "note"))
    return p, 900, 400, ("Offline modes: a file is decoded and driven through the same hop chain faster than realtime; "
                         "--analyze reports tempo, key and sections, --signal-dump writes what --signal would send for "
                         "the bench and the TouchDesigner comparison, --render-scene renders frames without a window, "
                         "all behind the analyze feature; --render-loop ships in every download.")


DIAGRAMS = {
    "tiers": tiers, "runtime": runtime, "dataflow": dataflow, "builds": builds,
    "hop": hop, "beat": beat, "frame": frame, "signal": signal, "round": round_, "model": model, "offline": offline,
}

if __name__ == "__main__":
    names = sys.argv[1:] or list(DIAGRAMS)
    OUT.mkdir(parents=True, exist_ok=True)
    for name in names:
        parts, w, h, aria = DIAGRAMS[name]()
        for theme, t in THEMES.items():
            path = OUT / f"{name}-{theme}.svg"
            path.write_text(document(parts, t, w, h, aria))
            print(path)
