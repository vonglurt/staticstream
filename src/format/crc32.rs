// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! CRC-32 (IEEE 802.3, reflected, polynomial 0xEDB88320) -- the checksum
//! Python's `zlib.crc32` computes, which is what version 0 of the format
//! stores. CRC-32C would be the better choice for a new format; version 0 is
//! not new, so it is the prototype's.

const fn table() -> [[u32; 256]; 4] {
    let mut t = [[0u32; 256]; 4];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    // Slicing-by-4: three more tables let the loop take four bytes a step.
    let mut i = 0;
    while i < 256 {
        let mut s = 1;
        while s < 4 {
            let prev = t[s - 1][i];
            t[s][i] = (prev >> 8) ^ t[0][(prev & 0xff) as usize];
            s += 1;
        }
        i += 1;
    }
    t
}

static T: [[u32; 256]; 4] = table();

/// Continue a CRC over more bytes; start from 0.
pub fn update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    let mut chunks = data.chunks_exact(4);
    for w in &mut chunks {
        c ^= u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        c = T[3][(c & 0xff) as usize]
            ^ T[2][((c >> 8) & 0xff) as usize]
            ^ T[1][((c >> 16) & 0xff) as usize]
            ^ T[0][(c >> 24) as usize];
    }
    for &b in chunks.remainder() {
        c = T[0][((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

pub fn crc32(data: &[u8]) -> u32 {
    update(0, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        // zlib.crc32 of these, from Python.
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    #[test]
    fn pieces_equal_whole() {
        let data: Vec<u8> = (0..999u32).map(|i| (i * 31 % 256) as u8).collect();
        let mut c = 0;
        for piece in data.chunks(7) {
            c = update(c, piece);
        }
        assert_eq!(c, crc32(&data));
    }
}
