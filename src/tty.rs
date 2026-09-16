// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The armor: a byte stream as lines a tty, an RS-232 line or a COM port
//! carries without complaint -- printable, 80 columns, no XON or XOFF.
//!
//! ```text
//! -----BEGIN SSTR ARMOR v0-----
//! S<offset, 9 hex>:<45 bytes as 60 base64url characters>:<CRC-32 of the rest, 8 hex>
//! -----END SSTR ARMOR <length hex> <first 40 hex of the SHA-256>-----
//! ```
//!
//! Every line names its own offset, so a line lost on the wire is a known
//! gap -- an erasure the Reed-Solomon decoder corrects at twice the rate of an
//! unknown error -- rather than a shift. A line that fails its CRC, or is not
//! armor at all (a kernel message on the same console), is dropped.

use std::cell::RefCell;
use std::io::{self, BufRead, Read};
use std::rc::Rc;

use crate::format::crc32;
use crate::format::reader::Erasures;

/// Payload bytes per line.
pub const LINE_BYTES: usize = 45;
/// Base64url characters for a full line: 45 bytes is 60, with no padding.
pub const LINE_CHARS: usize = LINE_BYTES / 3 * 4;
/// Hex digits of the offset.
pub const OFFSET_DIGITS: usize = 9;
/// A full line without its newline: `S`, offset, `:`, payload, `:`, CRC.
pub const LINE_WIDTH: usize = 1 + OFFSET_DIGITS + 1 + LINE_CHARS + 1 + 8;
/// The first line.
pub const BEGIN: &str = "-----BEGIN SSTR ARMOR v0-----";
/// How the last line starts.
pub const END_PREFIX: &str = "-----END SSTR ARMOR ";
/// The URL-safe base64 alphabet (RFC 4648 section 5).
pub const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
/// Software flow control's bytes, which no line may contain.
pub const XON: u8 = 0x11;
pub const XOFF: u8 = 0x13;

/// Bytes a second on an 8N1 line: ten bits to a byte.
pub fn line_bytes_per_second(baud: u32) -> f64 {
    baud as f64 / 10.0
}

/// Bytes of the armored stream a second at `baud`: 45 of every 81 on the wire.
pub fn armored_bytes_per_second(baud: u32) -> f64 {
    line_bytes_per_second(baud) * LINE_BYTES as f64 / (LINE_WIDTH + 1) as f64
}

/// Base64url without padding.
pub fn base64url(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() * 4 + 2) / 3);
    for g in data.chunks(3) {
        let n = (g[0] as u32) << 16 | (*g.get(1).unwrap_or(&0) as u32) << 8 | *g.get(2).unwrap_or(&0) as u32;
        let chars = g.len() + 1;
        for i in 0..chars {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn sextet(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    } as u32)
}

