# Script CHOP callbacks for the Keeping time view: this frame's values for the dials (one
# channel each, one sample), and a count of bars cut short (A) or held long (A+), from bar
# starts and beat wraps. op('fosfora') is the bundle's receiver as it arrives, op('steady')
# the one with Steady Bars on.
import math
S = {row: {'bp': None, 'bph': None, 'beats': 0, 'starts': 0, 'cut': 0, 'long': 0} for row in ('a', 's')}


def onSetupParameters(scriptOp):
    return


def onPulse(par):
    return


def onGetCookLevel(scriptOp):
    return getattr(CookLevel, 'WHEN_USED', CookLevel.ALWAYS)


def _count(row, bar_phase, beat_phase):
    st = S[row]
    if st['bph'] is not None and beat_phase < st['bph'] - 0.5:
        st['beats'] += 1
    st['bph'] = beat_phase
    if st['bp'] is not None and bar_phase < st['bp'] - 0.05:     # a bar started
        st['starts'] += 1
        if st['starts'] > 1 and 0 < st['beats'] < 4:
            st['cut'] += 1
        elif st['starts'] > 1 and st['beats'] > 4:
            st['long'] += 1
        st['beats'] = 0
    st['bp'] = bar_phase


def onCook(scriptOp):
    a, s, b = op('fosfora'), op('steady'), op('beat')
    _count('a', a['bar_phase'].eval(), a['beat_phase'].eval())
    _count('s', s['bar_phase'].eval(), s['beat_phase'].eval())
    rb = b['rampbar'].eval()
    vals = (('a_phase', a['bar_phase'].eval()), ('s_phase', s['bar_phase'].eval()), ('b_phase', rb),
            ('a_beat', a['beat_in_bar'].eval() - 1), ('s_beat', s['beat_in_bar'].eval() - 1),
            ('b_beat', min(3, math.floor(4 * rb))), ('s_hold', 1.0 if s['bar_phase'].eval() >= 0.9998 else 0.0),
            ('a_cut', S['a']['cut']), ('s_long', S['s']['long']), ('glow', op('env')['beat'].eval()),
            ('time', absTime.seconds % 1000.0), ('s_cut', S['s']['cut']))
    scriptOp.clear()
    scriptOp.numSamples = 1
    for name, v in vals:
        scriptOp.appendChan(name)[0] = float(v)
