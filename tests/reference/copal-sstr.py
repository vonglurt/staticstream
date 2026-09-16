#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson -- part of Copal Linux.
#
# copal-sstr.py -- a static stream: a stream written down so that it can be
# played back as a stream.
#
# A VCR did this for broadcast television: the signal went past, the tape
# kept it in order, and playing the tape gave the signal back at the pace it
# came. A file loses the pace and, usually, the tolerance for damage a stream
# has. A .sstr keeps both. It is a sequence of records, each stamped with the
# time its first byte arrived, each able to be found again after damage, and
# each protected twice:
#
#   inner  Reed-Solomon RS(255,223) over GF(2^8) on every record header and
#          body, the body's codewords interleaved so a burst is spread thin:
#          16 bad bytes per codeword corrected, 32 if their places are known
#   outer  one XOR parity record per GROUP data records (16): any one data
#          record of a group can be lost outright and rebuilt
#
# About 21.5 % more bytes than the stream carried. Nothing in a record depends
# on a codec: the payload is bytes, labelled with a MIME type, so an MPEG-TS
# capture replays byte for byte into whatever plays MPEG-TS.
#
# SIGNED, NOT ENCRYPTED. Every CHECKPOINT records (64) or seconds (10), a
# checkpoint lists the SHA-256 of each record since the last one, chains to
# the previous checkpoint's digest, and is signed with an SSH key by
# 'ssh-keygen -Y sign' (namespace sstr@copal). A capture cut off mid-write is
# still verifiable up to its last checkpoint; one damaged record breaks only
# its own entry, not the chain.
#
# THE TTY LAYER. 'armor' writes any byte stream -- a .sstr above all -- as
# lines a serial console survives: printable, 80 columns, no XON/XOFF bytes,
#
#   S<offset, 9 hex>:<45 bytes as 60 base64url chars>:<CRC-32 of the rest>
#
# 'recv' reads them back, drops lines whose CRC fails or that are not armor
# at all (a kernel message on the same console), and hands the byte ranges it
# lost to the Reed-Solomon decoder as erasures, which it corrects at twice the
# rate of unknown errors.
#
# Usage -- record to freeze a stream, play to let it go again:
#   ffmpeg ... -f mpegts - | copal-sstr.py record cap.sstr --type video/mp2t --key ~/.ssh/id_ed25519
#   copal-sstr.py record notes.sstr --input big.tar --deflate   # a file, compressed
#   copal-sstr.py play notes.sstr -o big.tar         # expanded back into the filesystem
#   copal-sstr.py play cap.sstr | mpv -              # as fast as it goes
#   copal-sstr.py play cap.sstr --paced | mpv -      # at the pace it was captured
#   copal-sstr.py play cap.sstr --follow | mpv -     # while it is still being written
#   copal-sstr.py play cap.sstr --serve :8080 --paced   # then: vlc http://HOST:8080/
#   copal-sstr.py verify cap.sstr [--allowed-signers FILE]
#   copal-sstr.py armor cap.sstr --baud 115200 > /dev/ttyS0
#   copal-sstr.py recv < /dev/ttyS0 > payload        # or: unarmor, then play --erasures
#
# The format is version 0 and a prototype: docs/static-stream-lab-report.md
# says what it is for, what it borrows and what was measured.

import argparse
import base64
import bisect
import hashlib
import json
import os
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import time
import zlib

MAGIC = b"\x89SST\r\n\x1a\n"          # PNG's trick: high bit, CRLF, ^Z, LF
SYNC = b"\xa7SSR"                       # the first copy of a record header
SYNC2 = b"\xa7SSr"                      # the second, straight after it
NSYM = 32
K = 255 - NSYM                          # 223 data bytes per full codeword
HDR_FMT = "<BBHQQII"                    # type, flags, reserved, seq, t_us, plain_len, body_crc
HDR_BARE = struct.calcsize(HDR_FMT)     # 28
HDR_PLAIN = HDR_BARE + 4                # and the header's own CRC-32
HDR_COPY = len(SYNC) + HDR_PLAIN + NSYM # 68
HDR_DISK = 2 * HDR_COPY                 # 136: a header is written twice
MAX_PLAIN = 1 << 24
T_HEAD, T_DATA, T_PARITY, T_CHECK, T_END = b"HDPCE"
TYPES = {T_HEAD, T_DATA, T_PARITY, T_CHECK, T_END}
F_DEFLATE = 1
NAMESPACE = "sstr@copal"
PAR_ENTRY = "<QQBII"                    # seq, t_us, flags, plain_len, body_crc


# --------------------------------------------------------------- GF(2^8) ---
# Primitive polynomial 0x11d, generator 2, first consecutive root 2^0: the
# conventional choice (CCSDS uses a different basis; this is not wire-compatible
# with it, only the same code parameters).

class ReedSolomonError(Exception):
    pass


GF_EXP = [0] * 512
GF_LOG = [0] * 256
_x = 1
for _i in range(255):
    GF_EXP[_i] = _x
    GF_LOG[_x] = _i
    _x <<= 1
    if _x & 0x100:
        _x ^= 0x11d
for _i in range(255, 512):
    GF_EXP[_i] = GF_EXP[_i - 255]


def gf_mul(a, b):
    return 0 if a == 0 or b == 0 else GF_EXP[GF_LOG[a] + GF_LOG[b]]


def gf_div(a, b):
    if b == 0:
        raise ZeroDivisionError
    return 0 if a == 0 else GF_EXP[(GF_LOG[a] + 255 - GF_LOG[b]) % 255]


def gf_pow(a, p):
    return GF_EXP[(GF_LOG[a] * p) % 255]


def gf_inverse(a):
    return GF_EXP[255 - GF_LOG[a]]


def poly_scale(p, x):
    return [gf_mul(c, x) for c in p]


def poly_add(p, q):
    r = [0] * max(len(p), len(q))
    for i, c in enumerate(p):
        r[i + len(r) - len(p)] = c
    for i, c in enumerate(q):
        r[i + len(r) - len(q)] ^= c
    return r


def poly_mul(p, q):
    r = [0] * (len(p) + len(q) - 1)
    for j, b in enumerate(q):
        for i, a in enumerate(p):
            r[i + j] ^= gf_mul(a, b)
    return r


def poly_eval(p, x):
    y = p[0]
    for c in p[1:]:
        y = gf_mul(y, x) ^ c
    return y


