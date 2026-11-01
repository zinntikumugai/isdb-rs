//! Frame and slot synchronisation of ISDB-S3 symbols, and the carrier
//! phase of every modulation slot from its 24 synchronisation symbols
//! (ARIB STD-B44 v2.1 Fig. 3.3-1, §3.8).
//!
//! Slot heads come from [`isdb_frontend::s3sync::measure`]: the slot sync
//! correlation at the 9 296-symbol period. SSync (36715Ah) heads one slot
//! parity. The other parity carries FSync (52F866h) or !FSync (AD0799h);
//! against the phase of the neighbouring SSync slots one slot in 120 has
//! the opposite sign to the other 59 of its parity. That slot is the frame
//! head, modulation slot #1 (Fig. 3.3-1 puts FSync in slot #1;
//! SPEC-NOTES "Frame marker").
//!
//! The carrier phase of a slot is the angle of its sync correlation (the
//! word's sign removed); between two slot heads it is interpolated
//! linearly, unwrapped.

use crate::Cplx;
use isdb_frontend::s3sync::{self, SYNC_LEN, sync_symbols};
use isdbs3_core::params::{
    FRAME_SYMBOLS, FSYNC, MAIN_RUN, MOD_SLOT, PILOT_SYMBOLS, RUNS, SLOTS, SSYNC, SYMBOL_RATE,
    SYNC_SYMBOLS, TMCC_RUN,
};

/// One modulation slot found in the symbol stream.
#[derive(Debug, Clone, Copy)]
pub struct SlotHead {
    /// Index of its first symbol in the stream.
    pub at: usize,
    /// Slot number within its frame (0 = slot #1), if the frame head was
    /// found before it.
    pub number: Option<usize>,
    /// Carrier phase at the slot head (unit phasor).
    pub phase: Cplx,
    /// Amplitude of the sync symbols (stream scale).
    pub amp: f64,
}

/// The synchronised stream.
#[derive(Debug, Clone)]
pub struct Synced {
    /// Every whole slot head.
    pub slots: Vec<SlotHead>,
    /// Indices into `slots` of each frame head (slot #1).
    pub frames: Vec<usize>,
    /// Es/N0 on the sync symbols, dB (the `s3sync` figure).
    pub sync_snr_db: f64,
    /// Offset of the first slot head (0 … 9 295).
    pub offset: usize,
    /// Parity of the SSync slots among `slots` (0 or 1).
    pub ssync_parity: usize,
}

fn corr(x: &[Cplx], at: usize, r: &[Cplx; SYNC_LEN]) -> Cplx {
    r.iter()
        .zip(&x[at..at + SYNC_LEN])
        .map(|(a, b)| b * a.conj())
        .sum()
}

/// Find slots and frames in `z` (one value per symbol, any scale).
pub fn synchronise(z: &[Cplx]) -> Option<Synced> {
    synced(z, s3sync::measure(z)?)
}

/// [`synchronise`] with the slot offset known (`Synced::offset` of an
/// earlier call on symbols at the same positions, e.g. before an
/// equaliser whose output is aligned with its input): no search.
pub fn synchronise_at(z: &[Cplx], offset: usize) -> Option<Synced> {
    synced(z, s3sync::measure_at(z, offset)?)
}

fn synced(z: &[Cplx], rep: s3sync::SyncReport) -> Option<Synced> {
    let fs = sync_symbols(FSYNC);
    let ss = sync_symbols(SSYNC);
    let n = rep.slots;
    // per slot: correlation with its parity's word
    let raw: Vec<(usize, Cplx, bool)> = (0..n)
        .map(|k| {
            let at = rep.offset + k * MOD_SLOT;
            let is_ss = k % 2 == rep.ssync_parity;
            let c = corr(z, at, if is_ss { &ss } else { &fs });
            (at, c, is_ss)
        })
        .collect();
    // sign of each FSync-parity slot against its SSync neighbours
    let sign: Vec<Option<bool>> = (0..n)
        .map(|k| {
            if raw[k].2 {
                return None;
            }
            let nb: Cplx = [k.wrapping_sub(1), k + 1]
                .iter()
                .filter(|&&j| j < n)
                .map(|&j| raw[j].1 / raw[j].1.norm().max(1e-30))
                .sum();
            (nb.norm() > 0.0).then(|| (raw[k].1 * nb.conj()).re < 0.0)
        })
        .collect();
    let neg = sign.iter().filter(|s| **s == Some(true)).count();
    let pos = sign.iter().filter(|s| **s == Some(false)).count();
    // the minority sign marks the frame head
    let marker = neg < pos;
    let mut frames = Vec::new();
    let mut slots = Vec::with_capacity(n);
    let mut number: Option<usize> = None;
    for k in 0..n {
        if sign[k] == Some(marker) {
            // a frame head; accept it only at the right spacing once locked
            if frames.last().is_none_or(|&f| k - f == SLOTS) {
                frames.push(k);
                number = Some(0);
            }
        } else if let Some(m) = number.as_mut() {
            *m = (*m + 1) % SLOTS;
        }
        let (at, c, _) = raw[k];
        // remove the word's sign: a slot negative against its SSync
        // neighbours carries the inverted word
        let c = if sign[k] == Some(true) { -c } else { c };
        slots.push(SlotHead {
            at,
            number,
            phase: c / c.norm().max(1e-30),
            amp: c.norm() / SYNC_LEN as f64,
        });
    }
    let _ = FRAME_SYMBOLS;
    Some(Synced {
        slots,
        frames,
        sync_snr_db: rep.snr_db,
        offset: rep.offset,
        ssync_parity: rep.ssync_parity,
    })
}

