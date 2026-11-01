//! The ISDB-S3 receiver over a block of symbols: synchronise, decode each
//! whole frame's TMCC and main signal, and hand every slot's data, in
//! order, to the caller with its stream identity.

use crate::Cplx;
use crate::main_signal::{
    FrameInput, Ldpc, decode_frames, learned_points, main_symbols, nominal_points, pilots,
};
use crate::sync::{synchronise, synchronise_at};
use crate::tmcc;
use isdbs3_core::slots::{Slot, layout};
use isdbs3_core::tmcc::{StreamType, TmccInfo};
use tracing::{debug, info};

/// Receiver options.
#[derive(Debug, Clone)]
pub struct RxOptions {
    /// Maximum LDPC iterations of the TMCC.
    pub max_iter: usize,
    /// The main signal's LDPC decoder.
    pub ldpc: Ldpc,
    /// Demap against the pilot-learned constellation (else the nominal
    /// one of B44 §3.8).
    pub learned_constellation: bool,
    /// Equalise the block with taps trained on the first frame's sync and
    /// pilot symbols ([`crate::eq`]).
    pub equalise: bool,
    /// Where the first slot head is expected (a stream cut on a frame
    /// boundary starts with one at 0): the synchronisation measures there
    /// instead of searching every offset of a slot, and searches only if
    /// no slot sync is found there.
    pub slot_offset: Option<usize>,
    /// Residual carrier offset of `z`, Hz (symbol `n` rotated by
    /// e^{j2π·f·n/Rs}), removed before anything else: a streaming caller
    /// passes the offset it tracks with [`Rx::residual_hz`]. 0 leaves the
    /// symbols as they are.
    ///
    /// The frame marker is told from !FSync by comparing an FSync-parity
    /// slot's phase with its two SSync neighbours, one slot (9 296 symbols)
    /// either side: their sum vanishes at |f| = Rs / (4 · 9 296) ≈ 908 Hz
    /// and flips sign beyond, after which the wrong word marks the frame
    /// and no TMCC decodes. The FPGA front end has no carrier loop, and a
    /// live stream drifted −22 Hz/s (−97 Hz → −830 Hz in 30 s, plan §0.18),
    /// so a stream must track it.
    pub carrier_hz: f64,
}

impl Default for RxOptions {
    fn default() -> Self {
        Self {
            max_iter: 50,
            ldpc: Ldpc::FIXED,
            learned_constellation: true,
            equalise: true,
            slot_offset: None,
            carrier_hz: 0.0,
        }
    }
}

/// One decoded slot, in transmission order.
#[derive(Debug, Clone)]
pub struct RxSlot {
    /// Frame number within this block (0 = first whole frame).
    pub frame: usize,
    /// Slot index 0 … 119.
    pub slot: usize,
    /// Relative stream number (TMCC).
    pub relative_stream: u8,
    /// Transmission stream id (TLV stream id or TS_ID) of that stream.
    pub stream_id: u16,
    /// Stream type of that stream (TMCC Table 3.11-4).
    pub stream_type: StreamType,
    /// Head-TLV indication from the slot header (first 16 bits).
    pub head: u16,
    /// The slot's data bytes, `None` if LDPC or BCH failed.
    pub data: Option<Vec<u8>>,
    /// LDPC iterations (None: not converged).
    pub ldpc_iterations: Option<usize>,
    /// BCH bits corrected (None: uncorrectable).
    pub bch_corrected: Option<usize>,
}

/// One frame's summary.
#[derive(Debug, Clone)]
pub struct RxFrame {
    /// Index of its slot #1 among the block's slots.
    pub slot_index: usize,
    /// Its TMCC, or the error.
    pub tmcc: Result<TmccInfo, tmcc::Error>,
    /// TMCC LDPC iterations.
    pub tmcc_iterations: Option<usize>,
    /// MER of the main symbols against the constellation used, dB.
    pub mer_db: Option<f64>,
}

