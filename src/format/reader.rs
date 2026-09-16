// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The reader: records in, the stream out -- repairing what it can, rebuilding
//! what parity allows, and checking what remains against signed checkpoints.
//!
//! This is the prototype's `Reader`, ported step for step, and its report is
//! the prototype's report, word for word: the two are compared by running both
//! over the same damaged files, and a difference in wording would hide a
//! difference in behaviour.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::format::json::{self, Value};
use crate::format::record::{self, Header};
use crate::format::sha256::{hex, unhex, Sha256};
use crate::format::{body_len, crc32, zlib, RecordType, FLAG_DEFLATE, HDR_COPY, HDR_DISK, MAGIC, SIGNING_NAMESPACE, SYNC, SYNC2};

/// Byte ranges of the stream known to be lost, in the order they were found.
#[derive(Default, Debug, Clone)]
pub struct Erasures {
    starts: Vec<u64>,
    ends: Vec<u64>,
}

impl Erasures {
    pub fn new() -> Erasures {
        Erasures::default()
    }

    pub fn from_ranges(ranges: &[(u64, u64)]) -> Erasures {
        let mut e = Erasures::new();
        for &(s, t) in ranges {
            e.add(s, t);
        }
        e
    }

    pub fn add(&mut self, start: u64, end: u64) {
        if end > start {
            self.starts.push(start);
            self.ends.push(end);
        }
    }

    pub fn ranges(&self) -> Vec<(u64, u64)> {
        self.starts.iter().copied().zip(self.ends.iter().copied()).collect()
    }

    /// One byte per byte of [start, end): 1 where it is known lost. None when
    /// nothing in the range is.
    pub fn mask(&self, start: u64, end: u64) -> Option<Vec<u8>> {
        if self.starts.is_empty() {
            return None;
        }
        let mut i = self.starts.partition_point(|&s| s <= start).saturating_sub(1);
        let mut m: Option<Vec<u8>> = None;
        while i < self.starts.len() && self.starts[i] < end {
            let s = self.starts[i].max(start);
            let e = self.ends[i].min(end);
            if e > s {
                let v = m.get_or_insert_with(|| vec![0u8; (end - start) as usize]);
                for b in &mut v[(s - start) as usize..(e - start) as usize] {
                    *b = 1;
                }
            }
            i += 1;
        }
        m
    }
}

/// A byte stream with a window of look-ahead: a file, a pipe, a growing file
/// followed as it is written, or an armored tty turned back into bytes.
pub struct Source<R: Read> {
    inner: R,
    buf: Vec<u8>,
    base: u64,
    pub follow: bool,
    pub done: bool,
    pub erasures: Rc<RefCell<Erasures>>,
}

impl<R: Read> Source<R> {
    pub fn new(inner: R, follow: bool, erasures: Rc<RefCell<Erasures>>) -> Source<R> {
        Source { inner, buf: Vec::new(), base: 0, follow, done: false, erasures }
    }

    /// What this source reads from.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    fn end(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    /// Have bytes up to `upto` in hand; false when the stream ended first.
    fn fill(&mut self, upto: u64) -> bool {
        while self.end() < upto {
            let want = ((upto - self.end()) as usize).clamp(1 << 16, 1 << 22);
            let old = self.buf.len();
            self.buf.resize(old + want, 0);
            let n = loop {
                match self.inner.read(&mut self.buf[old..]) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break 0,
                }
            };
            self.buf.truncate(old + n);
            if n == 0 {
                if self.follow && !self.done {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                return false;
            }
        }
        true
    }

    fn get(&self, start: u64, end: u64) -> &[u8] {
        &self.buf[(start - self.base) as usize..(end - self.base) as usize]
    }

    fn drop_before(&mut self, upto: u64) {
        if upto > self.base {
            let cut = ((upto - self.base) as usize).min(self.buf.len());
            self.buf.drain(..cut);
            self.base += cut as u64;
        }
    }

    fn find(&self, pat: &[u8], from: u64) -> Option<u64> {
        let start = from.saturating_sub(self.base) as usize;
        if start >= self.buf.len() {
            return None;
        }
        self.buf[start..].windows(pat.len()).position(|w| w == pat).map(|i| self.base + (start + i) as u64)
    }

