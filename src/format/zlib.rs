// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! zlib (RFC 1950) around DEFLATE (RFC 1951), written out by hand.
//!
//! The prototype, `tools/copal-sstr.py`, compresses a record body with
//! Python's `zlib.compress(chunk, 6)` when `record --deflate` is given and
//! reads it back with `zlib.decompress`. The port has to read every body the
//! prototype wrote and write bodies the prototype can read. The obvious crate
//! for that is a download, and copal-build compiles on fleet nodes that never
//! reach the internet, so the workspace takes no dependencies and this module
//! does the job with std alone.
//!
//! The decompressor takes all three block types (stored, fixed Huffman,
//! dynamic Huffman), so it reads Python's output at any level. It trusts
//! nothing: a truncated or corrupt stream is an `Err`, never a panic, and
//! the output is capped by the caller because a few kilobytes of DEFLATE can
//! claim gigabytes. Like Python, it ignores bytes after the Adler-32 trailer.
//!
//! The compressor does what zlib does at level 6: a 32 KiB window searched
//! through hash chains, lazy matching with zlib's level-6 limits, and each
//! block written as whichever of dynamic Huffman, fixed Huffman or stored is
//! smallest. Its bytes differ from zlib's; nothing needs them not to.

use std::fmt;

/// Why a stream could not be decompressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZlibError(pub String);

impl fmt::Display for ZlibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "zlib: {}", self.0)
    }
}

impl std::error::Error for ZlibError {}

fn err<T>(msg: &str) -> Result<T, ZlibError> {
    Err(ZlibError(msg.to_string()))
}

// ---------------------------------------------------------------------------
// Tables shared by both directions (RFC 1951 section 3.2.5).

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order code-length code lengths are sent in: the ones most likely to
/// be zero go last, so a header can stop early.
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

const MAX_BITS: usize = 15;
const WSIZE: usize = 32768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;

/// Adler-32 as zlib computes it for the trailer.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    // 5552 is the most bytes that can be summed before `b` could overflow
    // a u32, so the modulo runs once per chunk instead of once per byte.
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

// ---------------------------------------------------------------------------
// Decompression.

/// A little-endian bit reader. Past the end of the input it feeds zero bits
/// and counts them in `pad`, so a Huffman lookup near the end can always peek
/// 15 bits; spending any of those invented bits is reported as truncation.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
    pad: u32,
}

impl<'a> BitReader<'a> {
    /// Top the buffer up to at least 56 bits, enough for a length and a
    /// distance with their extra bits (48 at most) without another refill.
    #[inline]
    fn refill(&mut self) -> Result<(), ZlibError> {
        if self.cnt < self.pad {
            return err("truncated stream");
        }
        if self.pos + 8 <= self.data.len() {
            let mut w = [0u8; 8];
            w.copy_from_slice(&self.data[self.pos..self.pos + 8]);
            // Loads whole bytes only; the bits above `cnt` that the shift
            // also brings in are the next bytes themselves, so OR-ing them
            // again on the following refill is harmless.
            self.buf |= u64::from_le_bytes(w) << self.cnt;
            self.pos += ((63 - self.cnt) >> 3) as usize;
            self.cnt |= 56;
        } else {
            self.buf &= if self.cnt == 0 {
                0
            } else {
                u64::MAX >> (64 - self.cnt)
            };
            while self.cnt <= 56 {
                if self.pos < self.data.len() {
                    self.buf |= (self.data[self.pos] as u64) << self.cnt;
                    self.pos += 1;
                } else {
                    self.pad += 8;
                }
                self.cnt += 8;
            }
        }
        Ok(())
    }

    /// Take `n` <= 32 bits the caller knows are buffered.
    #[inline]
    fn take(&mut self, n: u32) -> u32 {
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.cnt -= n;
        v
    }

    fn bits(&mut self, n: u32) -> Result<u32, ZlibError> {
        if self.cnt < n + self.pad {
            self.refill()?;
        }
        let v = self.take(n);
        if self.cnt < self.pad {
            return err("truncated stream");
        }
        Ok(v)
    }

    /// Drop to a byte boundary and hand the buffered whole bytes back to the
    /// input, so a stored block or the trailer can be read as plain bytes.
    fn align_and_rewind(&mut self) -> Result<(), ZlibError> {
        let drop = self.cnt % 8;
        self.take(drop);
        if self.cnt < self.pad {
            return err("truncated stream");
        }
        self.pos -= ((self.cnt - self.pad) / 8) as usize;
        self.buf = 0;
        self.cnt = 0;
        self.pad = 0;
        Ok(())
    }
}

const FAST_BITS: u32 = 10;
const FAST_MASK: u64 = (1 << FAST_BITS) - 1;

