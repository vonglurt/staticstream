// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Reed-Solomon RS(255,223) over GF(2^8): the inner code of every record.
//!
//! Primitive polynomial 0x11d, generator 2, first consecutive root 2^0 -- the
//! prototype's choice, and the code must agree with it symbol for symbol,
//! since a record's parity bytes are on disk. CCSDS uses the same code
//! parameters in another basis, so this is not wire-compatible with that.
//!
//! `correct` decodes errors at unknown positions and erasures at known ones,
//! as long as 2 x errors + erasures <= 32. The algorithm is the prototype's
//! port of the Berlekamp-Massey decoder with Forney syndromes, kept close to
//! it on purpose: the two are cross-checked against each other, and a
//! divergence is easier to find in code that reads the same.
//!
//! No crate: the one Rust Reed-Solomon crate that corrects errors at unknown
//! positions has been unmaintained since 2018, and this workspace fetches
//! nothing.

use std::sync::OnceLock;

pub const NSYM: usize = crate::format::NSYM;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsError(pub &'static str);

impl std::fmt::Display for RsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

struct Tables {
    exp: [u8; 512],
    log: [u8; 256],
    gen: Vec<u8>,
    /// gen[1..] multiplied by each byte: the encoder's feedback rows.
    feedback: Vec<[u8; NSYM]>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        for i in 0..255 {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11d;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        let mul = |a: u8, b: u8| if a == 0 || b == 0 { 0 } else { exp[log[a as usize] as usize + log[b as usize] as usize] };
        let mut gen = vec![1u8];
        for i in 0..NSYM {
            let root = exp[i];
            let mut next = vec![0u8; gen.len() + 1];
            for (j, &g) in gen.iter().enumerate() {
                next[j] ^= g;
                next[j + 1] ^= mul(g, root);
            }
            gen = next;
        }
        let mut feedback = vec![[0u8; NSYM]; 256];
        for (c, row) in feedback.iter_mut().enumerate() {
            for j in 0..NSYM {
                row[j] = mul(gen[j + 1], c as u8);
            }
        }
        Tables { exp, log, gen, feedback }
    })
}

/// GF(256) multiplication.
///
/// Public because the OUTER code needs the same field as the inner one. The
/// parity record's Q row is a sum of `g^i * body_i` over this field
/// (`format::record`), and a crate with two Galois fields in it is a crate
/// with a bug waiting to be written: one set of tables, one primitive
/// polynomial, one answer to what `g^3` means.
pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let t = tables();
    t.exp[t.log[a as usize] as usize + t.log[b as usize] as usize]
}

fn div(a: u8, b: u8) -> Result<u8, RsError> {
    if b == 0 {
        return Err(RsError("division by zero"));
    }
    if a == 0 {
        return Ok(0);
    }
    let t = tables();
    Ok(t.exp[(t.log[a as usize] as usize + 255 - t.log[b as usize] as usize) % 255])
}

/// 2^p for any integer p, which is all the decoder raises -- and, at the
/// outer code, the coefficient of the record in place p of its group.
pub fn alpha(p: i64) -> u8 {
    tables().exp[p.rem_euclid(255) as usize]
}

/// 1/a in GF(256). `a` must not be zero, which the outer code guarantees by
/// only ever inverting a difference of two distinct powers of the generator.
pub fn inverse(a: u8) -> u8 {
    let t = tables();
    t.exp[255 - t.log[a as usize] as usize]
}

// Polynomials are big-endian: highest degree first, as in the prototype.

fn poly_scale(p: &[u8], x: u8) -> Vec<u8> {
    p.iter().map(|&c| mul(c, x)).collect()
}

fn poly_add(p: &[u8], q: &[u8]) -> Vec<u8> {
    let n = p.len().max(q.len());
    let mut r = vec![0u8; n];
    for (i, &c) in p.iter().enumerate() {
        r[i + n - p.len()] = c;
    }
    for (i, &c) in q.iter().enumerate() {
        r[i + n - q.len()] ^= c;
    }
    r
}

fn poly_mul(p: &[u8], q: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; p.len() + q.len() - 1];
    for (j, &b) in q.iter().enumerate() {
        for (i, &a) in p.iter().enumerate() {
            r[i + j] ^= mul(a, b);
        }
    }
    r
}

fn poly_eval(p: &[u8], x: u8) -> u8 {
    let mut y = p[0];
    for &c in &p[1..] {
        y = mul(y, x) ^ c;
    }
    y
}

