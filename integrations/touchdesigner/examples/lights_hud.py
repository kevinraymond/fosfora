

def line():
    f = op('fosfora')
    nxt = 'next phrase in {} beats'.format(int(f['phrase_beats_left'])) if int(f['phrase_len']) else 'finding the phrase'
    return '{}        {}'.format(op('show_callbacks').module.S['look'], nxt)