/// The carrier offset left in the symbols `s` was measured on, Hz: the
/// mean phase step between consecutive SSync slots (two slots apart, the
/// same word, so no sign to resolve), averaged as phasors. Unambiguous
/// within ±Rs / (4 · 9 296) ≈ ±908 Hz; `None` with fewer than two SSync
/// slots.
pub fn residual_hz(s: &Synced) -> Option<f64> {
    let ss: Vec<&SlotHead> = s
        .slots
        .iter()
        .enumerate()
        .filter(|(k, _)| k % 2 == s.ssync_parity)
        .map(|(_, h)| h)
        .collect();
    let sum: Cplx = ss.windows(2).map(|w| w[1].phase * w[0].phase.conj()).sum();
    (ss.len() >= 2 && sum.norm() > 0.0).then(|| {
        sum.arg() / std::f64::consts::TAU * isdbs3_core::params::SYMBOL_RATE
            / (2.0 * MOD_SLOT as f64)
    })
}

/// Spacing of the carrier offsets [`residual_hz`] cannot tell apart, Hz:
/// Rs / (2 · 9 296) ≈ 1 815.6 Hz (one turn per SSync-to-SSync step).
pub const ALIAS_HZ: f64 = SYMBOL_RATE / (2.0 * MOD_SLOT as f64);

/// A coarse carrier offset of the symbols `s` was measured on, Hz, from
/// their TMCC symbols: π/2-shift BPSK (§3.8), whose square carries no data,
/// in runs of 4 every 140 symbols; the mean phase step of the squared runs
/// from one run to the next in each slot is 2 · 2π · f · 140 / Rs.
/// Unambiguous within ±Rs / (4 · 140) ≈ ±60 kHz, and a few Hz accurate on
/// a frame of slots at the Es/N0 of a live stream, so it tells which of
/// [`residual_hz`]'s aliases ([`ALIAS_HZ`] apart) is the carrier. `None`
/// without a whole slot.
pub fn coarse_hz(z: &[Cplx], s: &Synced) -> Option<f64> {
    const STEP: usize = MAIN_RUN + TMCC_RUN;
    // the square of the bit-0 point: j on an even slot symbol, −j on an
    // odd one (`tmcc::bpsk_soft`)
    let sq0 = |j: usize| {
        if j.is_multiple_of(2) {
            Cplx::new(0.0, 1.0)
        } else {
            Cplx::new(0.0, -1.0)
        }
    };
    let mut acc = Cplx::new(0.0, 0.0);
    for h in s.slots.iter().take_while(|h| h.at + MOD_SLOT <= z.len()) {
        let mut prev: Option<Cplx> = None;
        for r in 0..RUNS {
            let j0 = SYNC_SYMBOLS + PILOT_SYMBOLS + r * STEP + MAIN_RUN;
            let run: Cplx = (j0..j0 + TMCC_RUN)
                .map(|j| {
                    let v = z[h.at + j];
                    v * v * sq0(j).conj()
                })
                .sum();
            if let Some(p) = prev {
                acc += run * p.conj();
            }
            prev = Some(run);
        }
    }
    (acc.norm() > 0.0)
        .then(|| acc.arg() * SYMBOL_RATE / (2.0 * std::f64::consts::TAU * STEP as f64))
}

