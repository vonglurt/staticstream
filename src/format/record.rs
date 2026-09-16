// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Records: the unit a static stream is made of, on disk.
//!
//! ```text
//! record = SYNC hdr32 rs32  SYNC2 hdr32 rs32  body
//! hdr32  = type u8 | flags u8 | reserved u16 | seq u64 | t_us u64
//!          | plain_len u32 | body_crc u32 | hdr_crc u32         (little-endian)
//! body   = RS(255,223) codewords of the plain body, interleaved byte by byte
//! ```
//!
//! Everything here must agree with `tools/copal-sstr.py` byte for byte: the
//! prototype's captures are read by this code, and this code's by the
//! prototype.

use crate::format::sha256::Sha256;
use crate::format::{body_len, crc32, rs, RecordType, HDR_BARE, HDR_COPY, HDR_DISK, HDR_PLAIN, K, MAX_PLAIN, NSYM, SYNC, SYNC2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub typ: RecordType,
    pub flags: u8,
    pub seq: u64,
    /// Microseconds from the start of the capture to this record's first byte.
    pub t_us: u64,
    pub plain_len: u32,
    pub body_crc: u32,
}

impl Header {
    /// The 28 bytes the header's CRC, the record hash and the prototype's
    /// `struct.pack("<BBHQQII", ...)` all cover.
    pub fn bare(&self) -> [u8; HDR_BARE] {
        let mut b = [0u8; HDR_BARE];
        b[0] = self.typ.byte();
        b[1] = self.flags;
        // b[2..4] reserved, zero
        b[4..12].copy_from_slice(&self.seq.to_le_bytes());
        b[12..20].copy_from_slice(&self.t_us.to_le_bytes());
        b[20..24].copy_from_slice(&self.plain_len.to_le_bytes());
        b[24..28].copy_from_slice(&self.body_crc.to_le_bytes());
        b
    }

    fn from_bare(b: &[u8]) -> Option<Header> {
        let typ = RecordType::from_byte(b[0])?;
        let le32 = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let le64 = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let plain_len = le32(20);
        if plain_len as usize > MAX_PLAIN {
            return None;
        }
        Some(Header { typ, flags: b[1], seq: le64(4), t_us: le64(12), plain_len, body_crc: le32(24) })
    }

    /// Bytes the whole record takes on disk.
    pub fn disk_len(&self) -> usize {
        HDR_DISK + body_len(self.plain_len as usize)
    }
}

/// A record, ready to write: both header copies and the protected body.
pub fn encode(h: &Header, body: &[u8]) -> Vec<u8> {
    // The header is one 64-byte codeword and is not interleaved, so a burst
    // longer than 32 bytes -- one lost armor line is 45 -- would take all of
    // it. Two copies: such a burst leaves at least one repairable.
    let bare = h.bare();
    let mut copy = Vec::with_capacity(HDR_PLAIN + NSYM);
    copy.extend_from_slice(&bare);
    copy.extend_from_slice(&crc32::crc32(&bare).to_le_bytes());
    let parity = rs::parity(&copy);
    copy.extend_from_slice(&parity);
    let mut out = Vec::with_capacity(HDR_DISK + body_len(body.len()));
    out.extend_from_slice(&SYNC);
    out.extend_from_slice(&copy);
    out.extend_from_slice(&SYNC2);
    out.extend_from_slice(&copy);
    out.extend_from_slice(&fec_encode(body));
    out
}

/// One header copy: the 64 bytes after a sync marker, which need not have
/// survived, with the positions known to be lost. Returns the header and the
/// number of bytes corrected.
pub fn decode_header(b: &[u8], erased: Option<&[u8]>) -> Option<(Header, usize)> {
    let b = &b[..HDR_PLAIN + NSYM];
    let crc_ok = |p: &[u8]| crc32::crc32(&p[..HDR_BARE]) == u32::from_le_bytes(p[HDR_BARE..HDR_PLAIN].try_into().unwrap());
    if crc_ok(b) {
        return Header::from_bare(b).map(|h| (h, 0));
    }
    let pos: Vec<usize> = erased.map(|m| m.iter().enumerate().filter(|(_, &v)| v != 0).map(|(i, _)| i).collect()).unwrap_or_default();
    let good = rs::correct(b, &pos).ok()?;
    if !crc_ok(&good) {
        return None;
    }
    let fixed = good.iter().zip(b).filter(|(x, y)| x != y).count();
    Header::from_bare(&good).map(|h| (h, fixed))
}

