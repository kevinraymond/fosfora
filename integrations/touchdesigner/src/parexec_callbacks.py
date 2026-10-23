# Parameter Execute DAT: the component's pulse parameters.


def onPulse(par):
    if par.name == 'Reset':
        parent().Reset()
    elif par.name == 'Help':
        parent().OpenDocs()
