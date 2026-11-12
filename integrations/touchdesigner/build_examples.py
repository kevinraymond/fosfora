"""Build the Fosfora Signal examples inside TouchDesigner and save them as one .tox.

Run from the TouchDesigner textport after build_fosfora_signal.py, with __file__ set
to this script's path:

    __file__ = '/path/to/integrations/touchdesigner/build_examples.py'; exec(open(__file__).read())

It creates /project1/fosfora_examples: two copies of fosfora_signal.tox, one as Fosfora
sends it and one with Steady Bars on, and four views that read them (Dashboard, Grid,
Lights, Keeping time), each in its own COMP with a README and a comment on every
operator. The View parameter shows all four tiled, or one at full size. The views'
callbacks and shaders live in examples/. It writes fosfora_examples.tox next to this file,
with fosfora_examples.json recording what it was built from and which fosfora_signal.tox
it embeds.
"""

import hashlib
import json
import os

try:
    _HERE = os.path.dirname(os.path.abspath(__file__))
except NameError:
    raise RuntimeError("set __file__ to this script's path before exec()")
exec(open(os.path.join(_HERE, 'tox_build.py')).read(), globals())
VERSION = '1.0.0'
DOCS_URL = 'https://github.com/kevinraymond/fosfora/blob/main/docs/TOUCHDESIGNER.md'
TOX = os.path.join(_HERE, 'fosfora_signal.tox')
W, H = 1280, 720
RING = 230  # the Dashboard's bar ring radius, pixels
VIEWS = (('all', 'All'), ('dashboard', 'Dashboard'), ('grid', 'Grid'), ('lights', 'Lights'),
         ('keeping_time', 'Keeping time'))

# The two accents of Fosfora's app and documentation: orange (#ff6040) for beats and
# events, blue (#5090e0, deeper #3a70c0) for structure. They stay apart for color
# blindness, and no signal rests on hue alone: each also differs in brightness or motion.
ORANGE = (1.0, 0.376, 0.251)
BLUE = (0.314, 0.565, 0.878)
DEEP_BLUE = (0.227, 0.439, 0.753)
NODE_COLOR = DEEP_BLUE

# Section tints for the Dashboard, (r, g, b): blue when calm, warming through a build to
# orange in a drop-like section.
TINTS = (
    ('in_intro', BLUE),
    ('in_build', (0.726, 0.452, 0.502)),
    ('in_drop', ORANGE),
    ('in_break', DEEP_BLUE),
    ('in_outro', BLUE),
    ('in_steady', (0.554, 0.717, 0.921)),
)

README = """Fosfora Signal examples {version}

Four views of fosfora --signal, all driven live. The View parameter (Examples page) shows
them tiled or one at full size. Each view is a COMP with its own README; open one to see
how it is wired.

fosfora_signal   the component as Fosfora sends it -> raw
fosfora_steady   the same component with Steady Bars on (every bar finishes) -> steady
dashboard        a bar ring, beat markers, a tunnel, and the section, key, tempo and phrase
grid             640 instanced boxes: an eighth-note march, a ripple per kick, the phrase
lights           eight moving heads and a haze machine run by the music's structure,
                 with an Art-Net DMX output (off)
keeping_time     bars as they arrive, Steady Bars, and TouchDesigner's own Beat CHOP

Open Window (Examples page) shows the picture in its own window, 1280 x 720 or filling a
display (Fill Display, Display); Esc closes it.

Every view reads the receivers through a Select CHOP named fosfora (and steady). To use a
view in your own project, copy it out with fosfora_signal and point those Select CHOPs at
your copy's output. Set Port here if fosfora --signal sends to another port.
Guide: {docs}
"""


def _src(name):
    with open(os.path.join(_HERE, 'examples', name)) as f:
        return f.read()


class Expr:
    def __init__(self, text):
        self.text = text


def _setpars(o, values):
    names = {p.name: p for p in o.pars()}
    for name, value in values.items():
        if name not in names:
            raise RuntimeError('{} has no parameter {!r}; it has: {}'.format(o.OPType, name, sorted(names)))
        p = names[name]
        if isinstance(value, Expr):
            p.expr = value.text
        else:
            p.val = value


def mk(host, kind, name, x, y, inputs=(), comment='', **pars):
    o = host.create(kind, name)
    o.nodeX, o.nodeY = x, y
    _setpars(o, pars)
    for i, src in enumerate(inputs):
        o.inputConnectors[i].connect(src)
    if comment:
        o.comment = comment
    return o


