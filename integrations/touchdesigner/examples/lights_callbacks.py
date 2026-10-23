# Script CHOP callbacks for the Lights view: eight fixtures in DMX order (dimmer, pan for each)
# and a haze channel, read by the beams shader and, scaled to 0..255, by dmx_out.
# op('fosfora') is the bundle's receiver with Steady Bars on.
import math
S = {'look': '', 'beats': 0, 'bph': None, 'pbar': None, 'phrases': 0}
LOOKS = ('intro', 'build', 'drop', 'break', 'steady')


def onSetupParameters(scriptOp):
    return


def onPulse(par):
    return


def onGetCookLevel(scriptOp):
    # Cook every frame while something reads it, not at all otherwise.
    return getattr(CookLevel, 'WHEN_USED', CookLevel.ALWAYS)


def onCook(scriptOp):
    f, L = op('fosfora'), op('looks')
    bp = f['beat_phase'].eval()
    if S['bph'] is not None and bp < S['bph'] - 0.5:                 # count beats as beat_phase wraps
        S['beats'] += 1
    S['bph'] = bp
    pos = S['beats'] + bp
    beat = int(f['beat_in_bar'].eval())
    plen, pbar = f['phrase_len'].eval(), f['phrase_bar'].eval()
    if S['pbar'] is not None and pbar < S['pbar']:
        S['phrases'] += 1
    S['pbar'] = pbar
    direction = 1 if S['phrases'] % 2 == 0 else -1                    # a new direction every phrase
    fill = plen > 0 and pbar >= plen                                   # the last bar of a phrase
    w = {k: L['in_' + k].eval() for k in LOOKS}
    w['steady'] += max(0.0, 1.0 - sum(w.values()))                     # no section yet: steady
    total = sum(w.values()) or 1.0
    w = {k: v / total for k, v in w.items()}
    predict, energy = L['predict_drop'].eval(), L['energy'].eval()
    kick = op('env')['kick'].eval()
    locked = f['locked'].eval() >= 0.5
    rate = 1.0 + w['build'] * min(1.0, 1.5 * L['build'].eval())         # the chase speeds up through a build
    strobe = 1.0 if (pos * 4.0) % 1.0 < 0.5 else 0.0                    # sixteenth-note flicker
    vals = []
    for i in range(8):
        k = i if direction > 0 else 7 - i
        fan = (i - 3.5) / 3.5
        chase = max(0.0, 1.0 - ((pos * rate - k) % 8.0) / 2.2)
        wash = 0.5 + 0.5 * math.sin(2 * math.pi * (pos / 4.0 + i / 8.0))
        hit = 1.0 - bp if beat == 1 else 0.0
        dim = (w['steady'] * chase
               + w['build'] * max(chase, predict * strobe * (1.0 if predict >= 0.8 else 0.5))
               + w['drop'] * (0.65 + 0.35 * hit)
               + w['intro'] * 0.45 * wash
               + w['break'] * 0.35 * wash)
        if fill:
            dim = max(0.15 * dim, 1.0 if (int(pos * 2) + i) % 2 == 0 else 0.08)
        dim = min(1.0, 0.07 + dim + 0.25 * kick) * (0.45 + 0.55 * min(1.0, 1.4 * energy))   # every light faintly on
        pan = (w['steady'] * 0.55 * fan
               + w['build'] * 0.55 * fan * (1.0 - predict)
               + w['drop'] * (0.55 * fan + 0.45 * math.sin(math.pi * pos) * (1 if i % 2 else -1))
               + (w['intro'] + w['break']) * 0.8 * math.sin(2 * math.pi * (pos / 8.0 + i / 16.0)))
        if not locked:
            dim, pan = (0.15 + 0.4 * energy) * wash, 0.4 * fan
        vals += [('f{}_dim'.format(i + 1), dim), ('f{}_pan'.format(i + 1), 0.5 + 0.5 * max(-1.0, min(1.0, pan)))]
    vals.append(('haze', min(1.0, L['bass'].eval() * 1.2)))
    scriptOp.clear()
    scriptOp.numSamples = 1
    for name, v in vals:
        scriptOp.appendChan(name)[0] = v
    top = max(w, key=w.get)
    S['look'] = ('wash only: no tempo lock yet' if not locked else
                 'fill: last bar of the phrase' if fill else
                 {'steady': 'chase, one step per beat', 'build': 'build: chase speeds up, beams pull in, flicker rises',
                  'drop': 'drop-like (experimental): all on, beams swing', 'intro': 'intro: slow wash',
                  'break': 'break: slow sweep'}[top])