/// The carrier offset of the symbols `s` was measured on, Hz: the alias of
/// [`residual_hz`] (precise, but ambiguous by multiples of [`ALIAS_HZ`])
/// nearest [`coarse_hz`] (unambiguous). The residual as it is when the
/// coarse estimate falls near the middle between two aliases (within a
/// quarter of their spacing of it: no clear choice) or is missing.
pub fn carrier_hz(z: &[Cplx], s: &Synced) -> Option<f64> {
    let fine = residual_hz(s)?;
    let Some(coarse) = coarse_hz(z, s) else {
        return Some(fine);
    };
    let k = ((coarse - fine) / ALIAS_HZ).round();
    let off = coarse - fine - k * ALIAS_HZ;
    Some(if off.abs() < ALIAS_HZ / 4.0 {
        fine + k * ALIAS_HZ
    } else {
        fine
    })
}

/// The carrier phase at symbol `j` (0 … 9 295) of slot `i`, interpolated
/// linearly between slot `i`'s head and the next one's (held after the
/// last slot), and the amplitude likewise.
pub fn phase_at(s: &[SlotHead], i: usize, j: usize) -> (Cplx, f64) {
    PhaseRamp::of(s, i).at(j)
}

/// [`phase_at`] for every symbol of one slot: the phase step to the next
/// head is taken once per slot instead of once per symbol, and
/// [`PhaseRamp::at`] gives the same values bit for bit.
#[derive(Debug, Clone, Copy)]
pub struct PhaseRamp {
    head: SlotHead,
    /// The phase step to the next head and its amplitude; `None` after
    /// the last slot (held).
    next: Option<(f64, f64)>,
}

impl PhaseRamp {
    /// The ramp of slot `i`.
    pub fn of(s: &[SlotHead], i: usize) -> Self {
        let a = s[i];
        Self {
            head: a,
            next: s
                .get(i + 1)
                .map(|b| ((b.phase * a.phase.conj()).arg(), b.amp)),
        }
    }

