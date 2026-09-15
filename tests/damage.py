#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# damage.py -- break a .sstr on purpose, the ways the Static Stream lab report
# broke them, so the Rust reader and the Python prototype can be shown the
# same wreck.
#
#   damage.py PROTOTYPE MODE IN OUT [ARGS]
#
#   ber RATE          flip that fraction of all bits, at random (seed 7)
#   burst START N     zero N bytes from START
#   wipe K [K...]     overwrite the K-th data records, headers and all, with noise
#   wipehead          overwrite both copies of the stream header record
#   truncate FRAC     keep that fraction of the file
#   tamper K          change one payload byte of the K-th data record and
#                     re-encode it, CRCs and Reed-Solomon parity recomputed
#
# PROTOTYPE is tools/copal-sstr.py, loaded for its record layout, so this
# script knows nothing the prototype does not.

import importlib.util
import random
import sys
import zlib

spec = importlib.util.spec_from_file_location("proto", sys.argv[1])
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
mode, src, dst, args = sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5:]
b = bytearray(open(src, "rb").read())
rnd = random.Random(7)


def records():
    pos, out = len(m.MAGIC), []
    while pos + m.HDR_DISK <= len(b):
        r = m.decode_header(b[pos + 4:pos + m.HDR_COPY])
        n = m.HDR_DISK + m.body_len(r.plen)
        out.append((r, pos, n))
        pos += n
    return out


if mode == "ber":
    nbits = len(b) * 8
    flips = int(nbits * float(args[0]))
    for bit in rnd.sample(range(nbits), flips):
        b[bit // 8] ^= 1 << (bit % 8)
    msg = "%d bits flipped" % flips
elif mode == "burst":
    s, n = int(args[0]), int(args[1])
    b[s:s + n] = bytes(n)
    msg = "%d bytes zeroed at %d" % (n, s)
elif mode == "wipe":
    data = [x for x in records() if x[0].typ == m.T_DATA]
    for k in map(int, args):
        r, pos, n = data[k]
        b[pos:pos + n] = bytes(rnd.randrange(256) for _ in range(n))
    msg = "randomised data records " + " ".join(args)
elif mode == "wipehead":
    r, pos, n = records()[0]
    b[pos:pos + 2 * n] = bytes(rnd.randrange(256) for _ in range(2 * n))
    msg = "randomised both copies of the stream header"
elif mode == "truncate":
    del b[int(len(b) * float(args[0])):]
    msg = "kept %d bytes" % len(b)
elif mode == "tamper":
    data = [x for x in records() if x[0].typ == m.T_DATA]
    r, pos, n = data[int(args[0])]
    body = bytearray(m.fec_decode(bytes(b[pos + m.HDR_DISK:pos + n]), r.plen, r.bcrc)[0])
    body[min(1000, len(body) - 1)] ^= 0xff
    b[pos:pos + n] = m.encode_record(m.Rec(r.typ, r.flags, r.seq, r.t_us, len(body), zlib.crc32(bytes(body))), bytes(body))
    msg = "record %d re-encoded with one payload byte changed" % r.seq
else:
    sys.exit("damage.py: no mode " + mode)
open(dst, "wb").write(b)
print(msg)