/// The header at the start of `rec` (at least `HDR_DISK` bytes): from either
/// copy, or from the two merged byte by byte where each lost what the other kept.
pub fn decode_record_header(rec: &[u8], erased: Option<&[u8]>) -> Option<(Header, usize)> {
    let n = HDR_PLAIN + NSYM;
    let (a0, b0) = (SYNC.len(), HDR_COPY + SYNC2.len());
    let (a, b) = (&rec[a0..a0 + n], &rec[b0..b0 + n]);
    let (ma, mb) = (erased.map(|m| &m[a0..a0 + n]), erased.map(|m| &m[b0..b0 + n]));
    let ma = ma.filter(|m| m.iter().any(|&v| v != 0));
    let mb = mb.filter(|m| m.iter().any(|&v| v != 0));
    if let Some(found) = decode_header(a, ma).or_else(|| decode_header(b, mb)) {
        return Some(found);
    }
    if ma.is_none() && mb.is_none() {
        return None;
    }
    let mut merged = a.to_vec();
    let mut mask = vec![0u8; n];
    for i in 0..n {
        let ea = ma.map_or(false, |m| m[i] != 0);
        let eb = mb.map_or(false, |m| m[i] != 0);
        if ea && !eb {
            merged[i] = b[i];
        } else if ea && eb {
            mask[i] = 1;
        }
    }
    let any = mask.iter().any(|&v| v != 0);
    decode_header(&merged, if any { Some(&mask) } else { None })
}

/// A plain body as interleaved codewords: full codewords of 255 bytes, a last
/// one shortened, then all first bytes, all second bytes, and so on, so a
/// burst of B bytes lands as about B / codewords bytes in each.
pub fn fec_encode(plain: &[u8]) -> Vec<u8> {
    let full = plain.len() / K;
    let mut f = Vec::with_capacity(full * 255);
    for piece in plain[..full * K].chunks(K) {
        f.extend_from_slice(piece);
        f.extend_from_slice(&rs::parity(piece));
    }
    let mut last = Vec::new();
    if plain.len() % K > 0 {
        let piece = &plain[full * K..];
        last.extend_from_slice(piece);
        last.extend_from_slice(&rs::parity(piece));
    }
    if full == 0 {
        return last;
    }
    let mut out = Vec::with_capacity(f.len() + last.len());
    for j in 0..255 {
        for c in 0..full {
            out.push(f[c * 255 + j]);
        }
        if j < last.len() {
            out.push(last[j]);
        }
    }
    out
}

/// The inverse of the interleave: the full codewords end to end, and the last.
fn deinterleave(body: &[u8], full: usize, lastlen: usize) -> (Vec<u8>, Vec<u8>) {
    if full == 0 {
        return (Vec::new(), body.to_vec());
    }
    let mut f = vec![0u8; full * 255];
    let mut last = vec![0u8; lastlen];
    let mut p = 0;
    for j in 0..255 {
        for c in 0..full {
            f[c * 255 + j] = body[p];
            p += 1;
        }
        if j < lastlen {
            last[j] = body[p];
            p += 1;
        }
    }
    (f, last)
}