    fn mask(&self, start: u64, end: u64) -> Option<Vec<u8>> {
        self.erasures.borrow().mask(start, end)
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Write zeros for a lost data record of known length, instead of nothing.
    pub gap_zero: bool,
    /// At the pace of capture.
    pub paced: bool,
    pub speed: f64,
    /// Seconds into the capture to start writing from.
    pub start: f64,
    pub allowed_signers: Option<PathBuf>,
    pub verbose: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { gap_zero: false, paced: false, speed: 1.0, start: 0.0, allowed_signers: None, verbose: false }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub records: u64,
    pub data: u64,
    pub parity: u64,
    pub checkpoints: u64,
    pub duplicates: u64,
    pub bytes: u64,
    pub header_fixed: u64,
    pub body_fixed: u64,
    pub bodies_repaired: u64,
    pub recovered: u64,
    pub data_lost: u64,
    pub unknown_lost: u64,
    pub parity_lost: u64,
    pub resyncs: u64,
    pub skipped: u64,
    pub ck_ok: u64,
    pub ck_bad: u64,
    pub ck_lost: u64,
    pub chain_gaps: u64,
    pub verified: u64,
    pub mismatched: u64,
    pub missing: u64,
    pub sig_good: u64,
    pub sig_bad: u64,
    pub sig_unverifiable: u64,
    pub truncated: bool,
}

pub struct Reader<R: Read> {
    pub src: Source<R>,
    out: Option<Box<dyn Write>>,
    opts: Options,
    pub meta: Option<Value>,
    pub end_rec: Option<Value>,
    stream_id: Option<Vec<u8>>,
    prev: Option<Vec<u8>>,
    pending: HashMap<u64, (Header, Option<Vec<u8>>)>,
    nond: HashSet<u64>,
    lengths: HashMap<u64, u32>,
    hashes: HashMap<u64, [u8; 32]>,
    // Bodies of good data records, kept after they are played until the parity
    // record of their group has been seen: the one that rebuilds a lost record
    // needs every other record of the group.
    bodies: HashMap<u64, Vec<u8>>,
    next_seq: u64,
    max_seq: i64,
    hold: i64,
    wall0: Option<Instant>,
    t0: u64,
    payload: Sha256,
    pub trust: Option<String>,
    pub last_signed: i64,
    pub s: Stats,
}

/// The most a deflated record may expand to: well past any chunk size.
const MAX_INFLATE: usize = 1 << 26;

impl<R: Read> Reader<R> {
    pub fn new(src: Source<R>, out: Option<Box<dyn Write>>, opts: Options) -> Reader<R> {
        Reader {
            src,
            out,
            opts,
            meta: None,
            end_rec: None,
            stream_id: None,
            prev: None,
            pending: HashMap::new(),
            nond: HashSet::new(),
            lengths: HashMap::new(),
            hashes: HashMap::new(),
            bodies: HashMap::new(),
            next_seq: 0,
            max_seq: -1,
            hold: 40,
            wall0: None,
            t0: 0,
            payload: Sha256::new(),
            trust: None,
            last_signed: -1,
            s: Stats::default(),
        }
    }

    fn log(&self, msg: impl AsRef<str>) {
        if self.opts.verbose {
            eprintln!("sstr: {}", msg.as_ref());
        }
    }

    /// Read to the end: every record found, every payload byte written.
    pub fn run(&mut self) -> io::Result<()> {
        let mut pos = MAGIC.len() as u64;
        if !self.src.fill(pos) || self.src.get(0, pos) != MAGIC {
            self.log("no file signature; scanning");
            pos = 0;
        }
        while let Some((h, fixed, at)) = self.next_header(pos) {
            let blen = body_len(h.plain_len as usize) as u64;
            let b0 = at + HDR_DISK as u64;
            if !self.src.fill(b0 + blen) {
                self.s.truncated = true;
                self.log(format!("truncated in record {}", h.seq));
                break;
            }
            let mask = self.src.mask(b0, b0 + blen);
            let (plain, body_fixed) = record::fec_decode(self.src.get(b0, b0 + blen), h.plain_len as usize, h.body_crc, mask.as_deref());
            self.s.header_fixed += fixed as u64;
            if body_fixed > 0 {
                self.s.body_fixed += body_fixed as u64;
                self.s.bodies_repaired += 1;
            }
            self.handle(h, plain)?;
            pos = b0 + blen;
            self.src.drop_before(pos);
        }
        self.release(true)
    }

