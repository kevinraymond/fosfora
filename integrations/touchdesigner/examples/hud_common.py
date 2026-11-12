# HUD text, read by the caption Text TOP. Section and key arrive as text in the bundle's
# receiver (its out_states DAT); the channels come from op('fosfora').
NAMES = {'intro': 'intro', 'build': 'build', 'drop': 'drop-like (experimental)', 'break': 'break',
         'steady': 'steady', 'outro': 'outro'}


def _state(name):
    cell = op('../fosfora_signal/out_states')[name, 'value']
    return cell.val if cell is not None else ''


def _section():
    value = _state('section')
    return NAMES.get(value, value) if value else 'listening'


def _key():
    return _state('key') or '-'
