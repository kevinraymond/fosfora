# OSC In DAT callbacks: every /fosfora/v1 message goes to the extension.


def onReceiveOSC(dat, rowIndex, message, byteData, timeStamp, address, args, peer):
    parent().Receive(address, args)