/// What [`receive`] found.
#[derive(Debug, Clone, Default)]
pub struct Rx {
    /// Es/N0 on the sync symbols before equalisation, dB.
    pub sync_snr_db: f64,
    /// Es/N0 on the sync symbols after equalisation, dB (when equalised).
    pub sync_snr_eq_db: Option<f64>,
    /// The equaliser taps used.
    pub taps: Vec<Cplx>,
    /// Whole frames found.
    pub frames: Vec<RxFrame>,
    /// Every valid slot of every frame whose TMCC decoded, in order.
    pub slots: Vec<RxSlot>,
    /// Index in `z` of the first symbol (slot #1's sync) of every whole
    /// frame the synchronisation found, decoded or not: where a caller
    /// streaming symbols can cut its buffer on a frame boundary.
    pub frame_starts: Vec<usize>,
    /// Carrier offset left in the symbols after [`RxOptions::carrier_hz`]
    /// was removed, Hz, from the phase steps between SSync slots (`None`
    /// without enough of them). Add it to `carrier_hz` for the next block.
    pub residual_hz: Option<f64>,
    /// Seconds spent before the main signal's decode: derotation, slot
    /// sync, equaliser, each frame's TMCC and symbols (for the caller's
    /// timing report only).
    pub prep_s: f64,
    /// Seconds spent in the main signal's decode: noise, demapper, LDPC
    /// and BCH (see `prep_s`).
    pub ldpc_s: f64,
}

/// Lowest sync-symbol Es/N0 at which [`RxOptions::slot_offset`] is
/// trusted, dB (else the offset is searched).
pub const HINT_MIN_SNR_DB: f64 = 3.0;

/// The transmission modes of a TMCC in one line: `16APSK 7/9 ×60 + QPSK
/// 1/2 ×60` (the modes in use with their slot counts).
pub fn describe_tmcc(t: &TmccInfo) -> String {
    let modes: Vec<String> = t
        .modes
        .iter()
        .filter(|m| m.is_used())
        .filter_map(|m| {
            Some(format!(
                "{} {} ×{}",
                m.modulation()?.name(),
                m.rate()?,
                m.slots
            ))
        })
        .collect();
    if modes.is_empty() {
        "no mode in use".into()
    } else {
        modes.join(" + ")
    }
}

/// Whether two TMCCs assign the slots differently: modes, stream types
/// and ids, slot-to-stream mapping or the EWS flag (the pointers and the
/// order of change differ from frame to frame by design).
fn assignment_differs(a: &TmccInfo, b: &TmccInfo) -> bool {
    a.modes != b.modes
        || a.stream_type != b.stream_type
        || a.slot_stream != b.slot_stream
        || a.stream_id != b.stream_id
        || a.ews != b.ews
}

/// Run the receiver over symbols `z` (one per symbol, any scale).
///
/// Events: a slot sync found by the search is `info!` ("slot sync at
/// offset …"); one confirmed at [`RxOptions::slot_offset`] is the
/// continuation of a stream and `debug!`; no sync is `info!`. The first
/// TMCC decoded after a search, and every TMCC whose slot assignment
/// differs from the previous frame's, is `info!` ("TMCC: …", the modes
/// through [`describe_tmcc`]); the other frames' TMCCs, a TMCC that did
/// not decode, each frame's LDPC / BCH failure counts and the block's
/// totals are `debug!`.
///
/// It is [`receive_front`] then [`receive_back`]; a streaming caller may
/// run the two on different threads to overlap one block's sync with the
/// previous block's decode.
pub fn receive(z: &[Cplx], opt: &RxOptions) -> Rx {
    receive_back(receive_front(z, opt), opt)
}

/// A block of symbols through [`receive_front`]: synchronised, equalised
/// and each frame's TMCC, main symbols and pilots taken out, the main
/// signal not yet decoded. It owns what it holds (no borrow of the
/// symbols).
#[derive(Debug)]
pub struct Front {
    /// What is known before the decode: the sync, the equaliser, the frame
    /// starts, the residual carrier (`slots` and the frames' MER are
    /// filled in by [`receive_back`]).
    rx: Rx,
    /// A frame without slots, or `None` for the next of `ready`.
    order: Vec<Option<RxFrame>>,
    ready: Vec<Ready>,
    /// Symbols in the block.
    symbols: usize,
    /// The front stopped early (no sync): `rx` is the result as it is.
    early: bool,
}