def res(o):
    _setpars(o, dict(outputresolution='custom', resolutionw=W, resolutionh=H))
    return o


def label(view, name, x, y, text, size=22, alignx='left', aligny='top', posx=40, posy=-30, alpha=0.85):
    return res(mk(view, textTOP, name, x, y, text=text, fontautosize='off', fontsizex=size, alignx=alignx,
                  aligny=aligny, positionx=posx, positiony=posy, fontcolorr=1, fontcolorg=1, fontcolorb=1,
                  fontalpha=alpha, bgalpha=0))


def docked(o, suffix, text):
    """Fill the DAT TouchDesigner docked to o (callbacks, a shader) instead of adding one."""
    (dat,) = [d for d in o.docked if d.isDAT and d.name.endswith(suffix)]
    dat.text = text
    return dat


def envelopes(view, names, fall, name='env', y=-200):
    """Pulses to envelopes: jump to 1, fall back over `fall` seconds."""
    sel = mk(view, selectCHOP, 'sel_' + name, -600, y, chops='fosfora', channames=names)
    lag = mk(view, lagCHOP, 'lag_' + name, -400, y, [sel], '{}: jump to 1, fall in {} s'.format(names, fall),
             lag1=0, lag2=fall)
    return mk(view, nullCHOP, name, -200, y, [lag])


def looks(view):
    """Levels and section weights, smoothed so section styles cross-fade."""
    sel = mk(view, selectCHOP, 'sel_looks', -600, -400, chops='fosfora',
             channames='energy build drums bass melody predict_drop in_intro in_build in_drop in_break in_steady')
    lag = mk(view, lagCHOP, 'lag_looks', -400, -400, [sel], 'levels and section weights, cross-fading over ~1 s',
             lag1=0.3, lag2=0.8)
    return mk(view, nullCHOP, 'looks', -200, -400, [lag])


def new_view(bundle, name, x, y, readme, comment, reads=(('fosfora', '../steady'),)):
    view = bundle.create(baseCOMP, name)
    view.nodeX, view.nodeY = x, y
    view.nodeWidth, view.nodeHeight = 360, 220      # big enough to read the view on its node
    view.comment = comment
    mk(view, textDAT, 'README', -800, 300).text = readme
    for i, (local, source) in enumerate(reads):
        mk(view, selectCHOP, local, -800, 100 - 150 * i, chops=source,
           comment="the bundle's {} receiver; point this at your own copy".format(source[3:]))
    return view


def finish(view, last):
    out = mk(view, outTOP, 'out', 1600, 0, [last], "this view's picture, the COMP's output")
    view.par.opviewer = './out'
    return out


