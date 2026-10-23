"""Fosfora Signal for TouchDesigner.

Turns the /fosfora/v1 OSC stream (`fosfora --signal`) into channels a network can
use directly: one-frame pulses instead of running counts, a bar phase that moves
every frame, section and key as labels and indices, and an online flag from the
heartbeat. Protocol: https://github.com/kevinraymond/fosfora/blob/main/docs/SIGNAL.md

This is the component's extension. The OSC In DAT callback feeds Receive() with
every message; the Script CHOP calls Cook() once per frame it is read; the poll
Execute DAT calls Poll() every frame to keep the Status parameter current.
"""

import math

VERSION = '1.0.0'
PREFIX = '/fosfora/v1/'
DOCS_URL = 'https://github.com/kevinraymond/fosfora/blob/main/docs/TOUCHDESIGNER.md'
SECTIONS = ('intro', 'build', 'drop', 'break', 'outro', 'steady')
NOTES = ('C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B')

# Text-valued addresses, as the first rows of the states table in this order.
TEXT_ROWS = tuple(a.replace('/', '_') for a in ('section', 'key', 'status/device', 'status/tier'))

# Events that become one-frame pulses, in output order.
PULSES = ('beat', 'downbeat', 'onset', 'kick', 'boundary', 'drop')

# Events whose argument is a running count; a gap in the count is a lost datagram.
COUNTED = {'beat': 'beat_count', 'downbeat': 'bar_count', 'drop': 'drop_count'}

# Events the wire does not count; the component counts what it received, so every
# pulse has a count channel a CHOP Execute DAT can watch without missing one.
LOCAL_COUNTED = {'onset': 'onset_count', 'kick': 'kick_count', 'boundary': 'boundary_count'}

# Single-number messages that pass straight through, address suffix -> channel.
CONTINUOUS = {
    'bpm': 'bpm_raw',
    'bpm/confidence': 'bpm_conf',
    'bpm/locked': 'locked',
    'build': 'build',
    'energy': 'energy',
    'stem/drums/energy': 'drums',
    'stem/bass/energy': 'bass',
    'stem/melody/energy': 'melody',
    'phrase/bar': 'phrase_bar',
    'phrase/beats_left': 'phrase_beats_left',
    'predict/drop': 'predict_drop',
    'status/uptime': 'uptime',
    'status/hop_hz': 'hop_hz',
}

# predict/drop tiers documented in SIGNAL.md (a drop-like moment, experimental):
# 0.5 = a build has committed, 0.8 = imminence evidence on top.
DROP_WARN = 0.5
DROP_IMMINENT = 0.8

# A Script CHOP cooked again within this many seconds has been cooking steadily.
# Seconds, not frames: a dropped frame advances absTime.frame by several.
STEADY_S = 0.5

# How often Poll() refreshes the Status parameter, in seconds.
POLL_S = 0.5

# Steady Bars: beats Fosfora must keep reporting a moved downbeat before the bars follow.
STEADY_HOLD_BEATS = 4


def _key_index(name):
    """'C' -> 0 ... 'B' -> 11, minor keys ('Am') -> 12 + tonic; -1 if unknown."""
    minor = name.endswith('m')
    tonic = name[:-1] if minor else name
    if tonic not in NOTES:
        return -1
    return NOTES.index(tonic) + (12 if minor else 0)