/// A canonical Huffman decoder: a 10-bit lookup table for the common short
/// codes and the counting method from zlib's puff.c for the rare long ones.
struct Huffman {
    /// `symbol << 4 | length`, or 0 when the code is longer than the table
    /// or not assigned at all.
    fast: [u16; 1 << FAST_BITS],
    count: [u16; MAX_BITS + 1],
    symbols: [u16; 288],
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, ZlibError> {
        let mut h = Huffman {
            fast: [0; 1 << FAST_BITS],
            count: [0; MAX_BITS + 1],
            symbols: [0; 288],
        };
        for &l in lengths {
            h.count[l as usize] += 1;
        }
        h.count[0] = 0;
        // An over-subscribed code has no consistent decoding. An incomplete
        // one is let through: its unassigned codes fail when met, which is
        // all a reader needs, and a lone distance code is legitimately so.
        let mut left: i32 = 1;
        for len in 1..=MAX_BITS {
            left = (left << 1) - h.count[len] as i32;
            if left < 0 {
                return err("over-subscribed Huffman code");
            }
        }
        let mut offs = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offs[len + 1] = offs[len] + h.count[len];
        }
        let mut next_code = [0u32; MAX_BITS + 2];
        for len in 2..=MAX_BITS {
            next_code[len] = (next_code[len - 1] + h.count[len - 1] as u32) << 1;
        }
        for (sym, &l) in lengths.iter().enumerate() {
            let l = l as usize;
            if l == 0 {
                continue;
            }
            h.symbols[offs[l] as usize] = sym as u16;
            offs[l] += 1;
            let code = next_code[l];
            next_code[l] += 1;
            if l as u32 <= FAST_BITS {
                let rev = reverse_bits(code, l as u32) as usize;
                let entry = ((sym as u16) << 4) | l as u16;
                let mut i = rev;
                while i < (1 << FAST_BITS) {
                    h.fast[i] = entry;
                    i += 1 << l;
                }
            }
        }
        Ok(h)
    }

    /// Decode one symbol; the reader must have been refilled.
    #[inline]
    fn decode(&self, r: &mut BitReader) -> Result<u16, ZlibError> {
        let e = self.fast[(r.buf & FAST_MASK) as usize];
        if e != 0 {
            r.take((e & 15) as u32);
            return Ok(e >> 4);
        }
        // Codes arrive most significant bit first, so walk them a bit at a
        // time against the canonical first code of each length.
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= ((r.buf >> (len - 1)) & 1) as i32;
            let count = self.count[len] as i32;
            if code - first < count {
                r.take(len as u32);
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        err("invalid Huffman code")
    }
}

fn reverse_bits(code: u32, len: u32) -> u32 {
    code.reverse_bits() >> (32 - len)
}

/// Decompress a complete zlib stream; checks the header (CM=8, FCHECK, no
/// preset dictionary) and the Adler-32 trailer. Rejects truncated or corrupt
/// input with `Err`, and output beyond `max_out` bytes likewise.
pub fn decompress(data: &[u8], max_out: usize) -> Result<Vec<u8>, ZlibError> {
    if data.len() < 2 {
        return err("truncated header");
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0f != 8 {
        return err("compression method is not deflate");
    }
    if cmf >> 4 > 7 {
        return err("window size over 32 KiB");
    }
    if ((cmf as u16) << 8 | flg as u16) % 31 != 0 {
        return err("header check bits wrong");
    }
    if flg & 0x20 != 0 {
        return err("preset dictionary required");
    }

    // A guess, not a promise: DEFLATE rarely shrinks by more than 4x on the
    // data this carries, and the cap keeps a hostile length from reserving.
    let mut out = Vec::with_capacity(max_out.min(data.len().saturating_mul(4)).min(1 << 26));
    let mut r = BitReader {
        data,
        pos: 2,
        buf: 0,
        cnt: 0,
        pad: 0,
    };

    loop {
        let last = r.bits(1)?;
        match r.bits(2)? {
            0 => {
                r.align_and_rewind()?;
                let p = r.pos;
                if data.len() < p + 4 {
                    return err("truncated stored block");
                }
                let len = u16::from_le_bytes([data[p], data[p + 1]]) as usize;
                let nlen = u16::from_le_bytes([data[p + 2], data[p + 3]]) as usize;
                if len != !nlen & 0xffff {
                    return err("stored block length check failed");
                }
                if data.len() < p + 4 + len {
                    return err("truncated stored block");
                }
                if out.len() + len > max_out {
                    return err("output exceeds limit");
                }
                out.extend_from_slice(&data[p + 4..p + 4 + len]);
                r.pos = p + 4 + len;
            }
            1 => {
                let (lit, dist) = fixed_decoders();
                inflate_block(&mut r, &mut out, &lit, &dist, max_out)?;
            }
            2 => {
                let (lit, dist) = read_dynamic_header(&mut r)?;
                inflate_block(&mut r, &mut out, &lit, &dist, max_out)?;
            }
            _ => return err("reserved block type"),
        }
        if last == 1 {
            break;
        }
    }

    r.align_and_rewind()?;
    let p = r.pos;
    if data.len() < p + 4 {
        return err("truncated Adler-32 trailer");
    }
    let want = u32::from_be_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]);
    if adler32(&out) != want {
        return err("Adler-32 mismatch");
    }
    Ok(out)
}

fn fixed_lit_lengths() -> [u8; 288] {
    let mut l = [0u8; 288];
    for (i, v) in l.iter_mut().enumerate() {
        *v = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    l
}

fn fixed_decoders() -> (Huffman, Huffman) {
    let lit = Huffman::new(&fixed_lit_lengths()).expect("fixed code is complete");
    let dist = Huffman::new(&[5u8; 30]).expect("fixed code is valid");
    (lit, dist)
}

fn read_dynamic_header(r: &mut BitReader) -> Result<(Huffman, Huffman), ZlibError> {
    let hlit = r.bits(5)? as usize + 257;
    let hdist = r.bits(5)? as usize + 1;
    let hclen = r.bits(4)? as usize + 4;
    // zlib refuses these too: symbols 286-287 and distances 30-31 can
    // never be used, so a header that sends them is not a zlib stream.
    if hlit > 286 || hdist > 30 {
        return err("too many length or distance symbols");
    }
    let mut cl_lengths = [0u8; 19];
    for &i in CL_ORDER.iter().take(hclen) {
        cl_lengths[i] = r.bits(3)? as u8;
    }
    let cl = Huffman::new(&cl_lengths)?;

    let total = hlit + hdist;
    let mut lengths = [0u8; 316];
    let mut i = 0;
    while i < total {
        r.refill()?;
        let sym = cl.decode(r)?;
        let (val, run) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                if i == 0 {
                    return err("repeat with no previous length");
                }
                (lengths[i - 1], 3 + r.take(2) as usize)
            }
            17 => (0, 3 + r.take(3) as usize),
            _ => (0, 11 + r.take(7) as usize),
        };
        if i + run > total {
            return err("code lengths overrun");
        }
        lengths[i..i + run].iter_mut().for_each(|x| *x = val);
        i += run;
    }
    if lengths[256] == 0 {
        return err("no end-of-block code");
    }
    let lit = Huffman::new(&lengths[..hlit])?;
    let dist = Huffman::new(&lengths[hlit..total])?;
    Ok((lit, dist))
}