/// A received body back to its plain bytes. `erased` marks, byte for byte,
/// what is known lost. Returns the plain body, or None when it cannot be
/// rebuilt, and the number of bytes corrected either way.
pub fn fec_decode(body: &[u8], plain_len: usize, crc: u32, erased: Option<&[u8]>) -> (Option<Vec<u8>>, usize) {
    let full = plain_len / K;
    let rest = plain_len % K;
    let lastlen = if rest > 0 { rest + NSYM } else { 0 };
    let (f, last) = deinterleave(body, full, lastlen);
    let mut cws: Vec<&[u8]> = f.chunks(255).collect();
    if lastlen > 0 {
        cws.push(&last);
    }
    let mut plain = Vec::with_capacity(plain_len);
    for c in &cws {
        plain.extend_from_slice(&c[..c.len() - NSYM]);
    }
    if crc32::crc32(&plain) == crc {
        return (Some(plain), 0);
    }
    let masks = erased.map(|m| deinterleave(m, full, lastlen));
    let mut out = Vec::with_capacity(plain_len);
    let mut fixed = 0;
    for (i, c) in cws.iter().enumerate() {
        let pos: Vec<usize> = match &masks {
            Some((mf, ml)) => {
                let m = if i < full { &mf[i * 255..(i + 1) * 255] } else { &ml[..] };
                m.iter().enumerate().filter(|(_, &v)| v != 0).map(|(j, _)| j).collect()
            }
            None => Vec::new(),
        };
        let data = &c[..c.len() - NSYM];
        if pos.is_empty() && rs::parity(data)[..] == c[c.len() - NSYM..] {
            out.extend_from_slice(data);
            continue;
        }
        match rs::correct(c, &pos) {
            Ok(good) => {
                fixed += good.iter().zip(c.iter()).filter(|(x, y)| x != y).count();
                out.extend_from_slice(&good[..good.len() - NSYM]);
            }
            Err(_) => return (None, fixed),
        }
    }
    let ok = crc32::crc32(&out) == crc;
    (if ok { Some(out) } else { None }, fixed)
}

/// SHA-256 of a record's 28 header bytes and its plain body: a checkpoint entry.
pub fn record_hash(h: &Header, plain: &[u8]) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(&h.bare());
    s.update(plain);
    s.finish()
}

/// Where a checkpoint chain starts.
pub fn genesis(stream_id: &[u8]) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(b"sstr-v0 genesis");
    s.update(stream_id);
    s.finish()
}

/// A checkpoint's digest: the previous digest, then each entry's seq and hash.
pub fn checkpoint_digest(prev: &[u8], entries: &[(u64, [u8; 32])]) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(b"sstr-v0 checkpoint");
    s.update(prev);
    for (seq, h) in entries {
        s.update(&seq.to_le_bytes());
        s.update(h);
    }
    s.finish()
}

/// What a checkpoint's signature is over: `ssh-keygen -Y sign` reads this.
pub fn signed_message(stream_id: &[u8], digest: &[u8]) -> Vec<u8> {
    let mut m = b"sstr-v0 ".to_vec();
    m.extend_from_slice(stream_id);
    m.extend_from_slice(digest);
    m
}

const PAR_ENTRY: usize = 25; // seq u64, t_us u64, flags u8, plain_len u32, body_crc u32

/// A parity body: the count, each data record's header fields, and the XOR of
/// their plain bodies, each padded to the longest.
pub fn encode_parity(group: &[(Header, Vec<u8>)]) -> Vec<u8> {
    let n = group.iter().map(|(_, b)| b.len()).max().unwrap_or(0);
    let mut out = Vec::with_capacity(2 + group.len() * PAR_ENTRY + n);
    out.extend_from_slice(&(group.len() as u16).to_le_bytes());
    for (h, _) in group {
        out.extend_from_slice(&h.seq.to_le_bytes());
        out.extend_from_slice(&h.t_us.to_le_bytes());
        out.push(h.flags);
        out.extend_from_slice(&h.plain_len.to_le_bytes());
        out.extend_from_slice(&h.body_crc.to_le_bytes());
    }
    let mut blob = vec![0u8; n];
    for (_, b) in group {
        for (x, y) in blob.iter_mut().zip(b) {
            *x ^= y;
        }
    }
    out.extend_from_slice(&blob);
    out
}

