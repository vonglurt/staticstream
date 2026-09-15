// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Static Stream, the format: a stream stopped in a file with its order, its
//! pace and its tolerance for damage intact, and let go again as a stream.
//!
//! Version 0 is defined by `tools/copal-sstr.py` in the copal repository and
//! set out in its `docs/static-stream-lab-report.md`. Phase 1 ports the
//! reader and writer. What is here in phase 0 is the layout's arithmetic --
//! the numbers the port and the prototype must share -- and a test that reads
//! the prototype to prove they still do.
//!
//! ```text
//! file      = MAGIC record*
//! record    = SYNC hdr32 rs32  SYNC2 hdr32 rs32  body
//! hdr32     = type u8 | flags u8 | reserved u16 | seq u64 | t_us u64
//!             | plain_len u32 | body_crc u32 | hdr_crc u32     (little-endian)
//! body      = RS(255,223) codewords of the plain body, interleaved
//! ```

/// The format version this crate reads and writes.
pub const FORMAT_VERSION: u32 = 0;

/// The file signature, after PNG's: a high-bit byte, CR LF, ^Z, LF.
pub const MAGIC: [u8; 8] = *b"\x89SST\r\n\x1a\n";
/// The sync marker before the first copy of a record header.
pub const SYNC: [u8; 4] = *b"\xa7SSR";
/// The sync marker before the second copy, straight after the first.
pub const SYNC2: [u8; 4] = *b"\xa7SSr";

/// Reed-Solomon parity bytes per codeword.
pub const NSYM: usize = 32;
/// Data bytes in a full codeword: RS(255,223).
pub const K: usize = 255 - NSYM;
/// The packed header fields, before their CRC-32.
pub const HDR_BARE: usize = 28;
/// The header with its CRC-32: the data of the header codeword.
pub const HDR_PLAIN: usize = HDR_BARE + 4;
/// One copy of a header on disk: sync, data, parity.
pub const HDR_COPY: usize = SYNC.len() + HDR_PLAIN + NSYM;
/// A record header on disk: written twice.
pub const HDR_DISK: usize = 2 * HDR_COPY;
/// The longest plain body a reader accepts.
pub const MAX_PLAIN: usize = 1 << 24;
/// Flag bit: the data record's body is a deflate stream.
pub const FLAG_DEFLATE: u8 = 1;
/// Data records per parity record, by default.
pub const GROUP: usize = 16;
/// The `ssh-keygen -Y sign -n` namespace checkpoints are signed under.
pub const SIGNING_NAMESPACE: &str = "sstr@copal";

/// What a record is, by its type byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordType {
    /// The stream header: JSON, written twice.
    Head,
    /// Payload bytes.
    Data,
    /// The XOR of a group of data records, and their lengths and CRCs.
    Parity,
    /// A signed list of record hashes, chained to the one before.
    Check,
    /// The payload's length and SHA-256, written twice.
    End,
}

impl RecordType {
    pub const ALL: [RecordType; 5] = [
        RecordType::Head,
        RecordType::Data,
        RecordType::Parity,
        RecordType::Check,
        RecordType::End,
    ];

    pub fn byte(self) -> u8 {
        match self {
            RecordType::Head => b'H',
            RecordType::Data => b'D',
            RecordType::Parity => b'P',
            RecordType::Check => b'C',
            RecordType::End => b'E',
        }
    }

    pub fn from_byte(b: u8) -> Option<RecordType> {
        RecordType::ALL.into_iter().find(|t| t.byte() == b)
    }
}

/// Bytes a plain body of `plain_len` takes on disk: full codewords of 255,
/// and a last one shortened to what is left plus its 32 parity bytes. Nothing
/// else about the body is stored, because this is all a reader needs.
pub fn body_len(plain_len: usize) -> usize {
    let (full, rest) = (plain_len / K, plain_len % K);
    full * 255 + if rest > 0 { rest + NSYM } else { 0 }
}

/// Bytes a whole record takes on disk: both header copies and the body.
pub fn record_len(plain_len: usize) -> usize {
    HDR_DISK + body_len(plain_len)
}

/// The redundancy by design, for full records: the inner code times the
/// outer, (1 + 32/223) x (1 + 1/group), before headers and control records.
pub fn design_overhead(group: usize) -> f64 {
    (1.0 + NSYM as f64 / K as f64) * (1.0 + 1.0 / group as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_sizes() {
        assert_eq!(HDR_PLAIN, 32);
        assert_eq!(HDR_COPY, 68);
        assert_eq!(HDR_DISK, 136);
    }

    #[test]
    fn body_lengths_match_the_prototype() {
        // (plain, on disk), the values copal-sstr.py's body_len() gives.
        for (plain, disk) in [(0, 0), (1, 33), (222, 254), (223, 255), (224, 288), (446, 510), (65536, 74944)] {
            assert_eq!(body_len(plain), disk, "plain_len {plain}");
        }
        assert_eq!(record_len(65536), 74944 + 136);
    }

    #[test]
    fn overhead_is_the_reports() {
        assert!((design_overhead(GROUP) - 1.2150).abs() < 0.0005);
    }

    #[test]
    fn type_bytes_round_trip() {
        for t in RecordType::ALL {
            assert_eq!(RecordType::from_byte(t.byte()), Some(t));
        }
        assert_eq!(RecordType::from_byte(b'X'), None);
    }

    /// The prototype is the definition of version 0 until phase 1 replaces it,
    /// so the constants are checked against its source. The copal checkout is
    /// found beside this one in ~/code, or at $STATICSTREAM_PROTOTYPE; without
    /// either the test says so and passes, so a lone checkout still builds.
    #[test]
    fn constants_match_the_prototype() {
        let path = std::env::var_os("STATICSTREAM_PROTOTYPE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../copal/tools/copal-sstr.py")
            });
        let Ok(src) = std::fs::read_to_string(&path) else {
            eprintln!("skipped: no prototype at {}", path.display());
            return;
        };
        for line in [
            r#"MAGIC = b"\x89SST\r\n\x1a\n""#,
            r#"SYNC = b"\xa7SSR""#,
            r#"SYNC2 = b"\xa7SSr""#,
            "NSYM = 32",
            r#"HDR_FMT = "<BBHQQII""#,
            "MAX_PLAIN = 1 << 24",
            r#"T_HEAD, T_DATA, T_PARITY, T_CHECK, T_END = b"HDPCE""#,
            "F_DEFLATE = 1",
            r#"NAMESPACE = "sstr@copal""#,
        ] {
            assert!(src.contains(line), "{} no longer has: {line}", path.display());
        }
    }
}
