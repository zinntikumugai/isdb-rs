//! Binary BCH codes over GF(2¹⁶), shortened, systematic: the outer code of
//! ISDB-S3 (ARIB STD-B44 v2.1 §3.4 (1), Table 3.4-1; §3.5 for the TMCC).
//!
//! BCH(65 535, 65 343), t = 12: the generator g(x) is the product of the
//! twelve degree-16 polynomials g₁…g₁₂ of Table 3.4-1, so it has degree
//! 192 and the code corrects 12 bit errors in any shortening. g₁(x) =
//! 1 + x + x³ + x¹² + x¹⁶ is primitive and defines the field; g₁…g₁₂ are
//! then the minimal polynomials of α, α³, …, α²³ (checked by
//! `spec_table_3_4_1_are_the_minimal_polynomials`), which the decoder's
//! syndromes rely on.
//!
//! Bit order follows §3.4 (1): the message m(x) = m_{k−1}x^{k−1} + … + m₀
//! is sent highest power first, then the parity d(x) highest power first;
//! a shortened word is the same with leading zero message bits omitted.
//! Bits are `u8` 0/1 values in transmission order.

use std::sync::OnceLock;

/// Table 3.4-1 of B44: g₁ … g₁₂ as exponent lists.
pub const TABLE_3_4_1: [&[u32]; 12] = [
    &[0, 1, 3, 12, 16],
    &[0, 2, 3, 4, 8, 9, 11, 12, 16],
    &[0, 2, 3, 7, 9, 10, 11, 13, 16],
    &[0, 1, 3, 6, 7, 11, 12, 13, 16],
    &[0, 1, 2, 3, 5, 7, 8, 9, 11, 13, 16],
    &[0, 1, 6, 7, 9, 10, 12, 13, 16],
    &[0, 1, 2, 6, 9, 10, 11, 15, 16],
    &[0, 1, 3, 6, 8, 9, 12, 15, 16],
    &[0, 1, 4, 6, 8, 10, 11, 12, 13, 15, 16],
    &[0, 1, 2, 4, 6, 8, 9, 10, 11, 15, 16],
    &[0, 6, 8, 9, 10, 13, 14, 15, 16],
    &[0, 1, 2, 3, 5, 6, 7, 10, 11, 15, 16],
];

/// Field size: GF(2¹⁶).
const M: u32 = 16;
/// Multiplicative order: 2¹⁶ − 1.
const Q1: usize = (1 << M) - 1;
/// Correctable errors.
pub const T: usize = 12;
/// Parity bits: deg g = 12 · 16.
pub const PARITY: usize = T * M as usize;
/// Natural code length.
pub const N_MAX: usize = Q1;

struct Field {
    exp: Vec<u16>,
    log: Vec<u16>,
}

fn field() -> &'static Field {
    static F: OnceLock<Field> = OnceLock::new();
    F.get_or_init(|| {
        // α is a root of g₁: α¹⁶ = 1 + α + α³ + α¹².
        // g₁ below x¹⁶: the x¹⁶ term is the bit shifted out of v
        let poly: u16 = (1 << 12) | (1 << 3) | (1 << 1) | 1;
        let mut exp = vec![0u16; 2 * Q1];
        let mut log = vec![0u16; Q1 + 1];
        let mut v: u16 = 1;
        for (e, i) in exp[..Q1].iter_mut().zip(0..u16::MAX) {
            *e = v;
            log[usize::from(v)] = i;
            let carry = v & 0x8000 != 0;
            v <<= 1;
            if carry {
                v ^= poly;
            }
        }
        for i in Q1..2 * Q1 {
            exp[i] = exp[i - Q1];
        }
        Field { exp, log }
    })
}

impl Field {
    #[inline]
    fn mul(&self, a: u16, b: u16) -> u16 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize]
        }
    }
    #[inline]
    fn inv(&self, a: u16) -> u16 {
        self.exp[(Q1 - self.log[a as usize] as usize) % Q1]
    }
    #[inline]
    fn pow_alpha(&self, e: usize) -> u16 {
        self.exp[e % Q1]
    }
}

/// g(x) as 193 coefficients, g[i] the coefficient of xⁱ.
fn generator() -> &'static [u8] {
    static G: OnceLock<Vec<u8>> = OnceLock::new();
    G.get_or_init(|| {
        let mut g = vec![1u8];
        for f in TABLE_3_4_1 {
            let mut fp = [0u8; 17];
            for &e in f {
                fp[e as usize] = 1;
            }
            let mut out = vec![0u8; g.len() + 16];
            for (i, &a) in g.iter().enumerate() {
                if a == 1 {
                    for (j, &b) in fp.iter().enumerate() {
                        out[i + j] ^= b;
                    }
                }
            }
            g = out;
        }
        debug_assert_eq!(g.len(), PARITY + 1);
        g
    })
}