impl Front {
    fn early(rx: Rx) -> Self {
        Self {
            rx,
            order: Vec::new(),
            ready: Vec::new(),
            symbols: 0,
            early: true,
        }
    }

    /// [`Rx::frame_starts`], known before the decode.
    pub fn frame_starts(&self) -> &[usize] {
        &self.rx.frame_starts
    }

    /// [`Rx::residual_hz`], known before the decode.
    pub fn residual_hz(&self) -> Option<f64> {
        self.rx.residual_hz
    }

    /// [`Rx::sync_snr_db`], known before the decode.
    pub fn sync_snr_db(&self) -> f64 {
        self.rx.sync_snr_db
    }

    /// [`Rx::sync_snr_eq_db`], known before the decode.
    pub fn sync_snr_eq_db(&self) -> Option<f64> {
        self.rx.sync_snr_eq_db
    }

    /// [`Rx::prep_s`] so far.
    pub fn prep_s(&self) -> f64 {
        self.rx.prep_s
    }
}

/// The first half of [`receive`] (its events are those of the sync, the
/// equaliser and the TMCCs): everything up to the main signal's decode.
pub fn receive_front(z: &[Cplx], opt: &RxOptions) -> Front {
    let t0 = std::time::Instant::now();
    let derotated;
    let z: &[Cplx] = if opt.carrier_hz == 0.0 {
        z
    } else {
        derotated = derotate(z, opt.carrier_hz);
        &derotated
    };
    // the hint holds when the sync words correlate there (≈ 13 dB on a live
    // stream; a wrong offset leaves them uncorrelated, ≤ 0 dB)
    let hinted = opt
        .slot_offset
        .and_then(|o| synchronise_at(z, o))
        .filter(|s| s.sync_snr_db > HINT_MIN_SNR_DB);
    let continuation = hinted.is_some();
    let Some(sy0) = hinted.or_else(|| synchronise(z)) else {
        info!(
            symbols = z.len(),
            carrier_hz = opt.carrier_hz,
            "no slot sync in {} symbols",
            z.len()
        );
        return Front::early(Rx {
            prep_s: t0.elapsed().as_secs_f64(),
            ..Rx::default()
        });
    };
    let residual_hz = crate::sync::residual_hz(&sy0);
    if continuation {
        debug!(
            offset = sy0.offset,
            frames = sy0.frames.len(),
            sync_snr_db = sy0.sync_snr_db,
            residual_hz = residual_hz.unwrap_or(f64::NAN),
            "slot sync continues at offset {}: {} whole frames, sync Es/N0 {:.1} dB, residual carrier {:+.1} Hz",
            sy0.offset,
            sy0.frames.len(),
            sy0.sync_snr_db,
            residual_hz.unwrap_or(f64::NAN)
        );
    } else {
        info!(
            offset = sy0.offset,
            frames = sy0.frames.len(),
            sync_snr_db = sy0.sync_snr_db,
            residual_hz = residual_hz.unwrap_or(f64::NAN),
            hinted = opt.slot_offset.is_some(),
            "slot sync {}at offset {}: {} whole frames, sync Es/N0 {:.1} dB, residual carrier {:+.1} Hz",
            if opt.slot_offset.is_some() {
                "found again "
            } else {
                ""
            },
            sy0.offset,
            sy0.frames.len(),
            sy0.sync_snr_db,
            residual_hz.unwrap_or(f64::NAN)
        );
    }
    let mut rx = Rx {
        sync_snr_db: sy0.sync_snr_db,
        residual_hz,
        ..Rx::default()
    };
    // equalise with taps trained on the first frame whose TMCC decodes
    let mut zeq = None;
    if opt.equalise {
        let es = 10f64.powf(sy0.sync_snr_db / 10.0);
        for &f in &sy0.frames {
            let Some(l) = tmcc::llrs(z, &sy0.slots, f, 1.0 / (2.0 * es)) else {
                break;
            };
            let Some(lay) = tmcc::decode(&l, opt.max_iter)
                .ok()
                .and_then(|t| layout(&t.info))
            else {
                continue;
            };
            if let Some(w) = crate::eq::train(z, &sy0.slots, f, &lay) {
                zeq = Some(crate::eq::apply(z, &w));
                rx.taps = w;
            }
            break;
        }
    }
    let z: &[Cplx] = zeq.as_deref().unwrap_or(z);
    let sy = if zeq.is_some() {
        // the equaliser's output is aligned with its input (centred taps):
        // the slot heads are where they were, only their phases and the
        // SNR change
        let Some(s) = synchronise_at(z, sy0.offset) else {
            rx.prep_s = t0.elapsed().as_secs_f64();
            return Front::early(rx);
        };
        debug!(
            taps = rx.taps.len(),
            sync_snr_db = sy0.sync_snr_db,
            sync_snr_eq_db = s.sync_snr_db,
            "equaliser of {} taps: sync Es/N0 {:.1} → {:.1} dB",
            rx.taps.len(),
            sy0.sync_snr_db,
            s.sync_snr_db
        );
        rx.sync_snr_eq_db = Some(s.sync_snr_db);
        s
    } else {
        sy0
    };
    rx.frame_starts = sy.frames.iter().map(|&f| sy.slots[f].at).collect();
    let es_n0 = 10f64.powf(sy.sync_snr_db / 10.0);
    // Each frame's TMCC, main symbols and pilots, frames in parallel; then
    // the frames in order up to the first incomplete one, as a sequential
    // pass would stop there.
    let preps: Vec<Prep> = parallel_map(&sy.frames, |&f| prepare(z, &sy.slots, f, es_n0, opt));
    let mut ready: Vec<Ready> = Vec::new();
    // a frame without slots, or None for the next of `ready`
    let mut order: Vec<Option<RxFrame>> = Vec::new();
    // the last TMCC decoded in this block, for the change events
    let mut last_tmcc: Option<TmccInfo> = None;
    for (fi, (prep, &f)) in preps.into_iter().zip(&sy.frames).enumerate() {
        let start = sy.slots[f].at;
        match prep {
            Prep::Stop => break,
            Prep::TmccError(e) => {
                debug!(
                    frame = fi,
                    start, "frame {fi} (symbol {start}): TMCC not decoded: {e}"
                );
                order.push(Some(RxFrame {
                    slot_index: f,
                    tmcc: Err(e),
                    tmcc_iterations: None,
                    mer_db: None,
                }));
            }
            Prep::NoLayout(t) => {
                debug!(
                    frame = fi,
                    start,
                    ldpc_iterations = t.ldpc_iterations,
                    "frame {fi} (symbol {start}): TMCC decoded ({}) but its slot allocation is inconsistent",
                    describe_tmcc(&t.info)
                );
                order.push(Some(RxFrame {
                    slot_index: f,
                    tmcc: Ok(t.info),
                    tmcc_iterations: Some(t.ldpc_iterations),
                    mer_db: None,
                }));
            }
            Prep::Ready { t, lay, ms, pil } => {
                let changed = last_tmcc
                    .as_ref()
                    .is_some_and(|p| assignment_differs(p, &t.info));
                if changed || (last_tmcc.is_none() && !continuation) {
                    info!(
                        frame = fi,
                        start,
                        ldpc_iterations = t.ldpc_iterations,
                        bch_corrected = t.bch_corrected,
                        change = t.info.change,
                        ews = t.info.ews,
                        "TMCC{}: {} (order of change {}, LDPC {} iterations, BCH {} bit(s) corrected)",
                        if changed { " changed" } else { "" },
                        describe_tmcc(&t.info),
                        t.info.change,
                        t.ldpc_iterations,
                        t.bch_corrected
                    );
                } else {
                    debug!(
                        frame = fi,
                        start,
                        ldpc_iterations = t.ldpc_iterations,
                        bch_corrected = t.bch_corrected,
                        "frame {fi} (symbol {start}): TMCC decoded, LDPC {} iterations, BCH {} bit(s) corrected",
                        t.ldpc_iterations,
                        t.bch_corrected
                    );
                }
                last_tmcc = Some(t.info.clone());
                order.push(None);
                ready.push((fi, f, t, lay, ms, pil));
            }
        }
    }
    rx.prep_s = t0.elapsed().as_secs_f64();
    Front {
        rx,
        order,
        ready,
        symbols: z.len(),
        early: false,
    }
}