/// A parity body back into the data records' headers and the XOR blob.
pub fn decode_parity(plain: &[u8]) -> Option<(Vec<Header>, &[u8])> {
    if plain.len() < 2 {
        return None;
    }
    let count = u16::from_le_bytes([plain[0], plain[1]]) as usize;
    let start = 2 + count * PAR_ENTRY;
    if plain.len() < start {
        return None;
    }
    let mut hs = Vec::with_capacity(count);
    for i in 0..count {
        let e = &plain[2 + i * PAR_ENTRY..2 + (i + 1) * PAR_ENTRY];
        hs.push(Header {
            typ: RecordType::Data,
            seq: u64::from_le_bytes(e[0..8].try_into().unwrap()),
            t_us: u64::from_le_bytes(e[8..16].try_into().unwrap()),
            flags: e[16],
            plain_len: u32::from_le_bytes(e[17..21].try_into().unwrap()),
            body_crc: u32::from_le_bytes(e[21..25].try_into().unwrap()),
        });
    }
    Some((hs, &plain[start..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(plain: &[u8]) -> Header {
        Header { typ: RecordType::Data, flags: 0, seq: 7, t_us: 123_456, plain_len: plain.len() as u32, body_crc: crc32::crc32(plain) }
    }

    fn noise(n: usize, seed: u64) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s as u8
            })
            .collect()
    }

    #[test]
    fn a_record_is_as_long_as_the_layout_says() {
        for len in [0, 1, 222, 223, 224, 4136, 65536] {
            let p = noise(len, len as u64 + 1);
            assert_eq!(encode(&header(&p), &p).len(), header(&p).disk_len(), "{len}");
        }
    }

    #[test]
    fn header_round_trip_and_repair() {
        let p = noise(5000, 3);
        let h = header(&p);
        let mut rec = encode(&h, &p);
        assert_eq!(decode_record_header(&rec, None), Some((h, 0)));
        // Ruin all of the first copy's data: the second copy answers.
        for b in &mut rec[4..36] {
            *b = 0xAA;
        }
        assert_eq!(decode_record_header(&rec, None).map(|x| x.0), Some(h));
        // Ruin 45 bytes across both copies, marked as erased: merged, it decodes.
        let mut rec = encode(&h, &p);
        let mut mask = vec![0u8; rec.len()];
        for i in 50..95 {
            rec[i] = 0;
            mask[i] = 1;
        }
        assert_eq!(decode_record_header(&rec, Some(&mask)).map(|x| x.0), Some(h));
    }

    #[test]
    fn body_survives_a_burst_and_erasures() {
        let p = noise(65536, 9);
        let body = fec_encode(&p);
        let mut hurt = body.clone();
        for b in &mut hurt[2000..6000] {
            *b ^= 0x5a;
        }
        let (out, fixed) = fec_decode(&hurt, p.len(), crc32::crc32(&p), None);
        assert!(out.as_deref() == Some(&p[..]), "the burst was not repaired");
        assert_eq!(fixed, 4000);
        let mut mask = vec![0u8; body.len()];
        let mut lost = body.clone();
        for i in 10_000..19_000 {
            lost[i] = 0;
            mask[i] = 1;
        }
        let (out, _) = fec_decode(&lost, p.len(), crc32::crc32(&p), Some(&mask));
        assert!(out.as_deref() == Some(&p[..]), "9,000 erased bytes were not rebuilt");
    }

    #[test]
    fn parity_rebuilds_one_record() {
        let bodies: Vec<Vec<u8>> = (0..5).map(|i| noise(1000 + i * 77, i as u64 + 20)).collect();
        let group: Vec<(Header, Vec<u8>)> = bodies.iter().map(|b| (header(b), b.clone())).collect();
        let par = encode_parity(&group);
        let (hs, blob) = decode_parity(&par).unwrap();
        assert_eq!(hs.len(), 5);
        let mut acc = blob.to_vec();
        for (i, b) in bodies.iter().enumerate() {
            if i != 2 {
                for (x, y) in acc.iter_mut().zip(b) {
                    *x ^= y;
                }
            }
        }
        acc.truncate(hs[2].plain_len as usize);
        assert_eq!(acc, bodies[2]);
        assert_eq!(crc32::crc32(&acc), hs[2].body_crc);
    }
}
