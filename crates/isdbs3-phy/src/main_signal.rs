//! The main signal of a frame: per valid slot, the codeword's symbols
//! (`isdbs3_core::slots`), max-log soft demapping against the constellation
//! (the nominal one of B44 §3.8, or the points learned from the pilots),
//! bit de-interleaving (§3.7), LDPC (§3.4 (2)), BCH (§3.4 (1)), energy
//! dispersal removal (§3.6 (1)), and the slot's header and data bits.
//!
//! Pilot (§3.9): each modulation slot's 32 pilot symbols are the
//! modulation's points in label order (16APSK: 0000 … 1111 twice), each
//! turned 0° / 180° by the pilot PRBS (§3.6 (3)). Averaging them over a
//! frame gives the transmitted points as received, compression of the
//! transponder included: B44 §2 introduces the pilot "for restrain of
//! deterioration" from the TWTA.

use crate::sync::{SlotHead, phase_at};
use crate::{Cplx, soft};
use isdbs3_core::constellation::points;
use isdbs3_core::dispersal::{SlotDispersal, pilot_signs};
use isdbs3_core::interleave::{Direction, direction, permutation};
use isdbs3_core::params::{
    BCH_PARITY_BITS, HEADER_BITS, MAIN_RUN, MOD_SLOT, Modulation, N_LDPC, PILOT_SYMBOLS, RUNS,
    SLOTS, STUFF_BITS, SYNC_SYMBOLS, TMCC_RUN,
};
use isdbs3_core::slots::Slot;

/// Offsets within a modulation slot of its 8 976 main symbols, in order.
pub fn main_offsets() -> impl Iterator<Item = usize> {
    (0..RUNS).flat_map(|r| {
        let base = SYNC_SYMBOLS + PILOT_SYMBOLS + r * (MAIN_RUN + TMCC_RUN);
        base..base + MAIN_RUN
    })
}

/// The frame's main symbols (1 077 120), derotated by the slot phase and
/// scaled by the sync amplitude.
pub fn main_symbols(z: &[Cplx], slots: &[SlotHead], f: usize) -> Option<Vec<Cplx>> {
    if f + SLOTS > slots.len() {
        return None;
    }
    let mut out = Vec::with_capacity(SLOTS * RUNS * MAIN_RUN);
    for i in f..f + SLOTS {
        let at = slots[i].at;
        if at + MOD_SLOT > z.len() {
            return None;
        }
        for j in main_offsets() {
            let (ph, amp) = phase_at(slots, i, j);
            out.push(z[at + j] / ph / amp.max(1e-30));
        }
    }
    Some(out)
}

/// The frame's pilot symbols, derotated, scaled and with the pilot PRBS
/// sign removed: 32 per modulation slot.
pub fn pilots(z: &[Cplx], slots: &[SlotHead], f: usize) -> Option<Vec<Cplx>> {
    if f + SLOTS > slots.len() {
        return None;
    }
    let signs = pilot_signs();
    let mut out = Vec::with_capacity(SLOTS * PILOT_SYMBOLS);
    for (k, i) in (f..f + SLOTS).enumerate() {
        let at = slots[i].at;
        if at + MOD_SLOT > z.len() {
            return None;
        }
        for j in SYNC_SYMBOLS..SYNC_SYMBOLS + PILOT_SYMBOLS {
            let (ph, amp) = phase_at(slots, i, j);
            let s = f64::from(signs[k * PILOT_SYMBOLS + j - SYNC_SYMBOLS]);
            out.push(z[at + j] / ph / amp.max(1e-30) * s);
        }
    }
    Some(out)
}

/// The constellation of `m` as received: the mean of the pilot symbols of
/// every modulation slot of that modulation, per label (pilot symbol t
/// carries label t mod 2^bits, §3.9). `None` if no slot carries `m`.
pub fn learned_points(pil: &[Cplx], layout: &[Slot], m: Modulation) -> Option<Vec<Cplx>> {
    let n = 1usize << m.bits();
    let mut sum = vec![Cplx::new(0.0, 0.0); n];
    let mut cnt = vec![0usize; n];
    for (k, s) in layout.iter().enumerate() {
        if s.modulation != m {
            continue;
        }
        for t in 0..PILOT_SYMBOLS {
            let lab = t % n;
            sum[lab] += pil[k * PILOT_SYMBOLS + t];
            cnt[lab] += 1;
        }
    }
    (cnt.iter().all(|&c| c > 0)).then(|| sum.iter().zip(&cnt).map(|(s, &c)| s / c as f64).collect())
}

