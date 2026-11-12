#!/usr/bin/env python3
"""The component's extension, exercised outside TouchDesigner with stand-ins for
the few TD objects it touches (absTime, its parameters, the states table, the
Script CHOP). Run: python3 integrations/touchdesigner/test_fosfora_signal_ext.py
"""
import os
SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'src', 'fosfora_signal_ext.py')

class Clock: frame = 0; seconds = 0.0
clock = Clock()
class Par:
    def __init__(self, v): self.v = v
    def eval(self): return self.v
    @property
    def val(self): return self.v
    @val.setter
    def val(self, v): self.v = v
class Pars:
    def __init__(self, **kw): self.__dict__.update({k: Par(v) for k, v in kw.items()})
class Table:
    def __init__(self): self.rows = []
    def clear(self): self.rows = []
    def appendRow(self, r): self.rows.append(r)
class OscIn:
    err = ''
    numRows = 1
    def errors(self): return self.err
class Comp:
    def __init__(self):
        self.par = Pars(Active=True, Port=9010, Status='', Pulseframes=1, Smoothphase=True, Steadybars=False, Holdtempo=True, Boundarygate=0.5, Sectiongate=0.0, Keygate=0.0, Timeout=3.0)
        self.t = Table(); self.oscin = OscIn(); self.script_errors = []
    def op(self, n): return {'states': self.t, 'oscin': self.oscin}.get(n)
    def clearScriptErrors(self): self.script_errors = []
    def addScriptError(self, m): self.script_errors.append(m)
class Chan:
    def __init__(self): self.v = None
    def __setitem__(self, i, v): self.v = v
class Script:
    def clear(self): self.ch = {}
    def appendChan(self, n): c = Chan(); self.ch[n] = c; return c

ns = {'absTime': clock, 'ui': None}
exec(compile(open(SRC).read(), SRC, 'exec'), ns)
comp = Comp(); ext = ns['FosforaSignalExt'](comp); s = Script()
def frame(msgs=()):
    clock.frame += 1; clock.seconds = clock.frame / 60
    for a, args in msgs: ext.Receive('/fosfora/v1/' + a, args)
    ext.Cook(s); return {k: c.v for k, c in s.ch.items()}

out = frame()
print('channels', len(out), list(out)[:8], '...')
# pulses: beat count 10 -> one-frame pulse; duplicate count -> no pulse; gap -> lost
r = frame([('beat', [10])]); assert r['beat'] == 1 and r['beat_count'] == 10, r
r = frame(); assert r['beat'] == 0
r = frame([('beat', [10])]); assert r['beat'] == 0, 'duplicate count must not pulse'
r = frame([('beat', [13])]); assert r['beat'] == 1 and r['lost'] == 2, r
r = frame([('beat', [0])]); assert r['beat'] == 1 and r['lost'] == 2, 'restart is not loss'
# two cooks in one frame see the same pulse
ext.Receive('/fosfora/v1/downbeat', [5]); clock.frame += 1; clock.seconds = clock.frame/60
ext.Cook(s); a = s.ch['downbeat'].v; ext.Cook(s); b = s.ch['downbeat'].v
assert a == b == 1, (a, b)
# onset strength repeated still pulses
r = frame([('onset', [0.5])]); assert r['onset'] == 1
r = frame([('onset', [0.5])]); assert r['onset'] == 1, 'equal strength must still pulse'
# boundary gate
r = frame([('section/boundary', [0.3, 6.0])]); assert r['boundary'] == 0 and r['boundary_conf'] == 0.3
r = frame([('section/boundary', [0.7, 6.0])]); assert r['boundary'] == 1 and r['boundary_age'] == 6.0
# section + key
r = frame([('section', ['drop', 0.8]), ('key', ['F#m', 0.6])])
assert r['section'] == 2 and r['in_drop'] == 1 and r['key'] == 18 and r['key_conf'] == 0.6, r
print('states', comp.t.rows)
# predict tiers
r = frame([('predict/drop', [0.85])]); assert r['drop_warn'] == 1 and r['drop_imminent'] == 1
# online
r = frame([('status/online', [1])]); assert r['online'] == 1
for _ in range(200): r = frame()
assert r['online'] == 0, 'heartbeat timeout'
# tempo hold
r = frame([('bpm', [128.0]), ('bpm/locked', [1])]); assert r['bpm'] == 128
r = frame([('bpm', [0.0]), ('bpm/locked', [0])]); assert r['bpm'] == 128 and r['locked'] == 0
# phase: 30 Hz packets at 60 fps, 120 bpm locked, packets slightly noisy; never small backward steps
import random; random.seed(1)
frame([('bpm', [120.0]), ('bpm/locked', [1])])
true = 0.0; prev = None; back = 0; wraps = 0; maxjump = 0
for i in range(600):
    true = (true + 120/240/60) % 1
    msgs = [('bar_phase', [(true + random.uniform(-0.004, 0.004)) % 1])] if i % 2 == 0 else []
    r = frame(msgs); p = r['bar_phase']
    if prev is not None:
        d = (p - prev + 0.5) % 1 - 0.5
        if d < 0: back += 1
        if p < prev - 0.5: wraps += 1
        maxjump = max(maxjump, abs(d))
    prev = p