class SteadyBars:
    """Bars that always finish, for the Steady Bars parameter.

    Fosfora restarts its bar wherever it currently believes the downbeat is, and that
    belief can move by a beat or two and back within seconds, which cuts a bar short:
    [1 2] [1 2 3 4]. Here beats are counted locally, gliding toward Fosfora's beat phase
    (snapping past a quarter of a beat), and Fosfora's bar line is followed only once it
    has held for STEADY_HOLD_BEATS, and only at the end of a bar: the last beat is held
    until the new downbeat arrives. Bars can run long, never short. The phrase position
    is counted from these bars and re-anchored to Fosfora's phrase_bar when the two
    disagree for two bars.
    """

    def __init__(self):
        self.t = None
        self.pos = None             # beats, counted here
        self.offset = 0             # this clock's bar lines: beats n with (n - offset) % 4 == 0
        self.cand = None            # a different bar line Fosfora is reporting
        self.cand_beat = 0
        self.pending = None         # followed at the next bar line
        self.hold = 0               # beats left to hold the last beat of a bar
        self.last_n = None
        self.bars = 0
        self.new_bar = False
        self.anchor = 0
        self.disagree_beat = None

    def update(self, now, phase, bpm, plen, pbar):
        """Advance to this frame; returns (beat_in_bar 0..3, beat_frac, bar_frac, bar_in_phrase)."""
        dt = 0.0 if self.t is None else min(0.1, max(0.0, now - self.t))
        self.t = now
        if self.pos is None:
            self.pos = phase * 4.0
        if bpm > 0:
            self.pos += dt * bpm / 60.0
        err = ((phase * 4.0) % 1.0 - self.pos % 1.0 + 0.5) % 1.0 - 0.5
        self.pos += err if abs(err) > 0.25 else err * min(1.0, dt * 6.0)
        n = math.floor(self.pos)
        off = round(self.pos - phase * 4.0) % 4
        if off == self.offset:
            self.cand = self.pending = None
        elif off != self.cand:
            self.cand, self.cand_beat = off, n
        elif n - self.cand_beat >= STEADY_HOLD_BEATS:
            self.pending = off
        self.new_bar = False
        if self.last_n is None:
            self.last_n, self.offset = n, off
        for beat in range(self.last_n + 1, n + 1):
            if self.hold > 0:
                self.hold -= 1
                if self.hold == 0:
                    self._bar()
                continue
            if (beat - self.offset) % 4 == 0:
                if self.pending is not None:
                    wait = (self.pending - beat) % 4
                    self.offset, self.pending, self.cand = self.pending, None, None
                    if wait:
                        self.hold = wait
                        continue
                self._bar()
        self.last_n = n
        in_bar = 3 if self.hold else (n - self.offset) % 4
        frac = self.pos - n
        bar_frac = 0.9999 if self.hold else (in_bar + frac) / 4.0
        if plen > 0 and pbar > 0:
            if (self.bars - self.anchor) % plen + 1 != pbar:
                if self.disagree_beat is None:
                    self.disagree_beat = n
                elif n - self.disagree_beat >= 8:
                    self.anchor, self.disagree_beat = self.bars - (pbar - 1), None
            else:
                self.disagree_beat = None
        in_phrase = (self.bars - self.anchor) % plen if plen > 0 else 0
        return in_bar, frac, bar_frac, in_phrase

    def _bar(self):
        self.bars += 1
        self.new_bar = True


