//! ISDB-S3 slot synchronisation on front-end symbols and the SNR measured
//! on the known synchronisation symbols: how good a recording is before
//! any TMCC or LDPC decoder exists, and the yardstick for the front end's
//! design choices.
//!
//! ARIB STD-B44 v2.1 Fig. 3.3-1: a modulation slot is 24 synchronisation
//! symbols (frame sync FSync 52F866h or its inverse !FSync AD0799h at the
//! head of odd slots, slot sync SSync 36715Ah at the head of even slots),
//! 32 pilot symbols, then 66 × (136 main + 4 TMCC) symbols = 9 296; 120
//! slots make a frame. §3.8: π/2-shift BPSK puts 0 / 1 at +45° / −135° on
//! odd symbols and turns them 90° counter-clockwise on even ones, the
//! first symbol of the frame being odd (9 296 is even, so every slot starts
//! on an odd symbol).
//!
//! The 24 sync symbols are found by correlation at the slot period. The
//! carrier phase comes from the SSync slots (which have no inverted twin),
//! each odd slot's word from its sign against its SSync neighbours. The
//! error on the known symbols, after removing each slot's phase and
//! amplitude, gives the SNR: Es/N0 of the sync symbols, which have the
//! main signal's mean power.
//!
//! This module only measures; it is not a receiver. The frame marker's
//! reading (which word marks the frame) is an observation (see the crate
//! README), not a settled interpretation of the standard.

use num_complex::Complex64 as C;
use std::f64::consts::{FRAC_PI_4, PI, TAU};
use tracing::debug;

/// Symbols per modulation slot.
pub const SLOT: usize = 24 + 32 + 66 * (136 + 4);
/// Symbols per frame (120 slots).
pub const FRAME: usize = 120 * SLOT;
/// Frame synchronisation word (odd slots).
pub const FSYNC: u32 = 0x52_F866;
/// Its inverse.
pub const NFSYNC: u32 = 0xAD_0799;
/// Slot synchronisation word (even slots).
pub const SSYNC: u32 = 0x36_715A;
/// Symbols of a synchronisation word.
pub const SYNC_LEN: usize = 24;

/// The 24 sync symbols of `word` (MSB first) as unit complex numbers,
/// starting on an odd symbol of the frame.
pub fn sync_symbols(word: u32) -> [C; SYNC_LEN] {
    std::array::from_fn(|k| {
        let bit = (word >> (SYNC_LEN - 1 - k)) & 1;
        let base = if bit == 0 {
            FRAC_PI_4
        } else {
            -3.0 * FRAC_PI_4
        };
        // k = 0 is an odd symbol (the first of the slot); even ones turn 90°.
        let turn = if k % 2 == 0 { 0.0 } else { 2.0 * FRAC_PI_4 };
        C::from_polar(1.0, base + turn)
    })
}

/// What [`measure`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncReport {
    /// Offset of the first slot head in the input (0..SLOT).
    pub offset: usize,
    /// Whole slots examined.
    pub slots: usize,
    /// Mean normalised correlation on the sync (1.0 = noiseless).
    pub peak: f64,
    /// The same statistic away from the sync.
    pub off_peak: f64,
    /// Parity of the SSync slots (0 or 1) relative to `offset`.
    pub ssync_parity: usize,
    /// Odd slots read as FSync and as !FSync (against the SSync phase).
    pub fsync: usize,
    /// See `fsync`.
    pub nfsync: usize,
    /// Es/N0 on the sync symbols, dB.
    pub snr_db: f64,
    /// Mean and RMS of the carrier phase step between SSync slots, rad.
    pub phase_step_mean: f64,
    /// See `phase_step_mean`.
    pub phase_step_rms: f64,
    /// Per slot: (index, carrier phase as a unit phasor, amplitude) for
    /// every SSync slot; used to derotate the rest of a slot.
    pub ssync_phase: Vec<(usize, C, f64)>,
}

impl SyncReport {
    /// Residual carrier offset implied by the mean phase step, Hz, for a
    /// symbol rate `rs`.
    pub fn residual_hz(&self, rs: f64) -> f64 {
        self.phase_step_mean / TAU * rs / (2.0 * SLOT as f64)
    }
}

/// The correlation of `x[at..at + SYNC_LEN]` with the sync word `r`
/// (the reference `position_mags` is tested against).
#[cfg(test)]
fn corr(x: &[C], at: usize, r: &[C; SYNC_LEN]) -> C {
    let w: &[C; SYNC_LEN] = x[at..at + SYNC_LEN].try_into().expect("a sync window");
    corr_w(w, r)
}