    fn try_header(&self, at: u64) -> Option<(Header, usize)> {
        let rec = self.src.get(at, at + HDR_DISK as u64);
        let mask = self.src.mask(at, at + HDR_DISK as u64);
        record::decode_record_header(rec, mask.as_deref())
    }

    fn next_header(&mut self, pos: u64) -> Option<(Header, usize, u64)> {
        if self.src.fill(pos + HDR_DISK as u64) {
            if let Some((h, fixed)) = self.try_header(pos) {
                return Some((h, fixed, pos));
            }
        }
        let mut p = pos + 1;
        loop {
            let more = self.src.fill(p + (1 << 20));
            let ia = self.src.find(&SYNC, p);
            let ib = self.src.find(&SYNC2, p);
            let (at, p_next) = match (ia, ib) {
                (None, None) => {
                    if !more {
                        self.s.skipped += self.src.end().saturating_sub(pos);
                        return None;
                    }
                    p = self.src.end() - SYNC.len() as u64;
                    continue;
                }
                (a, Some(b)) if a.map_or(true, |a| b < a) => (b.checked_sub(HDR_COPY as u64), b + 1),
                (Some(a), _) => (Some(a), a + 1),
                (None, Some(_)) => unreachable!(),
            };
            let at = match at {
                Some(at) if at >= pos => at,
                _ => {
                    p = p_next;
                    continue;
                }
            };
            if !self.src.fill(at + HDR_DISK as u64) {
                self.s.skipped += self.src.end().saturating_sub(pos);
                return None;
            }
            if let Some((h, fixed)) = self.try_header(at) {
                self.s.resyncs += 1;
                self.s.skipped += at - pos;
                self.log(format!("resync at byte {}, record {}, {} bytes skipped", at, h.seq, at - pos));
                return Some((h, fixed, at));
            }
            p = p_next;
        }
    }

    fn handle(&mut self, h: Header, plain: Option<Vec<u8>>) -> io::Result<()> {
        let s = h.seq;
        let dup = s < self.next_seq
            || self.pending.get(&s).map_or(false, |(_, p)| p.is_some())
            || (self.nond.contains(&s) && h.typ != RecordType::Data);
        if dup {
            self.s.duplicates += 1;
            return Ok(());
        }
        self.s.records += 1;
        self.max_seq = self.max_seq.max(s as i64);
        if h.typ == RecordType::Data {
            self.s.data += 1;
            if let Some(p) = &plain {
                self.hashes.insert(s, record::record_hash(&h, p));
                self.bodies.insert(s, p.clone());
                if self.bodies.len() as i64 > 8 * self.hold {
                    let floor = s as i64 - 4 * self.hold;
                    self.bodies.retain(|&k, _| k as i64 >= floor);
                }
            }
            self.pending.insert(s, (h, plain));
        } else {
            let Some(plain) = plain else {
                match h.typ {
                    RecordType::Parity => self.s.parity_lost += 1,
                    RecordType::Check => self.s.ck_lost += 1,
                    _ => {}
                }
                self.log(format!("record {} ({}) body lost", s, h.typ.byte() as char));
                self.s.records -= 1; // its repeat may still arrive
                return Ok(());
            };
            self.nond.insert(s);
            match h.typ {
                RecordType::Head => {
                    if let Ok(meta) = json::parse_bytes(&plain) {
                        self.stream_id = meta.get("stream_id").and_then(|v| v.as_str()).and_then(unhex);
                        self.prev = self.stream_id.as_ref().map(|id| record::genesis(id).to_vec());
                        self.meta = Some(meta);
                    }
                    self.hashes.insert(s, record::record_hash(&h, &plain));
                }
                RecordType::Parity => {
                    self.s.parity += 1;
                    self.apply_parity(&plain);
                }
                RecordType::Check => self.check(&plain),
                RecordType::End => {
                    self.end_rec = json::parse_bytes(&plain).ok();
                    self.hashes.insert(s, record::record_hash(&h, &plain));
                }
                RecordType::Data => unreachable!(),
            }
        }
        self.release(false)
    }

