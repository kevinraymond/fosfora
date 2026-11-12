# Execute DAT: keeps the Status parameter and the port error current, even while
# nothing reads the component's outputs. Poll() itself runs twice a second.


def onFrameStart(frame):
    parent().Poll()