/// The correlation of one window of `SYNC_LEN` symbols with the sync
/// word `r`.
fn corr_w(w: &[C; SYNC_LEN], r: &[C; SYNC_LEN]) -> C {
    r.iter().zip(w).map(|(a, b)| b * a.conj()).sum()
}

/// Find the slot sync in `x` (symbols at one per symbol, any scale) and
/// measure the SNR on the sync symbols. `None` when `x` holds fewer than
/// three slots.
///
/// The offset is the argmax of the mean sync correlation over every slot
/// for each of the 9 296 offsets (an exhaustive search, the first maximum
/// winning); the offsets are shared out between threads, each computing
/// its offsets' statistics exactly as one thread would, so the result
/// does not depend on the number of threads.
pub fn measure(x: &[C]) -> Option<SyncReport> {
    measure_with(x, None)
}

/// [`measure`] with the slot offset already known (from an earlier
/// [`measure`] of the same symbol positions, e.g. before an equaliser
/// whose output is aligned with its input): everything but the search.
/// `None` when `x` holds fewer than three slots.
pub fn measure_at(x: &[C], offset: usize) -> Option<SyncReport> {
    measure_with(x, Some(offset % SLOT))
}

fn measure_with(x: &[C], known: Option<usize>) -> Option<SyncReport> {
    if x.len() < 3 * SLOT + SYNC_LEN {
        return None;
    }
    let rms = (x.iter().map(C::norm_sqr).sum::<f64>() / x.len() as f64).sqrt();
    let norm = |v: &C| v / rms.max(1e-30);
    // the symbols at unit RMS: all of them for a search; with the offset
    // known only the sync windows are read, each normalised as it is read
    // (the same values, without a copy of the whole input)
    let full: Vec<C> = match known {
        None => x.iter().map(norm).collect(),
        Some(_) => Vec::new(),
    };
    let win = |at: usize| -> [C; SYNC_LEN] {
        if full.is_empty() {
            std::array::from_fn(|i| norm(&x[at + i]))
        } else {
            std::array::from_fn(|i| full[at + i])
        }
    };
    let corr = |at: usize, r: &[C; SYNC_LEN]| corr_w(&win(at), r);
    let fs = sync_symbols(FSYNC);
    let ss = sync_symbols(SSYNC);
    let slots = (x.len() - SYNC_LEN) / SLOT;
    // the per-position statistic max(|corr FSync|, |corr SSync|) / 24:
    // for a search, computed once for every position it visits; with the
    // offset known, only at the 33 offsets used below
    let m = match known {
        None => position_mags(&full, slots * SLOT, &fs, &ss),
        Some(_) => Vec::new(),
    };
    let at_pos = |n: usize| -> f64 {
        if m.is_empty() {
            corr(n, &fs).norm().max(corr(n, &ss).norm()) / SYNC_LEN as f64
        } else {
            m[n]
        }
    };
    let mag = |off: usize| -> f64 {
        (0..slots).map(|k| at_pos(off + k * SLOT)).sum::<f64>() / slots as f64
    };
    let (peak, offset) = match known {
        Some(o) => (mag(o), o),
        None => search(&mag),
    };
    let off_peak = (1..=32)
        .map(|j| mag((offset + j * 277) % SLOT))
        .sum::<f64>()
        / 32.0;
    let at = |k: usize| offset + k * SLOT;
    let parity = usize::from(
        (1..slots)
            .step_by(2)
            .map(|k| corr(at(k), &ss).norm())
            .sum::<f64>()
            > (0..slots)
                .step_by(2)
                .map(|k| corr(at(k), &ss).norm())
                .sum::<f64>(),
    );
    let (mut sig, mut err) = (0.0, 0.0);
    let mut add = |at: usize, r: &[C; SYNC_LEN], ph: C, amp: f64| {
        for (a, b) in r.iter().zip(&win(at)) {
            let ideal = a * ph * amp;
            sig += ideal.norm_sqr();
            err += (b - ideal).norm_sqr();
        }
    };
    let mut ssync_phase = Vec::new();
    for k in (parity..slots).step_by(2) {
        let r = corr(at(k), &ss);
        let (ph, amp) = (r / r.norm().max(1e-30), r.norm() / SYNC_LEN as f64);
        ssync_phase.push((k, ph, amp));
        add(at(k), &ss, ph, amp);
    }
    let (mut fsync, mut nfsync) = (0, 0);
    for k in ((1 - parity)..slots).step_by(2) {
        let nb: C = ssync_phase
            .iter()
            .filter(|(j, _, _)| j.abs_diff(k) == 1)
            .map(|(_, p, _)| *p)
            .sum();
        if nb.norm() == 0.0 {
            continue;
        }
        let r = corr(at(k), &fs);
        if (r * nb.conj()).re < 0.0 {
            nfsync += 1;
        } else {
            fsync += 1;
        }
        add(
            at(k),
            &fs,
            r / r.norm().max(1e-30),
            r.norm() / SYNC_LEN as f64,
        );
    }
    let steps: Vec<f64> = ssync_phase
        .windows(2)
        .map(|w| (w[1].1 * w[0].1.conj()).arg())
        .collect();
    let mean = steps.iter().sum::<f64>() / steps.len().max(1) as f64;
    let sd =
        (steps.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / steps.len().max(1) as f64).sqrt();
    let snr_db = 10.0 * (sig / err.max(1e-30)).log10();
    debug!(
        offset,
        slots,
        peak,
        off_peak,
        fsync,
        nfsync,
        snr_db,
        phase_step_mean = mean,
        searched = known.is_none(),
        "slot sync {} offset {offset}: {slots} slots, correlation {peak:.3} on / {off_peak:.3} off, FSync {fsync} / !FSync {nfsync}, sync Es/N0 {snr_db:.2} dB, phase step {mean:+.3} rad",
        if known.is_none() {
            "found at"
        } else {
            "measured at the known"
        }
    );
    Some(SyncReport {
        offset,
        slots,
        peak,
        off_peak,
        ssync_parity: parity,
        fsync,
        nfsync,
        snr_db,
        phase_step_mean: mean,
        phase_step_rms: sd,
        ssync_phase,
    })
}

