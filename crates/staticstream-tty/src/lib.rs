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
//! unknown error -- rather than a shift. Phase 1 writes the encoder and the
//! decoder; phase 0 fixes the numbers.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_eighty_columns() {
        assert_eq!(LINE_CHARS, 60);
        assert_eq!(LINE_WIDTH, 80);
        assert!(BEGIN.len() <= 80);
        // The end line with a 9-digit length and 40 hex digits of hash.
        assert!(END_PREFIX.len() + 9 + 1 + 40 + 5 <= 80);
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
}