    /// The carrier phase and amplitude at symbol `j` of the slot.
    #[inline]
    pub fn at(&self, j: usize) -> (Cplx, f64) {
        let a = self.head;
        let Some((d, b_amp)) = self.next else {
            return (a.phase, a.amp);
        };
        let t = j as f64 / MOD_SLOT as f64;
        (
            a.phase * Cplx::from_polar(1.0, d * t),
            a.amp + (b_amp - a.amp) * t,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rx::derotate;
    use isdbs3_core::params::SYMBOL_RATE;
    use std::f64::consts::FRAC_1_SQRT_2;

    /// `frames` frames of slots: each slot its sync word (FSync on slot #1,
    /// !FSync on the other odd slots, SSync on the even ones — the sign
    /// structure the synchronisation reads), then QPSK-like filler from a
    /// fixed LCG with π/2-shift BPSK on the TMCC symbols, all at unit amplitude plus Gaussian-ish noise; `lead`
    /// filler symbols first, so the first slot head is not at 0. Rotated by
    /// `hz` (symbol n × e^{j2π·hz·n/Rs}).
    fn stream(frames: usize, lead: usize, hz: f64) -> (Vec<Cplx>, Vec<usize>) {
        let (fs, nfs, ss) = (
            sync_symbols(FSYNC),
            sync_symbols(isdbs3_core::params::NFSYNC),
            sync_symbols(SSYNC),
        );
        let mut s = 0x1234_5678_u64;
        let mut rnd = || {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            s >> 33
        };
        let mut z = Vec::new();
        let filler = |rnd: &mut dyn FnMut() -> u64| {
            let q = rnd();
            Cplx::new(
                if q & 1 == 0 {
                    FRAC_1_SQRT_2
                } else {
                    -FRAC_1_SQRT_2
                },
                if q & 2 == 0 {
                    FRAC_1_SQRT_2
                } else {
                    -FRAC_1_SQRT_2
                },
            )
        };
        for _ in 0..lead {
            z.push(filler(&mut rnd));
        }
        let mut heads = Vec::new();
        for _f in 0..frames {
            for k in 0..SLOTS {
                if k == 0 {
                    heads.push(z.len());
                }
                let w = match k {
                    0 => &fs,
                    _ if k % 2 == 0 => &nfs,
                    _ => &ss,
                };
                z.extend_from_slice(w);
                let tmcc: Vec<usize> = crate::tmcc::tmcc_offsets().collect();
                for j in SYNC_LEN..MOD_SLOT {
                    if tmcc.binary_search(&j).is_ok() {
                        // π/2-shift BPSK, as the TMCC is (§3.8)
                        let b = if rnd() & 1 == 0 { 1.0 } else { -1.0 };
                        let a = if j % 2 == 0 { 1.0 } else { 3.0 };
                        z.push(Cplx::from_polar(b, a * std::f64::consts::FRAC_PI_4));
                    } else {
                        z.push(filler(&mut rnd));
                    }
                }
            }
        }
        // noise ≈ 20 dB below the symbols
        for v in &mut z {
            let (a, b) = (
                rnd() as f64 / f64::from(1u32 << 31) - 1.0,
                rnd() as f64 / f64::from(1u32 << 31) - 1.0,
            );
            *v += Cplx::new(a, b) * 0.17;
        }
        (derotate(&z, -hz), heads)
    }

    fn found_heads(z: &[Cplx]) -> Vec<usize> {
        let s = synchronise(z).expect("slot sync");
        s.frames.iter().map(|&f| s.slots[f].at).collect()
    }

    /// The frame heads are found where they are with a small carrier
    /// offset, and the residual estimate measures that offset.
    #[test]
    fn frame_heads_and_residual_with_a_small_offset() {
        for hz in [0.0, 300.0, -600.0] {
            let (z, heads) = stream(3, 1_000, hz);
            assert_eq!(found_heads(&z), heads, "{hz} Hz");
            let r = residual_hz(&synchronise(&z).unwrap()).unwrap();
            assert!((r - hz).abs() < 5.0, "{hz} Hz measured as {r:.1} Hz");
        }
    }

    /// Near ±Rs / (4 · 9 296) ≈ ±908 Hz the two SSync neighbours of an
    /// FSync-parity slot cancel (their rotations ±θ add to 2cos θ ≈ 0), the
    /// word's sign is read from noise, the minority rule picks random slots
    /// as frame heads and no TMCC decodes. (Further out the flip is common
    /// to every slot and the minority is the marker again — but a drifting
    /// stream passes through the point: a live one reached it in 30 s,
    /// plan §0.18.) With the offset removed first ([`RxOptions::carrier_hz`],
    /// tracked by a streaming caller) the heads are found.
    #[test]
    fn an_offset_at_the_neighbour_limit_loses_the_frame_heads_unless_removed() {
        let limit = SYMBOL_RATE / (4.0 * MOD_SLOT as f64);
        assert!((limit - 907.8).abs() < 1.0, "{limit}");
        for hz in [limit, -limit] {
            let (z, heads) = stream(3, 1_000, hz);
            assert_ne!(
                found_heads(&z),
                heads,
                "{hz} Hz: heads found without removing the offset"
            );
            let z2 = derotate(&z, hz);
            assert_eq!(found_heads(&z2), heads, "{hz} Hz removed");
        }
        // and well beyond it the common flip keeps the minority rule right
        let (z, heads) = stream(3, 1_000, 1_400.0);
        assert_eq!(found_heads(&z), heads, "1 400 Hz");
    }

    /// Beyond ±908 Hz the SSync phase steps measure an alias of the
    /// carrier, [`ALIAS_HZ`] apart (a 12 s slice from the middle of a
    /// 3-hour BS-14 recording, 2026-11-01: +2 539 Hz measured as +723 Hz,
    /// another +9 220 Hz as +142 Hz, and no TMCC decoded). The TMCC
    /// symbols' estimate tells the aliases apart: [`carrier_hz`] gives the
    /// carrier, within the unambiguous range as well as far beyond it.
    #[test]
    fn carrier_beyond_the_alias_spacing_is_resolved() {
        assert!((ALIAS_HZ - 1815.6).abs() < 0.1, "{ALIAS_HZ}");
        for hz in [
            0.0, 300.0, -600.0, 1_400.0, 2_539.0, -1_092.0, 9_220.0, -14_000.0, 40_000.0,
        ] {
            let (z, _) = stream(2, 1_000, hz);
            let s = synchronise(&z).expect("slot sync");
            let r = residual_hz(&s).unwrap();
            let k = ((hz - r) / ALIAS_HZ).round();
            assert!(
                (r + k * ALIAS_HZ - hz).abs() < 5.0,
                "{hz} Hz: residual {r:.1} Hz"
            );
            let c = coarse_hz(&z, &s).unwrap();
            assert!((c - hz).abs() < 100.0, "{hz} Hz: coarse {c:.1} Hz");
            let f = carrier_hz(&z, &s).unwrap();
            assert!((f - hz).abs() < 5.0, "{hz} Hz: carrier {f:.1} Hz");
        }
    }
}