/// The nominal constellation of `m` at rate `r`, at the sync symbols'
/// amplitude (unit power: the sync symbols have the main signal's mean
/// power, §3.8).
pub fn nominal_points(m: Modulation, r: isdbs3_core::CodeRate) -> Vec<Cplx> {
    points(m, r)
        .into_iter()
        .map(|(i, q)| Cplx::new(i, q))
        .collect()
}

/// Max-log LLRs of `sym` against `pts` (label-indexed), `bits` per symbol
/// MSB first, noise variance per complex dimension `n0` (E|n|²).
pub fn demap(sym: &[Cplx], pts: &[Cplx], bits: usize, n0: f64, out: &mut Vec<f32>) {
    out.clear();
    out.reserve(sym.len() * bits);
    // the constellations B44 uses, with the label loops unrolled at compile
    // time: the same distances and minima as the general loop below
    match (pts.len(), bits) {
        (2, 1) => demap_n::<2, 1>(sym, pts, n0, out),
        (4, 2) => demap_n::<4, 2>(sym, pts, n0, out),
        (8, 3) => demap_n::<8, 3>(sym, pts, n0, out),
        (16, 4) => demap_n::<16, 4>(sym, pts, n0, out),
        (32, 5) => demap_n::<32, 5>(sym, pts, n0, out),
        _ => demap_any(sym, pts, bits, n0, out),
    }
}

/// [`demap`] for any number of points: the reference the unrolled forms
/// match bit for bit.
fn demap_any(sym: &[Cplx], pts: &[Cplx], bits: usize, n0: f64, out: &mut Vec<f32>) {
    let mut d = vec![0f64; pts.len()];
    for &y in sym {
        for (di, p) in d.iter_mut().zip(pts) {
            *di = (y - p).norm_sqr();
        }
        for b in (0..bits).rev() {
            let (mut m0, mut m1) = (f64::MAX, f64::MAX);
            for (lab, &di) in d.iter().enumerate() {
                if (lab >> b) & 1 == 0 {
                    m0 = m0.min(di);
                } else {
                    m1 = m1.min(di);
                }
            }
            out.push(soft((m1 - m0) / n0));
        }
    }
}

/// [`demap`] for `N` = 2^`B` points: the label tests are constants once
/// the loops unroll, so each bit's two minima are straight-line code.
/// `(y - p).norm_sqr()` is spelled out as num-complex computes it
/// (re² + im²), and min is exact, so the LLRs are bit for bit those of
/// the general loop.
#[inline(never)]
fn demap_n<const N: usize, const B: usize>(
    sym: &[Cplx],
    pts: &[Cplx],
    n0: f64,
    out: &mut Vec<f32>,
) {
    let pr: [f64; N] = std::array::from_fn(|k| pts[k].re);
    let pi: [f64; N] = std::array::from_fn(|k| pts[k].im);
    let start = out.len();
    out.resize(start + sym.len() * B, 0.0);
    for (&y, o) in sym.iter().zip(out[start..].as_chunks_mut::<B>().0) {
        let mut d = [0f64; N];
        for k in 0..N {
            let (dr, di) = (y.re - pr[k], y.im - pi[k]);
            d[k] = dr * dr + di * di;
        }
        for (j, b) in (0..B).rev().enumerate() {
            // `if x < m` is f64::min here: distances are never NaN
            // (min ignores a NaN argument just as the comparison does)
            // and never -0
            let (mut m0, mut m1) = (f64::MAX, f64::MAX);
            for (lab, &dk) in d.iter().enumerate() {
                if (lab >> b) & 1 == 0 {
                    if dk < m0 {
                        m0 = dk;
                    }
                } else if dk < m1 {
                    m1 = dk;
                }
            }
            o[j] = soft((m1 - m0) / n0);
        }
    }
}

/// One decoded slot.
#[derive(Debug, Clone)]
pub struct SlotData {
    /// Slot index 0 … 119.
    pub slot: usize,
    /// LDPC iterations, or `None` if it did not converge.
    pub ldpc_iterations: Option<usize>,
    /// BCH bits corrected, or `None` if uncorrectable (or LDPC failed).
    pub bch_corrected: Option<usize>,
    /// The 176 header bits and the data bits after dispersal removal
    /// (meaningful only when BCH succeeded).
    pub header: Vec<u8>,
    /// The main data, packed MSB first.
    pub data: Vec<u8>,
}