# ---- Dashboard ------------------------------------------------------------------------------
def dashboard(bundle):
    ex = new_view(bundle, 'dashboard', -800, 300, '''Dashboard

Every channel at a glance. CHOPs (left) turn pulses into envelopes that jump to 1 and fall
back, smooth the continuous values so section styles cross-fade, and merge everything into
the Null CHOP ctl that the TOPs read. TOPs (right): the ring fills once per bar (bar_phase),
four markers show the beat in the bar, a feedback tunnel zooms with energy and build, the
bottom bar shows the position in the phrase, and the HUD shows section, key and BPM and, in
a corner, a likely drop-like moment (experimental).
Section styles   break: slow, long trails   build: zoom climbs with predict_drop
                 drop-like: downbeats strobe, kicks flash
''', 'Dashboard: every channel at a glance')
    mk(ex, textDAT, 'fmt', -800, -300, comment='HUD text helpers').text = _src('dashboard_hud.py')

    def c(name):
        return "op('ctl')['{}']".format(name)

    # ---- CHOPs: pulses -> envelopes, smoothed states, one control CHOP -------------
    src = 'fosfora'
    sel_fast = mk(ex, selectCHOP, 'sel_fast', -500, 300, chops=src, channames='beat kick onset')
    env_fast = mk(ex, lagCHOP, 'env_fast', -300, 300, [sel_fast], 'beat/kick/onset envelopes, 0.12 s fall',
                  lag1=0, lag2=0.12)
    sel_bar = mk(ex, selectCHOP, 'sel_bar', -500, 150, chops=src, channames='downbeat')
    env_bar = mk(ex, lagCHOP, 'env_bar', -300, 150, [sel_bar], 'downbeat envelope, 0.35 s fall', lag1=0, lag2=0.35)
    sel_big = mk(ex, selectCHOP, 'sel_big', -500, 0, chops=src, channames='drop boundary')
    env_big = mk(ex, lagCHOP, 'env_big', -300, 0, [sel_big], 'drop/boundary envelopes, 1.5 s fall', lag1=0, lag2=1.5)
    sel_cont = mk(ex, selectCHOP, 'sel_cont', -500, -150, chops=src,
                  channames='energy build predict_drop drums bass melody in_intro in_build in_drop in_break in_outro in_steady')
    smooth = mk(ex, lagCHOP, 'smooth', -300, -150, [sel_cont], 'smoothed levels; section styles cross-fade over ~1 s',
                lag1=0.4, lag2=0.8)
    sel_raw = mk(ex, selectCHOP, 'sel_raw', -500, -300, chops=src,
                 channames='bar_phase beat_in_bar phrase_bar phrase_len phrase_beats_left bpm locked section key drop_warn drop_imminent online')
    merge = mk(ex, mergeCHOP, 'merge', -100, 0, [env_fast, env_bar, env_big, smooth, sel_raw])
    mk(ex, nullCHOP, 'ctl', 100, 0, [merge], "every value the picture reads: op('ctl')[name]")

    # ---- TOPs: the picture ---------------------------------------------------------------
    x0 = 400
    px = dict(radiusunit='pixels', centerunit='pixels', borderwidthunit='pixels')
    track = res(mk(ex, circleTOP, 'ring_track', x0, 300, comment='the bar, faint',
                   radiusx=RING, radiusy=RING, fillalpha=0, borderr=1, borderg=1, borderb=1, borderalpha=0.18,
                   borderwidth=3, bgalpha=0, **px))
    swell = '{} + 18 * {}'.format(RING, c('beat'))
    ring = res(mk(ex, circleTOP, 'ring_bar', x0, 150, comment='fills once per bar (bar_phase), swells on each beat',
                  radiusx=Expr(swell), radiusy=Expr(swell),
                  fillalpha=0, borderr=1, borderg=1, borderb=1, borderalpha=1, borderwidth=9, bgalpha=0,
                  beginarcangle=90, endarcangle=Expr('90 - 360 * ' + c('bar_phase')), **px))
    dots = []
    for i in range(4):
        angle = 90 - 90 * i
        dots.append(res(mk(ex, circleTOP, 'beat{}'.format(i + 1), x0 + 200, 300 - 110 * i,
                           comment='beat {} of the bar: bright while it is the current beat'.format(i + 1),
                           radiusx=13, radiusy=13, bgalpha=0, borderalpha=0, **px,
                           centerx=Expr('{} * math.cos(math.radians({}))'.format(RING + 55, angle)),
                           centery=Expr('{} * math.sin(math.radians({}))'.format(RING + 55, angle)),
                           fillcolorr=1, fillcolorg=1, fillcolorb=1,
                           fillalpha=Expr('1.0 if int({}) == {} else 0.2'.format(c('beat_in_bar'), i + 1)))))
    wave_r = '{} + 140 * (1 - {})'.format(RING, c('kick'))
    wave = res(mk(ex, circleTOP, 'kick_wave', x0 + 200, -150, comment='each kick sends a ring outward',
                  radiusx=Expr(wave_r), radiusy=Expr(wave_r), fillalpha=0, borderr=1, borderg=1, borderb=1,
                  borderalpha=Expr('0.8 * ' + c('kick')), borderwidth=4, bgalpha=0, **px))
    shapes = mk(ex, compositeTOP, 'shapes', x0 + 400, 150, [track, ring, wave] + dots, 'ring + kick wave + beat markers',
                operand='add')

    fb = mk(ex, feedbackTOP, 'fb', x0 + 600, 300, [shapes], 'last frame of the tunnel', top='tunnel')
    zoom = mk(ex, transformTOP, 'fb_zoom', x0 + 800, 300, [fb],
              'zoom grows with energy, climbs with predict_drop in a build, punches on a drop-like moment',
              sx=Expr('1.008 + 0.02 * {e} + 0.05 * {pb} * {b} + 0.08 * {d} - 0.006 * {br}'.format(
                  e=c('energy'), pb=c('predict_drop'), b=c('in_build'), d=c('drop'), br=c('in_break'))),
              sy=Expr('me.par.sx'),
              rotate=Expr('0.25 + 0.6 * {}'.format(c('in_drop'))))
    fade = mk(ex, levelTOP, 'fb_fade', x0 + 1000, 300, [zoom], 'trail length: long in a break, short in a drop-like section',
              brightness1=Expr('0.86 + 0.1 * {br} - 0.1 * {d}'.format(br=c('in_break'), d=c('in_drop'))))
    soften = mk(ex, blurTOP, 'fb_soften', x0 + 1000, 450, [fade], 'trails soften as they age', size=2)
    tunnel = mk(ex, compositeTOP, 'tunnel', x0 + 1200, 150, [shapes, soften], 'shapes over their own trails',
                operand='maximum')

    style0 = mk(ex, levelTOP, 'style', x0 + 1400, 150, [tunnel],
                'brightness: kicks flash, downbeats strobe in a drop-like section, a break dims',
                brightness1=Expr('0.75 + 0.35 * {k} + 0.9 * {db} * {d} - 0.25 * {br}'.format(
                    k=c('kick'), db=c('downbeat'), d=c('in_drop'), br=c('in_break'))))

    def tint_expr(ch):
        weights = ' + '.join('{} * {}'.format(c(name), rgb[ch]) for name, rgb in TINTS)
        total = ' + '.join(c(name) for name, _ in TINTS)
        return '({w}) + max(0.0, 1.0 - ({t})) * {b}'.format(w=weights, t=total, b=BLUE[ch])

    tint = res(mk(ex, constantTOP, 'tint', x0 + 1400, 300, comment='section color: blue when calm, orange in a drop-like section',
                  colorr=Expr(tint_expr(0)), colorg=Expr(tint_expr(1)), colorb=Expr(tint_expr(2)), alpha=1))
    style = mk(ex, compositeTOP, 'tinted', x0 + 1500, 150, [style0, tint], operand='multiply')

    phrase_track = res(mk(ex, rectangleTOP, 'phrase_track', x0, -150, comment='the phrase, faint',
                          sizex=0.8, sizey=0.012, centery=-0.42, fillcolorr=BLUE[0], fillcolorg=BLUE[1],
                          fillcolorb=BLUE[2], fillalpha=0.35, bgalpha=0, borderalpha=0))
    phrase_fill = res(mk(ex, rectangleTOP, 'phrase_fill', x0, -300, comment='position in the phrase (phrase_bar / phrase_len)',
                         sizex=Expr('0.8 * {b} / max(1, {n})'.format(b=c('phrase_bar'), n=c('phrase_len'))),
                         sizey=0.012, centerx=Expr('-0.4 + me.par.sizex / 2'), centery=-0.42,
                         fillcolorr=ORANGE[0], fillcolorg=ORANGE[1], fillcolorb=ORANGE[2], fillalpha=0.95,
                         bgalpha=0, borderalpha=0))

    def text(name, y, comment, expr, size, alignx='left', aligny='top', posx=48, posy=-40, alpha='1'):
        return res(mk(ex, textTOP, name, x0 + 200, y, comment=comment, text=Expr(expr), fontautosize='off',
                      fontsizex=size, alignx=alignx, aligny=aligny, positionx=posx, positiony=posy,
                      fontcolorr=1, fontcolorg=1, fontcolorb=1, fontalpha=Expr(alpha), bgalpha=0))

    t_section = text('hud_section', -150, 'section name', "op('fmt').module.section()", 64)
    t_info = text('hud_info', -300, 'key and tempo', "op('fmt').module.info()", 28, posy=-120)
    t_phrase = text('hud_phrase', -450, 'phrase position', "op('fmt').module.phrase()", 22,
                    aligny='bottom', posy=72)
    t_warn = text('hud_warning', -600, 'a likely drop-like moment (experimental), small in a corner; '
                  'pulses on the beat when imminent', "op('fmt').module.warning()", 18, alignx='right', posx=-48,
                  alpha='0.7 + 0.3 * {} * {}'.format(c('drop_imminent'), c('beat')))
    t_off = text('hud_offline', -750, 'shown when the heartbeat stops', "op('fmt').module.offline()", 30,
                 alignx='center', aligny='center', posx=0, posy=-80)
    flash = res(mk(ex, constantTOP, 'drop_flash', x0 + 200, -900, comment='white flash on a drop-like moment',
                   colorr=1, colorg=1, colorb=1, alpha=Expr('0.85 * ' + c('drop'))))

    hud = mk(ex, compositeTOP, 'hud', x0 + 600, -450,
             [phrase_track, phrase_fill, t_section, t_info, t_phrase, t_warn, t_off, flash], operand='over')
    final = mk(ex, compositeTOP, 'final', x0 + 1000, 0, [hud, style], 'HUD over the picture', operand='over')
    finish(ex, final)
    return ex


