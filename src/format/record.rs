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
/// The outer code a parity record carries.
///
/// Version 0 is one row, the XOR, and rebuilds one lost record of a group.
/// Version 1 is two, P and Q, and rebuilds two. P IS VERSION 0'S ROW,
/// unchanged and first, which is why a version 1 parity record whose second
/// row was lost still rebuilds one by the arithmetic that always did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outer {
    /// The XOR of the group's bodies. Any one record.
    Xor,
    /// P and Q over GF(256). Any two.
    PQ,
}

impl Outer {
    /// How many rows of the padded width the blob holds.
    pub fn rows(self) -> usize {
        match self {
            Outer::Xor => 1,
            Outer::PQ => 2,
        }
    }

    /// The outer code of a stream at this format version.
    pub fn of_version(v: u64) -> Outer {
        if v >= 1 {
            Outer::PQ
        } else {
            Outer::Xor
        }
    }
}

pub fn encode_parity(group: &[(Header, Vec<u8>)]) -> Vec<u8> {
    encode_parity_with(group, Outer::Xor)
}

/// A parity body: the group's entries, then the outer code's rows.
///
/// Q's coefficient for the record in place `i` is `g^i`, g being the field's
/// generator -- so no two records of a group share a coefficient, and the
/// difference of any two of them is non-zero, which is the whole of what the
/// two-erasure solve needs to divide by.
pub fn encode_parity_with(group: &[(Header, Vec<u8>)], outer: Outer) -> Vec<u8> {
    let n = group.iter().map(|(_, b)| b.len()).max().unwrap_or(0);
    let mut out = Vec::with_capacity(2 + group.len() * PAR_ENTRY + n * outer.rows());
    out.extend_from_slice(&(group.len() as u16).to_le_bytes());
    for (h, _) in group {
        out.extend_from_slice(&h.seq.to_le_bytes());
        out.extend_from_slice(&h.t_us.to_le_bytes());
        out.push(h.flags);
        out.extend_from_slice(&h.plain_len.to_le_bytes());
        out.extend_from_slice(&h.body_crc.to_le_bytes());
    }
    let mut p = vec![0u8; n];
    for (_, b) in group {
        for (x, y) in p.iter_mut().zip(b) {
            *x ^= y;
        }
    }
    out.extend_from_slice(&p);
    if outer == Outer::PQ {
        let mut q = vec![0u8; n];
        for (i, (_, b)) in group.iter().enumerate() {
            let c = rs::alpha(i as i64);
            for (x, y) in q.iter_mut().zip(b) {
                *x ^= rs::mul(c, *y);
            }
        }
        out.extend_from_slice(&q);
    }
    out
}

/// The bodies of the records missing from a group, given the rows that
/// survived and where the holes are.
///
/// `n` is the padded width, `have[i]` the body of the record in place `i` when
/// it arrived and `None` when it did not, and `rows` the blob split into P
/// (and Q, at version 1). The answers come back padded to `n`; the caller
/// truncates each to its own `plain_len` and checks its CRC, because that --
/// not this arithmetic -- is what says a rebuild was right.
pub fn rebuild(have: &[Option<Vec<u8>>], rows: &[&[u8]], n: usize) -> Option<Vec<(usize, Vec<u8>)>> {
    let lost: Vec<usize> = have.iter().enumerate().filter(|(_, b)| b.is_none()).map(|(i, _)| i).collect();
    match lost.len() {
        0 => Some(Vec::new()),
        // One hole is the XOR, which is all version 0 ever needed and all
        // version 1 needs here too: P alone answers it.
        1 => {
            let p = *rows.first()?;
            let mut acc = p.to_vec();
            acc.resize(n, 0);
            for b in have.iter().flatten() {
                for (x, y) in acc.iter_mut().zip(b) {
                    *x ^= y;
                }
            }
            Some(vec![(lost[0], acc)])
        }
        // Two needs Q as well, and the solve of the two equations.
        2 => {
            let (p, q) = (*rows.first()?, *rows.get(1)?);
            let (a, b) = (lost[0], lost[1]);
            let (ga, gb) = (rs::alpha(a as i64), rs::alpha(b as i64));
            // Distinct places, so distinct powers, so a non-zero difference:
            // g has order 255 and a group is at most 16 records.
            let d = ga ^ gb;
            if d == 0 {
                return None;
            }
            let inv = rs::inverse(d);
            let mut pp = p.to_vec();
            let mut qq = q.to_vec();
            pp.resize(n, 0);
            qq.resize(n, 0);
            for (i, body) in have.iter().enumerate() {
                let Some(body) = body else { continue };
                let c = rs::alpha(i as i64);
                for (j, y) in body.iter().enumerate() {
                    pp[j] ^= *y;
                    qq[j] ^= rs::mul(c, *y);
                }
            }
            let mut da = vec![0u8; n];
            let mut db = vec![0u8; n];
            for j in 0..n {
                da[j] = rs::mul(qq[j] ^ rs::mul(gb, pp[j]), inv);
                db[j] = pp[j] ^ da[j];
            }
            Some(vec![(a, da), (b, db)])
        }
        // Three is beyond two rows, and saying so is the honest answer.
        _ => None,
    }
}