/// The remainder of dividend / divisor, divisor monic.
fn poly_rem(dividend: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut out = dividend.to_vec();
    if dividend.len() >= divisor.len() {
        for i in 0..dividend.len() - divisor.len() + 1 {
            let coef = out[i];
            if coef != 0 {
                for j in 1..divisor.len() {
                    if divisor[j] != 0 {
                        out[i + j] ^= mul(divisor[j], coef);
                    }
                }
            }
        }
    }
    out[out.len() - (divisor.len() - 1)..].to_vec()
}

/// The 32 parity bytes for `data` (at most 223 bytes; fewer is a shortened code).
pub fn parity(data: &[u8]) -> [u8; NSYM] {
    let fb = &tables().feedback;
    let mut reg = [0u8; NSYM];
    for &b in data {
        let row = &fb[(b ^ reg[0]) as usize];
        reg.copy_within(1.., 0);
        reg[NSYM - 1] = 0;
        for j in 0..NSYM {
            reg[j] ^= row[j];
        }
    }
    reg
}

fn syndromes(msg: &[u8]) -> Vec<u8> {
    let mut s = vec![0u8; NSYM + 1];
    for i in 0..NSYM {
        s[i + 1] = poly_eval(msg, alpha(i as i64));
    }
    s
}

fn errata_locator(coef_pos: &[usize]) -> Vec<u8> {
    let mut loc = vec![1u8];
    for &i in coef_pos {
        loc = poly_mul(&loc, &poly_add(&[1], &[alpha(i as i64), 0]));
    }
    loc
}

fn correct_errata(msg: &[u8], synd: &[u8], err_pos: &[usize]) -> Result<Vec<u8>, RsError> {
    let n = msg.len();
    let coef_pos: Vec<usize> = err_pos.iter().map(|&p| n - 1 - p).collect();
    let loc = errata_locator(&coef_pos);
    let synd_rev: Vec<u8> = synd.iter().rev().copied().collect();
    let mut divisor = vec![0u8; loc.len() + 1];
    divisor[0] = 1;
    let rem = poly_rem(&poly_mul(&synd_rev, &loc), &divisor);
    // X_i = 2^-(255 - c), which is 2^c.
    let x: Vec<u8> = coef_pos.iter().map(|&c| alpha(c as i64)).collect();
    let mut e = vec![0u8; n];
    for (i, &xi) in x.iter().enumerate() {
        let xi_inv = inverse(xi);
        let mut prime = 1u8;
        for (j, &xj) in x.iter().enumerate() {
            if j != i {
                prime = mul(prime, 1 ^ mul(xi_inv, xj));
            }
        }
        if prime == 0 {
            return Err(RsError("no error magnitude"));
        }
        let y = mul(xi, poly_eval(&rem, xi_inv));
        e[err_pos[i]] = div(y, prime)?;
    }
    Ok(poly_add(msg, &e))
}

fn error_locator(fsynd: &[u8], erase_count: usize) -> Result<Vec<u8>, RsError> {
    let mut loc = vec![1u8];
    let mut old = vec![1u8];
    for i in 0..NSYM - erase_count {
        let mut delta = fsynd[i];
        for j in 1..loc.len() {
            if j <= i {
                delta ^= mul(loc[loc.len() - 1 - j], fsynd[i - j]);
            }
        }
        old.push(0);
        if delta != 0 {
            if old.len() > loc.len() {
                let new = poly_scale(&old, delta);
                old = poly_scale(&loc, inverse(delta));
                loc = new;
            }
            loc = poly_add(&loc, &poly_scale(&old, delta));
        }
    }
    while loc.len() > 1 && loc[0] == 0 {
        loc.remove(0);
    }
    // The prototype's test, signed: the locator found from Forney syndromes
    // counts errors only, so `errs - erase_count` is often negative, and that
    // is fine -- the bound it guards is 2 x errors + erasures <= 32.
    let errs = loc.len() as i64 - 1;
    let erase_count = erase_count as i64;
    if (errs - erase_count) * 2 + erase_count > NSYM as i64 {
        return Err(RsError("too many errors"));
    }
    Ok(loc)
}

fn find_errors(loc: &[u8], n: usize) -> Result<Vec<usize>, RsError> {
    let pos: Vec<usize> = (0..n).filter(|&i| poly_eval(loc, alpha(i as i64)) == 0).map(|i| n - 1 - i).collect();
    if pos.len() != loc.len() - 1 {
        return Err(RsError("error locator does not factor in this codeword"));
    }
    Ok(pos)
}

fn forney_syndromes(synd: &[u8], pos: &[usize], n: usize) -> Vec<u8> {
    let mut f = synd[1..].to_vec();
    for &p in pos {
        let x = alpha((n - 1 - p) as i64);
        for j in 0..f.len() - 1 {
            f[j] = mul(f[j], x) ^ f[j + 1];
        }
    }
    f
}

