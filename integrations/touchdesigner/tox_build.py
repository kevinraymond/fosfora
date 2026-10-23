"""Helpers both builders exec() before they build; the test execs this file too, for the
digests. Nothing here runs at load time or touches TouchDesigner outside a function."""

import glob
import hashlib
import json
import os

# What each .tox is built from: a change to any of these needs a rebuild, and
# test_fosfora_signal_ext.py fails until it happens.
SIGNAL_SOURCES = ('build_fosfora_signal.py', 'tox_build.py', 'src/*.py')
EXAMPLE_SOURCES = ('build_examples.py', 'tox_build.py', 'examples/*')

# TouchDesigner's own operators, present in every install: a reference into them is not a leak
# (every Time COMP clones /sys/local/time; palette components point into /sys and /ui).
SYSTEM = ('/sys/', '/ui/')


def digest(here, patterns):
    """sha256 over the files the patterns name (relative to here), each pattern's matches sorted."""
    h = hashlib.sha256()
    for pattern in patterns:
        paths = sorted(glob.glob(os.path.join(here, pattern)))
        if not paths:
            raise FileNotFoundError(os.path.join(here, pattern))
        for path in paths:
            with open(path, 'rb') as f:
                h.update(os.path.relpath(path, here).replace(os.sep, '/').encode() + b'\0' + f.read() + b'\0')
    return h.hexdigest()


def assert_self_contained(comp):
    """A .tox is loaded under some other path: nothing inside may point outside it."""
    leaks = []
    for o in [comp] + list(comp.findChildren()):
        # A file path, a clone master or a global shortcut on any COMP follows it into
        # every user's project (two instances sharing a shortcut is an error).
        for name in ('externaltox', 'clone', 'opshortcut'):
            par = getattr(o.par, name, None)
            value = par.eval() if par is not None else None
            if value not in ('', None) and not str(getattr(value, 'path', value)).startswith(SYSTEM):
                leaks.append('{}.{} = {}'.format(o.path, name, value))
        for p in o.pars():
            if p.mode != ParMode.CONSTANT or not p.isOP:
                continue
            value = p.val
            if isinstance(value, str) and value.startswith('/') and not value.startswith(SYSTEM):
                leaks.append('{}.{} = {}'.format(o.path, p.name, value))
            target = p.eval()
            if (hasattr(target, 'path') and not (target.path + '/').startswith(comp.path + '/')
                    and not target.path.startswith(SYSTEM)):
                leaks.append('{}.{} -> {}'.format(o.path, p.name, target.path))
    if leaks:
        raise RuntimeError('{} points outside itself:\n  {}'.format(comp.path, '\n  '.join(leaks)))


def _op_refs(o):
    """Paths of every operator o's parameters point at."""
    refs = set()
    for p in o.pars():
        if p.isOP:
            target = p.eval()
            if hasattr(target, 'path'):
                refs.add(target.path)
    return refs


def assert_no_unused_docked(comp):
    """TouchDesigner docks DATs to some operators when it creates them: callbacks for a Script
    CHOP or OSC In DAT, shaders and an Info DAT for a GLSL TOP, keys for a Ramp TOP. A docked
    DAT is in use when its host points at it (callbacks, pixeldat, dat, ...) or it points at
    its host (an Info DAT). A builder that wires its own DAT instead of filling the docked one
    leaves the docked one behind as dead weight."""
    unused = []
    for o in [comp] + list(comp.findChildren()):
        docked = [d for d in o.docked if d.isDAT]
        if not docked:
            continue
        used = _op_refs(o)
        for d in docked:
            if d.path not in used and o.path not in _op_refs(d):
                unused.append('{} (docked to {}, which does not use it)'.format(d.path, o.name))
    if unused:
        raise RuntimeError('{} carries unused docked DATs:\n  {}'.format(comp.path, '\n  '.join(unused)))


def save_tox(comp, out_dir, stem, manifest):
    """Check comp, save <stem>.tox and write <stem>.json: manifest plus the build and the file's hash."""
    assert_self_contained(comp)
    assert_no_unused_docked(comp)
    path = comp.save(os.path.join(out_dir, stem + '.tox'))
    with open(path, 'rb') as f:
        tox = f.read()
    manifest = dict(manifest, touchdesigner_build=app.build,
                    tox_sha256=hashlib.sha256(tox).hexdigest(), tox_bytes=len(tox))
    with open(os.path.join(out_dir, stem + '.json'), 'w') as f:
        json.dump(manifest, f, indent=2)
        f.write('\n')
    return path, manifest