/// The LDPC decoder [`decode_frame`] uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ldpc {
    /// Floating-point layered normalised min-sum (`isdb_fec::ldpc::Code::decode`),
    /// one codeword at a time: the reference.
    Float {
        /// Iteration limit.
        max_iter: usize,
        /// Check-to-variable scale.
        scale: f32,
    },
    /// Fixed-point layered offset min-sum (`isdb_fec::ldpc::fixed`),
    /// [`LANES`] codewords at a time on the SIMD back end, bit-exact with
    /// its one-lane scalar form; the LLRs are quantised to `i8` as
    /// `round(llr · input_scale)`.
    Fixed {
        /// Iteration limit.
        max_iter: usize,
        /// LLR quantisation scale.
        input_scale: f32,
        /// Offset subtracted from the check-to-variable magnitudes.
        offset: i8,
        /// [`LANES`] codewords at a time (SIMD), or one (the scalar
        /// reference, the same results).
        simd: bool,
    },
}

/// Lanes of the fixed-point decoder (`isdb_fec::ldpc::fixed`): 16 `i8`
/// lanes fill a 128-bit vector (SSE2, NEON); measured fastest at that
/// baseline and within 2 % of 32 with AVX2 (plan §0.9).
pub const LANES: usize = 16;

impl Ldpc {
    /// The fixed-point decoder with the settings chosen on live
    /// recordings (plan §0.9).
    pub const FIXED: Self = Self::Fixed {
        max_iter: 50,
        input_scale: 1.0,
        offset: 1,
        simd: true,
    };
    /// [`Ldpc::FIXED`] one codeword at a time: the scalar reference of
    /// the SIMD decoder (the same bits and iterations).
    pub const SCALAR: Self = Self::Fixed {
        max_iter: 50,
        input_scale: 1.0,
        offset: 1,
        simd: false,
    };
    /// The floating-point reference.
    pub const FLOAT: Self = Self::Float {
        max_iter: 50,
        scale: 0.8,
    };
}

/// One codeword's LLRs in codeword order: slot `s`'s symbols in `ms`
/// demapped against `pts` (noise `n0`) and bit-de-interleaved (§3.7).
pub fn codeword_llrs(ms: &[Cplx], s: &Slot, pts: &[Cplx], n0: f64) -> Vec<f32> {
    let sym = &ms[s.symbol_start..s.symbol_start + s.symbols];
    let mut lab = Vec::new();
    demap(sym, pts, s.modulation.bits(), n0, &mut lab);
    let perm = cached_permutation(s.modulation, s.rate);
    let mut llr = vec![0f32; N_LDPC];
    for (i, &p) in perm.iter().enumerate() {
        llr[p as usize] = lab[i];
    }
    llr
}

/// [`permutation`] built once per (modulation, read-out direction), the
/// two things it depends on: every codeword of a frame used to build its
/// own 44 880-entry table.
fn cached_permutation(m: Modulation, r: isdbs3_core::CodeRate) -> &'static [u32] {
    static CACHE: [std::sync::OnceLock<Vec<u32>>; 15] = [const { std::sync::OnceLock::new() }; 15];
    let mi = match m {
        Modulation::Bpsk => 0,
        Modulation::Qpsk => 1,
        Modulation::Psk8 => 2,
        Modulation::Apsk16 => 3,
        Modulation::Apsk32 => 4,
    };
    let di = match direction(m, r) {
        Direction::None => 0,
        Direction::Forward => 1,
        Direction::Backward => 2,
    };
    CACHE[mi * 3 + di].get_or_init(|| permutation(m, r))
}

/// A decoded codeword: slot index, LDPC iterations (`None`: not
/// converged) and the hard bits.
pub(crate) type Decoded = (usize, Option<usize>, Vec<u8>);

/// One frame's input to [`decode_frames`].
pub struct FrameInput<'a> {
    /// The frame's main symbols (unit sync amplitude).
    pub ms: &'a [Cplx],
    /// Its slot layout (from its TMCC).
    pub layout: &'a [Slot],
    /// Noise variance per complex dimension.
    pub n0: f64,
    /// The constellation of each (modulation, rate) of this frame.
    pub pts_for: &'a (dyn Fn(Modulation, isdbs3_core::CodeRate) -> Vec<Cplx> + Sync),
}