class FosforaSignalExt:
    def __init__(self, ownerComp):
        self.ownerComp = ownerComp
        self.Reset()

    def Reset(self):
        """Forget everything received so far (the Reset parameter calls this)."""
        self.vals = {}
        self.extra = {}
        self.texts = {}
        self.counts = dict.fromkeys(COUNTED)
        self.local_counts = dict.fromkeys(LOCAL_COUNTED, 0)
        self.received = False
        self.lost = 0
        self.seq = dict.fromkeys(PULSES, 0)
        self.seen = dict.fromkeys(PULSES, 0)
        self.start = dict.fromkeys(PULSES, -(10 ** 9))
        self.arrived = dict.fromkeys(PULSES, -(10 ** 9))
        self.cooked_at = -(10 ** 9)
        self.phase_raw = None
        self.phase_at = 0.0
        self.phase_out = 0.0
        self.held_bpm = 0.0
        self.steady_bars = SteadyBars()
        self.status_at = -(10 ** 9)
        self.online_flag = 0
        self.section = -1
        self.section_conf = 0.0
        self.key = -1
        self.key_conf = 0.0
        self.polled_at = -(10 ** 9)
        self.status = None
        self.error = None
        self.ownerComp.clearScriptErrors()
        self._write_states()

    def OpenDocs(self):
        ui.viewFile(DOCS_URL)

    # ---- receive: one call per OSC message -------------------------------------

    def Receive(self, address, args):
        if not address.startswith(PREFIX) or not args:
            return
        name = address[len(PREFIX):]
        par = self.ownerComp.par
        self.received = True

        if name == 'bar_phase':
            self.phase_raw = float(args[0])
            self.phase_at = absTime.seconds
            return
        channel = CONTINUOUS.get(name)
        if channel is not None:
            self.vals[channel] = float(args[0])
            if name.startswith('status/'):
                self.status_at = absTime.seconds
            return

        if name in COUNTED:
            count = int(args[0])
            last = self.counts[name]
            if last is not None and count > last + 1:
                self.lost += count - last - 1
            # A smaller count means the sender restarted, not that events were lost.
            if last is None or count != last:
                self._event(name)
            self.counts[name] = count
        elif name == 'onset':
            self.vals['onset_strength'] = float(args[0])
            self._event('onset')
        elif name == 'stem/drums/onset':
            self.vals['kick_strength'] = float(args[0])
            self._event('kick')
        elif name == 'section/boundary':
            conf = float(args[0])
            self.vals['boundary_conf'] = conf
            self.vals['boundary_age'] = float(args[1]) if len(args) > 1 else 0.0
            if conf >= par.Boundarygate.eval():
                self._event('boundary')
        elif name == 'section':
            label, conf = str(args[0]), _conf(args)
            self.section_conf = conf
            if conf >= par.Sectiongate.eval() and label in SECTIONS:
                self.section = SECTIONS.index(label)
                self._set_text('section', label, conf)
        elif name == 'key':
            label, conf = str(args[0]), _conf(args)
            self.key_conf = conf
            if conf >= par.Keygate.eval():
                self.key = _key_index(label)
                self._set_text('key', label, conf)
        elif name == 'phrase/len':
            self.vals['phrase_len'] = float(args[0])
            self.vals['phrase_conf'] = _conf(args)
        elif name == 'status/online':
            self.online_flag = int(args[0])
            self.status_at = absTime.seconds
        elif isinstance(args[0], str):
            # status/device, status/tier, and any string address added to /v1 later.
            self._set_text(name.replace('/', '_'), args[0], _conf(args))
            if name.startswith('status/'):
                self.status_at = absTime.seconds
        else:
            # The raw feature bus (feat/<name>), link/*, and any numeric address
            # added to /v1 later: pass through under a channel-safe name.
            self.extra[name.replace('/', '_').replace('.', '_')] = float(args[0])

    def _event(self, name):
        self.seq[name] += 1
        self.arrived[name] = absTime.frame
        if name in self.local_counts:
            self.local_counts[name] += 1

    def _set_text(self, name, value, conf):
        entry = (value, round(conf, 2))
        if self.texts.get(name) != entry:
            self.texts[name] = entry
            self._write_states()

    def _write_states(self):
        table = self.ownerComp.op('states')
        if table is None:
            return
        table.clear()
        table.appendRow(['name', 'value', 'confidence'])
        for name in TEXT_ROWS:
            value, conf = self.texts.get(name, ('', 0.0))
            table.appendRow([name, value, conf])
        for name in sorted(n for n in self.texts if n not in TEXT_ROWS):
            value, conf = self.texts[name]
            table.appendRow([name, value, conf])

    # ---- cook: once per frame ----------------------------------------------------

    def Cook(self, scriptOp):
        # Touch the OSC In DAT so it cooks (and runs Receive) before we read state.
        oscin = self.ownerComp.op('oscin')
        if oscin is not None:
            oscin.numRows
        par = self.ownerComp.par
        frame = absTime.frame
        now = absTime.seconds
        vals = self.vals

        locked = vals.get('locked', 0.0) >= 0.5
        bpm_raw = vals.get('bpm_raw', 0.0)
        phase = self._bar_phase(now, locked, bpm_raw, par.Smoothphase.eval())
        plen = vals.get('phrase_len', 0.0)
        # Always advanced, so turning Steady Bars on takes effect at once.
        sb = self.steady_bars
        sb_beat, sb_frac, sb_bar, sb_phrase = sb.update(now, phase, bpm_raw, int(plen), int(vals.get('phrase_bar', 0.0)))
        steady_bars = bool(par.Steadybars.eval())

        hold = max(1, int(par.Pulseframes.eval()))
        steady = now - self.cooked_at < STEADY_S
        self.cooked_at = now
        out = []
        for name in PULSES:
            if steady_bars and name == 'downbeat':
                # Downbeats come from the steady bars, not from Fosfora's count.
                self.seen[name] = self.seq[name]
                if sb.new_bar:
                    self.start[name] = frame
            elif self.seq[name] != self.seen[name]:
                self.seen[name] = self.seq[name]
                # While this CHOP cooks steadily, every event since its last cook
                # is a pulse. After a pause (nothing was reading it) an older event
                # is history, not a pulse; one frame of slack covers the OSC In DAT
                # cooking after this CHOP within a frame.
                if steady or frame - self.arrived[name] <= 1:
                    self.start[name] = frame
            out.append((name, 1.0 if frame - self.start[name] < hold else 0.0))

        if locked and bpm_raw > 0:
            self.held_bpm = bpm_raw
        bpm = self.held_bpm if par.Holdtempo.eval() else bpm_raw
        if steady_bars:
            bar_phase, beat_phase, beat_in_bar = sb_bar, sb_frac, sb_beat + 1
            phrase_bar = float(sb_phrase + 1) if plen > 0 else vals.get('phrase_bar', 0.0)
            beats_left = plen * 4 - (sb_phrase * 4 + sb_beat) if plen > 0 else vals.get('phrase_beats_left', 0.0)
        else:
            beats = phase * 4.0
            bar_phase, beat_phase, beat_in_bar = phase, beats - int(beats), min(int(beats), 3) + 1
            phrase_bar, beats_left = vals.get('phrase_bar', 0.0), vals.get('phrase_beats_left', 0.0)
        out += [
            ('bpm', bpm),
            ('bpm_conf', vals.get('bpm_conf', 0.0)),
            ('locked', 1.0 if locked else 0.0),
            ('bar_phase', bar_phase),
            ('beat_phase', beat_phase),
            ('beat_in_bar', float(beat_in_bar)),
            ('phrase_bar', phrase_bar),
            ('phrase_beats_left', float(beats_left)),
            ('phrase_len', plen),
            ('phrase_conf', vals.get('phrase_conf', 0.0)),
        ]

        predict = vals.get('predict_drop', 0.0)
        out += [
            ('energy', vals.get('energy', 0.0)),
            ('build', vals.get('build', 0.0)),
            ('drums', vals.get('drums', 0.0)),
            ('bass', vals.get('bass', 0.0)),
            ('melody', vals.get('melody', 0.0)),
            ('predict_drop', predict),
            ('drop_warn', 1.0 if predict >= DROP_WARN else 0.0),
            ('drop_imminent', 1.0 if predict >= DROP_IMMINENT else 0.0),
            ('section', float(self.section)),
            ('section_conf', self.section_conf),
        ]
        out += [('in_' + s, 1.0 if self.section == i else 0.0) for i, s in enumerate(SECTIONS)]
        out += [
            ('key', float(self.key)),
            ('key_conf', self.key_conf),
            ('onset_strength', vals.get('onset_strength', 0.0)),
            ('kick_strength', vals.get('kick_strength', 0.0)),
            ('boundary_conf', vals.get('boundary_conf', 0.0)),
            ('boundary_age', vals.get('boundary_age', 0.0)),
        ]
        counts = {channel: float(self.counts[name] or 0) for name, channel in COUNTED.items()}
        if steady_bars:
            counts['bar_count'] = float(sb.bars)
        out += list(counts.items())
        out += [(channel, float(self.local_counts[name])) for name, channel in LOCAL_COUNTED.items()]
        online = self._online(now)
        out += [
            ('lost', float(self.lost)),
            ('online', 1.0 if online else 0.0),
            ('uptime', vals.get('uptime', 0.0)),
        ]
        out += sorted(self.extra.items())

        scriptOp.clear()
        scriptOp.numSamples = 1
        for name, value in out:
            scriptOp.appendChan(name)[0] = value

    def _online(self, now):
        return bool(self.online_flag) and (now - self.status_at) < self.ownerComp.par.Timeout.eval()

    # ---- poll: every frame, cheap; refreshes Status and the port error ----------

    def Poll(self):
        now = absTime.seconds
        if now - self.polled_at < POLL_S:
            return
        self.polled_at = now
        par = self.ownerComp.par
        oscin = self.ownerComp.op('oscin')
        port = int(par.Port.eval())
        error = None
        if not par.Active.eval():
            status = 'Off'
        elif oscin is not None and oscin.errors():
            # The OSC In DAT reports a port another program holds a cook or two
            # after it fails to bind; surface it on the component itself.
            error = 'Cannot listen on UDP port {}: another program is using it.'.format(port)
            status = 'Port {} in use'.format(port)
        elif not self.received:
            status = 'Waiting for fosfora --signal on port {}'.format(port)
        elif self._online(now):
            device = self.texts.get('status_device', ('', 0))[0]
            status = 'Online' + (': ' + device if device else '')
        elif self.status_at < 0:
            status = 'Receiving, waiting for the first heartbeat'
        elif not self.online_flag:
            status = 'Fosfora stopped'
        else:
            status = 'No heartbeat for {:.0f} s'.format(now - self.status_at)
        if status != self.status:
            self.status = status
            par.Status.val = status
        if error != self.error:
            self.error = error
            self.ownerComp.clearScriptErrors()
            if error:
                self.ownerComp.addScriptError(error)

    def _bar_phase(self, now, locked, bpm, smooth):
        """The bar phase at this frame.

        Messages arrive at the sender's TX rate (30 Hz by default), slower than most
        TouchDesigner frame rates, so between messages the phase is advanced at the
        current tempo. A small backward step (the next message landing just behind
        the extrapolation) is held rather than shown, so beat_in_bar never flickers.
        """
        if self.phase_raw is None:
            return 0.0
        if not smooth:
            return self.phase_raw
        phase = self.phase_raw
        dt = now - self.phase_at
        if locked and bpm > 0 and 0 < dt < 0.25:
            phase = (phase + dt * bpm / 240.0) % 1.0
        step = (phase - self.phase_out + 0.5) % 1.0 - 0.5
        if -0.1 < step < 0:
            phase = self.phase_out
        self.phase_out = phase
        return phase


def _conf(args):
    return float(args[1]) if len(args) > 1 else 1.0