print('phase: backward steps', back, 'wraps', wraps, 'max step', round(maxjump, 4), 'ideal step', round(120/240/60, 4))
assert back == 0
# stale events: arrive while nothing cooks the CHOP, then first cook -> no pulse
ext.Receive('/fosfora/v1/drop', [99]); clock.frame += 50; clock.seconds = clock.frame/60
ext.Cook(s); assert s.ch['drop'].v == 0 and s.ch['drop_count'].v == 99, 'stale drop must not pulse'
# event arriving one frame before the cook (DAT cooked after CHOP) still pulses
ext.Receive('/fosfora/v1/drop', [100]); clock.frame += 1; clock.seconds = clock.frame/60
ext.Cook(s); assert s.ch['drop'].v == 1, 'one-frame-late event must pulse'
# a dropped-frame stall: the DAT delivers after this frame's cook, then absTime.frame
# jumps by 10 (measured in TD 2025.33230: a 150 ms stall at 60 fps); the pulse survives
r = frame()
ext.Receive('/fosfora/v1/beat', [500]); clock.frame += 10; clock.seconds = clock.frame/60
ext.Cook(s); assert s.ch['beat'].v == 1, 'a pulse must survive a dropped-frame stall'
# key confidence change rewrites the table
ext.Receive('/fosfora/v1/key', ['F#m', 0.9]); assert ['key', 'F#m', 0.9] in comp.t.rows, comp.t.rows
# every pulse has a count: the wire's totals for beat/downbeat/drop, local ones for the rest
ext.Reset()
r = frame([('onset', [0.4]), ('stem/drums/onset', [0.9]), ('section/boundary', [0.8, 6.0]), ('section/boundary', [0.2, 6.0])])
assert (r['onset_count'], r['kick_count'], r['boundary_count']) == (1, 1, 1), 'a gated boundary is not counted'
r = frame([('onset', [0.4])]); assert r['onset_count'] == 2 and r['onset'] == 1
# Status: waiting -> receiving -> online -> no heartbeat -> stopped; port error on the COMP
def poll(msgs=(), dt=1.0):
    clock.frame += int(dt * 60); clock.seconds = clock.frame / 60
    for a, args in msgs: ext.Receive('/fosfora/v1/' + a, args)
    ext.Poll(); return comp.par.Status.v
ext.Reset()
assert poll() == 'Waiting for fosfora --signal on port 9010'
assert poll([('energy', [0.5])]) == 'Receiving, waiting for the first heartbeat'
assert poll([('status/online', [1]), ('status/device', ['monitor'])]) == 'Online: monitor'
assert poll(dt=5.0) == 'No heartbeat for 5 s'
assert poll([('status/online', [0])]) == 'Fosfora stopped'
comp.oscin.err = 'Unable to bind to port'
assert poll() == 'Port 9010 in use' and len(comp.script_errors) == 1, comp.script_errors
comp.oscin.err = ''
assert poll() == 'Fosfora stopped' and comp.script_errors == [], 'a cleared port error leaves the COMP'
comp.par.Active.v = False
assert poll() == 'Off'
comp.par.Active.v = True