def poly_div(dividend, divisor):
    out = list(dividend)
    for i in range(len(dividend) - len(divisor) + 1):
        coef = out[i]
        if coef:
            for j in range(1, len(divisor)):
                if divisor[j]:
                    out[i + j] ^= gf_mul(divisor[j], coef)
    sep = -(len(divisor) - 1)
    return out[:sep], out[sep:]


GEN = [1]
for _i in range(NSYM):
    GEN = poly_mul(GEN, [1, gf_pow(2, _i)])
# Encoding is the LFSR of polynomial division, with the 32-byte register held
# as one integer: three integer operations per data byte instead of 32 table
# lookups, which is what makes pure Python bearable here.
GTAB = [int.from_bytes(bytes(gf_mul(g, c) for g in GEN[1:]), "big") for c in range(256)]
MASK = (1 << (8 * NSYM)) - 1
SHIFT = 8 * (NSYM - 1)


def rs_parity(data):
    reg = 0
    for b in data:
        reg = ((reg << 8) & MASK) ^ GTAB[b ^ (reg >> SHIFT)]
    return reg.to_bytes(NSYM, "big")


def rs_syndromes(msg):
    return [0] + [poly_eval(msg, gf_pow(2, i)) for i in range(NSYM)]


def rs_errata_locator(coef_pos):
    loc = [1]
    for i in coef_pos:
        loc = poly_mul(loc, poly_add([1], [gf_pow(2, i), 0]))
    return loc


def rs_correct_errata(msg, synd, err_pos):
    coef_pos = [len(msg) - 1 - p for p in err_pos]
    loc = rs_errata_locator(coef_pos)
    _, rem = poly_div(poly_mul(synd[::-1], loc), [1] + [0] * len(loc))
    ev = rem[::-1]
    X = [gf_pow(2, -(255 - c)) for c in coef_pos]
    E = [0] * len(msg)
    for i, Xi in enumerate(X):
        Xi_inv = gf_inverse(Xi)
        prime = 1
        for j, Xj in enumerate(X):
            if j != i:
                prime = gf_mul(prime, 1 ^ gf_mul(Xi_inv, Xj))
        if prime == 0:
            raise ReedSolomonError("no error magnitude")
        y = gf_mul(Xi, poly_eval(ev[::-1], Xi_inv))
        E[err_pos[i]] = gf_div(y, prime)
    return poly_add(msg, E)


def rs_error_locator(fsynd, erase_count):
    loc, old = [1], [1]
    for i in range(NSYM - erase_count):
        delta = fsynd[i]
        for j in range(1, len(loc)):
            delta ^= gf_mul(loc[-(j + 1)], fsynd[i - j])
        old = old + [0]
        if delta:
            if len(old) > len(loc):
                new = poly_scale(old, delta)
                old = poly_scale(loc, gf_inverse(delta))
                loc = new
            loc = poly_add(loc, poly_scale(old, delta))
    while loc and loc[0] == 0:
        del loc[0]
    if (len(loc) - 1 - erase_count) * 2 + erase_count > NSYM:
        raise ReedSolomonError("too many errors")
    return loc


def rs_find_errors(loc, n):
    pos = [n - 1 - i for i in range(n) if poly_eval(loc, gf_pow(2, i)) == 0]
    if len(pos) != len(loc) - 1:
        raise ReedSolomonError("error locator does not factor in this codeword")
    return pos


def rs_forney_syndromes(synd, pos, n):
    f = list(synd[1:])
    for p in pos:
        x = gf_pow(2, n - 1 - p)
        for j in range(len(f) - 1):
            f[j] = gf_mul(f[j], x) ^ f[j + 1]
    return f


def rs_correct(cw, erase_pos=()):
    """cw: a received codeword (list of ints, data then NSYM parity), with the
    positions known to be bad. Returns the corrected codeword or raises."""
    msg = list(cw)
    erase_pos = sorted(set(erase_pos))
    if len(erase_pos) > NSYM:
        raise ReedSolomonError("too many erasures")
    for p in erase_pos:
        msg[p] = 0
    synd = rs_syndromes(msg)
    if max(synd) == 0:
        return msg
    fsynd = rs_forney_syndromes(synd, erase_pos, len(msg))
    loc = rs_error_locator(fsynd, len(erase_pos))
    err_pos = rs_find_errors(loc[::-1], len(msg))
    msg = rs_correct_errata(msg, synd, erase_pos + err_pos)
    if max(rs_syndromes(msg)):
        raise ReedSolomonError("could not correct")
    return msg


# --------------------------------------------------------- record bodies ---

def body_len(plain_len):
    full, rest = divmod(plain_len, K)
    return full * 255 + (rest + NSYM if rest else 0)


def _interleave(full_bytes, full, last):
    if full == 0:
        return bytes(last)
    return b"".join(full_bytes[j::255] + last[j:j + 1] for j in range(255))


def _deinterleave(body, full, lastlen):
    if full == 0:
        return bytearray(), bytearray(body)
    F, last, p = bytearray(full * 255), bytearray(lastlen), 0
    for j in range(255):
        F[j::255] = body[p:p + full]
        p += full
        if j < lastlen:
            last[j] = body[p]
            p += 1
    return F, last


def fec_encode(plain):
    full, rest = divmod(len(plain), K)
    F = bytearray()
    for i in range(full):
        piece = plain[i * K:(i + 1) * K]
        F += piece
        F += rs_parity(piece)
    last = b""
    if rest:
        piece = plain[full * K:]
        last = piece + rs_parity(piece)
    return _interleave(bytes(F), full, last)


def fec_decode(body, plain_len, crc, erased=None):
    """(plain, symbols corrected), plain None when the record cannot be rebuilt."""
    full, rest = divmod(plain_len, K)
    lastlen = rest + NSYM if rest else 0
    F, last = _deinterleave(body, full, lastlen)
    cws = [F[i * 255:(i + 1) * 255] for i in range(full)] + ([last] if lastlen else [])
    plain = b"".join(bytes(c[:-NSYM]) for c in cws)
    if zlib.crc32(plain) == crc:
        return plain, 0
    masks = None
    if erased:
        MF, ML = _deinterleave(erased, full, lastlen)
        masks = [MF[i * 255:(i + 1) * 255] for i in range(full)] + ([ML] if lastlen else [])
    out, fixed = [], 0
    for i, c in enumerate(cws):
        pos = [j for j, v in enumerate(masks[i]) if v] if masks else []
        if not pos and rs_parity(bytes(c[:-NSYM])) == bytes(c[-NSYM:]):
            out.append(bytes(c[:-NSYM]))
            continue
        try:
            good = rs_correct(c, pos)
        except (ReedSolomonError, ZeroDivisionError):
            return None, fixed
        fixed += sum(1 for a, b in zip(good, c) if a != b)
        out.append(bytes(good[:-NSYM]))
    plain = b"".join(out)
    return (plain if zlib.crc32(plain) == crc else None), fixed