fn inflate_block(
    r: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    max_out: usize,
) -> Result<(), ZlibError> {
    loop {
        r.refill()?;
        let sym = lit.decode(r)? as usize;
        if sym < 256 {
            if out.len() >= max_out {
                return err("output exceeds limit");
            }
            out.push(sym as u8);
            continue;
        }
        if sym == 256 {
            return Ok(());
        }
        let li = sym - 257;
        if li >= 29 {
            return err("invalid length symbol");
        }
        let len = LEN_BASE[li] as usize + r.take(LEN_EXTRA[li] as u32) as usize;
        let ds = dist.decode(r)? as usize;
        if ds >= 30 {
            return err("invalid distance symbol");
        }
        let d = DIST_BASE[ds] as usize + r.take(DIST_EXTRA[ds] as u32) as usize;
        if d > out.len() {
            return err("distance too far back");
        }
        if out.len() + len > max_out {
            return err("output exceeds limit");
        }
        let start = out.len() - d;
        if d >= len {
            out.extend_from_within(start..start + len);
        } else if d == 1 {
            let b = out[start];
            out.resize(out.len() + len, b);
        } else {
            // Overlapping copy: the match reads bytes it is itself writing.
            for k in 0..len {
                let b = out[start + k];
                out.push(b);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Compression.

// zlib's level 6 (deflate.c configuration_table): stop searching once a
// match is `NICE` long, search a quarter as hard when the previous match is
// already `GOOD`, and do not look for a better match past `MAX_LAZY`.
const GOOD: usize = 8;
const MAX_LAZY: usize = 16;
const NICE: usize = 128;
const MAX_CHAIN: usize = 128;
/// A 3-byte match this far back costs about as much as three literals.
const TOO_FAR: usize = 4096;

const HASH_BITS: u32 = 15;
/// Symbols per block, zlib's figure at the default memory level: short
/// enough that the Huffman codes follow changes in the data.
const BLOCK_SYMBOLS: usize = 16384;
const MATCH_FLAG: u32 = 1 << 31;

const fn build_len_code() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut c = 0;
    while c < 29 {
        let mut l = LEN_BASE[c] as usize;
        while l < LEN_BASE[c] as usize + (1 << LEN_EXTRA[c]) && l <= 258 {
            t[l - 3] = c as u8;
            l += 1;
        }
        c += 1;
    }
    t
}

/// Index `dist - 1` below 256, else `256 + ((dist - 1) >> 7)`, as zlib does:
/// from code 16 up every code spans a multiple of 128 distances.
const fn build_dist_code() -> [u8; 512] {
    let mut t = [0u8; 512];
    let mut c = 0;
    while c < 30 {
        let mut d = DIST_BASE[c] as usize;
        while d < DIST_BASE[c] as usize + (1 << DIST_EXTRA[c]) {
            let x = d - 1;
            if x < 256 {
                t[x] = c as u8;
            } else {
                t[256 + (x >> 7)] = c as u8;
            }
            d += 1;
        }
        c += 1;
    }
    t
}

const LEN_CODE: [u8; 256] = build_len_code();
const DIST_CODE: [u8; 512] = build_dist_code();

#[inline]
fn dist_code(dist_minus_1: usize) -> usize {
    if dist_minus_1 < 256 {
        DIST_CODE[dist_minus_1] as usize
    } else {
        DIST_CODE[256 + (dist_minus_1 >> 7)] as usize
    }
}

struct BitWriter {
    out: Vec<u8>,
    buf: u64,
    cnt: u32,
}

impl BitWriter {
    /// Append `n` <= 32 bits.
    #[inline]
    fn put(&mut self, bits: u32, n: u32) {
        self.buf |= (bits as u64) << self.cnt;
        self.cnt += n;
        if self.cnt >= 32 {
            self.out.extend_from_slice(&(self.buf as u32).to_le_bytes());
            self.buf >>= 32;
            self.cnt -= 32;
        }
    }

    fn align(&mut self) {
        while self.cnt > 0 {
            self.out.push(self.buf as u8);
            self.buf >>= 8;
            self.cnt = self.cnt.saturating_sub(8);
        }
        self.buf = 0;
    }
}

/// zlib-wrapped (RFC 1950) DEFLATE, level-6-like effort.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    // CMF 0x78: deflate, 32 KiB window. FLG 0x9c: "default" level, no
    // dictionary, check bits making the pair a multiple of 31 -- the same
    // two bytes Python writes at level 6.
    out.extend_from_slice(&[0x78, 0x9c]);
    let mut d = Deflater {
        data,
        w: BitWriter {
            out,
            buf: 0,
            cnt: 0,
        },
        head: vec![0; 1 << HASH_BITS],
        prev: vec![0; WSIZE],
        tokens: Vec::with_capacity(BLOCK_SYMBOLS),
        lit_freq: [0; 286],
        dist_freq: [0; 30],
        block_start: 0,
    };
    d.run();
    d.w.align();
    let mut out = d.w.out;
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

struct Deflater<'a> {
    data: &'a [u8],
    w: BitWriter,
    /// Most recent position per hash, and per position the one before it
    /// with the same hash. Positions are kept as u32 and may wrap on inputs
    /// past 4 GiB; a wrong candidate only costs a comparison, because every
    /// match is checked against the bytes and chains must go strictly back.
    head: Vec<u32>,
    prev: Vec<u32>,
    tokens: Vec<u32>,
    lit_freq: [u32; 286],
    dist_freq: [u32; 30],
    block_start: usize,
}

#[inline]
fn hash3(data: &[u8], i: usize) -> usize {
    let v = data[i] as u32 | (data[i + 1] as u32) << 8 | (data[i + 2] as u32) << 16;
    (v.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize
}

/// Length of the common prefix of `data[a..]` and `data[b..]`, at most `max`;
/// `b + max` must be within `data`.
#[inline]
fn match_len(data: &[u8], a: usize, b: usize, max: usize) -> usize {
    let (x, y) = (&data[a..a + max], &data[b..b + max]);
    let mut l = 0;
    while l + 8 <= max {
        let mut p = [0u8; 8];
        let mut q = [0u8; 8];
        p.copy_from_slice(&x[l..l + 8]);
        q.copy_from_slice(&y[l..l + 8]);
        let diff = u64::from_le_bytes(p) ^ u64::from_le_bytes(q);
        if diff != 0 {
            return l + (diff.trailing_zeros() / 8) as usize;
        }
        l += 8;
    }
    while l < max && x[l] == y[l] {
        l += 1;
    }
    l
}

impl<'a> Deflater<'a> {
    /// Enter position `i` in the hash chains; returns the previous head.
    #[inline]
    fn insert(&mut self, i: usize) -> u32 {
        let h = hash3(self.data, i);
        let old = self.head[h];
        self.prev[i & (WSIZE - 1)] = old;
        self.head[h] = i as u32;
        old
    }

    /// The longest match for `pos` that beats `prev_len`, as (length, distance),
    /// or (0, 0).
    fn longest_match(&self, pos: usize, mut cand: u32, prev_len: usize) -> (usize, usize) {
        let data = self.data;
        let max_len = MAX_MATCH.min(data.len() - pos);
        let mut best_len = prev_len.max(MIN_MATCH - 1);
        if best_len >= max_len {
            return (0, 0);
        }
        let mut chain = if prev_len >= GOOD {
            MAX_CHAIN >> 2
        } else {
            MAX_CHAIN
        };
        let nice = NICE.min(max_len);
        let (mut best_dist, mut last_dist) = (0, 0);
        loop {
            let dist = (pos as u32).wrapping_sub(cand) as usize;
            if dist <= last_dist || dist > WSIZE || dist > pos {
                break;
            }
            last_dist = dist;
            let c = pos - dist;
            if data[c + best_len] == data[pos + best_len] {
                let l = match_len(data, c, pos, max_len);
                if l > best_len {
                    best_len = l;
                    best_dist = dist;
                    if l >= nice {
                        break;
                    }
                }
            }
            chain -= 1;
            if chain == 0 {
                break;
            }
            cand = self.prev[c & (WSIZE - 1)];
        }
        if best_dist == 0 {
            (0, 0)
        } else {
            (best_len, best_dist)
        }
    }

    fn literal(&mut self, b: u8, covered: usize) {
        self.tokens.push(b as u32);
        self.lit_freq[b as usize] += 1;
        if self.tokens.len() >= BLOCK_SYMBOLS {
            self.flush_block(covered, false);
        }
    }

    fn matched(&mut self, len: usize, dist: usize, covered: usize) {
        self.tokens
            .push(MATCH_FLAG | ((len - 3) << 16) as u32 | (dist - 1) as u32);
        self.lit_freq[257 + LEN_CODE[len - 3] as usize] += 1;
        self.dist_freq[dist_code(dist - 1)] += 1;
        if self.tokens.len() >= BLOCK_SYMBOLS {
            self.flush_block(covered, false);
        }
    }

    /// zlib's deflate_slow: hold each match back one byte to see whether the
    /// next position starts a longer one.
    fn run(&mut self) {
        let n = self.data.len();
        let (mut prev_len, mut prev_dist, mut pending) = (0usize, 0usize, false);
        let mut i = 0;
        while i < n {
            let (mut cur_len, mut cur_dist) = (0, 0);
            if i + MIN_MATCH <= n {
                let cand = self.insert(i);
                if prev_len < MAX_LAZY {
                    let (l, d) = self.longest_match(i, cand, prev_len);
                    if !(l == MIN_MATCH && d > TOO_FAR) {
                        cur_len = l;
                        cur_dist = d;
                    }
                }
            }
            if prev_len >= MIN_MATCH && cur_len <= prev_len {
                let end = i - 1 + prev_len;
                self.matched(prev_len, prev_dist, end);
                for j in i + 1..end.min(n + 1 - MIN_MATCH) {
                    self.insert(j);
                }
                i = end;
                pending = false;
                prev_len = 0;
            } else {
                if pending {
                    self.literal(self.data[i - 1], i);
                }
                pending = true;
                prev_len = cur_len;
                prev_dist = cur_dist;
                i += 1;
            }
        }
        if pending {
            self.literal(self.data[n - 1], n);
        }
        self.flush_block(n, true);
    }

    /// Write the collected symbols covering `data[block_start..end]` as the
    /// cheapest of the three block types.
    fn flush_block(&mut self, end: usize, last: bool) {
        self.lit_freq[256] = 1;
        let lit_freq = self.lit_freq;
        let dist_freq = self.dist_freq;

        let mut extra = 0u64;
        for c in 0..29 {
            extra += lit_freq[257 + c] as u64 * LEN_EXTRA[c] as u64;
        }
        for c in 0..30 {
            extra += dist_freq[c] as u64 * DIST_EXTRA[c] as u64;
        }
        let cost = |freq: &[u32], lens: &[u8]| -> u64 {
            freq.iter()
                .zip(lens)
                .map(|(&f, &l)| f as u64 * l as u64)
                .sum()
        };

        let fixed_lit = fixed_lit_lengths();
        let fixed_dist = [5u8; 30];
        let fixed_bits = 3 + cost(&lit_freq, &fixed_lit) + cost(&dist_freq, &fixed_dist) + extra;

        // A Huffman code needs two symbols to be a complete code, and some
        // decoders reject anything less, so pad lonely alphabets with a
        // symbol that is never sent. The padding costs nothing in `cost`
        // because that uses the real counts.
        let mut lf = lit_freq;
        let mut df = dist_freq;
        force_two(&mut lf);
        force_two(&mut df);
        let lit_len = huffman_lengths(&lf, MAX_BITS);
        let dist_len = huffman_lengths(&df, MAX_BITS);
        let hlit = 257.max(lit_len.iter().rposition(|&l| l != 0).map_or(0, |p| p + 1));
        let hdist = 1.max(dist_len.iter().rposition(|&l| l != 0).map_or(0, |p| p + 1));
        let mut all = lit_len[..hlit].to_vec();
        all.extend_from_slice(&dist_len[..hdist]);
        let rle = rle_lengths(&all);
        let mut cl_freq = [0u32; 19];
        for &(s, _) in &rle {
            cl_freq[s as usize] += 1;
        }
        let mut clf = cl_freq;
        force_two(&mut clf);
        let cl_len = huffman_lengths(&clf, 7);
        let hclen = 4.max(
            CL_ORDER
                .iter()
                .rposition(|&i| cl_len[i] != 0)
                .map_or(0, |p| p + 1),
        );
        let dyn_bits = 3
            + 14
            + 3 * hclen as u64
            + cost(&cl_freq, &cl_len)
            + 2 * cl_freq[16] as u64
            + 3 * cl_freq[17] as u64
            + 7 * cl_freq[18] as u64
            + cost(&lit_freq, &lit_len)
            + cost(&dist_freq, &dist_len)
            + extra;

        let raw = end - self.block_start;
        let pieces = ((raw + 65534) / 65535).max(1) as u64;
        let stored_bits = raw as u64 * 8 + pieces * 42;

        if stored_bits <= fixed_bits.min(dyn_bits) {
            self.write_stored(end, last);
        } else if fixed_bits <= dyn_bits {
            self.w.put(last as u32, 1);
            self.w.put(1, 2);
            let (lc, dc) = (canonical_codes(&fixed_lit), canonical_codes(&fixed_dist));
            self.write_symbols(&lc, &fixed_lit, &dc, &fixed_dist);
        } else {
            let w = &mut self.w;
            w.put(last as u32, 1);
            w.put(2, 2);
            w.put((hlit - 257) as u32, 5);
            w.put((hdist - 1) as u32, 5);
            w.put((hclen - 4) as u32, 4);
            for &i in CL_ORDER.iter().take(hclen) {
                w.put(cl_len[i] as u32, 3);
            }
            let cl_code = canonical_codes(&cl_len);
            for &(s, x) in &rle {
                let s = s as usize;
                w.put(cl_code[s] as u32, cl_len[s] as u32);
                match s {
                    16 => w.put(x as u32, 2),
                    17 => w.put(x as u32, 3),
                    18 => w.put(x as u32, 7),
                    _ => {}
                }
            }
            let (lc, dc) = (canonical_codes(&lit_len), canonical_codes(&dist_len));
            self.write_symbols(&lc, &lit_len, &dc, &dist_len);
        }

        self.tokens.clear();
        self.lit_freq = [0; 286];
        self.dist_freq = [0; 30];
        self.block_start = end;
    }

    fn write_symbols(&mut self, lc: &[u16], ll: &[u8], dc: &[u16], dl: &[u8]) {
        let w = &mut self.w;
        for &t in &self.tokens {
            if t & MATCH_FLAG == 0 {
                w.put(lc[t as usize] as u32, ll[t as usize] as u32);
            } else {
                let len3 = ((t >> 16) & 0xff) as usize;
                let d1 = (t & 0xffff) as usize;
                let c = LEN_CODE[len3] as usize;
                w.put(lc[257 + c] as u32, ll[257 + c] as u32);
                w.put(
                    (len3 + 3 - LEN_BASE[c] as usize) as u32,
                    LEN_EXTRA[c] as u32,
                );
                let c = dist_code(d1);
                w.put(dc[c] as u32, dl[c] as u32);
                w.put(
                    (d1 + 1 - DIST_BASE[c] as usize) as u32,
                    DIST_EXTRA[c] as u32,
                );
            }
        }
        w.put(lc[256] as u32, ll[256] as u32);
    }

    fn write_stored(&mut self, end: usize, last: bool) {
        let block = &self.data[self.block_start..end];
        let mut chunks: Vec<&[u8]> = block.chunks(65535).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        let k = chunks.len();
        for (j, c) in chunks.into_iter().enumerate() {
            self.w.put((last && j + 1 == k) as u32, 1);
            self.w.put(0, 2);
            self.w.align();
            self.w
                .put(c.len() as u32 | ((!c.len() as u32 & 0xffff) << 16), 32);
            self.w.out.extend_from_slice(c);
        }
    }
}

fn force_two(freq: &mut [u32]) {
    let used = freq.iter().filter(|&&f| f != 0).count();
    let mut need = 2usize.saturating_sub(used);
    for f in freq.iter_mut() {
        if need == 0 {
            break;
        }
        if *f == 0 {
            *f = 1;
            need -= 1;
        }
    }
}

/// Huffman code lengths for `freq`, none longer than `limit`.
fn huffman_lengths(freq: &[u32], limit: usize) -> Vec<u8> {
    let mut lens = vec![0u8; freq.len()];
    let mut leaves: Vec<(u32, u16)> = freq
        .iter()
        .enumerate()
        .filter(|&(_, &f)| f != 0)
        .map(|(s, &f)| (f, s as u16))
        .collect();
    let n = leaves.len();
    if n == 0 {
        return lens;
    }
    if n == 1 {
        lens[leaves[0].1 as usize] = 1;
        return lens;
    }
    leaves.sort_unstable();

    // Two-queue construction: leaves in weight order, and internal nodes,
    // which are made in weight order too, so the two smallest are always
    // at the front of one queue or the other.
    let mut weight: Vec<u64> = leaves.iter().map(|&(f, _)| f as u64).collect();
    let mut parent = vec![0usize; 2 * n - 1];
    let (mut li, mut ii) = (0, n);
    for k in n..2 * n - 1 {
        let mut pick = || {
            if li < n && (ii >= k || weight[li] <= weight[ii]) {
                li += 1;
                li - 1
            } else {
                ii += 1;
                ii - 1
            }
        };
        let (a, b) = (pick(), pick());
        weight.push(weight[a] + weight[b]);
        parent[a] = k;
        parent[b] = k;
    }
    let mut depth = vec![0usize; 2 * n - 1];
    for k in (0..2 * n - 2).rev() {
        depth[k] = depth[parent[k]] + 1;
    }

    // Clip to `limit` and repair the Kraft sum, as miniz does: each pass
    // takes one code off the longest length and splits one shorter code
    // into two a bit longer, lowering the sum by exactly one unit.
    let mut count = vec![0u64; limit + 1];
    for &d in &depth[..n] {
        count[d.min(limit)] += 1;
    }
    let mut total: u64 = (1..=limit).map(|l| count[l] << (limit - l)).sum();
    while total > 1 << limit {
        count[limit] -= 1;
        for l in (1..limit).rev() {
            if count[l] != 0 {
                count[l] -= 1;
                count[l + 1] += 2;
                break;
            }
        }
        total -= 1;
    }

    // Longest codes to the rarest symbols.
    let mut j = 0;
    for l in (1..=limit).rev() {
        for _ in 0..count[l] {
            lens[leaves[j].1 as usize] = l as u8;
            j += 1;
        }
    }
    lens
}

/// Bit-reversed canonical codes, ready to be written least significant bit
/// first.
fn canonical_codes(lens: &[u8]) -> Vec<u16> {
    let mut count = [0u32; MAX_BITS + 1];
    for &l in lens {
        count[l as usize] += 1;
    }
    count[0] = 0;
    let mut next = [0u32; MAX_BITS + 1];
    let mut code = 0;
    for l in 1..=MAX_BITS {
        code = (code + count[l - 1]) << 1;
        next[l] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let c = next[l as usize];
                next[l as usize] += 1;
                reverse_bits(c, l as u32) as u16
            }
        })
        .collect()
}

/// Run-length code the concatenated literal/length and distance code
/// lengths with symbols 16 (repeat previous), 17 and 18 (runs of zeros),
/// as (symbol, extra bits value).
fn rle_lengths(lens: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lens.len() {
        let l = lens[i];
        let mut run = 1;
        while i + run < lens.len() && lens[i + run] == l {
            run += 1;
        }
        let mut r = run;
        if l == 0 {
            while r >= 11 {
                let k = r.min(138);
                out.push((18, (k - 11) as u8));
                r -= k;
            }
            if r >= 3 {
                out.push((17, (r - 3) as u8));
                r = 0;
            }
        } else {
            out.push((l, 0));
            r -= 1;
            while r >= 3 {
                let k = r.min(6);
                out.push((16, (k - 3) as u8));
                r -= k;
            }
        }
        for _ in 0..r {
            out.push((l, 0));
        }
        i += run;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Words drawn at random from a small vocabulary, with numbers and
    /// newlines: compresses roughly as a log or a script does.
    fn text_like(n: usize, seed: u64) -> Vec<u8> {
        const WORDS: &[&str] = &[
            "the",
            "stream",
            "record",
            "static",
            "copal",
            "node",
            "build",
            "error",
            "0x1f",
            "if",
            "then",
            "fi",
            "echo",
            "--deflate",
            "seq",
            "=",
            "{",
            "}",
            "return",
            "offset",
        ];
        let mut rng = XorShift(seed);
        let mut v = Vec::with_capacity(n + 16);
        while v.len() < n {
            let r = rng.next();
            match r % 11 {
                0 => v.extend_from_slice(format!("{}", r % 100_000).as_bytes()),
                1 => v.push(b'\n'),
                _ => v.extend_from_slice(WORDS[(r >> 8) as usize % WORDS.len()].as_bytes()),
            }
            v.push(b' ');
        }
        v.truncate(n);
        v
    }

    fn round_trip(data: &[u8]) -> Vec<u8> {
        let c = compress(data);
        let d = decompress(&c, data.len()).expect("round trip");
        assert!(d == data, "round trip differs, {} bytes", data.len());
        c
    }

    #[test]
    fn adler32_known_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let big = vec![0xffu8; 100_000];
        let (mut a, mut b) = (1u64, 0u64);
        for &x in &big {
            a = (a + x as u64) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(adler32(&big), ((b << 16) | a) as u32);
    }

    #[test]
    fn round_trip_empty() {
        let c = round_trip(b"");
        assert_eq!(&c[..2], &[0x78, 0x9c]);
    }

    #[test]
    fn round_trip_one_byte() {
        round_trip(b"x");
    }

    #[test]
    fn round_trip_all_byte_values() {
        let v: Vec<u8> = (0..=255u8).collect();
        round_trip(&v);
        let twice: Vec<u8> = v.iter().chain(v.iter()).copied().collect();
        round_trip(&twice);
    }

    #[test]
    fn round_trip_repetitive_text() {
        let line = b"Static Stream: a stream stopped in a file, and let go again.\n";
        let v: Vec<u8> = line.iter().copied().cycle().take(1 << 20).collect();
        let c = round_trip(&v);
        assert!(
            c.len() < v.len() / 100,
            "repetitive text compressed to {}",
            c.len()
        );
        let t = text_like(1 << 20, 7);
        let c = round_trip(&t);
        assert!(c.len() < t.len() / 2, "text compressed to {}", c.len());
    }

    #[test]
    fn round_trip_random() {
        let v = XorShift(0x9e37_79b9_7f4a_7c15).bytes(1 << 20);
        let c = round_trip(&v);
        // Stored blocks keep incompressible data within a few bytes a block.
        assert!(
            c.len() < v.len() + v.len() / 1000 + 64,
            "random grew to {}",
            c.len()
        );
    }

    #[test]
    fn round_trip_long_matches_across_blocks() {
        // Random stretches repeated from up to ~27 KiB back, inside the
        // window, between text that yields thousands of symbols, so blocks
        // of BLOCK_SYMBOLS end mid-stream and later matches reach behind them.
        let mut rng = XorShift(12345);
        let mut v = Vec::new();
        for k in 0..12 {
            let r = rng.bytes(12_000);
            v.extend_from_slice(&r);
            v.extend_from_slice(&text_like(15_000, k));
            let at = v.len();
            v.extend_from_slice(&r);
            v[at + 5_000] ^= 0x55;
            v.extend(std::iter::repeat(k as u8).take(1_000));
        }
        // A run far longer than MAX_MATCH, then a copy of the window's edge.
        v.extend(std::iter::repeat(b'q').take(100_000));
        let edge = v[v.len() - WSIZE - 1_000..v.len() - WSIZE + 1_000].to_vec();
        v.extend_from_slice(&edge);
        assert!(v.len() > 64 * 1024);
        let c = round_trip(&v);
        assert!(
            c.len() < v.len() / 2,
            "compressed to {} of {}",
            c.len(),
            v.len()
        );
    }

    #[test]
    fn round_trip_short_and_edge_lengths() {
        let mut rng = XorShift(99);
        for n in [2, 3, 4, 5, 257, 258, 259, 65535, 65536, 65537] {
            round_trip(&rng.bytes(n));
            round_trip(&vec![b'a'; n]);
            round_trip(&text_like(n, n as u64));
        }
    }

    #[test]
    fn huffman_lengths_respect_limit() {
        // Fibonacci weights make the deepest possible tree.
        let mut f = vec![0u32; 30];
        let (mut a, mut b) = (1u32, 1u32);
        for x in f.iter_mut() {
            *x = a;
            let c = a.saturating_add(b);
            a = b;
            b = c;
        }
        for limit in [7, 15] {
            let l = huffman_lengths(&f[..19.min(f.len())], limit);
            let l2 = huffman_lengths(&f, limit);
            for lens in [l, l2] {
                assert!(lens.iter().all(|&x| x as usize <= limit && x > 0));
                let kraft: u64 = lens.iter().map(|&x| 1u64 << (limit - x as usize)).sum();
                assert_eq!(kraft, 1 << limit);
            }
        }
    }

    #[test]
    fn decompress_hand_built_stored() {
        let mut s = vec![0x78, 0x01];
        // A non-final stored block, then a final one.
        s.extend_from_slice(&[0x00, 0x05, 0x00, 0xfa, 0xff]);
        s.extend_from_slice(b"hello");
        s.extend_from_slice(&[0x01, 0x06, 0x00, 0xf9, 0xff]);
        s.extend_from_slice(b" world");
        s.extend_from_slice(&adler32(b"hello world").to_be_bytes());
        assert_eq!(decompress(&s, 100).unwrap(), b"hello world");
        let mut bad = s.clone();
        bad[5] ^= 1;
        assert!(decompress(&bad, 100).is_err());
    }

    #[test]
    fn decompress_hand_built_fixed() {
        // Python's zlib.compress(..., 6) output for inputs short enough that
        // it chose fixed Huffman blocks, one of them with overlapping copies.
        let cases: [(&str, &[u8]); 3] = [
            ("789ccb48cdc9c90700062c0215", b"hello"),
            ("789c4b4cc404004fa60795", b"aaaaaaaaaaaaaaaaaaaa"),
            ("789c4b4c4a4e84a1080022d104f1", b"abcabcabcabcX"),
        ];
        for (h, want) in cases {
            let s = hex(h);
            assert_eq!((s[2] >> 1) & 3, 1, "fixed block");
            assert_eq!(decompress(&s, 1000).unwrap(), want);
        }
        // A fixed block written bit by bit: literal 'A' (0x41 -> code 0x71,
        // 8 bits), a length-3 distance-1 match (code 257, 7 bits; distance
        // code 0, 5 bits), end of block (7 zero bits).
        let mut w = BitWriter {
            out: vec![0x78, 0x01],
            buf: 0,
            cnt: 0,
        };
        w.put(1, 1);
        w.put(1, 2);
        w.put(reverse_bits(0x30 + 0x41, 8), 8);
        w.put(reverse_bits(1, 7), 7);
        w.put(0, 5);
        w.put(0, 7);
        w.align();
        let mut s = w.out;
        s.extend_from_slice(&adler32(b"AAAA").to_be_bytes());
        assert_eq!(decompress(&s, 10).unwrap(), b"AAAA");
    }

    #[test]
    fn decompress_python_dynamic_block() {
        let t = b"The dynamic block needs enough varied text: to be or not to be, that is the question; whether tis nobler in the mind to suffer. ".repeat(3);
        let s = hex(
            "78dae54ddb0d8330105bc503541da0acc1020931e404dc89e4d2c7f64d618c7ef92d8f99481f0d\
             bb4c889b4d2b944c15546b4bc633146182f3ed0fb8211256a0e697b8c1737048ed481c8dd5c574\
             c02bb31b05de13b5b8752a7a7676d1f4dbd636cf2c778c7ffeff05168588c9",
        );
        assert_eq!((s[2] >> 1) & 3, 2, "dynamic block");
        assert_eq!(decompress(&s, 1000).unwrap(), t);
    }

    #[test]
    fn rejects_truncated() {
        let v = text_like(5000, 1);
        let c = compress(&v);
        for cut in 0..c.len() {
            assert!(decompress(&c[..cut], v.len()).is_err(), "cut at {}", cut);
        }
        let s = compress(&XorShift(5).bytes(3000));
        for cut in 0..s.len() {
            assert!(
                decompress(&s[..cut], 3000).is_err(),
                "stored cut at {}",
                cut
            );
        }
    }

    #[test]
    fn rejects_bad_adler() {
        let mut c = compress(b"some data to check");
        let n = c.len();
        c[n - 1] ^= 0x01;
        assert_eq!(decompress(&c, 100), err("Adler-32 mismatch"));
    }

    #[test]
    fn rejects_bad_header() {
        let good = compress(b"header");
        let mut cm = good.clone();
        cm[0] = 0x77; // CM 7
        cm[1] = 0x01;
        assert!(decompress(&cm, 100).is_err());
        let mut fcheck = good.clone();
        fcheck[1] ^= 0x01;
        assert!(decompress(&fcheck, 100).is_err());
        // FDICT set with valid check bits.
        let mut dict = good.clone();
        dict[1] = 0xbb;
        assert_eq!((0x78u16 << 8 | 0xbb) % 31, 0);
        assert!(decompress(&dict, 100).is_err());
        let mut window = good.clone();
        window[0] = 0x88;
        window[1] = 0x1c - (0x881c % 31) as u8;
        assert!(decompress(&window, 100).is_err());
        assert!(decompress(&[], 100).is_err());
        assert!(decompress(&[0x78], 100).is_err());
    }

    #[test]
    fn rejects_output_over_limit() {
        let v = text_like(10_000, 2);
        let c = compress(&v);
        assert!(decompress(&c, v.len()).is_ok());
        assert_eq!(decompress(&c, v.len() - 1), err("output exceeds limit"));
        let r = XorShift(3).bytes(10_000);
        let c = compress(&r);
        assert_eq!(decompress(&c, 9_999), err("output exceeds limit"));
        // A bomb: 64 MiB of zeros must stop at the cap, not allocate it.
        let z = compress(&vec![0u8; 64 << 20]);
        assert_eq!(decompress(&z, 1 << 20), err("output exceeds limit"));
    }

    #[test]
    fn ignores_bytes_after_trailer_like_python() {
        let mut c = compress(b"hello");
        c.extend_from_slice(b"xx");
        assert_eq!(decompress(&c, 10).unwrap(), b"hello");
    }

    #[test]
    fn corrupt_input_never_panics() {
        let v = text_like(4000, 11);
        let c = compress(&v);
        let mut rng = XorShift(0xdead_beef);
        for _ in 0..5000 {
            let mut m = c.clone();
            for _ in 0..1 + rng.next() % 4 {
                let i = 2 + (rng.next() as usize) % (m.len() - 2);
                m[i] ^= 1 << (rng.next() % 8);
            }
            let _ = decompress(&m, 8000);
        }
        for _ in 0..2000 {
            let mut m = vec![0x78, 0x9c];
            let len = (rng.next() % 300) as usize;
            m.extend(rng.bytes(len));
            let _ = decompress(&m, 100_000);
        }
    }

    fn python(script: &str, input: &[u8]) -> Option<Vec<u8>> {
        let mut child = Command::new("python3")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut stdin = child.stdin.take()?;
        let data = input.to_vec();
        // Write from another thread so a full stdout pipe cannot deadlock us.
        let feeder = std::thread::spawn(move || stdin.write_all(&data));
        let mut out = Vec::new();
        child.stdout.take()?.read_to_end(&mut out).ok()?;
        feeder.join().ok()?.ok()?;
        if child.wait().ok()?.success() {
            Some(out)
        } else {
            None
        }
    }

    #[test]
    fn interoperates_with_python_zlib() {
        if python("pass", b"").is_none() {
            eprintln!("python3 not on PATH; skipping the zlib interop test");
            return;
        }
        let mut samples = vec![
            Vec::new(),
            b"a".to_vec(),
            (0..=255u8).collect(),
            text_like(300_000, 21),
            XorShift(77).bytes(200_000),
        ];
        let mut mixed = text_like(50_000, 5);
        mixed.extend(XorShift(8).bytes(20_000));
        mixed.extend(std::iter::repeat(b'z').take(100_000));
        mixed.extend_from_within(10_000..60_000);
        samples.push(mixed);

        for data in &samples {
            for level in [1, 6, 9] {
                let script = format!(
                    "import sys,zlib; sys.stdout.buffer.write(zlib.compress(sys.stdin.buffer.read(), {}))",
                    level
                );
                let z = python(&script, data).expect("python compress");
                let back = decompress(&z, data.len()).expect("decompress python output");
                assert!(
                    back == *data,
                    "python level {} stream, {} bytes",
                    level,
                    data.len()
                );
            }
            let ours = compress(data);
            let back = python(
                "import sys,zlib; sys.stdout.buffer.write(zlib.decompress(sys.stdin.buffer.read()))",
                &ours,
            )
            .expect("python decompress of our stream");
            assert!(
                back == *data,
                "python read our stream, {} bytes",
                data.len()
            );
        }
    }
}
