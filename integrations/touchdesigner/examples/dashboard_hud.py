# Text for the Dashboard view's HUD, read by the Text TOPs' expressions. Section and key
# arrive as text in the bundle's receiver (its out_states DAT); the rest comes from op('ctl').

def _state(name):
    cell = op('../fosfora_signal/out_states')[name, 'value']
    return cell.val if cell is not None else ''


def _ctl(name):
    return op('ctl')[name].eval()


def section():
    name = _state('section')
    return 'DROP-LIKE' if name == 'drop' else name.upper() or 'LISTENING'


def info():
    key = _state('key') or '-'
    if _ctl('locked') >= 0.5:
        return '{}   {:.0f} BPM'.format(key, _ctl('bpm'))
    return '{}   finding the tempo'.format(key)


def phrase():
    n = int(_ctl('phrase_len'))
    if n <= 0:
        return ''
    return 'BAR {} / {}     {} BEATS TO NEXT PHRASE'.format(int(_ctl('phrase_bar')), n, int(_ctl('phrase_beats_left')))


def warning():
    if _ctl('drop_imminent') >= 0.5:
        return 'drop-like moment imminent (experimental)'
    if _ctl('drop_warn') >= 0.5:
        return 'drop-like moment likely (experimental)'
    return ''


def offline():
    return '' if _ctl('online') >= 0.5 else 'NO SIGNAL  -  run: fosfora --signal --host <this machine>'