class Rec:
    __slots__ = ("typ", "flags", "seq", "t_us", "plen", "bcrc", "fixed")

    def __init__(self, typ, flags, seq, t_us, plen, bcrc, fixed=0):
        self.typ, self.flags, self.seq, self.t_us = typ, flags, seq, t_us
        self.plen, self.bcrc, self.fixed = plen, bcrc, fixed

    def bare(self):
        return struct.pack(HDR_FMT, self.typ, self.flags, 0, self.seq, self.t_us, self.plen, self.bcrc)


def encode_record(rec, body):
    # The header is one 64-byte codeword, not interleaved, so a single burst
    # longer than 32 bytes -- one lost armor line is 45 -- would take all of
    # it. Two copies: such a burst leaves at least one of them repairable, and
    # the reader merges two partly erased copies byte by byte.
    bare = rec.bare()
    plain = bare + struct.pack("<I", zlib.crc32(bare))
    copy = plain + rs_parity(plain)
    return SYNC + copy + SYNC2 + copy + fec_encode(body)


def decode_header(b, erased=None):
    """b: the 64 bytes after a sync marker, which need not have survived."""
    b = b[:HDR_PLAIN + NSYM]
    plain, fixed = bytes(b[:HDR_PLAIN]), 0
    if zlib.crc32(plain[:HDR_BARE]) != struct.unpack("<I", plain[HDR_BARE:])[0]:
        pos = [i for i, v in enumerate(erased) if v] if erased else []
        try:
            good = rs_correct(list(b), pos)
        except (ReedSolomonError, ZeroDivisionError):
            return None
        fixed = sum(1 for x, y in zip(good, b) if x != y)
        plain = bytes(good[:HDR_PLAIN])
        if zlib.crc32(plain[:HDR_BARE]) != struct.unpack("<I", plain[HDR_BARE:])[0]:
            return None
    typ, flags, _, seq, t_us, plen, bcrc = struct.unpack(HDR_FMT, plain[:HDR_BARE])
    if typ not in TYPES or plen > MAX_PLAIN:
        return None
    return Rec(typ, flags, seq, t_us, plen, bcrc, fixed)


def record_hash(rec, plain):
    return hashlib.sha256(rec.bare() + plain).digest()


def checkpoint_digest(prev, entries):
    return hashlib.sha256(b"sstr-v0 checkpoint" + prev +
                          b"".join(struct.pack("<Q", s) + h for s, h in entries)).digest()


def genesis(stream_id):
    return hashlib.sha256(b"sstr-v0 genesis" + stream_id).digest()


# ---------------------------------------------------------------- writer ---

class Writer:
    def __init__(self, out, meta, key=None, group=16, ckpt_records=64, ckpt_secs=10.0, deflate=False):
        self.out, self.key, self.group_size = out, key, group
        self.ckpt_records, self.ckpt_secs, self.deflate = ckpt_records, ckpt_secs, deflate
        self.t0 = time.monotonic()
        self.last_ckpt = self.t0
        self.seq = 0
        self.stream_id = os.urandom(16)
        self.prev = genesis(self.stream_id)
        self.group, self.window = [], []
        self.data_records = self.total = 0
        self.payload = hashlib.sha256()
        meta = dict(meta, format="sstr", version=0, stream_id=self.stream_id.hex())
        if key:
            meta["key"] = public_key(key)
        self.meta = meta
        out.write(MAGIC)
        self.record(T_HEAD, json.dumps(meta, indent=1).encode() + b"\n", repeat=2)

    def now(self):
        return int((time.monotonic() - self.t0) * 1e6)

    def record(self, typ, body, flags=0, t_us=None, repeat=1, hashed=True):
        rec = Rec(typ, flags, self.seq, self.now() if t_us is None else t_us, len(body), zlib.crc32(body))
        disk = encode_record(rec, body)
        for _ in range(repeat):
            self.out.write(disk)
        self.out.flush()
        if hashed:
            self.window.append((self.seq, record_hash(rec, body)))
        self.seq += 1
        return rec

    def data(self, chunk, t_us):
        body, flags = chunk, 0
        if self.deflate:
            z = zlib.compress(chunk, 6)
            if len(z) < len(chunk) * 0.95:
                body, flags = z, F_DEFLATE
        rec = self.record(T_DATA, body, flags, t_us)
        self.group.append((rec, body))
        self.data_records += 1
        self.total += len(chunk)
        self.payload.update(chunk)
        due = (len(self.window) >= self.ckpt_records or
               time.monotonic() - self.last_ckpt >= self.ckpt_secs)
        if len(self.group) >= self.group_size or due:
            self.parity()
        if due:
            self.checkpoint()

    def parity(self):
        if not self.group:
            return
        n = max(len(b) for _, b in self.group)
        acc = 0
        for _, b in self.group:
            acc ^= int.from_bytes(b.ljust(n, b"\0"), "big")
        entries = b"".join(struct.pack(PAR_ENTRY, r.seq, r.t_us, r.flags, r.plen, r.bcrc) for r, _ in self.group)
        self.record(T_PARITY, struct.pack("<H", len(self.group)) + entries + acc.to_bytes(n, "big"), hashed=False)
        self.group = []

    def checkpoint(self):
        if not self.window:
            return
        entries, self.window = self.window, []
        d = checkpoint_digest(self.prev, entries)
        # The stream header rides along, as MPEG-TS repeats its program tables:
        # a reader that lost the header record, or joined late, has the key and
        # the content type again at the next checkpoint.
        body = {"prev": self.prev.hex(), "entries": [[s, h.hex()] for s, h in entries],
                "digest": d.hex(), "sig": self.sign(d) if self.key else None, "stream": self.meta}
        self.prev = d
        self.record(T_CHECK, json.dumps(body).encode(), repeat=2, hashed=False)
        self.last_ckpt = time.monotonic()

    def sign(self, d):
        r = subprocess.run(["ssh-keygen", "-q", "-Y", "sign", "-f", self.key, "-n", NAMESPACE],
                           input=b"sstr-v0 " + self.stream_id + d, capture_output=True)
        if r.returncode:
            raise SystemExit("copal-sstr: ssh-keygen could not sign: %s" % r.stderr.decode(errors="replace").strip())
        return r.stdout.decode()

    def finish(self, interrupted=False):
        self.parity()
        end = {"data_records": self.data_records, "bytes": self.total,
               "sha256": self.payload.hexdigest(), "interrupted": interrupted}
        self.record(T_END, json.dumps(end).encode(), repeat=2)
        self.checkpoint()


