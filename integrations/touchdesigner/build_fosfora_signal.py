"""Build the Fosfora Signal component inside TouchDesigner and save it as a .tox.

Run from the TouchDesigner textport, with __file__ set to this script's path:

    __file__ = '/path/to/integrations/touchdesigner/build_fosfora_signal.py'; exec(open(__file__).read())

It creates /project1/fosfora_signal from the sources in src/ (replacing any
previous copy) and writes fosfora_signal.tox next to this file, with
fosfora_signal.json recording what it was built from. The .tox is the artifact
users drag into their own networks; src/ and this script are what get reviewed,
and test_fosfora_signal_ext.py fails if they changed since the .tox was built.
Run build_examples.py after it: the examples embed this .tox.
"""

import os

try:
    _HERE = os.path.dirname(os.path.abspath(__file__))
except NameError:
    raise RuntimeError("set __file__ to this script's path before exec()")
exec(open(os.path.join(_HERE, 'tox_build.py')).read(), globals())
SRC = os.path.join(_HERE, 'src')
DOCS_URL = 'https://github.com/kevinraymond/fosfora/blob/main/docs/TOUCHDESIGNER.md'
PROTOCOL_URL = 'https://github.com/kevinraymond/fosfora/blob/main/docs/SIGNAL.md'


def _read(name):
    with open(os.path.join(SRC, name)) as f:
        return f.read()


def _par(group):
    """append*() returns a ParGroup in current builds; the first Par is the one we want."""
    try:
        return group[0]
    except TypeError:
        return group


def _setpars(o, **values):
    names = {p.name for p in o.pars()}
    for name, value in values.items():
        if name not in names:
            raise RuntimeError('{} has no parameter {!r}; it has: {}'.format(o.OPType, name, sorted(names)))
        setattr(o.par, name, value)


def _custom(page, kind, name, label, default, help_text, lo=None, hi=None, clamp=False):
    p = _par(getattr(page, 'append' + kind)(name, label=label))
    if default is not None:
        p.default = default
        p.val = default
    if lo is not None:
        p.normMin, p.min, p.clampMin = lo, lo, clamp
    if hi is not None:
        p.normMax, p.max, p.clampMax = hi, hi, clamp
    try:
        p.help = help_text
    except AttributeError:
        pass
    return p


def _version():
    for line in _read('fosfora_signal_ext.py').splitlines():
        if line.startswith('VERSION = '):
            return line.split('=', 1)[1].strip().strip("'\"")
    return 'dev'


README = """Fosfora Signal for TouchDesigner {version}

Run Fosfora's analysis with:  fosfora --signal --host <this machine's IP> --port 9010
(on the same machine, plain  fosfora --signal  is enough). The Status parameter says
whether anything is arriving. Guide: {docs}  Protocol: {protocol}

out_signal (CHOP, output 1, one sample per frame)
  pulses, 1 for Pulse Frames frames when the event arrives:
    beat downbeat onset kick boundary drop   (drop = a drop-like moment, experimental)
  (a pulse only shows while something reads this CHOP; for logic that must
  never miss one, watch the matching _count channel with a CHOP Execute DAT)
  musical time:
    bpm (holds the last locked tempo when Hold Tempo is on), bpm_conf, locked,
    bar_phase 0..1, beat_phase 0..1, beat_in_bar 1..4 (Steady Bars on: every bar finishes),
    phrase_bar, phrase_beats_left, phrase_len (8|16|32), phrase_conf
  levels 0..1:
    energy build drums bass melody   (drums/bass/melody are spectral proxies, not stems)
  drop-like moments (experimental: they miss some drops, fire on some moments that
  only feel like one, and a prediction can carry across a song change):
    predict_drop 0..1 (rises BEFORE a likely drop-like moment), drop_warn (>= 0.5),
    drop_imminent (>= 0.8)
  states:
    section 0 intro 1 build 2 drop (drop-like) 3 break 4 outro 5 steady (-1 none yet), section_conf,
    in_intro in_build in_drop in_break in_outro in_steady (1 in that section),
    key 0..11 major C..B, 12..23 minor Cm..Bm (-1 none yet), key_conf
  event details: onset_strength kick_strength boundary_conf boundary_age (seconds ago)
  health: beat_count bar_count drop_count (Fosfora's running totals),
    onset_count kick_count boundary_count (received here),
    lost (events missed on the network), online (heartbeat within Timeout), uptime
  anything else Fosfora sends (the --feat-bus features, Ableton Link) appears
  after these, named after its address.

out_states (DAT, output 2): section, key, input device and tier as text, with confidence.
"""


