# Script CHOP callbacks: the extension builds every channel, every frame it is read.


def onSetupParameters(scriptOp):
    return


def onPulse(par):
    return


def onCook(scriptOp):
    parent().Cook(scriptOp)


def onGetCookLevel(scriptOp):
    # Cook every frame while something reads the output, and not at all otherwise.
    # WHEN_USED arrived in 2025.31550; older builds fall back to ALWAYS.
    return getattr(CookLevel, 'WHEN_USED', CookLevel.ALWAYS)