def public_key(key):
    if os.path.exists(key + ".pub"):
        with open(key + ".pub") as f:
            return f.read().strip()
    r = subprocess.run(["ssh-keygen", "-y", "-f", key], capture_output=True, text=True)
    if r.returncode:
        raise SystemExit("copal-sstr: no public key for %s: %s" % (key, r.stderr.strip()))
    return r.stdout.strip()


def pump(fd, chunk, align, flush_ms, on_chunk):
    """Read fd as a stream: whole chunks as they fill, and whatever whole
    multiple of align has waited flush_ms, stamped with its arrival."""
    buf, first, first_t = bytearray(), 0.0, 0.0
    flush = flush_ms / 1000.0
    while True:
        timeout = max(0.0, first + flush - time.monotonic()) if buf else None
        ready, _, _ = select.select([fd], [], [], timeout)
        if ready:
            b = os.read(fd, 1 << 16)
            if not b:
                break
            if not buf:
                first, first_t = time.monotonic(), time.time()
            buf += b
            while len(buf) >= chunk:
                on_chunk(bytes(buf[:chunk]), first_t)
                del buf[:chunk]
                first, first_t = time.monotonic(), time.time()
        if buf and time.monotonic() >= first + flush:
            n = len(buf) // align * align
            if n:
                on_chunk(bytes(buf[:n]), first_t)
                del buf[:n]
            first, first_t = time.monotonic(), time.time()
    if buf:
        on_chunk(bytes(buf), first_t)