    fn apply_parity(&mut self, plain: &[u8]) {
        let Some((entries, blob)) = record::decode_parity(plain) else {
            return;
        };
        for e in &entries {
            self.lengths.insert(e.seq, e.plain_len);
        }
        // THE ROWS ARE COUNTED, NOT LOOKED UP. Every entry carries its
        // plain_len, so the padded width is the largest of them and the number
        // of rows is what the blob has room for: one at version 0, two at
        // version 1. A parity record therefore answers for itself, which
        // matters because after a resync it can be the first record a reader
        // meets -- before any header, and so before any version.
        let n = entries.iter().map(|e| e.plain_len as usize).max().unwrap_or(0);
        if n == 0 {
            return;
        }
        let rows: Vec<&[u8]> = blob.chunks(n).collect();

        // The group in its own order, which is the order Q's coefficients are
        // in: the entry table is written in it and read back in it.
        let have: Vec<Option<Vec<u8>>> =
            entries.iter().map(|e| self.bodies.get(&e.seq).cloned()).collect();
        let Some(fixed) = record::rebuild(&have, &rows, n) else {
            // More holes than rows. The good bodies go, because nothing else
            // will come for this group.
            for e in &entries {
                self.bodies.remove(&e.seq);
            }
            return;
        };
        for e in &entries {
            self.bodies.remove(&e.seq);
        }
        for (at, mut acc) in fixed {
            let lost = entries[at];
            acc.truncate(lost.plain_len as usize);
            // THE CRC IS WHAT SAYS A REBUILD WAS RIGHT, not the arithmetic.
            // A group with two holes and a third record quietly corrupt would
            // solve to two wrong bodies, and this is where that stops.
            if acc.len() != lost.plain_len as usize
                || crc32::crc32(&acc) != lost.body_crc
                || lost.seq < self.next_seq
            {
                continue;
            }
            self.hashes.insert(lost.seq, record::record_hash(&lost, &acc));
            self.pending.insert(lost.seq, (lost, Some(acc)));
            self.s.recovered += 1;
            self.max_seq = self.max_seq.max(lost.seq as i64);
            self.log(format!("record {} rebuilt from parity", lost.seq));
        }
    }

    fn check(&mut self, plain: &[u8]) {
        let Ok(c) = json::parse_bytes(plain) else {
            return;
        };
        self.s.checkpoints += 1;
        if self.meta.is_none() {
            if let Some(stream) = c.get("stream").filter(|v| matches!(v, Value::Obj(_))) {
                self.stream_id = stream.get("stream_id").and_then(|v| v.as_str()).and_then(unhex);
                self.meta = Some(stream.clone());
                self.log("stream header taken from a checkpoint");
            }
        }
        let prev = c.get("prev").and_then(|v| v.as_str()).and_then(unhex).unwrap_or_default();
        let entries: Vec<(u64, [u8; 32])> = c
            .get("entries")
            .and_then(|v| v.as_array())
            .unwrap_or(&[])
            .iter()
            .filter_map(|e| {
                let a = e.as_array()?;
                let h: [u8; 32] = unhex(a.get(1)?.as_str()?)?.try_into().ok()?;
                Some((a.first()?.as_u64()?, h))
            })
            .collect();
        let d = record::checkpoint_digest(&prev, &entries);
        if c.get("digest").and_then(|v| v.as_str()) != Some(hex(&d).as_str()) {
            self.s.ck_bad += 1;
            self.log("checkpoint digest does not match its entries");
            return;
        }
        if let Some(p) = &self.prev {
            if *p != prev {
                self.s.chain_gaps += 1;
                self.log("checkpoint does not follow the last one seen: one was lost");
            }
        }
        self.prev = Some(d.to_vec());
        let mut good_sig = false;
        if let Some(sig) = c.get("sig").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            match self.verify_sig(&d, sig) {
                None => self.s.sig_unverifiable += 1,
                Some(true) => {
                    self.s.sig_good += 1;
                    good_sig = true;
                }
                Some(false) => self.s.sig_bad += 1,
            }
        }
        let mut ok = true;
        for (s, h) in &entries {
            match self.hashes.remove(s) {
                None => self.s.missing += 1,
                Some(mine) if mine == *h => self.s.verified += 1,
                Some(_) => {
                    self.s.mismatched += 1;
                    ok = false;
                    self.log(format!("record {} does not match its checkpoint entry", s));
                }
            }
        }
        if ok {
            self.s.ck_ok += 1;
        } else {
            self.s.ck_bad += 1;
        }
        if good_sig {
            if let Some((last, _)) = entries.last() {
                self.last_signed = self.last_signed.max(*last as i64);
            }
        }
        if self.end_rec.is_some() {
            self.src.done = true;
        }
    }