# Steady Bars: at 120 BPM, Fosfora's downbeat moves two beats mid-bar at 5.5 s and stays;
# from 12.2 s it flips one beat for 1.5 s (three beats, across a bar line) and back. Shown
# raw, bars get cut; steady, every bar finishes, the lasting move is followed once (one long
# bar), and the flip, shorter than the hold, never.
def steady_run(on):
    ext.Reset(); comp.par.Steadybars.v = on
    bars, beats, last_bp, last_bib, starts, pulses = [], 0, None, None, 0, 0
    f0 = clock.frame
    for i in range(int(24 * 60)):
        t = (clock.frame + 1 - f0) / 60
        shift = 2 if t >= 5.5 else 0
        if 12.2 <= t < 13.7:
            shift += 1
        msgs = [('bar_phase', [((2 * t + shift) / 4) % 1])] if i % 2 == 0 else []
        r = frame(msgs + ([('bpm', [120.0]), ('bpm/locked', [1])] if i == 0 else []))
        bp, bib = r['beat_phase'], r['beat_in_bar']
        if last_bp is not None and bp < last_bp - 0.5:
            beats += 1
        last_bp = bp
        if last_bib is not None and bib < last_bib:     # a bar started
            starts += 1
            if starts > 1:
                bars.append(beats)
            beats = 0
        last_bib = bib
        pulses += r['downbeat'] == 1
    return bars, starts, pulses, r
raw_bars, _, _, _ = steady_run(False)
sb_bars, sb_starts, sb_pulses, r = steady_run(True)
print('bar lengths in beats, raw:', raw_bars, ' steady:', sb_bars)
assert min(raw_bars) < 4, 'the scenario must cut a raw bar'
assert min(sb_bars) >= 4, 'Steady Bars must never cut a bar short'
assert sum(1 for b in sb_bars if b > 4) == 1, 'one lasting move: one long bar; the brief flip is not followed'
assert sb_pulses == sb_starts, 'a downbeat pulse on every steady bar start ({} vs {})'.format(sb_pulses, sb_starts)
assert r['bar_count'] == sb_pulses, 'bar_count counts the steady downbeats'
comp.par.Steadybars.v = False

# Every address SIGNAL.md documents is handled by name, or deliberately passed through.
import re
HERE = os.path.dirname(os.path.abspath(__file__))
documented = set(re.findall(r'^\| `/fosfora/v1/([a-z_/]+)`', open(os.path.join(HERE, '..', '..', 'docs', 'SIGNAL.md')).read(), re.M))
assert len(documented) > 25, documented
src = open(SRC).read()
handled = {a for a in documented if "'{}'".format(a) in src}
passthrough = {a for a in documented if a.startswith('link/')}
missing = documented - handled - passthrough
assert not missing, 'SIGNAL.md documents addresses the component does not name: {}'.format(sorted(missing))

# The checked-in .tox files were built from exactly these sources, the examples from
# exactly this component (see the two builders and tox_build.py).
import hashlib, json
tb = {}
exec(compile(open(os.path.join(HERE, 'tox_build.py')).read(), 'tox_build.py', 'exec'), tb)
def checked(stem, patterns):
    manifest = json.load(open(os.path.join(HERE, stem + '.json')))
    assert manifest['sources_sha256'] == tb['digest'](HERE, patterns), \
        '{} is older than its sources: rebuild it in TouchDesigner (docs/TOUCHDESIGNER.md, Rebuilding)'.format(stem + '.tox')
    tox = open(os.path.join(HERE, stem + '.tox'), 'rb').read()
    assert manifest['tox_sha256'] == hashlib.sha256(tox).hexdigest(), '{}.tox is not the file {}.json describes'.format(stem, stem)
    return manifest
signal = checked('fosfora_signal', tb['SIGNAL_SOURCES'])
assert "VERSION = '{}'".format(signal['version']) in src, 'the .tox version differs from the extension VERSION'
examples = checked('fosfora_examples', tb['EXAMPLE_SOURCES'])
assert examples['signal_tox_sha256'] == signal['tox_sha256'], \
    'fosfora_examples.tox embeds an older fosfora_signal.tox: rebuild the examples after the component'
assert "VERSION = '{}'".format(examples['version']) in open(os.path.join(HERE, 'build_examples.py')).read()
print('ALL OK')