# ---- Grid -----------------------------------------------------------------------------------
def grid(bundle):
    ex = new_view(bundle, 'grid', -400, 300, '''Grid

640 instanced boxes, 32 columns x 20 rows, driven by Fosfora's musical signals.
columns   an eighth-note march over 4-bar groups; two march in the last bar of a phrase
ripples   every kick sends its own ring out from the center (they overlap)
sparks    each onset lights one box
rows      fill front to back over the phrase; a new phrase sweeps front to back
sweep     a confirmed section change sweeps left to right
floor     breathes with the bass; a slow wave follows the melody
jitter    build tension and the drop-like prediction (experimental) shake the boxes,
          and the camera pushes in as it rises
drop-like the march grows and downbeats flash (experimental)
lock      no march until the tempo locks
Reads the receiver with Steady Bars on, so bars always finish and the march never jumps.
grid is a Script CHOP (numpy) computing each box's position, height and color: blue at rest,
orange where a beat or an event lands.
''', "Grid: the music's structure on 640 instanced boxes")
    envelopes(ex, 'kick', 0.15)
    looks(ex)
    hud = mk(ex, textDAT, 'hud', 400, -350, comment='HUD text')
    hud.text = _src('hud_common.py') + _src('grid_hud.py')
    boxes = mk(ex, scriptCHOP, 'grid', -200, 250, comment='one sample per box: position, height, color')
    docked(boxes, '_callbacks', _src('grid_callbacks.py'))
    geo = mk(ex, geometryCOMP, 'geo', 0, 250, comment='instanced boxes; swings once per phrase')
    for child in list(geo.children):
        child.destroy()
    box = geo.create(boxSOP, 'box')
    _setpars(box, dict(sizex=0.2, sizey=1, sizez=0.2, ty=0.5))
    box.render = True
    box.display = True
    mk(ex, phongMAT, 'mat', 0, 50, diffr=1, diffg=1, diffb=1, ambr=0.35, ambg=0.35, ambb=0.35)
    progress = "((op('fosfora')['phrase_bar'] - 1 + op('fosfora')['bar_phase']) / max(1, op('fosfora')['phrase_len']))"
    _setpars(geo, dict(instancing=True, instancecountmode='oplength', instanceop='grid', instancetx='tx',
                       instancetz='tz', instancesy='sy', instancecolorop='grid', instancer='cr', instanceg='cg',
                       instanceb='cb', instancecolormode='multiply', material='mat',
                       ry=Expr('10 * math.sin(2 * math.pi * {})'.format(progress))))
    mk(ex, cameraCOMP, 'cam', 200, 250, comment='pushes in as the drop-like prediction rises',
       ty=Expr("6.0 - 1.2 * op('looks')['predict_drop']"), tz=Expr("10.8 - 2.5 * op('looks')['predict_drop']"), rx=-28)
    mk(ex, lightCOMP, 'light', 200, 100, tx=3, ty=8, tz=6)
    render = res(mk(ex, renderTOP, 'render', 400, 200, camera='cam', geometry='geo', lights='light'))
    glow = mk(ex, blurTOP, 'bloom', 600, 300, [render], 'soft glow around the bright boxes', size=24)
    lit = mk(ex, compositeTOP, 'lit', 800, 200, [render, glow], operand='add')
    cap = label(ex, 'caption', 600, -200, Expr("op('hud').module.line()"))
    legend = label(ex, 'legend', 600, -350, 'columns: eighths over 4 bars   ripples: kicks   sparks: onsets   rows: phrase'
                   '   floor: bass   jitter: tension', 16, aligny='bottom', posy=24, alpha=0.6)
    pic = mk(ex, compositeTOP, 'picture', 1000, 0, [cap, legend, lit], operand='over')
    finish(ex, pic)
    return ex