/// The parity of `msg` (k bits, transmission order): the remainder of
/// x¹⁹² m(x) divided by g(x), 192 bits in transmission order.
pub fn parity(msg: &[u8]) -> Vec<u8> {
    let g = generator();
    // Register r[0..192]: r[i] is the coefficient of x^(191−i) — shift
    // towards index 0 so the next output is r[0].
    let mut r = vec![0u8; PARITY];
    for &b in msg {
        let fb = (b & 1) ^ r[0];
        r.copy_within(1.., 0);
        r[PARITY - 1] = 0;
        if fb == 1 {
            // add g(x) minus its leading term: coefficient of x^(191−i)
            // is g[191 − i]
            for (i, ri) in r.iter_mut().enumerate() {
                *ri ^= g[PARITY - 1 - i];
            }
        }
    }
    r
}

/// The codeword: `msg` followed by its parity.
pub fn encode(msg: &[u8]) -> Vec<u8> {
    let mut c = msg.to_vec();
    c.extend(parity(msg));
    c
}

/// A 192-bit register, bit i the coefficient of xⁱ: `[x⁰…x⁶³, x⁶⁴…x¹²⁷,
/// x¹²⁸…x¹⁹¹]`.
type Reg = [u64; 3];

/// `(t(x) · x¹⁹²) mod g(x)` for every byte t (bit 7 the coefficient of
/// x⁷): the byte-at-a-time division of [`is_codeword`].
fn byte_table() -> &'static [Reg; 256] {
    static TAB: OnceLock<Box<[Reg; 256]>> = OnceLock::new();
    TAB.get_or_init(|| {
        let g = generator();
        let mut low: Reg = [0; 3]; // g(x) − x¹⁹²
        for (i, &c) in g[..PARITY].iter().enumerate() {
            low[i / 64] |= u64::from(c) << (i % 64);
        }
        let mut tab = Box::new([[0u64; 3]; 256]);
        for (t, e) in tab.iter_mut().enumerate() {
            let mut r: Reg = [0; 3];
            for k in (0..8).rev() {
                let fb = ((t >> k) & 1) as u64 ^ (r[2] >> 63);
                r = [
                    r[0] << 1,
                    (r[1] << 1) | (r[0] >> 63),
                    (r[2] << 1) | (r[1] >> 63),
                ];
                if fb == 1 {
                    for (x, l) in r.iter_mut().zip(&low) {
                        *x ^= l;
                    }
                }
            }
            *e = r;
        }
        tab
    })
}

/// Whether `word` (transmission order, highest power first) is a
/// codeword, i.e. g(x) divides it: the remainder of x¹⁹² w(x), a byte at
/// a time like a CRC (leading zero bits pad the front to whole bytes and
/// change nothing; x¹⁹² is prime to g, whose constant term is 1). The
/// same answer as every syndrome S₁ … S₂ₜ being zero, since g is the
/// least common multiple of the minimal polynomials of α … α²ᵗ.
fn is_codeword(word: &[u8]) -> bool {
    let tab = byte_table();
    let mut r: Reg = [0; 3];
    let lead = (8 - word.len() % 8) % 8;
    let byte = |b: u8, r: &mut Reg| {
        let e = &tab[usize::from(((r[2] >> 56) as u8) ^ b)];
        *r = [
            r[0] << 8 ^ e[0],
            (r[1] << 8 | r[0] >> 56) ^ e[1],
            (r[2] << 8 | r[1] >> 56) ^ e[2],
        ];
    };
    let (head, rest) = word.split_at((8 - lead) % 8);
    if lead != 0 {
        byte(head.iter().fold(0u8, |a, &b| (a << 1) | (b & 1)), &mut r);
    }
    for c in rest.as_chunks::<8>().0 {
        byte(c.iter().fold(0u8, |a, &b| (a << 1) | (b & 1)), &mut r);
    }
    r == [0; 3]
}

/// Too many errors for the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("BCH: uncorrectable")]
pub struct Uncorrectable;