/// The second half of [`receive`]: the main signal of each frame
/// [`receive_front`] made ready, decoded (`opt` as given to it).
pub fn receive_back(front: Front, opt: &RxOptions) -> Rx {
    let t1 = std::time::Instant::now();
    let Front {
        mut rx,
        order,
        ready,
        symbols,
        early,
    } = front;
    if early {
        return rx;
    }
    let pts: Vec<_> = ready
        .iter()
        .map(|(_, _, _, lay, _, pil)| {
            move |m, r| {
                if opt.learned_constellation
                    && let Some(p) = learned_points(pil, lay, m)
                {
                    return p;
                }
                nominal_points(m, r)
            }
        })
        .collect();
    let n0s: Vec<f64> = ready
        .iter()
        .zip(&pts)
        .map(|((_, _, _, lay, ms, _), p)| noise_of(ms, lay, p))
        .collect();
    let inputs: Vec<FrameInput<'_>> = ready
        .iter()
        .zip(&pts)
        .zip(&n0s)
        .map(|(((_, _, _, lay, ms, _), p), &n0)| FrameInput {
            ms,
            layout: lay,
            n0,
            pts_for: p,
        })
        .collect();
    let decoded = decode_frames(&inputs, &opt.ldpc);
    // one item per ready frame (decode_frames: one result per input)
    let mut done = ready.into_iter().zip(n0s).zip(decoded);
    for o in order {
        if let Some(frame) = o {
            rx.frames.push(frame);
            continue;
        }
        let Some((((fi, f, t, lay, _, _), n0), frame_slots)) = done.next() else {
            break;
        };
        let (mut slots, mut ldpc_failed, mut bch_failed, mut iterations) =
            (0usize, 0usize, 0usize, 0usize);
        for d in frame_slots {
            let s: &Slot = &lay[d.slot];
            let rel = t.info.slot_stream[d.slot];
            let head = u16::from_be_bytes([bits_byte(&d.header, 0), bits_byte(&d.header, 1)]);
            let ok = d.bch_corrected.is_some();
            slots += 1;
            match d.ldpc_iterations {
                Some(n) if ok => iterations += n,
                Some(n) => {
                    iterations += n;
                    bch_failed += 1;
                }
                None => ldpc_failed += 1,
            }
            rx.slots.push(RxSlot {
                frame: fi,
                slot: s.index,
                relative_stream: rel,
                stream_id: t.info.stream_id[usize::from(rel & 0x0F)],
                stream_type: t.info.stream_type[usize::from(rel & 0x0F)],
                head,
                data: ok.then_some(d.data),
                ldpc_iterations: d.ldpc_iterations,
                bch_corrected: d.bch_corrected,
            });
        }
        let mer_db = -10.0 * n0.log10();
        debug!(
            frame = fi,
            slots,
            ldpc_failed,
            bch_failed,
            mean_ldpc_iterations = iterations as f64 / (slots - ldpc_failed).max(1) as f64,
            mer_db,
            "frame {fi}: {slots} slots, LDPC failed {ldpc_failed}, BCH failed {bch_failed}, mean {:.2} LDPC iterations; main MER {mer_db:.1} dB",
            iterations as f64 / (slots - ldpc_failed).max(1) as f64
        );
        rx.frames.push(RxFrame {
            slot_index: f,
            tmcc: Ok(t.info),
            tmcc_iterations: Some(t.ldpc_iterations),
            mer_db: Some(mer_db),
        });
    }
    debug!(
        symbols,
        frames = rx.frames.len(),
        tmcc_ok = rx.frames.iter().filter(|f| f.tmcc.is_ok()).count(),
        slots = rx.slots.len(),
        failed = rx.slots.iter().filter(|s| s.data.is_none()).count(),
        sync_snr_db = rx.sync_snr_db,
        sync_snr_eq_db = rx.sync_snr_eq_db.unwrap_or(f64::NAN),
        "{} symbols: {} frames, TMCC ok {}; {} slots, {} failed; sync Es/N0 {:.1} dB{}",
        symbols,
        rx.frames.len(),
        rx.frames.iter().filter(|f| f.tmcc.is_ok()).count(),
        rx.slots.len(),
        rx.slots.iter().filter(|s| s.data.is_none()).count(),
        rx.sync_snr_db,
        rx.sync_snr_eq_db
            .map_or(String::new(), |e| format!(" ({e:.1} dB equalised)"))
    );
    rx.ldpc_s = t1.elapsed().as_secs_f64();
    rx
}