/// A parity body back into the data records' headers and the outer code's
/// rows, still joined: the caller splits them by the padded width, which is
/// the largest `plain_len` among the entries.
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

    /// A group of `n` bodies of assorted lengths, and the padded width.
    fn a_group(n: usize) -> (Vec<Vec<u8>>, Vec<(Header, Vec<u8>)>, usize) {
        let bodies: Vec<Vec<u8>> = (0..n).map(|i| noise(700 + i * 131, i as u64 + 3)).collect();
        let group: Vec<(Header, Vec<u8>)> = bodies.iter().map(|b| (header(b), b.clone())).collect();
        let width = bodies.iter().map(|b| b.len()).max().unwrap_or(0);
        (bodies, group, width)
    }

    /// The rows of a parity blob, split by the padded width the entries give.
    ///
    /// THE ROW COUNT IS NOT A FIELD. Every entry carries its plain_len, so the
    /// width is the largest of them and the number of rows is what is left
    /// over -- which is how a parity record says how many it has without being
    /// told the stream's version, and a parity record can be the first thing a
    /// reader sees after a resync.
    fn rows_of<'a>(blob: &'a [u8], hs: &[Header]) -> Vec<&'a [u8]> {
        let n = hs.iter().map(|h| h.plain_len as usize).max().unwrap_or(0);
        if n == 0 {
            return Vec::new();
        }
        blob.chunks(n).collect()
    }

    #[test]
    fn version_1_keeps_version_0s_row_and_adds_one() {
        let (_, group, n) = a_group(9);
        let v0 = encode_parity_with(&group, Outer::Xor);
        let v1 = encode_parity_with(&group, Outer::PQ);
        let (hs0, blob0) = decode_parity(&v0).unwrap();
        let (hs1, blob1) = decode_parity(&v1).unwrap();
        assert_eq!(hs0.len(), hs1.len(), "the entry table is the same table");
        assert_eq!(blob1.len(), blob0.len() + n, "one more row, of the padded width");
        // P IS VERSION 0'S ROW. Not "computed the same way" -- the same bytes.
        assert_eq!(&blob1[..n], blob0, "P is the XOR, unchanged");
        assert_eq!(rows_of(blob0, &hs0).len(), 1);
        assert_eq!(rows_of(blob1, &hs1).len(), 2);
    }

    #[test]
    fn one_hole_is_rebuilt_at_either_version() {
        let (bodies, group, n) = a_group(7);
        for outer in [Outer::Xor, Outer::PQ] {
            let par = encode_parity_with(&group, outer);
            let (hs, blob) = decode_parity(&par).unwrap();
            let rows = rows_of(blob, &hs);
            for lost in 0..bodies.len() {
                let have: Vec<Option<Vec<u8>>> = bodies
                    .iter()
                    .enumerate()
                    .map(|(i, b)| if i == lost { None } else { Some(b.clone()) })
                    .collect();
                let got = rebuild(&have, &rows, n).expect("one hole is always answerable");
                assert_eq!(got.len(), 1);
                let (at, mut body) = got.into_iter().next().unwrap();
                assert_eq!(at, lost);
                body.truncate(hs[lost].plain_len as usize);
                assert_eq!(body, bodies[lost], "{outer:?}, record {lost}");
                assert_eq!(crc32::crc32(&body), hs[lost].body_crc);
            }
        }
    }

    /// THE DONE-CONDITION OF THE PHASE, in the small: two records of a group
    /// lost, and both come back. Every pair of places in a full group of 16,
    /// because a solve that works for one pair and not another is the kind of
    /// arithmetic mistake that hides behind a single example.
    #[test]
    fn version_1_rebuilds_two_lost_records_of_a_group() {
        let (bodies, group, n) = a_group(crate::format::GROUP);
        let par = encode_parity_with(&group, Outer::PQ);
        let (hs, blob) = decode_parity(&par).unwrap();
        let rows = rows_of(blob, &hs);
        assert_eq!(rows.len(), 2);
        let mut pairs = 0;
        for a in 0..bodies.len() {
            for b in (a + 1)..bodies.len() {
                let have: Vec<Option<Vec<u8>>> = bodies
                    .iter()
                    .enumerate()
                    .map(|(i, x)| if i == a || i == b { None } else { Some(x.clone()) })
                    .collect();
                let got = rebuild(&have, &rows, n).expect("two holes, two rows");
                assert_eq!(got.len(), 2);
                for (at, mut body) in got {
                    body.truncate(hs[at].plain_len as usize);
                    assert_eq!(body, bodies[at], "pair ({a}, {b}), record {at}");
                    assert_eq!(crc32::crc32(&body), hs[at].body_crc);
                }
                pairs += 1;
            }
        }
        assert_eq!(pairs, crate::format::GROUP * (crate::format::GROUP - 1) / 2, "every pair was tried");
    }

    /// And version 0 cannot, which is the other half of the result: a battery
    /// that only ever passes is measuring the battery.
    #[test]
    fn version_0_cannot_rebuild_two() {
        let (bodies, group, n) = a_group(crate::format::GROUP);
        let par = encode_parity_with(&group, Outer::Xor);
        let (hs, blob) = decode_parity(&par).unwrap();
        let rows = rows_of(blob, &hs);
        assert_eq!(rows.len(), 1, "one row is all version 0 has");
        let have: Vec<Option<Vec<u8>>> = bodies
            .iter()
            .enumerate()
            .map(|(i, x)| if i == 3 || i == 11 { None } else { Some(x.clone()) })
            .collect();
        assert!(rebuild(&have, &rows, n).is_none(), "two holes and one row is not solvable");
    }

    #[test]
    fn three_holes_are_beyond_two_rows() {
        let (bodies, group, n) = a_group(crate::format::GROUP);
        let par = encode_parity_with(&group, Outer::PQ);
        let (hs, blob) = decode_parity(&par).unwrap();
        let rows = rows_of(blob, &hs);
        let have: Vec<Option<Vec<u8>>> = bodies
            .iter()
            .enumerate()
            .map(|(i, x)| if i < 3 { None } else { Some(x.clone()) })
            .collect();
        assert!(rebuild(&have, &rows, n).is_none(), "and it says so rather than guessing");
    }

    #[test]
    fn a_stream_version_picks_its_outer_code() {
        assert_eq!(Outer::of_version(0), Outer::Xor);
        assert_eq!(Outer::of_version(1), Outer::PQ);
        assert_eq!(Outer::Xor.rows(), 1);
        assert_eq!(Outer::PQ.rows(), 2);
    }
}
