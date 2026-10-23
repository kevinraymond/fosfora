"""TouchDesigner side of the beat-tracker comparison (dev tooling, runs inside TD).

    __file__ = '/path/to/td_harness.py'; exec(open(__file__).read())

with HARNESS = {...} set beforehand (see run()). Builds /project1/fosfora_bench:
an Audio File In CHOP locked to the container's own timeline, a gain gate in
front of the analyzers, and fresh copies of each analyzer per track. With the
project's Realtime flag off every frame is cooked, so each cook carries exactly
one frame of audio and nothing is dropped. A driver logs every cook (frame, the
audio slice's sample range, every analyzer channel) to one JSON file per track;
bench/td_compare/td_logs_to_dumps.py turns those into Signal dumps for scoring.
"""

import json
import os

DRIVER = r'''
# fosfora_bench driver -- DEVELOPMENT ONLY. Logs every cook of every analyzer.
# It lives OUTSIDE the bench container: an Execute DAT's frame callbacks follow
# its own component's timeline, and the bench pauses that timeline between tracks.
import json, os, traceback

B = op(BENCH_PATH)


def _st():
	return B.fetch('bench', None, search=False)


def _cfg():
	return B.fetch('cfg', None, search=False)


def onFrameEnd(frame):
	st = _st()
	if not st or st['state'] in ('done', 'error'):
		return
	try:
		_step(st)
	except Exception:
		st['state'] = 'error'
		project.realTime = True
		with open(os.path.join(_cfg()['out_dir'], 'ERROR.txt'), 'w') as f:
			f.write(traceback.format_exc())


def _step(st):
	cfg = _cfg()
	t = B.time
	if st['state'] == 'setup':
		track = st['queue'][st['i']]
		B.Setup(track)
		t.play = False
		t.frame = 1
		B.op('gate').par.gain = 0
		st.update(state='load', wait=cfg['settle_frames'], rows=[], spikes=[])
	elif st['state'] == 'load':
		_touch()
		st['wait'] -= 1
		if st['wait'] <= 0:
			t.frame = 1
			t.play = True
			B.op('gate').par.gain = 1
			st['state'] = 'run'
			st['end_frame'] = int(st['queue'][st['i']]['duration_s'] * 60) + cfg['tail_frames']
	elif st['state'] == 'run':
		afin = B.op('afin')
		row = [int(t.frame), afin.start, afin.end, afin.numSamples]
		if st['rows'] and row[0] == st['rows'][-1][0]:
			return  # the play transition repeats frame 1: log each cook once
		if st['rows'] and (row[1] - st['rows'][-1][1] != afin.numSamples or afin.numSamples != round(afin.rate / project.cookRate)):
			# A skipped or short cook means audio the analyzers never heard. Measured
			# 2026-10-16: the local timeline jumped 3 frames on a display-sleep resume and
			# twice without a visible cause. Rerun the track from scratch (fresh analyzers).
			st['retries'] = st.get('retries', 0) + 1
			with open(os.path.join(cfg['out_dir'], 'progress.txt'), 'a') as f:
				f.write('RETRY {} {} at row {}: start step {} n {}\n'.format(
					st['i'], st['queue'][st['i']]['track'], len(st['rows']), row[1] - st['rows'][-1][1], afin.numSamples))
			if st['retries'] > 3:
				raise RuntimeError('track {} skipped frames on 4 attempts'.format(st['queue'][st['i']]['track']))
			t.play = False
			st['state'] = 'setup'
			return
		for name in st['systems']:
			o = B.op('out_' + name)
			row += [c.eval() for c in o.chans()] if o is not None else []
		st['rows'].append(row)
		if st['queue'][st['i']].get('calib'):
			vals = afin.numpyArray()[0] if afin.numChans else []
			st['spikes'].append([int(i) for i, v in enumerate(vals) if v > 0.5])
		if t.frame >= st['end_frame']:
			t.play = False
			_write(st)
			st['i'] += 1
			st['retries'] = 0
			if st['i'] >= len(st['queue']):
				st['state'] = 'done'
				project.realTime = True
				with open(os.path.join(cfg['out_dir'], 'DONE.txt'), 'w') as f:
					f.write('ok\n')
			else:
				st['state'] = 'setup'


def _touch():
	for name in _st()['systems']:
		o = B.op('out_' + name)
		if o is not None:
			o.numChans


def _write(st):
	cfg = _cfg()
	track = st['queue'][st['i']]
	chans = {}
	for name in st['systems']:
		o = B.op('out_' + name)
		chans[name] = [c.name for c in o.chans()] if o is not None else []
	out = dict(track=track, rate=B.op('afin').rate, cook_rate=project.cookRate, channels=chans,
			   columns=['frame', 'start', 'end', 'n'], rows=st['rows'], spikes=st['spikes'])
	with open(os.path.join(cfg['out_dir'], '{}__{}.json'.format(track['dataset'], track['track'])), 'w') as f:
		json.dump(out, f)
	with open(os.path.join(cfg['out_dir'], 'progress.txt'), 'a') as f:
		f.write('{} {} rows {}\n'.format(st['i'], track['track'], len(st['rows'])))
'''