    /// Some(true) for a good signature by the key in the stream header,
    /// Some(false) for a bad one, None when no key is known yet.
    fn verify_sig(&mut self, digest: &[u8], sig: &str) -> Option<bool> {
        let key = self.meta.as_ref()?.get("key")?.as_str()?.to_string();
        let id = self.stream_id.clone()?;
        let dir = TempDir::new().ok()?;
        let sigf = dir.path.join("sig");
        let allowed = dir.path.join("allowed");
        std::fs::write(&sigf, sig).ok()?;
        std::fs::write(&allowed, format!("sstr namespaces=\"{}\" {}\n", SIGNING_NAMESPACE, key)).ok()?;
        let good = run_with_input(
            Command::new("ssh-keygen").arg("-Y").arg("verify").arg("-f").arg(&allowed).args(["-I", "sstr", "-n", SIGNING_NAMESPACE, "-s"]).arg(&sigf),
            &record::signed_message(&id, digest),
        )
        .map(|o| o.status.success())
        .unwrap_or(false);
        if good && self.trust.is_none() {
            self.trust = Some(self.find_trust(&sigf));
        }
        Some(good)
    }

    fn find_trust(&self, sigf: &Path) -> String {
        let Some(allowed) = &self.opts.allowed_signers else {
            return "no --allowed-signers given".into();
        };
        match Command::new("ssh-keygen").args(["-Y", "find-principals", "-s"]).arg(sigf).arg("-f").arg(allowed).output() {
            Ok(o) if o.status.success() => format!("trusted as {}", String::from_utf8_lossy(&o.stdout).trim()),
            _ => format!("NOT in {}", allowed.display()),
        }
    }

    fn release(&mut self, force: bool) -> io::Result<()> {
        while self.next_seq as i64 <= self.max_seq {
            let s = self.next_seq;
            if self.nond.remove(&s) {
                self.next_seq += 1;
                continue;
            }
            match self.pending.get(&s) {
                Some((h, Some(_))) => {
                    let (h, plain) = (*h, self.pending.remove(&s).unwrap().1.unwrap());
                    self.emit(&h, plain)?;
                }
                entry => {
                    if !force && self.max_seq - s as i64 <= self.hold {
                        return Ok(());
                    }
                    let known = entry.map(|(h, _)| *h);
                    if known.is_some() || self.lengths.contains_key(&s) {
                        self.s.data_lost += 1;
                        let n = self.lengths.get(&s).copied().unwrap_or_else(|| known.map_or(0, |h| h.plain_len));
                        self.log(format!("data record {} lost ({} bytes)", s, n));
                        if self.opts.gap_zero && n > 0 && !known.map_or(false, |h| h.flags & FLAG_DEFLATE != 0) {
                            if let Some(out) = &mut self.out {
                                out.write_all(&vec![0u8; n as usize])?;
                            }
                        }
                    } else {
                        self.s.unknown_lost += 1;
                        self.log(format!("record {} never arrived", s));
                    }
                }
            }
            self.pending.remove(&s);
            self.next_seq += 1;
        }
        Ok(())
    }