/// `max(|corr(x, n, fs)|, |corr(x, n, ss)|) / 24` for every position
/// `n < len`, bit for bit as [`corr`] and `Complex::norm` compute it (the
/// same operations in the same order per position), across positions in
/// structure-of-arrays form so the correlation loops vectorise, on all
/// cores.
fn position_mags(x: &[C], len: usize, fs: &[C; SYNC_LEN], ss: &[C; SYNC_LEN]) -> Vec<f64> {
    const B: usize = 256;
    let re: Vec<f64> = x.iter().map(|v| v.re).collect();
    let im: Vec<f64> = x.iter().map(|v| v.im).collect();
    let workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .clamp(1, 32);
    let chunk = len.div_ceil(workers).div_ceil(B).max(1) * B;
    let mut out = vec![0.0; len];
    let (re, im) = (&re, &im);
    std::thread::scope(|sc| {
        for (ci, o) in out.chunks_mut(chunk).enumerate() {
            sc.spawn(move || {
                for (bi, ob) in o.chunks_mut(B).enumerate() {
                    let n0 = ci * chunk + bi * B;
                    let n = ob.len();
                    // b · conj(a) summed from zero, a = the word's symbol:
                    // re += b.re·a.re − b.im·(−a.im), im += b.re·(−a.im) + b.im·a.re
                    let mut fr = [0.0f64; B];
                    let mut fi = [0.0f64; B];
                    let mut sr = [0.0f64; B];
                    let mut si = [0.0f64; B];
                    for t in 0..SYNC_LEN {
                        let (far, fai) = (fs[t].re, -fs[t].im);
                        let (sar, sai) = (ss[t].re, -ss[t].im);
                        let br = &re[n0 + t..n0 + t + n];
                        let bi = &im[n0 + t..n0 + t + n];
                        for j in 0..n {
                            fr[j] += br[j] * far - bi[j] * fai;
                            fi[j] += br[j] * fai + bi[j] * far;
                            sr[j] += br[j] * sar - bi[j] * sai;
                            si[j] += br[j] * sai + bi[j] * sar;
                        }
                    }
                    for j in 0..n {
                        ob[j] = fr[j].hypot(fi[j]).max(sr[j].hypot(si[j])) / SYNC_LEN as f64;
                    }
                }
            });
        }
    });
    out
}