def _comment(o, text):
    o.comment = text
    return o


def _docked_callbacks(o, source):
    """A Script CHOP or OSC In DAT arrives with a callbacks DAT docked to it and already
    wired in: fill that one rather than adding a second, unused one beside it."""
    (cb,) = [d for d in o.docked if d.isDAT]
    cb.text = _read(source)
    o.par.callbacks = cb.name
    return cb


def build(parent_path='/project1', name='fosfora_signal', out_dir=_HERE):
    host = op(parent_path)
    old = host.op(name)
    if old:
        old.destroy()
    comp = host.create(baseCOMP, name)
    comp.nodeX, comp.nodeY = 600, 0
    version = _version()
    _comment(comp, 'Fosfora Signal {}: beats, tempo, bar phase, sections, key and a drop-like prediction (experimental) '
             'from fosfora --signal over OSC. Wire output 1 (CHOP) or 2 (DAT). {}'.format(version, DOCS_URL))
    comp.color = (0.227, 0.439, 0.753)   # Fosfora's deeper blue (#3a70c0)
    for tag in ('fosfora', 'osc', 'audio'):
        comp.tags.add(tag)

    page = comp.appendCustomPage('Fosfora')
    status = _custom(page, 'Str', 'Status', 'Status', 'Starting',
                     'What the component is receiving: waiting, online (with the input device), '
                     'stopped, no heartbeat, or a port another program holds.')
    status.readOnly = True
    _custom(page, 'Toggle', 'Active', 'Active', True, 'Listen for Fosfora.').startSection = True
    _custom(page, 'Int', 'Port', 'Port', 9010, 'UDP port fosfora --signal sends to.', 1, 65535, True)
    _custom(page, 'Int', 'Pulseframes', 'Pulse Frames', 1, 'How many frames each pulse stays at 1.', 1, 10)
    _custom(page, 'Toggle', 'Smoothphase', 'Smooth Phase', True,
            'Advance bar_phase every frame at the current tempo instead of stepping at the send rate.')
    _custom(page, 'Toggle', 'Steadybars', 'Steady Bars', False,
            'Bars always finish: a moved downbeat is followed only after Fosfora reports it for a whole bar, '
            'at the next bar line, holding the last beat until it arrives. Changes bar_phase, beat_phase, '
            'beat_in_bar, downbeat, bar_count and the phrase position.')
    _custom(page, 'Toggle', 'Holdtempo', 'Hold Tempo', True,
            'bpm keeps the last locked tempo while the lock is lost, instead of following the raw estimate.')
    _custom(page, 'Float', 'Boundarygate', 'Boundary Gate', 0.5,
            'Minimum confidence for a boundary pulse. 0.5 keeps under a third of boundaries at higher precision.',
            0, 1, True).startSection = True
    _custom(page, 'Float', 'Sectiongate', 'Section Gate', 0.0,
            'Minimum confidence for section to change.', 0, 1, True)
    _custom(page, 'Float', 'Keygate', 'Key Gate', 0.0, 'Minimum confidence for key to change.', 0, 1, True)
    _custom(page, 'Float', 'Timeout', 'Offline Timeout', 3.0,
            'Seconds without a heartbeat before online falls to 0.', 0.5, 10).startSection = True
    _custom(page, 'Pulse', 'Reset', 'Reset', None, 'Forget everything received so far.')
    about = comp.appendCustomPage('About')
    for p in (_custom(about, 'Str', 'Version', 'Version', version, 'Component version.'),
              _custom(about, 'Str', 'Schema', 'Schema', '/fosfora/v1', 'OSC schema this component reads.'),
              _custom(about, 'Str', 'Builtwith', 'Built With', 'TouchDesigner ' + app.build,
                      'The TouchDesigner build this .tox was saved in.')):
        p.readOnly = True
    _custom(about, 'Pulse', 'Help', 'Open Docs', None, 'Open the TouchDesigner guide in a browser.')

    readme = comp.create(textDAT, 'README')
    readme.text = README.format(version=version, docs=DOCS_URL, protocol=PROTOCOL_URL)
    readme.nodeX, readme.nodeY = -400, 200

    ext = _comment(comp.create(textDAT, 'FosforaSignalExt'), 'The extension: all the logic.')
    ext.text = _read('fosfora_signal_ext.py')
    ext.nodeX, ext.nodeY = -400, 0
    states = comp.create(tableDAT, 'states')
    states.nodeX, states.nodeY = 200, -200

    ext_pars = sorted(p.name for p in comp.pars() if getattr(p.page, 'name', '') == 'Extensions')
    expr = "op('./FosforaSignalExt').module.FosforaSignalExt(me)"
    if 'ext0object' in ext_pars:
        _setpars(comp, ext0object=expr, ext0promote=True)
    elif 'extension1' in ext_pars:
        _setpars(comp, extension1=expr, promoteextension1=True)
    else:
        raise RuntimeError('no extension parameters found; Extensions page has: {}'.format(ext_pars))
    if 'initextonstart' in {p.name for p in comp.pars()}:
        comp.par.initextonstart = True
    comp.initializeExtensions()

    signal = _comment(comp.create(scriptCHOP, 'signal'), 'Builds every channel from the received state.')
    signal.nodeX, signal.nodeY = 0, -200
    _docked_callbacks(signal, 'signal_callbacks.py')
    out_signal = comp.create(outCHOP, 'out_signal')
    out_signal.nodeX, out_signal.nodeY = 400, -100
    out_signal.inputConnectors[0].connect(signal)
    _comment(out_signal, 'Output 1: pulses, musical time, levels, drop-like prediction, states, health.')
    out_states = _comment(comp.create(outDAT, 'out_states'), 'Output 2: section, key, device, tier as text.')
    out_states.nodeX, out_states.nodeY = 400, -300
    out_states.inputConnectors[0].connect(states)

    parexec = comp.create(parameterexecuteDAT, 'parexec')
    parexec.text = _read('parexec_callbacks.py')
    parexec.nodeX, parexec.nodeY = -400, -200
    _setpars(parexec, op='..', pars='Reset Help', onpulse=True, valuechange=False)

    poll = _comment(comp.create(executeDAT, 'poll'), 'Keeps the Status parameter current, twice a second.')
    poll.text = _read('poll_callbacks.py')
    poll.nodeX, poll.nodeY = -400, -320
    _setpars(poll, framestart=True, active=True)

    # Last: once the OSC In DAT has its callbacks it starts calling the extension.
    oscin = _comment(comp.create(oscinDAT, 'oscin'), 'Receives /fosfora/v1 on the Port parameter.')
    oscin.nodeX, oscin.nodeY = 0, 0
    _docked_callbacks(oscin, 'oscin_callbacks.py')
    _setpars(oscin, maxlines=1)
    oscin.par.port.expr = 'parent().par.Port'
    oscin.par.active.expr = 'parent().par.Active'

    # Show the channels on the node itself.
    if 'opviewer' in {p.name for p in comp.pars()}:
        comp.par.opviewer = './out_signal'
    comp.viewer = True

    path, _ = save_tox(comp, out_dir, 'fosfora_signal',
                       {'version': version, 'sources_sha256': digest(_HERE, SIGNAL_SOURCES)})
    print('built', comp.path, 'version', version, 'in', app.build, '->', path)
    return comp


build()
