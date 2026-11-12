# Execute DAT: Fosfora drives this component's own timeline. The tempo follows Fosfora's
# BPM, and TouchDesigner's Beat CHOP is restarted at the steady bars' bar lines (row A+).
# A restart lands a frame or two late, so it is sent early by a lead this learns once per bar.


def onFrameStart(frame):
    f, t, b, s = op('fosfora'), op('local/time'), op('beat'), op('steady')
    bpm = f['bpm'].eval()
    locked = f['locked'].eval() >= 0.5
    if bpm > 0 and locked and abs(t.par.tempo.eval() - bpm) > 0.05:
        t.par.tempo = round(bpm, 2)
    bar = s['bar_phase'].eval()
    holding = bar >= 0.9998
    lead = me.fetch('lead', 0.02)
    sent = me.fetch('sent', False)
    if not sent and not holding and bar >= 1.0 - lead:     # restart early, for the bar about to start
        b.par.resetpulse.pulse()
        sent = True
    bar_n = s['bar_count'].eval()
    if me.fetch('last_bar', None) != bar_n:                # A+ started a bar: restart now unless already sent
        if not sent:
            b.par.resetpulse.pulse()
        sent = False
        me.store('last_bar', bar_n)
    me.store('sent', sent)
    b.cook(force=True)
    err = (bar - b['rampbar'].eval() + 0.5) % 1.0 - 0.5
    held = op('now')['s_long'].eval()
    if held != me.fetch('held_seen', None):                 # A+ just held a bar: judge B again in 2 bars
        me.store('held_seen', held)
        me.store('quiet_from', bar_n + 2)
    if holding or bar_n < me.fetch('quiet_from', 0):
        return                                             # B cannot hold a bar; judge it on settled bars only
    if locked and 0.45 < bar < 0.55 and not me.fetch('learned', False):
        me.store('lead', min(0.15, max(0.0, lead + max(-0.01, min(0.01, 0.5 * err)))))
        me.store('learned', True)
    if bar < 0.45:
        me.store('learned', False)
    me.store('ema', 0.97 * me.fetch('ema', 0.0) + 0.03 * err)
