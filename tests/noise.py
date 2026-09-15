#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# noise.py -- a bad serial line, the same bad line every time (seed 11):
#
#   noise.py DROP BER JUNK < armor > received
#
#   DROP  the fraction of armor lines lost outright
#   BER   the chance each bit of every other byte is flipped
#   JUNK  a kernel message every JUNK lines, as on a shared console (0: none)

import random
import sys

drop, ber, junk = float(sys.argv[1]), float(sys.argv[2]), int(sys.argv[3])
rnd = random.Random(11)
out = sys.stdout.buffer
for n, line in enumerate(sys.stdin.buffer, 1):
    if junk and n % junk == 0:
        out.write(b"[  812.004113] usb 1-1: new high-speed USB device number 7 using xhci_hcd\r\n")
    if line.startswith(b"S") and rnd.random() < drop:
        continue
    b = bytearray(line)
    for i in range(len(b) - 1):
        for bit in range(8):
            if rnd.random() < ber:
                b[i] ^= 1 << bit
    out.write(bytes(b))