/// `z` with a carrier offset of `hz` removed: symbol `n` multiplied by
/// e^{−j2π·hz·n/Rs}. The phasor is recomputed every 4 096 symbols from the
/// index, so rounding does not accumulate over long blocks. Those 4 096-
/// symbol runs are independent, so they are shared out between all cores
/// (the output is the same on any number of threads): on a streamed
/// 24-frame call this pass alone took 0.13 s of 0.8 s on one core.
pub fn derotate(z: &[Cplx], hz: f64) -> Vec<Cplx> {
    const RUN: usize = 4096;
    let w = -std::f64::consts::TAU * hz / isdbs3_core::params::SYMBOL_RATE;
    let step = Cplx::from_polar(1.0, w);
    let mut out = vec![Cplx::new(0.0, 0.0); z.len()];
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    // whole runs per thread, at least 16 of them
    let per = z.len().div_ceil(workers).div_ceil(RUN).max(16) * RUN;
    std::thread::scope(|sc| {
        for (t, (o, i)) in out.chunks_mut(per).zip(z.chunks(per)).enumerate() {
            sc.spawn(move || {
                for (j, (oc, c)) in o.chunks_mut(RUN).zip(i.chunks(RUN)).enumerate() {
                    let k = t * (per / RUN) + j;
                    let mut p = Cplx::from_polar(1.0, w * (k * RUN) as f64);
                    for (y, &v) in oc.iter_mut().zip(c) {
                        *y = v * p;
                        p *= step;
                    }
                }
            });
        }
    });
    out
}