    fn emit(&mut self, h: &Header, plain: Vec<u8>) -> io::Result<()> {
        let data = if h.flags & FLAG_DEFLATE != 0 {
            match zlib::decompress(&plain, MAX_INFLATE) {
                Ok(d) => d,
                Err(_) => {
                    self.s.data_lost += 1;
                    return Ok(());
                }
            }
        } else {
            plain
        };
        self.payload.update(&data);
        self.s.bytes += data.len() as u64;
        if (h.t_us as f64) < self.opts.start * 1e6 {
            return Ok(());
        }
        if self.opts.paced {
            let now = Instant::now();
            let wall0 = *self.wall0.get_or_insert_with(|| {
                self.t0 = h.t_us;
                now
            });
            let offset = (h.t_us as f64 - self.t0 as f64) / 1e6 / self.opts.speed;
            if offset > 0.0 {
                let due = wall0 + Duration::from_secs_f64(offset);
                if due > now {
                    std::thread::sleep(due - now);
                }
            }
        }
        if let Some(out) = &mut self.out {
            out.write_all(&data)?;
            out.flush()?;
        }
        Ok(())
    }

    /// The payload's SHA-256 so far, in hex.
    pub fn payload_hex(&self) -> String {
        hex(&self.payload.clone().finish())
    }

    /// The report `verify` prints, line for line the prototype's.
    pub fn summary(&self) -> String {
        let s = &self.s;
        let empty = Value::Obj(Vec::new());
        let m = self.meta.as_ref().unwrap_or(&empty);
        let text = |k: &str| m.get(k).map(display).unwrap_or_else(|| "?".into());
        let mut lines = vec![format!("stream       {}  {}  created {}", text("stream_id"), text("content_type"), text("created"))];
        for k in ["source", "license", "note"] {
            if let Some(v) = m.get(k).filter(|v| truthy(v)) {
                lines.push(format!("{:<12} {}", k, display(v)));
            }
        }
        match m.get("key").and_then(|v| v.as_str()).filter(|k| !k.is_empty()) {
            Some(key) => {
                let fp = fingerprint(key).unwrap_or_default();
                let shown = if fp.is_empty() { key.chars().take(40).collect() } else { fp };
                lines.push(format!("key          {}; {}", shown, self.trust.as_deref().unwrap_or("no good signature seen")));
            }
            None => lines.push("key          none: checkpoints detect damage, not tampering".into()),
        }
        lines.push(format!(
            "records      {} ({} data, {} parity, {} checkpoints), {} repeats skipped",
            s.records, s.data, s.parity, s.checkpoints, s.duplicates
        ));
        let digest = self.payload_hex();
        match &self.end_rec {
            Some(end) => {
                let matches = end.get("sha256").and_then(|v| v.as_str()) == Some(digest.as_str())
                    && end.get("bytes").and_then(|v| v.as_u64()) == Some(s.bytes);
                lines.push(format!(
                    "payload      {} bytes, sha256 {} ({})",
                    s.bytes,
                    &digest[..16],
                    if matches { "matches the end record" } else { "DOES NOT match the end record" }
                ));
            }
            None => lines.push(format!("payload      {} bytes; no end record{}", s.bytes, if s.truncated { ", file truncated" } else { "" })),
        }
        lines.push(format!(
            "repaired     {} header bytes, {} body bytes in {} records, {} records rebuilt from parity",
            s.header_fixed, s.body_fixed, s.bodies_repaired, s.recovered
        ));
        lines.push(format!(
            "lost         {} data records, {} records of unknown type, {} parity; {} bytes skipped in {} resyncs",
            s.data_lost, s.unknown_lost, s.parity_lost, s.skipped, s.resyncs
        ));
        lines.push(format!(
            "checkpoints  {} good, {} bad, {} chain gaps; entries {} verified, {} mismatched, {} unaccounted",
            s.ck_ok, s.ck_bad, s.chain_gaps, s.verified, s.mismatched, s.missing
        ));
        lines.push(format!(
            "signatures   {} good, {} bad, {} not checkable (no key seen yet); signed through record {} of {}",
            s.sig_good, s.sig_bad, s.sig_unverifiable, self.last_signed, self.max_seq
        ));
        match &self.end_rec {
            Some(end) => lines.push(format!(
                "end          present{}",
                if end.get("interrupted").and_then(|v| v.as_bool()).unwrap_or(false) { ", writer was interrupted" } else { "" }
            )),
            None => lines.push("end          missing: the capture stopped without closing".into()),
        }
        lines.join("\n")
    }

