

def line():
    f = op('fosfora')
    n = int(f['phrase_len'])
    phrase = 'bar {} of {}'.format(int(f['phrase_bar']), n) if n else 'finding the phrase'
    tempo = '{:.0f} BPM'.format(f['bpm'].eval()) if f['locked'] >= 0.5 else 'finding the tempo'
    return '{}    {}    {}    {}'.format(_section(), _key(), tempo, phrase)