/// Correct a received codeword -- data then 32 parity bytes, 33 to 255 bytes
/// long -- given the positions known to be bad. Returns the corrected
/// codeword, or an error when there is more damage than the code can take;
/// damage beyond it is reported, not silently "corrected" into another word,
/// except in the rare case where it lands within reach of a different one.
pub fn correct(cw: &[u8], erasures: &[usize]) -> Result<Vec<u8>, RsError> {
    if cw.len() <= NSYM || cw.len() > 255 {
        return Err(RsError("not a codeword length"));
    }
    let n = cw.len();
    let mut erase: Vec<usize> = erasures.iter().copied().filter(|&p| p < n).collect();
    erase.sort_unstable();
    erase.dedup();
    if erase.len() > NSYM {
        return Err(RsError("too many erasures"));
    }
    let mut msg = cw.to_vec();
    for &p in &erase {
        msg[p] = 0;
    }
    let synd = syndromes(&msg);
    if synd.iter().all(|&s| s == 0) {
        return Ok(msg);
    }
    let fsynd = forney_syndromes(&synd, &erase, n);
    let loc = error_locator(&fsynd, erase.len())?;
    let rev: Vec<u8> = loc.iter().rev().copied().collect();
    let err_pos = find_errors(&rev, n)?;
    let mut all = erase.clone();
    all.extend_from_slice(&err_pos);
    let fixed = correct_errata(&msg, &synd, &all)?;
    if syndromes(&fixed).iter().any(|&s| s != 0) {
        return Err(RsError("could not correct"));
    }
    Ok(fixed)
}

/// The generator polynomial, for tests that check the encoder against division.
pub fn generator() -> &'static [u8] {
    &tables().gen
}

#[cfg(test)]
mod tests {
    use super::*;

    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn encoder_is_polynomial_division() {
        let data: Vec<u8> = (0..223u32).map(|i| i as u8).collect();
        let mut dividend = data.clone();
        dividend.extend_from_slice(&[0; NSYM]);
        assert_eq!(poly_rem(&dividend, generator()), parity(&data).to_vec());
    }

    #[test]
    fn a_clean_codeword_has_zero_syndromes() {
        let data = b"Static Stream";
        let mut cw = data.to_vec();
        cw.extend_from_slice(&parity(data));
        assert!(syndromes(&cw).iter().all(|&s| s == 0));
    }

    /// The prototype's own test: within 2e + f <= 32, always corrected; beyond
    /// it, reported. 2,000 and 1,000 trials there; the same here.
    #[test]
    fn corrects_within_the_bound_and_reports_beyond_it() {
        let mut r = XorShift(0x9E37_79B9_7F4A_7C15);
        let (mut ok, mut beyond_reported, mut beyond_wrong) = (0, 0, 0);
        for trial in 0..3000 {
            let k = [223, 100, 32, 1][r.below(4)];
            let data: Vec<u8> = (0..k).map(|_| r.next() as u8).collect();
            let mut cw = data.clone();
            cw.extend_from_slice(&parity(&data));
            let n = cw.len();
            let (e, errs) = if trial < 2000 {
                let e = r.below(NSYM.min(n) + 1);
                (e, r.below((NSYM - e) / 2 + 1).min(n - e))
            } else {
                (0, (17 + r.below(9)).min(n))
            };
            let mut positions: Vec<usize> = (0..n).collect();
            for i in 0..e + errs {
                let j = i + r.below(n - i);
                positions.swap(i, j);
            }
            let mut rx = cw.clone();
            for &p in &positions[..e + errs] {
                rx[p] ^= 1 + (r.next() % 255) as u8;
            }
            match correct(&rx, &positions[..e]) {
                Ok(fixed) if trial < 2000 => {
                    assert_eq!(fixed, cw, "trial {trial}: k={k} erasures={e} errors={errs}");
                    ok += 1;
                }
                Err(err) if trial < 2000 => panic!("trial {trial}: k={k} erasures={e} errors={errs}: {err}"),
                Ok(fixed) => {
                    if fixed != cw {
                        beyond_wrong += 1;
                    }
                }
                Err(_) => beyond_reported += 1,
            }
        }
        assert_eq!(ok, 2000);
        // Beyond the bound a decoder may land on another codeword; the
        // prototype's run saw none in 1,000, and neither should this.
        assert_eq!(beyond_wrong, 0);
        assert_eq!(beyond_reported, 1000);
    }
}