def cmd_write(a):
    fd = sys.stdin.fileno() if a.input == "-" else os.open(a.input, os.O_RDONLY)
    out = sys.stdout.buffer if a.output == "-" else open(a.output, "wb")
    align = a.align or (188 if a.type == "video/mp2t" else 1)
    chunk = max(align, a.chunk // align * align)
    meta = {"created": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "content_type": a.type,
            "chunk": chunk, "align": align,
            "fec": {"inner": "RS(255,223) GF(2^8)/0x11d, interleaved per record",
                    "outer": "XOR, one per %d data records" % a.group},
            "checkpoint": {"records": a.checkpoint_records, "seconds": a.checkpoint_secs}}
    for k in ("source", "license", "note"):
        if getattr(a, k):
            meta[k] = getattr(a, k)
    w = Writer(out, meta, a.key, a.group, a.checkpoint_records, a.checkpoint_secs, a.deflate)
    wall0 = time.time()

    def on_chunk(data, arrived):
        w.data(data, max(0, int((arrived - wall0) * 1e6)))

    def stop(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    try:
        pump(fd, chunk, align, a.flush_ms, on_chunk)
        w.finish()
    except KeyboardInterrupt:
        w.finish(interrupted=True)
    if out is not sys.stdout.buffer:
        out.close()
    return 0


# ---------------------------------------------------------------- reader ---

class Erasures:
    """Byte ranges of the stream known to be lost, in order of discovery."""

    def __init__(self, ranges=()):
        self.starts, self.ends = [], []
        for s, e in ranges:
            self.add(s, e)

    def add(self, s, e):
        if e > s:
            self.starts.append(s)
            self.ends.append(e)

    def mask(self, start, end):
        if not self.starts:
            return None
        i = max(0, bisect.bisect_right(self.starts, start) - 1)
        m, hit = None, False
        while i < len(self.starts) and self.starts[i] < end:
            s, e = max(self.starts[i], start), min(self.ends[i], end)
            if e > s:
                if m is None:
                    m = bytearray(end - start)
                m[s - start:e - start] = b"\1" * (e - s)
                hit = True
            i += 1
        return m if hit else None


class Source:
    def __init__(self, f, follow=False, erasures=None):
        self.read = getattr(f, "read1", None) or f.read
        self.buf, self.base = bytearray(), 0
        self.follow, self.done = follow, False
        self.erasures = erasures or Erasures()

    def end(self):
        return self.base + len(self.buf)

    def fill(self, upto):
        while self.end() < upto:
            b = self.read(max(1 << 16, upto - self.end()))
            if not b:
                if self.follow and not self.done:
                    time.sleep(0.1)
                    continue
                return False
            self.buf += b
        return True

    def get(self, s, e):
        return bytes(self.buf[s - self.base:e - self.base])

    def drop(self, upto):
        cut = upto - self.base
        if cut > 0:
            del self.buf[:cut]
            self.base = upto


class Reader:
    def __init__(self, src, out, gap="skip", paced=False, speed=1.0, start=0.0,
                 allowed_signers=None, verbose=False):
        self.src, self.out, self.gap, self.paced, self.speed = src, out, gap, paced, speed
        self.start_us, self.allowed, self.verbose = int(start * 1e6), allowed_signers, verbose
        self.meta, self.end_rec, self.stream_id = None, None, None
        self.prev = None
        self.pending, self.nond, self.lengths, self.hashes = {}, set(), {}, {}
        # Bodies of good data records, kept after they are played until the
        # parity record of their group has been seen: the one that rebuilds a
        # lost record needs every other record of the group.
        self.bodies = {}
        self.next_seq, self.max_seq = 0, -1
        self.hold = 40
        self.wall0 = self.t0 = None
        self.payload = hashlib.sha256()
        self.trust = None
        self.last_signed = -1
        self.s = dict(records=0, data=0, parity=0, checkpoints=0, duplicates=0, bytes=0,
                      header_fixed=0, body_fixed=0, bodies_repaired=0, recovered=0,
                      data_lost=0, unknown_lost=0, parity_lost=0, resyncs=0, skipped=0,
                      ck_ok=0, ck_bad=0, ck_lost=0, chain_gaps=0, verified=0, mismatched=0,
                      missing=0, sig_good=0, sig_bad=0, sig_unverifiable=0, truncated=False)

    def log(self, msg):
        if self.verbose:
            print("sstr: " + msg, file=sys.stderr)

    def run(self):
        src = self.src
        pos = len(MAGIC)
        if not src.fill(pos) or src.get(0, pos) != MAGIC:
            self.log("no file signature; scanning")
            pos = 0
        while True:
            found = self.next_header(pos)
            if found is None:
                break
            rec, at = found
            blen = body_len(rec.plen)
            if not src.fill(at + HDR_DISK + blen):
                self.s["truncated"] = True
                self.log("truncated in record %d" % rec.seq)
                break
            plain, fixed = fec_decode(src.get(at + HDR_DISK, at + HDR_DISK + blen), rec.plen, rec.bcrc,
                                      src.erasures.mask(at + HDR_DISK, at + HDR_DISK + blen))
            self.s["header_fixed"] += rec.fixed
            if fixed:
                self.s["body_fixed"] += fixed
                self.s["bodies_repaired"] += 1
            self.handle(rec, plain)
            pos = at + HDR_DISK + blen
            src.drop(pos)
        self.release(force=True)
        return self.s

    def try_header(self, at):
        """The record header at byte at, from either copy or the two merged."""
        src, n = self.src, HDR_PLAIN + NSYM
        a0, b0 = at + len(SYNC), at + HDR_COPY + len(SYNC2)
        A, B = src.get(a0, a0 + n), src.get(b0, b0 + n)
        mA, mB = src.erasures.mask(a0, a0 + n), src.erasures.mask(b0, b0 + n)
        rec = decode_header(A, mA) or decode_header(B, mB)
        if rec or not (mA or mB):
            return rec
        merged, mask = bytearray(A), bytearray(n)
        for i in range(n):
            ea, eb = mA and mA[i], mB and mB[i]
            if ea and not eb:
                merged[i] = B[i]
            elif ea and eb:
                mask[i] = 1
        return decode_header(bytes(merged), mask if any(mask) else None)

    def next_header(self, pos):
        src = self.src
        if src.fill(pos + HDR_DISK):
            rec = self.try_header(pos)
            if rec:
                return rec, pos
        p = pos + 1
        while True:
            more = src.fill(p + (1 << 20))
            ia = src.buf.find(SYNC, p - src.base)
            ib = src.buf.find(SYNC2, p - src.base)
            if ia < 0 and ib < 0:
                if not more:
                    self.s["skipped"] += max(0, src.end() - pos)
                    return None
                p = src.end() - len(SYNC)
                continue
            if ib >= 0 and (ia < 0 or ib < ia):
                at, p_next = src.base + ib - HDR_COPY, src.base + ib + 1
            else:
                at, p_next = src.base + ia, src.base + ia + 1
            if at < pos:
                p = p_next
                continue
            if not src.fill(at + HDR_DISK):
                self.s["skipped"] += src.end() - pos
                return None
            rec = self.try_header(at)
            if rec:
                self.s["resyncs"] += 1
                self.s["skipped"] += at - pos
                self.log("resync at byte %d, record %d, %d bytes skipped" % (at, rec.seq, at - pos))
                return rec, at
            p = p_next

    def handle(self, rec, plain):
        s = rec.seq
        if s < self.next_seq or (s in self.pending and self.pending[s][1] is not None) or \
                (s in self.nond and rec.typ != T_DATA):
            self.s["duplicates"] += 1
            return
        self.s["records"] += 1
        self.max_seq = max(self.max_seq, s)
        if rec.typ == T_DATA:
            self.s["data"] += 1
            self.pending[s] = (rec, plain)
            if plain is not None:
                self.hashes[s] = record_hash(rec, plain)
                self.bodies[s] = plain
                if len(self.bodies) > 8 * self.hold:
                    for old in [k for k in self.bodies if k < s - 4 * self.hold]:
                        del self.bodies[old]
        else:
            if plain is None:
                if rec.typ == T_PARITY:
                    self.s["parity_lost"] += 1
                elif rec.typ == T_CHECK:
                    self.s["ck_lost"] += 1
                self.log("record %d (%s) body lost" % (s, chr(rec.typ)))
                self.s["records"] -= 1   # its repeat may still arrive
                return
            self.nond.add(s)
            if rec.typ == T_HEAD:
                self.meta = json.loads(plain)
                self.stream_id = bytes.fromhex(self.meta["stream_id"])
                self.prev = genesis(self.stream_id)
                self.hashes[s] = record_hash(rec, plain)
            elif rec.typ == T_PARITY:
                self.s["parity"] += 1
                self.apply_parity(plain)
            elif rec.typ == T_CHECK:
                self.check(plain)
            elif rec.typ == T_END:
                self.end_rec = json.loads(plain)
                self.hashes[s] = record_hash(rec, plain)
        self.release()

    def apply_parity(self, plain):
        (count,) = struct.unpack("<H", plain[:2])
        esz = struct.calcsize(PAR_ENTRY)
        entries = []
        for i in range(count):
            seq, t_us, flags, plen, bcrc = struct.unpack(PAR_ENTRY, plain[2 + i * esz:2 + (i + 1) * esz])
            entries.append(Rec(T_DATA, flags, seq, t_us, plen, bcrc))
        blob = plain[2 + count * esz:]
        for e in entries:
            self.lengths[e.seq] = e.plen
        missing = [e for e in entries if e.seq not in self.bodies]
        if len(missing) != 1:
            for e in entries:
                self.bodies.pop(e.seq, None)
            return
        acc = int.from_bytes(blob, "big")
        n = len(blob)
        for e in entries:
            if e is not missing[0]:
                acc ^= int.from_bytes(self.bodies.pop(e.seq).ljust(n, b"\0"), "big")
        e = missing[0]
        body = acc.to_bytes(n, "big")[:e.plen]
        if zlib.crc32(body) != e.bcrc or e.seq < self.next_seq:
            return
        self.pending[e.seq] = (e, body)
        self.hashes[e.seq] = record_hash(e, body)
        self.s["recovered"] += 1
        self.max_seq = max(self.max_seq, e.seq)
        self.log("record %d rebuilt from parity" % e.seq)

    def check(self, plain):
        c = json.loads(plain)
        self.s["checkpoints"] += 1
        if self.meta is None and c.get("stream"):
            self.meta = c["stream"]
            self.stream_id = bytes.fromhex(self.meta["stream_id"])
            self.log("stream header taken from a checkpoint")
        prev = bytes.fromhex(c["prev"])
        entries = [(s, bytes.fromhex(h)) for s, h in c["entries"]]
        d = checkpoint_digest(prev, entries)
        if d.hex() != c["digest"]:
            self.s["ck_bad"] += 1
            self.log("checkpoint digest does not match its entries")
            return
        if self.prev is not None and prev != self.prev:
            self.s["chain_gaps"] += 1
            self.log("checkpoint does not follow the last one seen: one was lost")
        self.prev = d
        good_sig = False
        if c.get("sig"):
            good_sig = self.verify_sig(d, c["sig"])
            self.s["sig_unverifiable" if good_sig is None else "sig_good" if good_sig else "sig_bad"] += 1
        ok = True
        for s, h in entries:
            mine = self.hashes.pop(s, None)
            if mine is None:
                self.s["missing"] += 1
            elif mine == h:
                self.s["verified"] += 1
            else:
                self.s["mismatched"] += 1
                ok = False
                self.log("record %d does not match its checkpoint entry" % s)
        self.s["ck_ok" if ok else "ck_bad"] += 1
        if good_sig and entries:
            self.last_signed = max(self.last_signed, entries[-1][0])
        if self.end_rec is not None:
            self.src.done = True

    def verify_sig(self, d, sig):
        key = (self.meta or {}).get("key")
        if not key or self.stream_id is None:
            return None
        with tempfile.TemporaryDirectory() as t:
            sigf, allowed = os.path.join(t, "sig"), os.path.join(t, "allowed")
            with open(sigf, "w") as f:
                f.write(sig)
            with open(allowed, "w") as f:
                f.write('sstr namespaces="%s" %s\n' % (NAMESPACE, key))
            r = subprocess.run(["ssh-keygen", "-Y", "verify", "-f", allowed, "-I", "sstr", "-n", NAMESPACE,
                                "-s", sigf], input=b"sstr-v0 " + self.stream_id + d, capture_output=True)
            if r.returncode == 0 and self.trust is None:
                self.trust = self.find_trust(sigf)
        return r.returncode == 0

    def find_trust(self, sigf):
        if not self.allowed:
            return "no --allowed-signers given"
        r = subprocess.run(["ssh-keygen", "-Y", "find-principals", "-s", sigf, "-f", self.allowed],
                           capture_output=True, text=True)
        return ("trusted as " + r.stdout.strip()) if r.returncode == 0 else "NOT in " + self.allowed

    def release(self, force=False):
        while self.next_seq <= self.max_seq:
            s = self.next_seq
            if s in self.nond:
                self.nond.discard(s)
                self.next_seq += 1
                continue
            e = self.pending.get(s)
            if e and e[1] is not None:
                self.emit(*e)
            else:
                if not force and self.max_seq - s <= self.hold:
                    return
                if e is not None or s in self.lengths:
                    self.s["data_lost"] += 1
                    n = self.lengths.get(s, e[0].plen if e else 0)
                    self.log("data record %d lost (%d bytes)" % (s, n))
                    if self.gap == "zero" and n and not (e and e[0].flags & F_DEFLATE):
                        self.out.write(bytes(n))
                else:
                    self.s["unknown_lost"] += 1
                    self.log("record %d never arrived" % s)
            self.pending.pop(s, None)
            self.next_seq += 1

    def emit(self, rec, plain):
        data = plain
        if rec.flags & F_DEFLATE:
            try:
                data = zlib.decompress(plain)
            except zlib.error:
                self.s["data_lost"] += 1
                return
        self.payload.update(data)
        self.s["bytes"] += len(data)
        if rec.t_us < self.start_us:
            return
        if self.paced:
            if self.wall0 is None:
                self.wall0, self.t0 = time.monotonic(), rec.t_us
            wait = self.wall0 + (rec.t_us - self.t0) / 1e6 / self.speed - time.monotonic()
            if wait > 0:
                time.sleep(wait)
        if self.out:
            self.out.write(data)
            self.out.flush()


def open_input(path, follow):
    if path == "-":
        return sys.stdin.buffer
    return open(path, "rb", buffering=0)


def load_erasures(path):
    if not path:
        return None
    with open(path) as f:
        return Erasures(json.load(f))


def summary(r):
    s, m = r.s, r.meta or {}
    lines = []
    lines.append("stream       %s  %s  created %s" % (m.get("stream_id", "?"), m.get("content_type", "?"),
                                                      m.get("created", "?")))
    for k in ("source", "license", "note"):
        if m.get(k):
            lines.append("%-12s %s" % (k, m[k]))
    key = m.get("key")
    if key:
        with tempfile.NamedTemporaryFile("w", suffix=".pub") as t:
            t.write(key + "\n")
            t.flush()
            fp = subprocess.run(["ssh-keygen", "-l", "-f", t.name], capture_output=True, text=True).stdout.strip()
        lines.append("key          %s; %s" % (fp or key[:40], r.trust or "no good signature seen"))
    else:
        lines.append("key          none: checkpoints detect damage, not tampering")
    lines.append("records      %d (%d data, %d parity, %d checkpoints), %d repeats skipped"
                 % (s["records"], s["data"], s["parity"], s["checkpoints"], s["duplicates"]))
    end = r.end_rec
    if end:
        match = (end["sha256"] == r.payload.hexdigest() and end["bytes"] == s["bytes"])
        lines.append("payload      %d bytes, sha256 %s (%s)" % (s["bytes"], r.payload.hexdigest()[:16],
                                                               "matches the end record" if match else
                                                               "DOES NOT match the end record"))
    else:
        lines.append("payload      %d bytes; no end record%s" % (s["bytes"], ", file truncated" if s["truncated"] else ""))
    lines.append("repaired     %d header bytes, %d body bytes in %d records, %d records rebuilt from parity"
                 % (s["header_fixed"], s["body_fixed"], s["bodies_repaired"], s["recovered"]))
    lines.append("lost         %d data records, %d records of unknown type, %d parity; %d bytes skipped in %d resyncs"
                 % (s["data_lost"], s["unknown_lost"], s["parity_lost"], s["skipped"], s["resyncs"]))
    lines.append("checkpoints  %d good, %d bad, %d chain gaps; entries %d verified, %d mismatched, %d unaccounted"
                 % (s["ck_ok"], s["ck_bad"], s["chain_gaps"], s["verified"], s["mismatched"], s["missing"]))
    lines.append("signatures   %d good, %d bad, %d not checkable (no key seen yet); signed through record %d of %d"
                 % (s["sig_good"], s["sig_bad"], s["sig_unverifiable"], r.last_signed, r.max_seq))
    if end:
        lines.append("end          present%s" % (", writer was interrupted" if end.get("interrupted") else ""))
    else:
        lines.append("end          missing: the capture stopped without closing")
    return "\n".join(lines)


def status(r):
    s = r.s
    bad = s["data_lost"] or s["mismatched"] or s["ck_bad"] or s["sig_bad"] or s["truncated"] or r.end_rec is None
    if r.end_rec and (r.end_rec["sha256"] != r.payload.hexdigest()):
        bad = True
    # Asked whose key it must be, a good signature by anyone else is a failure.
    if r.allowed and not (r.trust or "").startswith("trusted"):
        bad = True
    return 1 if bad else 0


def cmd_play(a):
    if a.serve:
        return serve(a)
    out = open(a.output, "wb") if a.output else sys.stdout.buffer
    src = Source(open_input(a.input, a.follow), a.follow, load_erasures(a.erasures))
    r = Reader(src, out, a.gap, a.paced, a.speed, a.start, a.allowed_signers, a.verbose)
    r.run()
    if a.output:
        out.close()
    if a.verbose or a.output:
        print(summary(r), file=sys.stderr)
    return status(r)


def read_meta(path):
    """The header record's JSON, from the first records of a .sstr."""
    src = Source(open(path, "rb", buffering=0))
    r = Reader(src, None)
    found = r.next_header(len(MAGIC))
    if found:
        rec, at = found
        blen = body_len(rec.plen)
        if rec.typ == T_HEAD and src.fill(at + HDR_DISK + blen):
            plain, _ = fec_decode(src.get(at + HDR_DISK, at + HDR_DISK + blen), rec.plen, rec.bcrc)
            if plain:
                return json.loads(plain)
    return {}


def serve(a):
    """The VideoLAN split: this is the server, and any player that opens
    http://HOST:PORT/ is a client. Each client gets its own reader from the
    start of the capture (or --start), so a capture still being written with
    --follow is a timeshift buffer: join late and watch from the beginning."""
    import socket
    import threading
    if a.input == "-":
        raise SystemExit("copal-sstr: --serve needs a file, not stdin")
    host, _, port = a.serve.rpartition(":")
    ctype = read_meta(a.input).get("content_type", "application/octet-stream")
    srv = socket.create_server((host or "0.0.0.0", int(port)))
    print("sstr: serving %s (%s) on http://%s:%s/" % (a.input, ctype, host or "0.0.0.0", port), file=sys.stderr)

    def client(conn, addr):
        with conn:
            conn.settimeout(10)
            try:
                req = conn.recv(4096).split(b"\r\n", 1)[0].decode(errors="replace")
                conn.settimeout(None)
                conn.sendall(b"HTTP/1.0 200 OK\r\nContent-Type: " + ctype.encode() +
                             b"\r\nCache-Control: no-cache\r\n\r\n")
                out = conn.makefile("wb", buffering=0)
                r = Reader(Source(open(a.input, "rb", buffering=0), a.follow), out, a.gap, a.paced,
                           a.speed, a.start)
                t0 = time.monotonic()
                r.run()
                out.flush()
                print("sstr: %s:%d %r: %d bytes in %.1f s" % (addr[0], addr[1], req, r.s["bytes"],
                                                            time.monotonic() - t0), file=sys.stderr)
            except (BrokenPipeError, ConnectionResetError, socket.timeout) as e:
                print("sstr: %s:%d left: %s" % (addr[0], addr[1], e), file=sys.stderr)

    while True:
        conn, addr = srv.accept()
        if a.once:
            client(conn, addr)
            return 0
        threading.Thread(target=client, args=(conn, addr), daemon=True).start()


def cmd_verify(a):
    src = Source(open_input(a.input, False), False, load_erasures(a.erasures))
    r = Reader(src, None, allowed_signers=a.allowed_signers, verbose=a.verbose)
    r.run()
    print(summary(r))
    return status(r)


# -------------------------------------------------------------- tty layer ---

LINE_BYTES = 45
LINE = re.compile(rb"^S([0-9a-f]{9}):([A-Za-z0-9_-]{1,60}):([0-9a-f]{8})\r?$")


def armor_line(off, data):
    head = "S%09x:%s" % (off, base64.urlsafe_b64encode(data).rstrip(b"=").decode())
    return "%s:%08x\n" % (head, zlib.crc32(head.encode()))


def cmd_armor(a):
    fd = sys.stdin.fileno() if a.input == "-" else os.open(a.input, os.O_RDONLY)
    out = sys.stdout.buffer
    rate = a.baud / 10.0 if a.baud else 0
    st = {"off": 0, "sent": 0, "t0": time.monotonic()}
    h = hashlib.sha256()

    def send(text):
        b = text.encode()
        out.write(b)
        out.flush()
        st["sent"] += len(b)
        if rate:
            wait = st["t0"] + st["sent"] / rate - time.monotonic()
            if wait > 0:
                time.sleep(wait)

    def on_chunk(data, _arrived):
        h.update(data)
        for i in range(0, len(data), LINE_BYTES):
            piece = data[i:i + LINE_BYTES]
            send(armor_line(st["off"], piece))
            st["off"] += len(piece)

    send("-----BEGIN SSTR ARMOR v0-----\n")
    pump(fd, LINE_BYTES, LINE_BYTES, a.flush_ms, on_chunk)
    send("-----END SSTR ARMOR %x %s-----\n" % (st["off"], h.hexdigest()[:40]))
    return 0


class Unarmor:
    """Armor lines in, the byte stream out, with what was lost as erasures."""

    def __init__(self, f, erasures):
        self.f, self.er = f, erasures
        self.off, self.pending, self.ended = 0, bytearray(), False
        self.total = None
        self.s = dict(lines=0, crc_failed=0, not_armor=0, lost_bytes=0, repeats=0)

    def read1(self, n=1 << 16):
        while not self.pending and not self.ended:
            raw = self.f.readline()
            if not raw:
                self.ended = True
                break
            line = raw.rstrip(b"\r\n")
            if line.startswith(b"-----END SSTR ARMOR"):
                parts = line.split()
                try:
                    self.total = int(parts[3], 16)
                except (IndexError, ValueError):
                    continue
                if self.total > self.off:
                    self.lose(self.total - self.off)
                self.ended = True
                break
            m = LINE.match(line)
            if not m:
                if line and not line.startswith(b"-----BEGIN SSTR ARMOR"):
                    self.s["not_armor"] += 1
                continue
            head = line[:line.rfind(b":")]
            if zlib.crc32(head) != int(m.group(3), 16):
                self.s["crc_failed"] += 1
                continue
            try:
                data = base64.urlsafe_b64decode(m.group(2) + b"=" * (-len(m.group(2)) % 4))
            except ValueError:
                self.s["crc_failed"] += 1
                continue
            off = int(m.group(1), 16)
            if off < self.off:
                self.s["repeats"] += 1
                continue
            if off > self.off:
                self.lose(off - self.off)
            self.s["lines"] += 1
            self.pending += data
            self.off += len(data)
        out = bytes(self.pending[:n])
        del self.pending[:n]
        return out

    def lose(self, n):
        self.er.add(self.off, self.off + n)
        self.pending += bytes(n)
        self.off += n
        self.s["lost_bytes"] += n


def cmd_unarmor(a):
    er = Erasures()
    u = Unarmor(sys.stdin.buffer if a.input == "-" else open(a.input, "rb"), er)
    out = sys.stdout.buffer
    while True:
        b = u.read1()
        if not b:
            break
        out.write(b)
    if a.erasures:
        with open(a.erasures, "w") as f:
            json.dump([[s, e] for s, e in zip(er.starts, er.ends)], f)
    print("unarmor: %d lines, %d failed CRC, %d not armor, %d bytes lost%s"
          % (u.s["lines"], u.s["crc_failed"], u.s["not_armor"], u.s["lost_bytes"],
             "" if u.total is not None else ", no END line"), file=sys.stderr)
    return 0


def cmd_recv(a):
    er = Erasures()
    u = Unarmor(sys.stdin.buffer if a.input == "-" else open(a.input, "rb"), er)
    r = Reader(Source(u, False, er), None if a.verify else sys.stdout.buffer, a.gap, a.paced, a.speed, 0.0,
               a.allowed_signers, a.verbose)
    r.run()
    print("recv: %d lines, %d failed CRC, %d not armor, %d bytes lost on the line"
          % (u.s["lines"], u.s["crc_failed"], u.s["not_armor"], u.s["lost_bytes"]), file=sys.stderr)
    if a.verify or a.verbose:
        print(summary(r), file=sys.stderr)
    return status(r)


def main():
    p = argparse.ArgumentParser(prog="copal-sstr.py", description="static stream capture, playback and tty armor")
    sub = p.add_subparsers(dest="cmd", required=True)

    w = sub.add_parser("record", aliases=["write"], help="capture stdin (or --input) into a .sstr")
    w.add_argument("output", help="the .sstr, or - for stdout")
    w.add_argument("--input", default="-")
    w.add_argument("--type", default="application/octet-stream", help="MIME type of the payload")
    w.add_argument("--chunk", type=int, default=65536, help="bytes per data record (rounded to --align)")
    w.add_argument("--align", type=int, default=0, help="packet size to cut on; 188 for video/mp2t")
    w.add_argument("--flush-ms", type=int, default=200, help="longest a partial chunk waits")
    w.add_argument("--group", type=int, default=16, help="data records per parity record")
    w.add_argument("--checkpoint-records", type=int, default=64)
    w.add_argument("--checkpoint-secs", type=float, default=10.0)
    w.add_argument("--key", help="SSH private key (or a public key held by ssh-agent) to sign checkpoints")
    w.add_argument("--deflate", action="store_true", help="compress a record when that saves 5 %%")
    w.add_argument("--source", help="where the stream came from (URL)")
    w.add_argument("--license", help="the license the source states")
    w.add_argument("--note")
    w.set_defaults(fn=cmd_write)

    for name, fn, hlp in (("play", cmd_play, "write the payload to stdout"),
                          ("verify", cmd_verify, "check, repair in memory, and report")):
        q = sub.add_parser(name, help=hlp)
        q.add_argument("input", help="the .sstr, or - for stdin")
        q.add_argument("--erasures", help="JSON byte ranges known lost, from unarmor")
        q.add_argument("--allowed-signers", help="an OpenSSH allowed_signers file to name the key")
        q.add_argument("-v", "--verbose", action="store_true")
        if name == "play":
            q.add_argument("--paced", action="store_true", help="at the pace of capture")
            q.add_argument("--speed", type=float, default=1.0)
            q.add_argument("--start", type=float, default=0.0, help="seconds into the capture")
            q.add_argument("--follow", action="store_true", help="keep reading a file still being written")
            q.add_argument("--gap", choices=("skip", "zero"), default="skip")
            q.add_argument("-o", "--output", help="expand into this file instead of stdout")
            q.add_argument("--serve", metavar="[HOST]:PORT", help="serve over HTTP to players, VLC-style")
            q.add_argument("--once", action="store_true", help="with --serve: one client, then exit")
        q.set_defaults(fn=fn)

    ar = sub.add_parser("armor", help="bytes to CRC-checked base64url lines for a tty")
    ar.add_argument("input", nargs="?", default="-")
    ar.add_argument("--baud", type=int, default=0, help="pace output to this line rate, 8N1")
    ar.add_argument("--flush-ms", type=int, default=200)
    ar.set_defaults(fn=cmd_armor)

    ua = sub.add_parser("unarmor", help="armor lines back to bytes")
    ua.add_argument("input", nargs="?", default="-")
    ua.add_argument("--erasures", help="write the lost byte ranges here, for play/verify")
    ua.set_defaults(fn=cmd_unarmor)

    rv = sub.add_parser("recv", help="armor lines of a .sstr straight to its payload")
    rv.add_argument("input", nargs="?", default="-")
    rv.add_argument("--verify", action="store_true", help="report only, write no payload")
    rv.add_argument("--allowed-signers")
    rv.add_argument("--paced", action="store_true")
    rv.add_argument("--speed", type=float, default=1.0)
    rv.add_argument("--gap", choices=("skip", "zero"), default="skip")
    rv.add_argument("-v", "--verbose", action="store_true")
    rv.set_defaults(fn=cmd_recv)

    a = p.parse_args()
    try:
        return a.fn(a)
    except BrokenPipeError:
        return 0


if __name__ == "__main__":
    sys.exit(main())