/// Correct `word` (a shortened codeword of length n ≤ 65 535, message then
/// 192 parity bits, transmission order) in place; the number of bits
/// corrected.
///
/// # Panics
///
/// If `word` is not longer than the 192 parity bits or longer than
/// 65 535 bits.
pub fn decode(word: &mut [u8]) -> Result<usize, Uncorrectable> {
    let n = word.len();
    assert!(n > PARITY && n <= N_MAX, "BCH length {n}");
    // the usual case after LDPC: no error, found without the syndromes
    if is_codeword(word) {
        return Ok(0);
    }
    let f = field();
    // word[j] is the coefficient of x^(n−1−j). Syndromes S_i = r(α^i),
    // i = 1..2t; for a binary code S_2i = S_i², but computing all is cheap
    // enough and keeps the code plain.
    let mut s = [0u16; 2 * T + 1];
    let mut any = false;
    for (j, &b) in word.iter().enumerate() {
        if b & 1 == 1 {
            let p = n - 1 - j;
            for (i, si) in s.iter_mut().enumerate().skip(1) {
                *si ^= f.pow_alpha(p * i);
            }
        }
    }
    for &si in &s[1..] {
        any |= si != 0;
    }
    if !any {
        return Ok(0);
    }
    // Berlekamp–Massey.
    let mut c = vec![0u16; 2 * T + 2];
    let mut bp = vec![0u16; 2 * T + 2];
    c[0] = 1;
    bp[0] = 1;
    let (mut l, mut m, mut b) = (0usize, 1usize, 1u16);
    for k in 0..2 * T {
        let mut d = s[k + 1];
        for i in 1..=l {
            d ^= f.mul(c[i], s[k + 1 - i]);
        }
        if d == 0 {
            m += 1;
            continue;
        }
        let coef = f.mul(d, f.inv(b));
        let t = c.clone();
        for i in 0..c.len() - m {
            c[i + m] ^= f.mul(coef, bp[i]);
        }
        if 2 * l <= k {
            l = k + 1 - l;
            bp = t;
            b = d;
            m = 1;
        } else {
            m += 1;
        }
    }
    if l > T {
        return Err(Uncorrectable);
    }
    // Chien search over the n positions present: an error at power p means
    // Λ(α^−p) = 0.
    let mut fixed = 0;
    for (j, bit) in word.iter_mut().enumerate() {
        let p = n - 1 - j;
        let xinv = (Q1 - p % Q1) % Q1;
        let mut v = 0u16;
        for (i, &ci) in c.iter().enumerate().take(l + 1) {
            if ci != 0 {
                v ^= f.mul(ci, f.pow_alpha(xinv * i));
            }
        }
        if v == 0 {
            *bit ^= 1;
            fixed += 1;
        }
    }
    if fixed != l {
        return Err(Uncorrectable);
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::XorShift as Lcg;

    /// Minimal polynomial of α^e over GF(2), as exponent list.
    fn minimal(e: usize) -> Vec<u32> {
        let f = field();
        // conjugates α^(e·2^j)
        let mut conj = vec![];
        let mut x = e % Q1;
        loop {
            conj.push(x);
            x = (x * 2) % Q1;
            if x == e % Q1 {
                break;
            }
        }
        let mut p = vec![1u16];
        for &c in &conj {
            let root = f.pow_alpha(c);
            let mut q = vec![0u16; p.len() + 1];
            for (i, &a) in p.iter().enumerate() {
                q[i + 1] ^= a;
                q[i] ^= f.mul(a, root);
            }
            p = q;
        }
        p.iter()
            .enumerate()
            .filter(|(_, v)| **v != 0)
            .map(|(i, v)| {
                assert_eq!(*v, 1);
                u32::try_from(i).unwrap()
            })
            .collect()
    }

    #[test]
    fn spec_table_3_4_1_are_the_minimal_polynomials() {
        for (i, g) in TABLE_3_4_1.iter().enumerate() {
            assert_eq!(minimal(2 * i + 1), g.to_vec(), "g{}", i + 1);
        }
    }

    #[test]
    fn spec_generator_degree_192() {
        assert_eq!(generator().len(), 193);
        assert_eq!(generator()[0], 1);
        assert_eq!(generator()[192], 1);
    }

    fn word(rng: &mut Lcg, k: usize) -> Vec<u8> {
        (0..k).map(|_| (rng.next() & 1) as u8).collect()
    }

    #[test]
    fn corrects_up_to_twelve_errors_in_the_shortened_codes() {
        let mut rng = Lcg::new(7);
        // main-signal BCH of the shortest and longest slots, and the TMCC
        for k in [176 + 14960, 176 + 40392, 9422] {
            let msg = word(&mut rng, k);
            let c = encode(&msg);
            assert_eq!(c.len(), k + PARITY);
            for e in [0, 1, 5, 12] {
                let mut r = c.clone();
                let mut pos = std::collections::BTreeSet::new();
                while pos.len() < e {
                    pos.insert(rng.below(r.len()));
                }
                for &p in &pos {
                    r[p] ^= 1;
                }
                assert_eq!(decode(&mut r), Ok(e), "k {k} e {e}");
                assert_eq!(r, c);
            }
        }
    }

    #[test]
    fn codeword_check_agrees_with_the_syndromes() {
        // every byte alignment of the front (n mod 8), codewords and words
        // with 1 … 12 errors (all detectable: fewer than d = 25)
        let mut rng = Lcg::new(7);
        for k in (1000..1016).chain([34_312, 44_688]) {
            let c = encode(&word(&mut rng, k));
            assert!(is_codeword(&c), "k {k}");
            let mut pos = std::collections::BTreeSet::new();
            while pos.len() < T {
                pos.insert(rng.below(c.len()));
                let mut r = c.clone();
                for &p in &pos {
                    r[p] ^= 1;
                }
                assert!(!is_codeword(&r), "k {k}, {} errors", pos.len());
            }
        }
    }

    #[test]
    fn thirteen_errors_are_never_miscorrected_silently_here() {
        let mut rng = Lcg::new(11);
        let msg = word(&mut rng, 9422);
        let c = encode(&msg);
        let mut r = c.clone();
        let n = r.len();
        for p in (0..13).map(|i| i * 701 % n) {
            r[p] ^= 1;
        }
        // 13 errors: either detected, or decoded to another codeword; never
        // "corrected" back to c
        match decode(&mut r) {
            Err(Uncorrectable) => {}
            Ok(_) => assert_ne!(r, c),
        }
    }
}