# ---- Lights ---------------------------------------------------------------------------------
def lights(bundle):
    ex = new_view(bundle, 'lights', -800, 40, '''Lights

Eight moving-head lights and a haze machine, run the way a lighting operator would, from
Fosfora's musical signals. The look follows the section:
steady    a chase, one step per beat
build     the chase speeds up with the build, beams pull together, a flicker rises
          with the drop-like prediction (experimental)
drop-like all on, every downbeat hits, beams swing on the beat (experimental)
intro     a slow wash        break   slow sweeps
phrase    the chase changes direction every phrase; the last bar of a phrase is a fill
kicks     every light punches     energy   the master dimmer     bass   the haze
lock      no chase until the tempo locks, only a wash
show      Script CHOP: 17 channels in DMX order (dimmer and pan per light, then haze)
beams     GLSL TOP drawing the beams from those channels
dmx_out   Art-Net DMX Out, OFF. Turn Active on and set Net Address to run real lights
          (to_dmx scales 0..1 to 0..255).
''', "Lights: eight moving heads run by the music's structure, with a DMX output (off)")
    envelopes(ex, 'kick', 0.15)
    looks(ex)
    hud = mk(ex, textDAT, 'hud', 400, -350, comment='HUD text')
    hud.text = _src('hud_common.py') + _src('lights_hud.py')
    show = mk(ex, scriptCHOP, 'show', -200, 250, comment='17 channels: dimmer and pan per light, then haze')
    docked(show, '_callbacks', _src('lights_callbacks.py'))
    # Created off: a new DMX Out CHOP starts active on Art-Net.
    dmx = mk(ex, dmxoutCHOP, 'dmx_out', 200, 450, comment='Art-Net out, OFF: turn Active on to run real lights',
             active=False)
    to_dmx = mk(ex, mathCHOP, 'to_dmx', 0, 450, [show], '0..1 to DMX 0..255', gain=255)
    dmx.inputConnectors[0].connect(to_dmx)
    tex = mk(ex, choptoTOP, 'show_tex', 0, 250, comment='the show CHOP as a texture, one channel per row',
             chop='show', dataformat='r', format='rgba32float')
    beams = res(mk(ex, glslTOP, 'beams', 200, 250, [tex], 'draws the beams and floor pools from the show CHOP'))
    docked(beams, '_pixel', _src('lights_beams.glsl'))
    cap = label(ex, 'caption', 600, -200, Expr("op('hud').module.line()"))
    legend = label(ex, 'legend', 600, -350, 'DMX out (off): 8 lights x dimmer + pan, then a haze channel',
                   16, aligny='bottom', posy=24, alpha=0.6)
    pic = mk(ex, compositeTOP, 'picture', 800, 0, [cap, legend, beams], operand='over')
    finish(ex, pic)
    return ex


