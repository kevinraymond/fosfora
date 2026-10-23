# Script CHOP callbacks for the Grid view: one sample per box (32 columns x 20 rows), read by
# geo's instancing. op('fosfora') is the bundle's receiver with Steady Bars on, so bars always
# finish and the march never jumps. Every event is caught from its running count, so none is
# missed between frames.
import math
import numpy as np

NX, NZ, SP = 32, 20, 0.25
_X, _Z = np.meshgrid(np.arange(NX), np.arange(NZ))
XI, ZI = _X.ravel(), _Z.ravel()
XP = (XI - (NX - 1) / 2) * SP
ZP = (ZI - (NZ - 1) / 2) * SP
D = np.hypot(XP, ZP)
SEED = XI * 12.9898 + ZI * 78.233
BARLINE = np.where(XI % 8 == 0, 0.06, 0.0)          # 8 eighth-note columns per bar
# Fosfora's two accents: blue boxes at rest, orange where a beat or an event lands.
BLUE = np.array([0.314, 0.565, 0.878])[:, None]
ORANGE = np.array([1.0, 0.376, 0.251])[:, None]
S = {'kicks': None, 'onsets': None, 'bounds': None, 'pbar': None, 'ripples': [], 'sparks': [], 'sweeps': []}


def onSetupParameters(scriptOp):
    return


def onPulse(par):
    return


def onGetCookLevel(scriptOp):
    # Cook every frame while something reads it, not at all otherwise.
    return getattr(CookLevel, 'WHEN_USED', CookLevel.ALWAYS)


def _new(key, count):
    prev, S[key] = S[key], count
    return prev is not None and count != prev


def onCook(scriptOp):
    f, L = op('fosfora'), op('looks')
    now = absTime.seconds
    if _new('kicks', f['kick_count'].eval()):            # every kick adds its own ripple
        S['ripples'].append((now, 0.5 + 0.7 * min(1.0, f['kick_strength'].eval())))
    if _new('onsets', f['onset_count'].eval()):          # onsets spark a box, a different one each time
        n = int(f['onset_count'].eval())
        S['sparks'].append((now, (n * 7919) % (NX * NZ), min(1.0, f['onset_strength'].eval())))
    if _new('bounds', f['boundary_count'].eval()):       # a confirmed section change sweeps across
        S['sweeps'].append((now, 'x'))
    plen, pbar = f['phrase_len'].eval(), f['phrase_bar'].eval()
    if S['pbar'] is not None and pbar < S['pbar']:       # a new phrase sweeps front to back
        S['sweeps'].append((now, 'z'))
    S['pbar'] = pbar
    S['ripples'] = [r for r in S['ripples'] if now - r[0] < 2.5][-12:]
    S['sparks'] = [s for s in S['sparks'] if now - s[0] < 0.4][-60:]
    S['sweeps'] = [s for s in S['sweeps'] if now - s[0] < 1.6]

    march = (1.0 if f['locked'].eval() >= 0.5 else 0.0) * (1.0 - L['in_break'].eval())   # no march without a lock, or in a break
    beat, bp = max(1, int(f['beat_in_bar'].eval())), f['beat_phase'].eval()
    bar = (int(pbar) - 1) % 4 if plen > 0 else int(f['bar_count'].eval()) % 4             # 4-bar groups, on the phrase
    pos8 = bar * 8 + (beat - 1) * 2 + 2 * bp                                              # eighth notes into the group
    step, frac = int(pos8) % NX, pos8 % 1.0
    fill = plen > 0 and pbar >= plen                                                      # last bar of a phrase: two columns march
    progress = (pbar - 1 + f['bar_phase'].eval()) / plen if plen > 0 else 0.0
    bass, drums, melody = L['bass'].eval(), L['drums'].eval(), L['melody'].eval()
    tension = 0.12 * L['build'].eval() * L['in_build'].eval() + 0.35 * L['predict_drop'].eval()
    droplike = L['in_drop'].eval()
    flash = droplike * (1.0 - bp) if beat == 1 else 0.0

    age = (step - XI) % NX + frac
    if fill:
        age = np.minimum(age, (step + NX // 2 - XI) % NX + frac)
    glow = march * np.clip(1.0 - age / 4.0, 0.0, 1.0)
    ripple = np.zeros_like(D)
    for t0, s in S['ripples']:
        dt = now - t0
        ripple += s * math.exp(-1.6 * dt) * np.exp(-((D - 4.5 * dt) ** 2) / 0.2)
    sweep = np.zeros_like(D)
    for t0, axis in S['sweeps']:
        dt = now - t0
        p = XP if axis == 'x' else ZP
        sweep += math.exp(-1.2 * dt) * np.exp(-((p - (-4.5 + 7.5 * dt)) ** 2) / 0.15)
    sp = np.zeros_like(D)
    for t0, i, s in S['sparks']:
        sp[i] = max(sp[i], s * math.exp(-(now - t0) * 9.0))
    row = np.where((ZI + 0.5) / NZ <= progress, 0.14, 0.0)
    wave = melody * 0.3 * (0.5 + 0.5 * np.sin(ZP * 3.2 - now * 1.8 + XP * 0.8))
    jitter = tension * (0.5 + 0.5 * np.sin(SEED + now * 31.0))
    height = (0.06 + 0.3 * bass + glow * (0.8 + 0.6 * drums) * (1.0 + 0.5 * droplike)
              + 0.8 * ripple + 0.7 * sweep + 0.5 * sp + wave + jitter + 0.4 * flash)
    rest = np.clip(0.12 + row + 0.3 * wave + 0.3 * sweep + BARLINE, 0.0, 1.0)          # structure, blue
    hit = np.clip(0.8 * glow + 0.7 * ripple + 0.9 * sp + 0.5 * flash + 0.3 * sweep, 0.0, 1.0)  # beats and events, orange
    color = np.clip(BLUE * rest * (1.0 - hit) + ORANGE * hit, 0.0, 1.0)
    scriptOp.clear()
    scriptOp.numSamples = NX * NZ
    for name, values in (('tx', XP), ('tz', ZP), ('sy', height), ('cr', color[0]), ('cg', color[1]), ('cb', color[2])):
        scriptOp.appendChan(name).vals = values.tolist()