/// A frame ready to decode: its index among the block's frames, its slot
/// #1 index, TMCC, layout, main symbols and pilots.
type Ready = (usize, usize, tmcc::Decoded, Vec<Slot>, Vec<Cplx>, Vec<Cplx>);

/// One frame before its main signal is decoded.
enum Prep {
    /// The frame is incomplete: this and every later frame are dropped.
    Stop,
    /// Its TMCC failed.
    TmccError(tmcc::Error),
    /// Its TMCC decoded but gives no slot layout.
    NoLayout(tmcc::Decoded),
    /// Ready to decode.
    Ready {
        t: tmcc::Decoded,
        lay: Vec<Slot>,
        ms: Vec<Cplx>,
        pil: Vec<Cplx>,
    },
}

fn prepare(
    z: &[Cplx],
    slots: &[crate::sync::SlotHead],
    f: usize,
    es_n0: f64,
    opt: &RxOptions,
) -> Prep {
    let Some(l) = tmcc::llrs(z, slots, f, 1.0 / (2.0 * es_n0)) else {
        return Prep::Stop;
    };
    let t = match tmcc::decode(&l, opt.max_iter) {
        Ok(d) => d,
        Err(e) => return Prep::TmccError(e),
    };
    let Some(lay) = layout(&t.info) else {
        return Prep::NoLayout(t);
    };
    match (main_symbols(z, slots, f), pilots(z, slots, f)) {
        (Some(ms), Some(pil)) => Prep::Ready { t, lay, ms, pil },
        _ => Prep::Stop,
    }
}