/// Base64url back to bytes; None for a character outside the alphabet or a
/// length no encoding produces (one character over a group of four).
pub fn unbase64url(s: &[u8]) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for g in s.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in g.iter().enumerate() {
            n |= sextet(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if g.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if g.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// One armor line, newline included.
pub fn armor_line(offset: u64, data: &[u8]) -> String {
    let head = format!("S{:09x}:{}", offset, base64url(data));
    format!("{}:{:08x}\n", head, crc32::crc32(head.as_bytes()))
}

/// The last line: the stream's length and the first 40 hex of its SHA-256.
pub fn end_line(total: u64, sha256_hex: &str) -> String {
    format!("{}{:x} {}-----\n", END_PREFIX, total, &sha256_hex[..sha256_hex.len().min(40)])
}

/// A line that is armor: its offset, its payload characters, and whether its
/// CRC holds. None when it is not armor at all.
fn parse(line: &[u8]) -> Option<(u64, &[u8], bool)> {
    let hexes = |b: &[u8]| b.iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c));
    if line.len() < 1 + OFFSET_DIGITS + 1 + 1 + 1 + 8 || line[0] != b'S' || line[1 + OFFSET_DIGITS] != b':' {
        return None;
    }
    let (head, crc) = (&line[..line.len() - 9], &line[line.len() - 8..]);
    if line[line.len() - 9] != b':' || !hexes(crc) || !hexes(&line[1..1 + OFFSET_DIGITS]) {
        return None;
    }
    let payload = &head[1 + OFFSET_DIGITS + 1..];
    if payload.is_empty() || payload.len() > LINE_CHARS || !payload.iter().all(|&c| sextet(c).is_some()) {
        return None;
    }
    let offset = u64::from_str_radix(std::str::from_utf8(&line[1..1 + OFFSET_DIGITS]).ok()?, 16).ok()?;
    let want = u32::from_str_radix(std::str::from_utf8(crc).ok()?, 16).ok()?;
    Some((offset, payload, crc32::crc32(head) == want))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnarmorStats {
    pub lines: u64,
    pub crc_failed: u64,
    pub not_armor: u64,
    pub lost_bytes: u64,
    pub repeats: u64,
}

/// Armor lines in, the byte stream out, with every range it had to leave as
/// zeros added to `erasures` -- which the reader reading this stream consults
/// before it decodes the bytes it covers.
pub struct Unarmor<R: BufRead> {
    inner: R,
    erasures: Rc<RefCell<Erasures>>,
    off: u64,
    pending: Vec<u8>,
    at: usize,
    ended: bool,
    /// The length the END line gave, if it arrived.
    pub total: Option<u64>,
    pub stats: UnarmorStats,
}

impl<R: BufRead> Unarmor<R> {
    pub fn new(inner: R, erasures: Rc<RefCell<Erasures>>) -> Unarmor<R> {
        Unarmor { inner, erasures, off: 0, pending: Vec::new(), at: 0, ended: false, total: None, stats: UnarmorStats::default() }
    }

    fn lose(&mut self, n: u64) {
        self.erasures.borrow_mut().add(self.off, self.off + n);
        self.pending.resize(self.pending.len() + n as usize, 0);
        self.off += n;
        self.stats.lost_bytes += n;
    }

    fn next_lines(&mut self) -> io::Result<()> {
        let mut raw = Vec::new();
        while self.at >= self.pending.len() && !self.ended {
            self.pending.clear();
            self.at = 0;
            raw.clear();
            if self.inner.read_until(b'\n', &mut raw)? == 0 {
                self.ended = true;
                break;
            }
            while matches!(raw.last(), Some(b'\n' | b'\r')) {
                raw.pop();
            }
            let line = &raw[..];
            if line.starts_with(b"-----END SSTR ARMOR") {
                let total = std::str::from_utf8(line)
                    .ok()
                    .and_then(|t| t.split_whitespace().nth(3))
                    .and_then(|t| u64::from_str_radix(t, 16).ok());
                let Some(total) = total else { continue };
                self.total = Some(total);
                if total > self.off {
                    self.lose(total - self.off);
                }
                self.ended = true;
                break;
            }
            let Some((offset, payload, crc_ok)) = parse(line) else {
                if !line.is_empty() && !line.starts_with(b"-----BEGIN SSTR ARMOR") {
                    self.stats.not_armor += 1;
                }
                continue;
            };
            if !crc_ok {
                self.stats.crc_failed += 1;
                continue;
            }
            let Some(data) = unbase64url(payload) else {
                self.stats.crc_failed += 1;
                continue;
            };
            if offset < self.off {
                self.stats.repeats += 1;
                continue;
            }
            if offset > self.off {
                self.lose(offset - self.off);
            }
            self.stats.lines += 1;
            self.pending.extend_from_slice(&data);
            self.off += data.len() as u64;
        }
        Ok(())
    }
}

impl<R: BufRead> Read for Unarmor<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.next_lines()?;
        let n = (self.pending.len() - self.at).min(buf.len());
        buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_eighty_columns() {
        assert_eq!(LINE_CHARS, 60);
        assert_eq!(LINE_WIDTH, 80);
        assert_eq!(armor_line(0, &[7u8; LINE_BYTES]).len(), LINE_WIDTH + 1);
        assert!(end_line(0x1_0000_0000, &"f".repeat(64)).len() <= 81);
    }

    #[test]
    fn the_alphabet_is_printable_and_flow_control_safe() {
        for &c in ALPHABET.iter() {
            assert!((0x21..0x7f).contains(&c), "{c:#x}");
            assert!(c != XON && c != XOFF);
        }
    }

    #[test]
    fn rates_match_the_report() {
        assert_eq!(line_bytes_per_second(115_200), 11_520.0);
        assert!((armored_bytes_per_second(115_200) - 6_400.0).abs() < 0.5);
    }

    #[test]
    fn base64url_round_trips_every_length() {
        for n in 0..50 {
            let data: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37).wrapping_add(250)).collect();
            assert_eq!(unbase64url(base64url(&data).as_bytes()), Some(data));
        }
        assert_eq!(base64url(b"\xfb\xff"), "-_8");
        assert_eq!(unbase64url(b"A"), None);
    }

    #[test]
    fn the_prototypes_first_line_decodes() {
        // The first line of an armored capture from the Static Stream lab report.
        let line = b"S000000000:iVNTVA0KGgqnU1NSSAAAAAAAAAAAAAAAUwAAAAAAAADVAQAAuuWhqNzNTFu3:a579734a";
        let (off, payload, ok) = parse(line).unwrap();
        assert_eq!((off, ok), (0, true));
        let bytes = unbase64url(payload).unwrap();
        assert_eq!(&bytes[..8], b"\x89SST\r\n\x1a\n");
        assert_eq!(&bytes[8..13], b"\xa7SSRH");
    }

    #[test]
    fn lost_lines_become_erasures_and_noise_is_ignored() {
        let data: Vec<u8> = (0..500u32).map(|i| (i * 13) as u8).collect();
        let mut text = format!("{BEGIN}\n");
        for (i, piece) in data.chunks(LINE_BYTES).enumerate() {
            if i == 3 {
                continue; // lost on the wire
            }
            if i == 5 {
                text.push_str("[  812.004113] usb 1-1: new high-speed USB device\r\n");
            }
            let mut line = armor_line((i * LINE_BYTES) as u64, piece);
            if i == 7 {
                line = line.replacen('A', "B", 1); // a bit flip the CRC catches
            }
            text.push_str(&line);
        }
        text.push_str(&end_line(data.len() as u64, &"0".repeat(64)));
        let er = Rc::new(RefCell::new(Erasures::new()));
        let mut u = Unarmor::new(text.as_bytes(), er.clone());
        let mut out = Vec::new();
        u.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), data.len());
        assert_eq!(u.stats.not_armor, 1);
        assert_eq!(u.total, Some(500));
        let ranges = er.borrow().ranges();
        assert!(ranges.contains(&(135, 180)), "{ranges:?}");
        for (i, (a, b)) in out.iter().zip(&data).enumerate() {
            let erased = ranges.iter().any(|&(s, e)| (s..e).contains(&(i as u64)));
            assert!(erased || a == b, "byte {i}");
        }
    }
}