/// The first offset in 0..[`SLOT`] with the largest `mag`, and that
/// value (strictly greater replaces, as in a sequential scan from 0;
/// non-positive values never win, offset 0 then).
fn search(mag: &(dyn Fn(usize) -> f64 + Sync)) -> (f64, usize) {
    let workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .clamp(1, 32);
    let chunk = SLOT.div_ceil(workers);
    let parts: Vec<(f64, usize)> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..SLOT)
            .step_by(chunk)
            .map(|lo| {
                sc.spawn(move || {
                    let (mut peak, mut offset) = (0.0, 0);
                    for off in lo..(lo + chunk).min(SLOT) {
                        let m = mag(off);
                        if m > peak {
                            (peak, offset) = (m, off);
                        }
                    }
                    (peak, offset)
                })
            })
            .collect();
        hs.into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    // chunks in order: a later chunk wins only with a strictly larger peak
    let (mut peak, mut offset) = (0.0, 0);
    for (m, o) in parts {
        if m > peak {
            (peak, offset) = (m, o);
        }
    }
    (peak, offset)
}

/// The output trim (symbol periods) that maximises the sync-symbol Es/N0
/// of the front end on the IQ excerpt `x` (12-bit components in `i16`),
/// with every other setting from `d`: a coarse search over `candidates`,
/// then the parabola through the best one and its neighbours. Returns the
/// trim and the Es/N0 the model reaches with it (rounded back to the
/// nearest candidate when the parabola is not concave).
///
/// The trim exists because the timing loop's non-data-aided detector
/// locks a fixed fraction of a symbol away from the instant of least
/// intersymbol interference on the lab's ISDB-S3 signals (crate README,
/// "The timing trim"); the known synchronisation symbols, which only the
/// host can find, measure where that instant is.
pub fn best_trim(
    x: &[[i16; 2]],
    d: &crate::params::Design,
    candidates: &[f64],
) -> Result<(f64, f64), crate::params::ParamError> {
    // the candidates run independently, one thread each
    let score = |t: f64| -> Result<f64, crate::params::ParamError> {
        let mut dd = d.clone();
        dd.trim = t;
        let mut fe = crate::fast::FastFrontend::new(crate::params::Params::design(&dd)?)?;
        let mut out = Vec::new();
        fe.process(x, &mut out);
        // the loop settles within the first slots: skip three
        let z: Vec<C> = out
            .iter()
            .skip(3 * SLOT)
            .map(|s| C::new(f64::from(s.zk[0]), f64::from(s.zk[1])))
            .collect();
        Ok(measure(&z).map_or(f64::NEG_INFINITY, |r| r.snr_db))
    };
    let scores: Vec<f64> = std::thread::scope(|sc| {
        let handles: Vec<_> = candidates
            .iter()
            .map(|&t| sc.spawn(move || score(t)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect::<Result<_, _>>()
    })?;
    let i = (0..scores.len())
        .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
        .ok_or(crate::params::ParamError::NoTrimCandidates)?;
    let chosen = |trim: f64, snr_db: f64| {
        debug!(
            trim,
            snr_db,
            candidates = candidates.len(),
            samples = x.len(),
            "trim {trim:+.3} symbol chosen of {} candidates on {} samples: sync Es/N0 {snr_db:.2} dB",
            candidates.len(),
            x.len()
        );
        (trim, snr_db)
    };
    if i == 0 || i + 1 == scores.len() {
        return Ok(chosen(candidates[i], scores[i]));
    }
    // Vertex of the parabola through the three points around the best.
    let (x0, x1, x2) = (candidates[i - 1], candidates[i], candidates[i + 1]);
    let (y0, y1, y2) = (scores[i - 1], scores[i], scores[i + 1]);
    let den = (x0 - x1) * (x0 - x2) * (x1 - x2);
    let a = (x2 * (y1 - y0) + x1 * (y0 - y2) + x0 * (y2 - y1)) / den;
    let b = (x2 * x2 * (y0 - y1) + x1 * x1 * (y2 - y0) + x0 * x0 * (y1 - y2)) / den;
    if a >= 0.0 {
        return Ok(chosen(x1, y1));
    }
    let v = (-b / (2.0 * a)).clamp(x0, x2);
    Ok(chosen(
        v,
        y1.max(a * v * v + b * v + (y1 - a * x1 * x1 - b * x1)),
    ))
}

/// The data symbols (main and TMCC) of each pair of slots that starts on
/// an SSync slot followed by another SSync slot two slots later, derotated
/// by the phase interpolated between the two and scaled to unit sync
/// amplitude: for ring statistics of the main signal. Pilot and sync
/// symbols are left out.
pub fn derotated_data(x: &[C], r: &SyncReport) -> Vec<C> {
    let rms = (x.iter().map(C::norm_sqr).sum::<f64>() / x.len() as f64).sqrt();
    let mut out = Vec::new();
    for w in r.ssync_phase.windows(2) {
        let (k0, p0, a0) = w[0];
        let (k1, p1, a1) = w[1];
        if k1 != k0 + 2 {
            continue;
        }
        // Phase moves linearly from slot k0's sync to slot k1's sync.
        let dphi = (p1 * p0.conj()).arg();
        let span = (2 * SLOT) as f64;
        let amp = 0.5 * (a0 + a1);
        for k in [k0, k0 + 1] {
            let base = r.offset + k * SLOT;
            for j in (SYNC_LEN + 32)..SLOT {
                let n = base + j;
                if n >= x.len() {
                    break;
                }
                let t = ((k - k0) * SLOT + j) as f64 / span;
                let ph = p0 * C::from_polar(1.0, dphi * t);
                out.push(x[n] / rms / ph / amp);
            }
        }
    }
    out
}

/// Wrap an angle to (−π, π].
pub fn wrap(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_trim_without_candidates_is_an_error() {
        let d = crate::params::Design::new(40e6, 33.7561e6, 0.03);
        assert!(matches!(
            best_trim(&[[0, 0]; 64], &d, &[]),
            Err(crate::params::ParamError::NoTrimCandidates)
        ));
    }

    /// A synthetic slot stream: sync words, random QPSK elsewhere, a
    /// phase ramp and noise of a given SNR.
    fn stream(slots: usize, snr_db: f64, ramp: f64) -> Vec<C> {
        let mut s = 0x2468_ace1u32;
        let mut rnd = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f64::from(s >> 8) / f64::from(1u32 << 24)
        };
        let sigma = (10f64.powf(-snr_db / 10.0) / 2.0).sqrt();
        let (fs, ss) = (sync_symbols(FSYNC), sync_symbols(SSYNC));
        let mut x = Vec::with_capacity(slots * SLOT);
        for k in 0..slots {
            for j in 0..SLOT {
                let v = if j < SYNC_LEN {
                    if k % 2 == 1 { ss[j] } else { fs[j] }
                } else {
                    C::from_polar(1.0, FRAC_PI_4 + TAU / 4.0 * (rnd() * 4.0).floor())
                };
                let n = x.len() as f64;
                // Box–Muller noise
                let (u1, u2) = (rnd().max(1e-12), rnd());
                let g = (-2.0 * u1.ln()).sqrt();
                let w = C::new(g * (TAU * u2).cos(), g * (TAU * u2).sin()) * sigma;
                x.push(v * C::from_polar(1.0, ramp * n) + w);
            }
        }
        x
    }

    #[test]
    fn position_mags_equal_corr_bit_for_bit() {
        let x = stream(5, 9.0, 3e-5);
        let (fs, ss) = (sync_symbols(FSYNC), sync_symbols(SSYNC));
        let len = x.len() - SYNC_LEN;
        let m = position_mags(&x, len, &fs, &ss);
        for (n, v) in m.iter().enumerate() {
            let r = corr(&x, n, &fs).norm().max(corr(&x, n, &ss).norm()) / SYNC_LEN as f64;
            assert_eq!(v.to_bits(), r.to_bits(), "position {n}");
        }
    }

    #[test]
    fn measure_at_the_found_offset_equals_measure() {
        let x = stream(30, 12.0, 2e-6);
        let a = measure(&x[777..]).unwrap();
        let b = measure_at(&x[777..], a.offset).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn sync_words_are_inverses() {
        let (f, n) = (sync_symbols(FSYNC), sync_symbols(NFSYNC));
        assert!(f.iter().zip(&n).all(|(a, b)| (a + b).norm() < 1e-12));
        assert_eq!(SLOT, 9296);
        assert_eq!(FRAME, 1_115_520);
    }

    #[test]
    fn measures_the_snr_of_a_synthetic_stream() {
        for snr in [8.0, 14.0, 20.0] {
            let x = stream(40, snr, 1e-6);
            let r = measure(&x[1000..]).unwrap();
            assert_eq!(r.offset, SLOT - 1000);
            assert!((r.snr_db - snr).abs() < 1.0, "{snr}: {r:?}");
            assert!(r.peak > 0.8 && r.off_peak < 0.4);
        }
    }
}