/// `f` over `items` on all cores, results in order.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let chunk = items.len().div_ceil(workers).max(1);
    let f = &f;
    std::thread::scope(|sc| {
        let hs: Vec<_> = items
            .chunks(chunk)
            .map(|c| sc.spawn(move || c.iter().map(f).collect::<Vec<R>>()))
            .collect();
        hs.into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    })
}

fn bits_byte(bits: &[u8], i: usize) -> u8 {
    bits[8 * i..8 * i + 8]
        .iter()
        .fold(0u8, |a, &b| (a << 1) | (b & 1))
}

/// Mean squared distance of the main symbols to their nearest point of
/// their slot's constellation (a sample of each slot): the N0 the
/// receiver demaps with.
pub fn noise_of(
    ms: &[Cplx],
    lay: &[Slot],
    pts_for: &dyn Fn(isdbs3_core::Modulation, isdbs3_core::CodeRate) -> Vec<Cplx>,
) -> f64 {
    let (mut e, mut n) = (0.0, 0usize);
    for s in lay.iter().filter(|s| !s.dummy) {
        let pts = pts_for(s.modulation, s.rate);
        for y in ms[s.symbol_start..s.symbol_start + s.symbols]
            .iter()
            .step_by(16)
        {
            e += pts
                .iter()
                .map(|p| (y - p).norm_sqr())
                .fold(f64::MAX, f64::min);
            n += 1;
        }
    }
    e / n.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parallel [`derotate`] gives exactly what one pass over the
    /// 4 096-symbol runs in order gives, whatever the length.
    #[test]
    fn derotate_matches_a_sequential_pass_bit_for_bit() {
        for len in [0, 1, 4095, 4096, 70_001, 1_000_000] {
            let z: Vec<Cplx> = (0..len)
                .map(|n| Cplx::new((n % 7) as f64 - 3.0, (n % 5) as f64 - 2.0))
                .collect();
            let hz = -767.25;
            let w = -std::f64::consts::TAU * hz / isdbs3_core::params::SYMBOL_RATE;
            let step = Cplx::from_polar(1.0, w);
            let mut want = Vec::with_capacity(len);
            for (k, c) in z.chunks(4096).enumerate() {
                let mut p = Cplx::from_polar(1.0, w * (k * 4096) as f64);
                for &v in c {
                    want.push(v * p);
                    p *= step;
                }
            }
            let got = derotate(&z, hz);
            assert_eq!(got.len(), len);
            assert!(
                got.iter()
                    .zip(&want)
                    .all(|(a, b)| a.re.to_bits() == b.re.to_bits()
                        && a.im.to_bits() == b.im.to_bits()),
                "length {len}"
            );
        }
    }
    /// An `Rx` as text without its timings, for comparing two runs.
    fn untimed(mut r: Rx) -> String {
        r.prep_s = 0.0;
        r.ldpc_s = 0.0;
        format!("{r:?}")
    }

    /// [`receive`] is [`receive_front`] then [`receive_back`] on symbols
    /// that are no ISDB-S3 signal (whatever path the front takes on them;
    /// a real stream is the opt-in test in tests/captures.rs).
    #[test]
    fn front_then_back_is_receive_on_junk() {
        let n = 3 * isdbs3_core::params::FRAME_SYMBOLS;
        let z: Vec<Cplx> = (0..n)
            .map(|k| {
                Cplx::new(
                    ((k * 7919) % 13) as f64 - 6.0,
                    ((k * 104_729) % 11) as f64 - 5.0,
                )
            })
            .collect();
        let opt = RxOptions::default();
        let f = receive_front(&z, &opt);
        assert_eq!(untimed(receive_back(f, &opt)), untimed(receive(&z, &opt)));
    }
}