# ---- Keeping time ---------------------------------------------------------------------------
def keeping_time(bundle):
    ex = new_view(bundle, 'keeping_time', -400, 40, '''Keeping time

Three ways to keep a patch in time with live music, side by side: a dial each (the ring
fills over the bar, the dots are its four beats) and an 8-second trace of each bar phase
underneath (one tooth per bar).

A   the receiver as Fosfora sends it. Exact and immediate. Fosfora restarts its bar
    wherever it currently hears the downbeat, and that can move by a beat or two (and
    back), so a bar can be cut short: a short tooth in the trace.

A+  the receiver with Steady Bars on. Beats still land on Fosfora's beats; every bar
    finishes; a moved downbeat is followed only once it holds for a bar, at the next bar
    line, where the last beat is held (it pulses) until the new downbeat arrives: a flat
    top in the trace. Bars run long, never short.

B   TouchDesigner's own clock driven by Fosfora: this view's timeline (local/time) takes
    Fosfora's tempo, and TouchDesigner's Beat CHOP (beat) restarts on A+'s bars, sent
    early by a lead the clock_b Execute DAT learns. Worth it for a patch already built on
    the Beat CHOP: it follows live music with one change. It knows only the tempo and where
    the bar starts.
''', "Keeping time: bars as they arrive, Steady Bars, and TouchDesigner's own clock",
                  reads=(('fosfora', '../raw'), ('steady', '../steady')))
    envelopes(ex, 'beat', 0.15)
    local = ex.create(baseCOMP, 'local')
    local.nodeX, local.nodeY = -800, -500
    local.comment = "this view's own timeline, so its tempo can follow Fosfora without touching the project's"
    local.create(timeCOMP, 'time')
    mk(ex, beatCHOP, 'beat', -400, 250, comment="TouchDesigner's own beat, on this view's timeline",
       rampbar=True, beat=True, bpm=True)
    clock = mk(ex, executeDAT, 'clock_b', -400, 100, comment="Fosfora drives local/time; the Beat CHOP restarts on A+'s bars")
    clock.text = _src('keeping_clock_b.py')
    _setpars(clock, dict(framestart=True, active=True))
    now = mk(ex, scriptCHOP, 'now', -200, 250, comment="this frame's values for the dials, and the cut and held bar counts")
    docked(now, '_callbacks', _src('keeping_now_callbacks.py'))
    sel = mk(ex, selectCHOP, 'phases', -200, 100, [now], channames='a_phase s_phase b_phase')
    mk(ex, trailCHOP, 'hist', 0, 100, [sel], 'the last 8 seconds of each bar phase', wlength=8,
       wlengthunit='seconds', capture='timeslice')
    now_tex = mk(ex, choptoTOP, 'now_tex', 0, 250, comment='one channel per row', chop='now', dataformat='r',
                 format='rgba32float')
    hist_tex = mk(ex, choptoTOP, 'hist_tex', 200, 100, comment='one bar phase per row', chop='hist', dataformat='r',
                  format='rgba32float')
    dials = res(mk(ex, glslTOP, 'dials', 400, 200, [now_tex, hist_tex], 'draws the dials and traces'))
    docked(dials, '_pixel', _src('keeping_dials.glsl'))

    texts = [label(ex, 'title', 600, 450, 'Keeping time with the music', 26, posy=-22)]
    dial_y = H - int(H * 0.63)
    for i, (key, name, sub) in enumerate((('a', 'A', 'as it arrives'), ('s', 'A+', 'Steady Bars on'),
                                          ('b', 'B', "TouchDesigner's Beat CHOP"))):
        posx = (0.2 + 0.3 * i - 0.5) * W
        texts.append(label(ex, 'name_' + key, 600, 380 - 60 * i, name, 34, alignx='center', aligny='center',
                           posx=posx, posy=H // 2 - dial_y))
        texts.append(label(ex, 'sub_' + key, 800, 380 - 60 * i, sub, 20, alignx='center', posx=posx,
                           posy=-(dial_y + 135)))
        texts.append(label(ex, 'trace_' + key, 1000, 380 - 60 * i, name, 18,
                           posy=-(H - int(H * (0.34 - 0.075 * i)) + 4), alpha=0.7))
    texts.append(label(ex, 'readout', 600, 0, Expr(
        "'since this started:   bars cut short  A {}   A+ {}        bars held long  A+ {}'.format("
        "int(op('now')['a_cut']), int(op('now')['s_cut']), int(op('now')['s_long']))"), 18,
        aligny='bottom', posy=62, alpha=0.9))
    texts.append(label(ex, 'notes', 600, -100,
                       "A is immediate and exact; a moved downbeat cuts the bar short.  A+ waits a bar, and its bars always finish.\n"
                       "B is for a patch already built on TouchDesigner's Beat CHOP; it knows only the tempo and the bar start.",
                       15, aligny='bottom', posy=16, alpha=0.6))
    pic = mk(ex, compositeTOP, 'picture', 800, 0, texts + [dials], operand='over')
    finish(ex, pic)
    return ex


# ---- the bundle -------------------------------------------------------------------------------
def _pages(bundle):
    """The Examples page (View, Port) and the About page, as on the component."""
    page = bundle.appendCustomPage('Examples')
    view = page.appendMenu('View', label='View')[0]
    view.menuNames = [n for n, _ in VIEWS]
    view.menuLabels = [shown for _, shown in VIEWS]
    view.default = view.val = 'all'
    view.help = 'All four views tiled, or one at full size.'
    port = page.appendInt('Port', label='Port')[0]
    port.default = port.val = 9010
    port.normMin, port.min, port.clampMin = 1, 1, True
    port.normMax, port.max, port.clampMax = 65535, 65535, True
    port.help = 'UDP port fosfora --signal sends to (both receivers listen on it).'
    opener = page.appendPulse('Openwindow', label='Open Window')[0]
    opener.startSection = True
    opener.help = 'Show the picture in its own window, 1280 x 720 or filling a display. Esc closes it.'
    fill = page.appendToggle('Filldisplay', label='Fill Display')[0]
    fill.help = 'Open the window filling the whole display, for a projector or a second screen.'
    display = page.appendInt('Display', label='Display')[0]
    display.normMin, display.min, display.clampMin = 0, 0, True
    display.normMax = 3
    display.help = 'Which display the window opens on; 0 is the primary display.'
    about = bundle.appendCustomPage('About')
    for name, shown, value in (('Version', 'Version', VERSION),
                               ('Builtwith', 'Built With', 'TouchDesigner ' + app.build)):
        p = about.appendStr(name, label=shown)[0]
        p.default = p.val = value
        p.readOnly = True
    about.appendPulse('Help', label='Open Docs')
    parexec = mk(bundle, parameterexecuteDAT, 'parexec', -1100, 650, comment='Open Docs opens the guide')
    parexec.text = ('def onPulse(par):\n'
                    '    if par.name == \'Openwindow\':\n'
                    '        op(\'window\').par.winopen.pulse()\n'
                    '    else:\n'
                    '        ui.viewFile({!r})\n').format(DOCS_URL)
    _setpars(parexec, dict(op='..', pars='Help Openwindow', onpulse=True, valuechange=False))
    # The window Open Window shows: the bundle's picture at 1280 x 720, or filling a display.
    mk(bundle, windowCOMP, 'window', -320, -400, comment='Open Window shows out here; Esc closes it',
       winop='out', title='Fosfora examples', justifyoffsetto='specifydisplay',
       display=Expr('parent().par.Display'), justifyh='center', justifyv='center',
       size=Expr("'fill' if parent().par.Filldisplay else 'custom'"), winw=W, winh=H,
       borders=Expr('not parent().par.Filldisplay'), blocksleep=True, closeescape=True)


def _receiver(bundle, name, y, steady_bars, null_name, comment):
    fs = bundle.loadTox(TOX)
    fs.name = name
    fs.nodeX, fs.nodeY = -1300, y
    fs.par.Steadybars = steady_bars
    # Follows the bundle's Port; falls back to 9010 if this copy is moved somewhere without one.
    fs.par.Port.expr = "parent().par.Port if hasattr(parent().par, 'Port') else 9010"
    fs.comment = comment
    null = mk(bundle, nullCHOP, null_name, -1100, y, comment='every channel of ' + name)
    null.inputConnectors[0].connect(fs.outputConnectors[0])
    return fs


def build(parent_path='/project1', name='fosfora_examples', out_dir=_HERE):
    with open(os.path.join(_HERE, 'fosfora_signal.json')) as f:
        signal = json.load(f)
    with open(TOX, 'rb') as f:
        if hashlib.sha256(f.read()).hexdigest() != signal['tox_sha256']:
            raise RuntimeError('fosfora_signal.tox is not the file fosfora_signal.json describes; rebuild it first')
    host = op(parent_path)
    old = host.op(name)
    if old:
        old.destroy()
    bundle = host.create(baseCOMP, name)
    bundle.nodeX, bundle.nodeY = 600, -400
    bundle.nodeWidth, bundle.nodeHeight = 1600, 900  # saved in the .tox: a large picture on the node when dragged in
    bundle.comment = ('Fosfora Signal examples {}: four views of fosfora --signal (Dashboard, Grid, Lights, Keeping '
                      'time). View picks them; each is a COMP you can open or copy. {}'.format(VERSION, DOCS_URL))
    bundle.color = NODE_COLOR
    for tag in ('fosfora', 'osc', 'audio', 'example'):
        bundle.tags.add(tag)
    _pages(bundle)
    mk(bundle, textDAT, 'README', -1300, 650).text = README.format(version=VERSION, docs=DOCS_URL)
    _receiver(bundle, 'fosfora_signal', 400, False, 'raw', 'the component as Fosfora sends it')
    _receiver(bundle, 'fosfora_steady', 150, True, 'steady', 'the same component with Steady Bars on')

    views = [dashboard(bundle), grid(bundle), lights(bundle), keeping_time(bundle)]
    tiled = res(mk(bundle, layoutTOP, 'tiled', -800, -150, comment='all four views, two by two',
                   align='gridrows', numrows=2, numcols=2, fit='fitbest', borderl=2, borderr=2, borderb=2,
                   bordert=2, bunit='pixels'))
    for i, v in enumerate(views):
        tiled.inputConnectors[i].connect(v.outputConnectors[0])
    pick = res(mk(bundle, switchTOP, 'view', -560, -150, comment='the View parameter: all four, or one',
                  index=Expr('parent().par.View.menuIndex')))
    pick.inputConnectors[0].connect(tiled)
    for i, v in enumerate(views):
        pick.inputConnectors[i + 1].connect(v.outputConnectors[0])
    out = mk(bundle, nullTOP, 'out', -320, -150, [pick])
    out.viewer = True
    bundle.par.opviewer = './out'
    bundle.viewer = True

    path, _ = save_tox(bundle, out_dir, 'fosfora_examples', {
        'version': VERSION,
        'sources_sha256': digest(_HERE, EXAMPLE_SOURCES),
        'signal_version': signal['version'],
        'signal_tox_sha256': signal['tox_sha256'],
    })
    print('built', bundle.path, 'version', VERSION, 'in', app.build, '->', path)
    return bundle


build()