/// Decode every valid slot of a frame from its main symbols `ms` (unit
/// sync amplitude), with the constellation `pts_for(modulation, rate)`
/// and the LDPC decoder `ldpc`.
/// The slot dispersal runs across the frame's valid slots in order, so
/// every slot's dispersed bits are produced even when its decode fails.
pub fn decode_frame(
    ms: &[Cplx],
    layout: &[Slot],
    n0: f64,
    ldpc: &Ldpc,
    pts_for: &(dyn Fn(Modulation, isdbs3_core::CodeRate) -> Vec<Cplx> + Sync),
) -> Vec<SlotData> {
    decode_frames(
        &[FrameInput {
            ms,
            layout,
            n0,
            pts_for,
        }],
        ldpc,
    )
    // one frame in, one out: its slots
    .into_iter()
    .flatten()
    .collect()
}

/// [`decode_frame`] for several frames at once: every codeword of every
/// frame is demapped and decoded in one pool of threads (each codeword
/// independently, so the results are those of [`decode_frame`] frame by
/// frame), then each frame's dispersal and BCH run in slot order, frames
/// in parallel.
pub fn decode_frames(frames: &[FrameInput<'_>], ldpc: &Ldpc) -> Vec<Vec<SlotData>> {
    // every codeword: (frame, slot)
    let items: Vec<(usize, &Slot)> = frames
        .iter()
        .enumerate()
        .flat_map(|(f, fr)| fr.layout.iter().filter(|s| !s.dummy).map(move |s| (f, s)))
        .collect();
    let llrs = |&(f, s): &(usize, &Slot)| -> Vec<f32> {
        let fr = &frames[f];
        codeword_llrs(fr.ms, s, &(fr.pts_for)(s.modulation, s.rate), fr.n0)
    };
    // decode the codewords (independent) …
    let mut decoded: Vec<(usize, usize, Option<usize>, Vec<u8>)> = match *ldpc {
        Ldpc::Float { max_iter, scale } => {
            let chunk = items.len().div_ceil(workers()).max(1);
            std::thread::scope(|sc| {
                let handles: Vec<_> = items
                    .chunks(chunk)
                    .map(|grp| {
                        sc.spawn(move || {
                            grp.iter()
                                .map(|it| {
                                    let mut llr = llrs(it);
                                    let mut bits = Vec::new();
                                    let n = isdbs3_core::ldpc::code(it.1.rate)
                                        .decode(&mut llr, &mut bits, max_iter, scale)
                                        .ok()
                                        .map(|d| d.iterations);
                                    (it.0, it.1.index, n, bits)
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
                    .collect()
            })
        }
        Ldpc::Fixed {
            max_iter,
            input_scale,
            offset,
            simd,
        } => {
            // streams of codewords of one rate, one per thread; within a
            // stream the decoder refills a lane as soon as it stops
            let mut rates: Vec<isdbs3_core::CodeRate> = items.iter().map(|it| it.1.rate).collect();
            rates.sort_by_key(|r| r.numerator());
            rates.dedup();
            let mut batches: Vec<Vec<(usize, &Slot)>> = Vec::new();
            for r in rates {
                let same: Vec<(usize, &Slot)> =
                    items.iter().copied().filter(|it| it.1.rate == r).collect();
                let per = same
                    .len()
                    .div_ceil(workers())
                    .max(if simd { LANES } else { 1 });
                batches.extend(same.chunks(per).map(<[(usize, &Slot)]>::to_vec));
            }
            let p = isdb_fec::ldpc::fixed::Params::new(max_iter, offset);
            std::thread::scope(|sc| {
                let handles: Vec<_> = batches
                    .iter()
                    .map(|b| {
                        sc.spawn(move || {
                            let q: Vec<Vec<i8>> = b
                                .iter()
                                .map(|it| {
                                    let mut v = Vec::new();
                                    isdb_fec::ldpc::fixed::quantise(&llrs(it), input_scale, &mut v);
                                    v
                                })
                                .collect();
                            let refs: Vec<&[i8]> = q.iter().map(Vec::as_slice).collect();
                            let code = isdbs3_core::ldpc::code(b[0].1.rate);
                            let res = if simd {
                                isdb_fec::ldpc::fixed::Decoder::<LANES>::default()
                                    .decode_all(code, &refs, p)
                            } else {
                                isdb_fec::ldpc::fixed::Decoder::<1>::default()
                                    .decode_all(code, &refs, p)
                            };
                            b.iter()
                                .zip(res)
                                .map(|(it, (r, bits))| {
                                    (it.0, it.1.index, r.ok().map(|d| d.iterations), bits)
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
                    .collect()
            })
        }
    };
    decoded.sort_by_key(|d| (d.0, d.1));
    let mut per_frame: Vec<Vec<Decoded>> = vec![Vec::new(); frames.len()];
    for (f, slot, it, bits) in decoded {
        per_frame[f].push((slot, it, bits));
    }
    // … then undo the dispersal in slot order (its PRBS runs across
    // slots) and check the BCH, frames in parallel
    std::thread::scope(|sc| {
        let handles: Vec<_> = per_frame
            .into_iter()
            .zip(frames)
            .map(|(dec, fr)| sc.spawn(move || finish_frame(dec, fr.layout)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    })
}

/// Dispersal removal (in slot order) and BCH of one frame's decoded
/// codewords, sorted by slot (also stage 8 of the fixed-point chain).
pub(crate) fn finish_frame(decoded: Vec<Decoded>, layout: &[Slot]) -> Vec<SlotData> {
    let mut disp = SlotDispersal::new();
    decoded
        .into_iter()
        .map(|(slot, it, bits)| {
            let s = layout[slot];
            let k = s.rate.k_ldpc();
            let mut msg = bits[..k].to_vec();
            // Fig. 3.2-1: BCH (step 2) before dispersal (step 3), which
            // covers header, data, BCH parity and stuffing; the BCH word
            // is header + data + parity, the stuffing outside it.
            let spread = HEADER_BITS + s.rate.data_bits() + BCH_PARITY_BITS + STUFF_BITS;
            debug_assert_eq!(spread, k);
            disp.apply(&mut msg[..spread]);
            let bch = it.and_then(|_| isdb_fec::bch::decode(&mut msg[..k - STUFF_BITS]).ok());
            let header = msg[..HEADER_BITS].to_vec();
            let data = msg[HEADER_BITS..HEADER_BITS + s.rate.data_bits()]
                .chunks(8)
                .map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b))
                .collect();
            SlotData {
                slot,
                ldpc_iterations: it,
                bch_corrected: bch,
                header,
                data,
            }
        })
        .collect()
}

/// Decoder threads.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrolled_demap_matches_the_general_loop() {
        // received points around every constellation (noise, far outliers,
        // ties on the points themselves), bit for bit
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut u = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let r = isdbs3_core::CodeRate::new(93).unwrap();
        for m in [
            Modulation::Bpsk,
            Modulation::Qpsk,
            Modulation::Psk8,
            Modulation::Apsk16,
            Modulation::Apsk32,
        ] {
            let pts = nominal_points(m, r);
            let mut sym: Vec<Cplx> = (0..4000)
                .map(|i| {
                    pts[i % pts.len()] + Cplx::new(u(), u()) * if i % 7 == 0 { 8.0 } else { 0.4 }
                })
                .collect();
            sym.extend(&pts);
            for n0 in [0.01, 0.3, 2.0] {
                let (mut fast, mut slow) = (Vec::new(), Vec::new());
                demap(&sym, &pts, m.bits(), n0, &mut fast);
                demap_any(&sym, &pts, m.bits(), n0, &mut slow);
                assert_eq!(fast.len(), slow.len());
                assert!(
                    fast.iter()
                        .zip(&slow)
                        .all(|(a, b)| a.to_bits() == b.to_bits()),
                    "{m:?} n0 {n0}"
                );
            }
        }
    }

    #[test]
    fn spec_8976_main_symbols_per_slot() {
        let o: Vec<usize> = main_offsets().collect();
        assert_eq!(o.len(), 8_976);
        assert_eq!(o[0], 56);
        assert_eq!(o[136], 56 + 140);
    }

    #[test]
    fn demap_is_exact_on_the_points() {
        let r = isdbs3_core::CodeRate::new(93).unwrap();
        let pts = nominal_points(Modulation::Apsk16, r);
        let mut out = Vec::new();
        demap(&pts, &pts, 4, 0.01, &mut out);
        for (lab, l) in out.chunks(4).enumerate() {
            for (b, v) in l.iter().enumerate() {
                let bit = (lab >> (3 - b)) & 1;
                assert_eq!(*v < 0.0, bit == 1, "label {lab:04b} bit {b}");
            }
        }
    }
}