SETUP = r'''
# fosfora_bench per-track setup: fresh analyzers, new file.
import os


class BenchExt:
	def __init__(self, owner):
		self.owner = owner

	def Setup(self, track):
		B = self.owner
		cfg = B.fetch('cfg', search=False)
		B.op('afin').par.file = os.path.join(cfg['audio_dir'], track['wav'])
		for name, spec in cfg['systems'].items():
			for old in (B.op(name), B.op('out_' + name)):
				if old is not None:
					old.destroy()
			if spec['kind'] == 'tox':
				o = B.loadTox(spec['path'])
				o.name = name
			else:
				o = B.create(spec['optype'], name)
			if spec.get('inner'):
				# Palette toxes wrap the real component one level down, and wires
				# cannot cross network levels: lift it out of the wrapper.
				lifted = B.copy(o.op(spec['inner']))
				o.destroy()
				lifted.name = name
				o = lifted
			o.nodeX, o.nodeY = 400, spec['y']
			target = o
			for k, v in spec.get('pars', {}).items():
				setattr(target.par, k, v)
			for path, pars in spec.get('inner_pars', {}).items():
				inner = target.op(path)
				for k, v in pars.items():
					setattr(inner.par, k, v)
			target.inputConnectors[0].connect(B.op('gate'))
			out = B.create(nullCHOP, 'out_' + name)
			out.nodeX, out.nodeY = 650, spec['y']
			out.inputConnectors[0].connect(target.outputConnectors[0] if target.isCOMP else target)
			src = target.inputConnectors[0].connections
			if not src or src[0].owner != B.op('gate'):
				raise RuntimeError('{} input is not wired to the gate'.format(name))
			if not out.inputs:
				raise RuntimeError('out_{} is not wired to {}'.format(name, name))
'''


def build(cfg, parent_path='/project1', name='fosfora_bench'):
    host = op(parent_path)
    old = host.op(name)
    if old:
        old.destroy()
    B = host.create(baseCOMP, name)
    B.nodeX, B.nodeY = 1200, 400
    B.comment = 'DEV ONLY: beat-tracker comparison harness'
    local = B.create(baseCOMP, 'local')
    tm = local.create(timeCOMP, 'time')
    tm.par.independent = True
    tm.par.rate = 60
    tm.par.start = 1
    tm.par.end = 10 ** 7
    tm.par.rangestart = 1
    tm.par.rangeend = 10 ** 7

    afin = B.create(audiofileinCHOP, 'afin')
    afin.par.playmode = 'locked'
    afin.par.repeat = 'off'
    afin.par.fade = False
    afin.nodeX, afin.nodeY = -200, 0
    gate = B.create(mathCHOP, 'gate')
    gate.inputConnectors[0].connect(afin)
    gate.nodeX, gate.nodeY = 100, 0
    gate.comment = 'silence while a track loads, so analyzers never hear a repeated slice'

    ext = B.create(textDAT, 'BenchExt')
    ext.text = SETUP
    B.par.ext0object = "op('./BenchExt').module.BenchExt(me)"
    B.par.ext0promote = True
    B.initializeExtensions()

    old_drv = host.op(name + '_driver')
    if old_drv:
        old_drv.destroy()
    drv = host.create(executeDAT, name + '_driver')
    drv.text = DRIVER.replace('BENCH_PATH', repr(B.path))
    drv.par.frameend = True
    drv.nodeX, drv.nodeY = B.nodeX, B.nodeY + 150
    drv.comment = 'DEV ONLY: drives fosfora_bench'

    os.makedirs(cfg['out_dir'], exist_ok=True)
    B.store('cfg', cfg)
    return B


def run(cfg):
    """cfg: audio_dir, out_dir, tracks (list of index entries), systems, settle_frames, tail_frames."""
    B = build(cfg)
    queue = list(cfg['tracks'])
    B.store('bench', dict(state='setup', i=0, queue=queue, systems=list(cfg['systems'])))
    project.realTime = False
    print('bench started:', len(queue), 'tracks, systems', list(cfg['systems']), '-> realtime off')


run(HARNESS)