    /// 0 when everything checked out, 1 otherwise: the prototype's rule.
    pub fn status(&self) -> i32 {
        let s = &self.s;
        let mut bad = s.data_lost > 0 || s.mismatched > 0 || s.ck_bad > 0 || s.sig_bad > 0 || s.truncated || self.end_rec.is_none();
        if let Some(end) = &self.end_rec {
            if end.get("sha256").and_then(|v| v.as_str()) != Some(self.payload_hex().as_str()) {
                bad = true;
            }
        }
        // Asked whose key it must be, a good signature by anyone else is a failure.
        if self.opts.allowed_signers.is_some() && !self.trust.as_deref().unwrap_or("").starts_with("trusted") {
            bad = true;
        }
        i32::from(bad)
    }
}

/// A JSON value as Python's `%s` shows a string or a number.
fn display(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Num(_) | Value::Bool(_) | Value::Null => {
            let t = v.to_json();
            match t.as_str() {
                "true" => "True".into(),
                "false" => "False".into(),
                "null" => "None".into(),
                _ => t,
            }
        }
        other => other.to_json(),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Num(n) => *n != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Arr(a) => !a.is_empty(),
        Value::Obj(o) => !o.is_empty(),
    }
}

fn fingerprint(key: &str) -> Option<String> {
    let dir = TempDir::new().ok()?;
    let path = dir.path.join("key.pub");
    std::fs::write(&path, format!("{key}\n")).ok()?;
    let out = Command::new("ssh-keygen").arg("-l").arg("-f").arg(&path).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run_with_input(cmd: &mut Command, input: &[u8]) -> io::Result<std::process::Output> {
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    child.stdin.take().expect("piped").write_all(input)?;
    child.wait_with_output()
}

/// A directory that removes itself: the one-line allowed_signers file and the
/// signature `ssh-keygen -Y verify` wants as files.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> io::Result<TempDir> {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("sstr-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Read a capture through, as `sstr verify` does: its exit status (0 when
/// everything checked out), its payload's SHA-256 in hex, and what was found.
/// What `sstr verify FILE` prints, and the code it would exit with.
///
/// The Workspace's Inspector shows a capture by calling this and drawing the
/// lines it returns, rather than reading the header itself and formatting it
/// again. `display`, `truthy` and `fingerprint` stay private that way, and the
/// pane cannot drift from the command: what the Inspector shows about a
/// capture is what `sstr verify` says about it, in the same words. That is
/// phase 3's rule one step before Services makes it a command line people see.
pub fn inspect(path: &Path) -> io::Result<(i32, String)> {
    let file = std::fs::File::open(path)?;
    let mut r = Reader::new(Source::new(file, false, Rc::new(RefCell::new(Erasures::new()))), None, Options::default());
    r.run()?;
    Ok((r.status(), r.summary()))
}

pub fn check_file(path: &Path) -> io::Result<(i32, String, Stats)> {
    let file = std::fs::File::open(path)?;
    let mut r = Reader::new(Source::new(file, false, Rc::new(RefCell::new(Erasures::new()))), None, Options::default());
    r.run()?;
    Ok((r.status(), r.payload_hex(), r.s.clone()))
}

/// The stream header of a capture file, from its first records.
pub fn read_meta(path: &Path) -> Option<Value> {
    let file = std::fs::File::open(path).ok()?;
    let mut r = Reader::new(Source::new(file, false, Rc::new(RefCell::new(Erasures::new()))), None, Options::default());
    let (h, _, at) = r.next_header(MAGIC.len() as u64)?;
    if h.typ != RecordType::Head {
        return None;
    }
    let b0 = at + HDR_DISK as u64;
    let blen = body_len(h.plain_len as usize) as u64;
    if !r.src.fill(b0 + blen) {
        return None;
    }
    let (plain, _) = record::fec_decode(r.src.get(b0, b0 + blen), h.plain_len as usize, h.body_crc, None);
    json::parse_bytes(&plain?).ok()
}
