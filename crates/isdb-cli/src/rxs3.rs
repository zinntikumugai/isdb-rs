//! `isdb rx --system isdbs3`: ISDB-S3 reception, from a recording or a
//! radio, to one TLV stream file per TLV stream id.
//!
//! The chain is `isdbs3-phy`'s: the single-carrier front end with the
//! timing trim, slot and frame sync, the pilot-trained equaliser, TMCC,
//! the main signal per frame (LDPC, BCH), then `isdb-tlv`'s assembler per
//! stream. A TLV stream file is the TLV packets back to back, each with
//! its 4-byte header (ARIB STD-B32 Part 3 §3.5). Like ISDB-S, a radio
//! delivers one contiguous capture after another, so each stream is
//! discontinuous at every block boundary (reported).

use crate::bands::Channel;
use crate::satpipe::{self, Block, Timing};
use anyhow::{Context, Result, bail};
use isdb_iq::{C32, Recording};
use isdb_radio::{DeviceSpec, RadioConfig};
use isdb_tlv::Assembler;
use isdb_tlv::stats::StreamStats;
use isdbs3_core::params::FRAME_SYMBOLS;
use isdbs3_phy::frontend::{Prior, Symbols, Trim, symbols_with_prior};
use isdbs3_phy::rx::{Front, Rx, RxOptions, receive, receive_back, receive_front};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

/// Default sample rate from a radio: ≥ (1 + 0.03) × 33.756 Mbaud = 34.8
/// Msps; 40 Msps is the rate the ISDB-S front end shares (VERIFICATION.md
/// "Symbol front end").
pub const DEFAULT_RATE: f64 = 40e6;
/// Default analog bandwidth.
pub const DEFAULT_BW: f64 = 56e6;
/// TLV stream id of the relative streams the TMCC leaves unassigned.
pub const UNUSED_ID: u16 = 0xFFFF;

/// Per-stream output files.
pub struct Outputs {
    template: Option<String>,
    only: Vec<u16>,
    files: BTreeMap<u16, Box<dyn Write>>,
    /// TLV packets written per stream id.
    pub written: BTreeMap<u16, u64>,
}

impl Outputs {
    /// `template`: a path with `{tlv_id}` (or `{ts_id}`) replaced by the
    /// stream id (4 hex digits); without a placeholder exactly one stream
    /// must be selected with `only`. `-` writes to stdout.
    pub fn new(template: Option<&str>, only: &[u16]) -> Result<Self> {
        if let Some(t) = template
            && !t.contains("{tlv_id}")
            && !t.contains("{ts_id}")
            && only.len() != 1
        {
            bail!(
                "--out {t:?} names one file: select one TLV stream with --ts-id, or put {{tlv_id}} in the path for one file per stream"
            );
        }
        Ok(Self {
            template: template.map(str::to_owned),
            only: only.to_vec(),
            files: BTreeMap::new(),
            written: BTreeMap::new(),
        })
    }

    fn write(&mut self, id: u16, bytes: &[u8]) -> Result<()> {
        let Some(t) = &self.template else {
            return Ok(());
        };
        if id == UNUSED_ID || !(self.only.is_empty() || self.only.contains(&id)) {
            return Ok(());
        }
        let w = match self.files.entry(id) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                let path = t
                    .replace("{tlv_id}", &format!("{id:04x}"))
                    .replace("{ts_id}", &format!("{id:04x}"));
                let w: Box<dyn Write> = if path == "-" {
                    Box::new(std::io::stdout().lock())
                } else {
                    // written on its own thread: a slow disk must not
                    // hold up the decoder of a live stream
                    Box::new(crate::bgwrite::BackgroundFile::new(
                        crate::capture::create(std::path::Path::new(&path), true)
                            .with_context(|| format!("create {path}"))?,
                    ))
                };
                e.insert(w)
            }
        };
        w.write_all(bytes)?;
        *self.written.entry(id).or_default() += 1;
        Ok(())
    }

    /// Flush every file.
    pub fn flush(&mut self) -> Result<()> {
        for w in self.files.values_mut() {
            w.flush()?;
        }
        Ok(())
    }
}

/// Counters over a whole run.
#[derive(Debug)]
pub struct Stats {
    /// Seconds spent in the front end (all blocks), in the receiver, and
    /// in the whole run (the stages overlap, so the wall time is less
    /// than their sum).
    pub front_s: f64,
    /// See `front_s`.
    pub back_s: f64,
    /// See `front_s`.
    pub wall_s: f64,
    /// Blocks that reused the previous block's acquisition.
    pub reused: u64,
    /// The LDPC decoder the blocks are decoded with.
    pub ldpc: isdbs3_phy::main_signal::Ldpc,
    /// Blocks decoded.
    pub blocks: u64,
    /// Samples read.
    pub samples: u64,
    /// Whole frames found.
    pub frames: u64,
    /// Frames whose TMCC decoded.
    pub tmcc_ok: u64,
    /// Valid slots decoded / LDPC failures / BCH failures.
    pub slots: u64,
    /// See `slots`.
    pub ldpc_failed: u64,
    /// See `slots`.
    pub bch_failed: u64,
    /// LDPC iterations (sum) and BCH bits corrected (sum).
    pub ldpc_iterations: u64,
    /// See `ldpc_iterations`.
    pub bch_corrected: u64,
    mer_sum: f64,
    mer_n: u64,
    /// Sync Es/N0 before and after equalisation (last block), dB.
    pub sync_snr_db: f64,
    /// See `sync_snr_db`.
    pub sync_snr_eq_db: Option<f64>,
    /// Carrier offset and trim of the last block.
    pub carrier_offset_hz: f64,
    /// See `carrier_offset_hz`.
    pub trim: f64,
    /// The last TMCC.
    pub tmcc: Option<isdbs3_core::TmccInfo>,
    /// Per stream id: assembler counters and content counters.
    pub streams: BTreeMap<u16, (isdb_tlv::tlv::Stats, StreamStats, usize)>,
    /// Streamed reception (FPGA or a words file): the word stream's
    /// counters; `None` for block-wise reception.
    pub words: Option<crate::symstream::WordStats>,
    /// Carrier offset left in the last decoded symbols after the tracked
    /// offset was removed, Hz.
    pub residual_hz: Option<f64>,
    /// Streamed reception: the carrier offset tracked in the symbols (the
    /// drift since the front end was configured), Hz, and its extremes.
    pub tracked_hz: f64,
    /// See `tracked_hz`.
    pub tracked_range_hz: (f64, f64),
    /// Decoded by the ISDB-S3 chain (`Slots`: the FPGA's, a recording of
    /// it, or the CPU model): its blocks, status and losses
    /// ([`crate::slotrx`]); `None` for the host receiver.
    pub chain: Option<crate::slotrx::ChainStats>,
    /// Which receiver decoded the slots, for the summary.
    pub decoded_by: &'static str,
    /// Seconds of the receiver's stages, summed over its calls: before
    /// the main signal's decode (sync, equaliser, TMCC, symbols), the
    /// decode (LDPC), and the TLV assembly and writes.
    pub prep_s: f64,
    /// See `prep_s`.
    pub ldpc_s: f64,
    /// See `prep_s`.
    pub emit_s: f64,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            front_s: 0.0,
            back_s: 0.0,
            wall_s: 0.0,
            reused: 0,
            ldpc: isdbs3_phy::main_signal::Ldpc::FIXED,
            blocks: 0,
            samples: 0,
            frames: 0,
            tmcc_ok: 0,
            slots: 0,
            ldpc_failed: 0,
            bch_failed: 0,
            ldpc_iterations: 0,
            bch_corrected: 0,
            mer_sum: 0.0,
            mer_n: 0,
            sync_snr_db: 0.0,
            sync_snr_eq_db: None,
            carrier_offset_hz: 0.0,
            trim: 0.0,
            residual_hz: None,
            tracked_hz: 0.0,
            tracked_range_hz: (0.0, 0.0),
            prep_s: 0.0,
            ldpc_s: 0.0,
            emit_s: 0.0,
            tmcc: None,
            streams: BTreeMap::new(),
            words: None,
            chain: None,
            decoded_by: DECODED_BY_HOST,
        }
    }
}

/// [`Stats::decoded_by`]: the host's receiver (`isdbs3_phy::rx`).
pub const DECODED_BY_HOST: &str = "host receiver";

impl Stats {
    /// Add one frame's main-signal MER, dB.
    pub fn add_mer(&mut self, db: f64) {
        self.mer_sum += db;
        self.mer_n += 1;
    }
}

impl Stats {
    /// Mean main-signal MER, dB.
    pub fn mer_db(&self) -> Option<f64> {
        (self.mer_n > 0).then(|| self.mer_sum / self.mer_n as f64)
    }

    /// JSON summary.
    pub fn json(&self) -> Value {
        json!({
            "system": "ISDB-S3",
            "decoded_by": self.decoded_by,
            "chain": self.chain.as_ref().map(crate::slotrx::ChainStats::json),
            "blocks": self.blocks,
            "time": json!({
                "wall_s": self.wall_s,
                "front_end_s": self.front_s,
                "receiver_s": self.back_s,
                "reused_acquisitions": self.reused,
            }),
            "stream": self.words.map(|w| json!({
                "blocks": w.blocks,
                "symbols": w.symbols,
                "losses": w.losses,
                "blocks_lost": w.blocks_lost,
                "blocks_dropped": w.blocks_dropped,
                "words_skipped": w.words_skipped,
                "resyncs": w.resyncs,
                "max_queued_symbols": w.max_queued_symbols,
                "stopped": w.stopped,
                "tracked_carrier_hz": self.tracked_hz,
                "tracked_carrier_range_hz": [self.tracked_range_hz.0, self.tracked_range_hz.1],
            })),
            "sync_snr_db": self.sync_snr_db,
            "sync_snr_eq_db": self.sync_snr_eq_db,
            "carrier_offset_hz": self.carrier_offset_hz,
            "trim": self.trim,
            "frames": self.frames,
            "tmcc_ok": self.tmcc_ok,
            "slots": self.slots,
            "ldpc_failed": self.ldpc_failed,
            "bch_failed": self.bch_failed,
            "ldpc_mean_iterations": self.ldpc_iterations as f64 / (self.slots - self.ldpc_failed).max(1) as f64,
            "bch_corrected_bits": self.bch_corrected,
            "main_mer_db": self.mer_db(),
            "sync_esn0_db": self.sync_snr_db,
            "sync_esn0_equalised_db": self.sync_snr_eq_db,
            "carrier_offset_hz": self.carrier_offset_hz,
            "trim": self.trim,
            "tmcc": self.tmcc.as_ref().map(|t| json!({
                "modes": t.modes.iter().filter(|m| m.is_used()).map(|m| json!({
                    "modulation": m.modulation().map(|x| x.name()),
                    "code_rate": m.rate().map(|r| format!("{}/120", r.numerator())),
                    "slots": m.slots,
                    "obo_db": f64::from(m.obo) / 10.0,
                })).collect::<Vec<_>>(),
                "streams": (0..16).filter(|&k| t.stream_type[k] != isdbs3_core::StreamType::None).map(|k| json!({
                    "relative": k,
                    "type": t.stream_type[k].code(),
                    "id": t.stream_id[k],
                    "slots": t.slot_stream.iter().filter(|&&s| usize::from(s) == k).count(),
                })).collect::<Vec<_>>(),
                "ews": t.ews,
            })),
            "streams": self.streams.iter().map(|(id, (a, s, slots))| json!({
                "id": id,
                "slots_per_frame": slots,
                "tlv_packets": a.packets,
                "lost_slots": a.lost_slots,
                "pointer_mismatches": a.pointer_mismatches,
                "skipped_bytes": a.skipped_bytes,
                "by_type": s.by_type.iter().map(|(k, (n, b))| (k.clone(), json!({"packets": n, "bytes": b}))).collect::<serde_json::Map<_, _>>(),
                "ip_errors": s.ip_errors,
                "mmtp_packet_ids": s.mmtp.iter().map(|(k, (n, _))| (format!("{k:#06x}"), json!(n))).collect::<serde_json::Map<_, _>>(),
                "mmt_si_tables": s.mmt_tables.iter().map(|(k, n)| (format!("{k:#04x} {}", isdb_tlv::mmtp::table_name(*k)), json!(n))).collect::<serde_json::Map<_, _>>(),
                "tlv_si_tables": s.tlv_si.iter().map(|(k, n)| (format!("{k:#04x} {}", isdb_tlv::si::table_name(*k)), json!(n))).collect::<serde_json::Map<_, _>>(),
                "mpu_payloads": s.mpu,
                "scrambled_mpu_payloads": s.mpu_with_extension,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The front end of one block: symbols, and the acquisition the next
/// block of the same signal may reuse (`prior`).
pub fn front(
    x: &[C32],
    fs: f64,
    carrier_offset_hz: Option<f64>,
    prior: Option<&Prior>,
) -> Result<(Symbols, Prior)> {
    symbols_with_prior(x, fs, carrier_offset_hz, Trim::Auto, prior)
        .context("ISDB-S3 front end parameters for this sample rate")
}

/// The rest of one block of `samples` samples: receive its symbols `s`,
/// count and write.
pub fn back(s: &Symbols, samples: u64, out: &mut Outputs, stats: &mut Stats) -> Result<()> {
    stats.blocks += 1;
    stats.samples += samples;
    stats.carrier_offset_hz = s.carrier_offset_hz;
    stats.trim = s.trim;
    if s.z.len() < 2 * FRAME_SYMBOLS {
        return Ok(());
    }
    // a fresh assembler per block: the blocks are not contiguous
    let mut asm: BTreeMap<u16, Assembler> = BTreeMap::new();
    decode(&s.z, false, false, 0.0, &mut asm, out, stats)?;
    close(asm, stats);
    Ok(())
}

/// Receive the symbols `z` and write their TLV packets through the
/// per-stream assemblers `asm` (which carry partial packets across calls
/// on a contiguous stream). With `keep_last` (a stream that continues),
/// the last whole frame found and everything after it are left for the
/// next call: only the frames before it are counted and written, and the
/// returned index is where it starts. Without, every frame is taken and
/// `z.len()` is returned. `carrier_hz` is removed from `z` first
/// ([`RxOptions::carrier_hz`]); the offset left after it is in
/// `stats.residual_hz`.
#[allow(clippy::too_many_arguments)]
pub fn decode(
    z: &[isdbs3_phy::Cplx],
    keep_last: bool,
    aligned: bool,
    carrier_hz: f64,
    asm: &mut BTreeMap<u16, Assembler>,
    out: &mut Outputs,
    stats: &mut Stats,
) -> Result<usize> {
    let r = receive(
        z,
        &RxOptions {
            ldpc: stats.ldpc,
            // a continuing stream is cut on a frame start (slot head at 0)
            slot_offset: aligned.then_some(0),
            carrier_hz,
            ..RxOptions::default()
        },
    );
    stats.residual_hz = r.residual_hz;
    stats.sync_snr_db = r.sync_snr_db;
    stats.sync_snr_eq_db = r.sync_snr_eq_db;
    let upto = keep_last.then(|| r.frame_starts.last().copied()).flatten();
    if keep_last && upto.is_none() {
        // no whole frame yet: keep everything for the next call
        stats.prep_s += r.prep_s;
        return Ok(0);
    }
    emit(&r, upto, asm, out, stats)?;
    Ok(upto.unwrap_or(z.len()))
}

/// Count the frames and slots of `r` that start before `upto` (all of
/// them with `None`) and write their TLV packets through `asm`: the part
/// of [`decode`] after the receiver.
fn emit(
    r: &Rx,
    upto: Option<usize>,
    asm: &mut BTreeMap<u16, Assembler>,
    out: &mut Outputs,
    stats: &mut Stats,
) -> Result<()> {
    let te = std::time::Instant::now();
    stats.prep_s += r.prep_s;
    stats.ldpc_s += r.ldpc_s;
    // `r.frames[i]`, `r.frame_starts[i]` and the slots with `frame == i`
    // are the same frame (the receiver pushes one entry per frame in sync
    // order, up to the first incomplete one); a frame is taken when it
    // starts before `upto`
    let taken =
        |fi: usize| -> bool { upto.is_none_or(|u| r.frame_starts.get(fi).is_some_and(|&a| a < u)) };
    for (fi, f) in r.frames.iter().enumerate() {
        if !taken(fi) {
            continue;
        }
        stats.frames += 1;
        if let Ok(t) = &f.tmcc {
            stats.tmcc_ok += 1;
            stats.tmcc = Some(t.clone());
        }
        if let Some(m) = f.mer_db {
            stats.add_mer(m);
        }
    }
    for sl in r.slots.iter().filter(|sl| taken(sl.frame)) {
        count_slot(stats, sl.ldpc_iterations, sl.bch_corrected);
        push_slot(sl.stream_id, sl.data.as_deref(), sl.head, asm, out, stats)?;
    }
    stats.emit_s += te.elapsed().as_secs_f64();
    Ok(())
}

/// Count one valid slot's LDPC and BCH results.
pub fn count_slot(stats: &mut Stats, ldpc_iterations: Option<usize>, bch: Option<usize>) {
    stats.slots += 1;
    match ldpc_iterations {
        Some(i) => stats.ldpc_iterations += i as u64,
        None => stats.ldpc_failed += 1,
    }
    match bch {
        Some(c) => stats.bch_corrected += c as u64,
        None => stats.bch_failed += u64::from(ldpc_iterations.is_some()),
    }
}

/// One valid slot of stream `stream_id` (`data` `None`: LDPC or BCH
/// failed; `head` its head-TLV indication) through the stream's assembler
/// in `asm`; its complete TLV packets counted and written.
pub fn push_slot(
    stream_id: u16,
    data: Option<&[u8]>,
    head: u16,
    asm: &mut BTreeMap<u16, Assembler>,
    out: &mut Outputs,
    stats: &mut Stats,
) -> Result<()> {
    let mut pk = Vec::new();
    asm.entry(stream_id)
        .or_default()
        .push_slot(data, head, &mut pk);
    let e = stats.streams.entry(stream_id).or_default();
    if let Some(t) = &stats.tmcc {
        e.2 = t
            .slot_stream
            .iter()
            .filter(|&&k| t.stream_id[usize::from(k & 0x0F)] == stream_id)
            .count();
    }
    for p in &pk {
        e.1.add(p);
        out.write(stream_id, &p.encode()?)?;
    }
    Ok(())
}

/// Add the assemblers' counters to `stats` (at the end of a contiguous
/// stretch: a block, a loss, the end of the run).
pub fn close(asm: BTreeMap<u16, Assembler>, stats: &mut Stats) {
    for (id, a) in asm {
        let e = &mut stats.streams.entry(id).or_default().0;
        e.packets += a.stats.packets;
        e.lost_slots += a.stats.lost_slots;
        e.pointer_mismatches += a.stats.pointer_mismatches;
        e.skipped_bytes += a.stats.skipped_bytes;
    }
}

/// The summary lines printed at the end.
pub fn report(stats: &Stats, secs: f64) {
    if let Some(w) = stats.words {
        info!(
            target: "isdb::rx",
            wall_s = stats.wall_s,
            seconds = secs,
            speed = secs / stats.wall_s.max(1e-9),
            back_s = stats.back_s,
            max_queued_symbols = w.max_queued_symbols,
            "time: {:.2} s for {secs:.3} s of signal ({:.2}× real time, startup included); receiver {:.2} s; at most {:.2} s of symbols queued behind the receiver",
            stats.wall_s,
            secs / stats.wall_s.max(1e-9),
            stats.back_s,
            w.max_queued_symbols as f64 / isdbs3_phy::frontend::system().symbol_rate_hz
        );
        info!(
            target: "isdb::rx",
            prep_s = stats.prep_s,
            ldpc_s = stats.ldpc_s,
            emit_s = stats.emit_s,
            "stages: sync and preparation {:.2} s, LDPC {:.2} s, TLV {:.2} s",
            stats.prep_s,
            stats.ldpc_s,
            stats.emit_s
        );
        info!(
            target: "isdb::rx",
            blocks = w.blocks,
            losses = w.losses,
            blocks_lost = w.blocks_lost,
            blocks_dropped = w.blocks_dropped,
            words_skipped = w.words_skipped,
            resyncs = w.resyncs,
            stopped = w.stopped,
            "stream: {} blocks of symbols; {} loss(es) ({} blocks missing, {} of them dropped on the host, {} words skipped), {} resync(s){}: each is a discontinuity in the TLV streams",
            w.blocks,
            w.losses,
            w.blocks_lost,
            w.blocks_dropped,
            w.words_skipped,
            w.resyncs,
            if w.stopped {
                "; stopped with symbols still queued"
            } else {
                ""
            }
        );
        info!(
            target: "isdb::rx",
            tracked_hz = stats.tracked_hz,
            tracked_min_hz = stats.tracked_range_hz.0,
            tracked_max_hz = stats.tracked_range_hz.1,
            carrier_offset_hz = stats.carrier_offset_hz,
            "carrier drift tracked in the symbols: {:+.0} Hz at the end ({:+.0} … {:+.0} Hz over the run), on top of the front end's {:+.0} Hz",
            stats.tracked_hz,
            stats.tracked_range_hz.0,
            stats.tracked_range_hz.1,
            stats.carrier_offset_hz
        );
    } else if stats.chain.is_some() {
        info!(
            target: "isdb::rx",
            wall_s = stats.wall_s,
            seconds = secs,
            speed = secs / stats.wall_s.max(1e-9),
            "time: {:.2} s for {secs:.3} s of signal ({:.2}× real time); the slots were decoded before the host (the host assembles TLV packets)",
            stats.wall_s,
            secs / stats.wall_s.max(1e-9)
        );
    } else if stats.wall_s > 0.0 {
        info!(
            target: "isdb::rx",
            wall_s = stats.wall_s,
            seconds = secs,
            speed = secs / stats.wall_s,
            front_s = stats.front_s,
            back_s = stats.back_s,
            reused = stats.reused,
            blocks = stats.blocks,
            "time: {:.2} s for {secs:.3} s of signal ({:.2}× real time); front end {:.2} s, receiver {:.2} s (overlapping); {} of {} block(s) reused the previous acquisition",
            stats.wall_s,
            secs / stats.wall_s,
            stats.front_s,
            stats.back_s,
            stats.reused,
            stats.blocks
        );
    }
    info!(
        target: "isdb::rx",
        seconds = secs,
        blocks = stats.blocks,
        frames = stats.frames,
        tmcc_ok = stats.tmcc_ok,
        slots = stats.slots,
        ldpc_failed = stats.ldpc_failed,
        bch_failed = stats.bch_failed,
        bch_corrected = stats.bch_corrected,
        ldpc_iterations = stats.ldpc_iterations as f64 / (stats.slots - stats.ldpc_failed).max(1) as f64,
        sync_snr_db = stats.sync_snr_db,
        sync_snr_eq_db = stats.sync_snr_eq_db.unwrap_or(f64::NAN),
        mer_db = stats.mer_db().unwrap_or(f64::NAN),
        decoded_by = stats.decoded_by,
        "done: ISDB-S3 (decoded by the {}), {:.3} s of signal{}; {} frames, TMCC ok {}; {} slots, LDPC failed {}, BCH failed {} ({} bits corrected), mean {:.2} LDPC iterations; {}main MER {}",
        stats.decoded_by,
        secs,
        if stats.chain.is_some() {
            String::new()
        } else {
            format!(" in {} block(s)", stats.blocks)
        },
        stats.frames,
        stats.tmcc_ok,
        stats.slots,
        stats.ldpc_failed,
        stats.bch_failed,
        stats.bch_corrected,
        stats.ldpc_iterations as f64 / (stats.slots - stats.ldpc_failed).max(1) as f64,
        // the chain reports no sync Es/N0 (its status has the correlation
        // peak, in the chain line)
        if stats.chain.is_some() {
            String::new()
        } else {
            format!(
                "sync Es/N0 {:.1} dB{}; ",
                stats.sync_snr_db,
                stats
                    .sync_snr_eq_db
                    .map_or(String::new(), |e| format!(" ({e:.1} dB equalised)"))
            )
        },
        stats.mer_db().map_or("-".into(), |m| format!("{m:.1} dB"))
    );
    if let Some(t) = &stats.tmcc {
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
        info!(target: "isdb::rx", "  TMCC: {}", modes.join(" + "));
    }
    for (id, (a, s, slots)) in &stats.streams {
        let name = if *id == UNUSED_ID {
            "unassigned slots (stream id FFFFh, not written)".to_string()
        } else {
            format!("TLV stream {id:#06x}")
        };
        let types: Vec<String> = s
            .by_type
            .iter()
            .map(|(k, (n, _))| format!("{k} {n}"))
            .collect();
        let mut tables: Vec<String> = s
            .mmt_tables
            .keys()
            .map(|k| isdb_tlv::mmtp::table_name(*k).to_string())
            .chain(
                s.tlv_si
                    .keys()
                    .map(|k| isdb_tlv::si::table_name(*k).to_string()),
            )
            .collect();
        tables.dedup();
        info!(
            target: "isdb::rx",
            stream_id = id,
            slots,
            packets = a.packets,
            lost_slots = a.lost_slots,
            pointer_mismatches = a.pointer_mismatches,
            "  {name} ({slots} slots/frame): {} TLV packets ({}); lost slots {}, head-indication mismatches {}; signalling: {}",
            a.packets,
            types.join(", "),
            a.lost_slots,
            a.pointer_mismatches,
            if tables.is_empty() {
                "-".into()
            } else {
                tables.join(", ")
            }
        );
    }
    if stats.blocks > 1 {
        info!(
            target: "isdb::rx",
            blocks = stats.blocks,
            "  the {} blocks were captured one after another with gaps between them: each stream is discontinuous at every block boundary",
            stats.blocks
        );
    }
}

/// Decode the blocks `produce` sends through [`satpipe::run`] and count
/// the stage times; `after` runs after each block is written (status
/// lines).
fn pipeline(
    produce: impl FnOnce(&std::sync::mpsc::SyncSender<Block>) -> Result<()> + Send,
    carrier_offset_hz: Option<f64>,
    out: &mut Outputs,
    stats: &mut Stats,
    mut after: impl FnMut(&Stats),
) -> Result<()> {
    let stats = std::cell::RefCell::new(stats);
    satpipe::run(
        produce,
        |b: &Block, prior: Option<&Prior>| front(&b.x, b.fs, carrier_offset_hz, prior),
        |s: Symbols, n: u64| back(&s, n, out, &mut stats.borrow_mut()),
        |t: Timing| {
            let mut st = stats.borrow_mut();
            st.front_s = t.front_s;
            st.back_s = t.back_s;
            st.wall_s = t.wall_s;
            st.reused = t.reused;
            after(&st);
        },
    )
}

/// Offline: every contiguous segment of a recording.
pub fn run_file(
    path: &str,
    max_samples: Option<u64>,
    carrier_offset_hz: Option<f64>,
    out: &mut Outputs,
    stats: &mut Stats,
) -> Result<f64> {
    let mut rec = Recording::open(std::path::Path::new(path), None, None).with_context(|| {
        format!("{path}: an ISDB-S3 recording needs its SigMF metadata (rate and format)")
    })?;
    let fs = rec.sample_rate;
    pipeline(
        move |tx| {
            let mut left = max_samples.unwrap_or(u64::MAX);
            for seg in rec.segments() {
                if left == 0 {
                    break;
                }
                let n = (seg.end - seg.start).min(left);
                left -= n;
                let x = rec.read(seg.start, usize::try_from(n)?)?;
                if tx.send(Block { x, fs }).is_err() {
                    break;
                }
            }
            Ok(())
        },
        carrier_offset_hz,
        out,
        stats,
        |_| {},
    )?;
    Ok(stats.samples as f64 / fs)
}

/// Lowest sync Es/N0 of a decode call whose residual carrier is added to
/// the tracked offset, dB (a call that found no real slot sync measures
/// noise).
pub const TRACK_MIN_SNR_DB: f64 = 3.0;

/// A chunk's front for the back of [`run_stream`], or the end of a
/// contiguous stretch, in stream order.
enum Back {
    /// A front and where the chunk is cut (`None`: taken whole).
    Chunk {
        front: Box<Front>,
        upto: Option<usize>,
    },
    /// The stretch ended: close its assemblers.
    End,
}

/// Streamed reception: decode the `Symbols` words of `bytes` (a radio's
/// FPGA front end, or a words file) as one continuous stream, cut on frame
/// boundaries ([`crate::symstream`]); a loss in the words ends a contiguous
/// stretch (the streams are discontinuous there, reported). Returns the
/// seconds of signal (symbols / `symbol_rate_hz`).
///
/// Two threads: the stream's thread runs each chunk's front
/// ([`receive_front`]: sync, equaliser, each frame's TMCC and symbols),
/// which decides where the chunk is cut and the carrier the next one
/// removes, and hands it on; this thread decodes the fronts
/// ([`receive_back`]: LDPC, BCH) and writes their TLV packets, in order.
/// The next chunk's sync so overlaps the previous chunk's LDPC, and the
/// output is what one thread doing both gives.
pub fn run_stream(
    bytes: crate::symstream::Bytes,
    source: crate::symstream::Source,
    symbol_rate_hz: f64,
    status: bool,
    stop: &AtomicBool,
    out: &mut Outputs,
    stats: &mut Stats,
) -> Result<f64> {
    let t0 = std::time::Instant::now();
    let ldpc = stats.ldpc;
    let stats = std::sync::Mutex::new(stats);
    let lock = || {
        stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    // one front waits while the back decodes another: at most two chunks'
    // fronts exist at once besides the one being made
    let (tx, rx) = std::sync::mpsc::sync_channel::<Back>(1);
    let (w, back) = std::thread::scope(|sc| {
        // the front owns the senders: when it returns they drop and the
        // back's loop ends
        let front = sc.spawn(move || {
            let tx_end = tx.clone();
            // whether the buffer starts where the previous call cut (a frame start)
            let mut aligned = false;
            // the carrier offset tracked in the symbols: the front end's NCO
            // is fixed at the acquisition's offset and the signal drifts
            // (−22 Hz/s on a live stream, plan §0.18), so each call removes
            // what the previous ones measured. Kept across losses: the drift
            // is the signal's.
            let mut tracked = 0.0f64;
            let mut next_status = 1.0;
            crate::symstream::run(
                bytes,
                source,
                crate::symstream::CHUNK_FRAMES * FRAME_SYMBOLS,
                stop,
                |z, cut| {
                    let t = std::time::Instant::now();
                    let more = cut == crate::symstream::Cut::More;
                    let f = receive_front(
                        z,
                        &RxOptions {
                            ldpc,
                            // a continuing stream is cut on a frame start (slot head at 0)
                            slot_offset: aligned.then_some(0),
                            carrier_hz: tracked,
                            ..RxOptions::default()
                        },
                    );
                    let upto = more.then(|| f.frame_starts().last().copied()).flatten();
                    {
                        let mut st = lock();
                        st.residual_hz = f.residual_hz();
                        st.sync_snr_db = f.sync_snr_db();
                        st.sync_snr_eq_db = f.sync_snr_eq_db();
                        if let Some(r) = st.residual_hz
                            && st.sync_snr_db > TRACK_MIN_SNR_DB
                        {
                            tracked += r;
                            st.tracked_hz = tracked;
                            st.tracked_range_hz = (
                                st.tracked_range_hz.0.min(tracked),
                                st.tracked_range_hz.1.max(tracked),
                            );
                        }
                        st.back_s += t.elapsed().as_secs_f64();
                    }
                    let used = if more && upto.is_none() {
                        // no whole frame yet: keep everything for the next call
                        lock().prep_s += f.prep_s();
                        0
                    } else {
                        let front = Box::new(f);
                        if tx.send(Back::Chunk { front, upto }).is_err() {
                            bail!("the receiver's decode stopped");
                        }
                        upto.unwrap_or(z.len())
                    };
                    // the next buffer starts at the cut (a frame start) when
                    // this call cut on one; after a loss or with nothing cut, not
                    aligned = more && used > 0;
                    Ok(used)
                },
                |_| {
                    // a contiguous stretch ended: its assemblers close after
                    // the chunks before it are written
                    if tx_end.send(Back::End).is_err() {
                        bail!("the receiver's decode stopped");
                    }
                    Ok(())
                },
                |w| {
                    let mut st = lock();
                    st.words = Some(*w);
                    st.wall_s = t0.elapsed().as_secs_f64();
                    let secs = w.symbols as f64 / symbol_rate_hz;
                    if status && secs >= next_status {
                        next_status = secs.floor() + 1.0;
                        info!(
                            target: "isdb::rx",
                            seconds = secs,
                            wall_s = st.wall_s,
                            frames = st.frames,
                            tmcc_ok = st.tmcc_ok,
                            slots = st.slots,
                            ldpc_failed = st.ldpc_failed,
                            bch_failed = st.bch_failed,
                            losses = w.losses,
                            blocks_lost = w.blocks_lost,
                            mer_db = st.mer_db().unwrap_or(f64::NAN),
                            queued_s = w.queued_symbols as f64 / symbol_rate_hz,
                            "{secs:.2} s of signal after {:.2} s: {} frames, TMCC ok {}; {} slots, LDPC failed {}, BCH failed {}; losses {} ({} blocks); MER {}; {:.1} s queued",
                            st.wall_s,
                            st.frames,
                            st.tmcc_ok,
                            st.slots,
                            st.ldpc_failed,
                            st.bch_failed,
                            w.losses,
                            w.blocks_lost,
                            st.mer_db().map_or("-".into(), |m| format!("{m:.1} dB")),
                            w.queued_symbols as f64 / symbol_rate_hz
                        );
                    }
                },
            )
        });
        let back = (|| -> Result<()> {
            let opt = RxOptions {
                ldpc,
                ..RxOptions::default()
            };
            let mut asm = BTreeMap::<u16, Assembler>::new();
            for m in &rx {
                match m {
                    Back::Chunk { front, upto } => {
                        let t = std::time::Instant::now();
                        let r = receive_back(*front, &opt);
                        let mut st = lock();
                        emit(&r, upto, &mut asm, out, &mut st)?;
                        st.back_s += t.elapsed().as_secs_f64();
                    }
                    Back::End => close(std::mem::take(&mut asm), &mut lock()),
                }
            }
            Ok(())
        })();
        // on an error here the front's next send fails and it stops
        drop(rx);
        let w = front
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
        (w, back)
    });
    // the decode's own error explains a front that stopped because of it
    back?;
    let w = w?;
    let mut st = lock();
    st.words = Some(w);
    st.wall_s = t0.elapsed().as_secs_f64();
    Ok(w.symbols as f64 / symbol_rate_hz)
}

/// Lowest sync-symbol Es/N0 the acquisition accepts, dB. The carrier gives
/// 8–9 dB on 0.05 s of a live 40 Msps BS7/BS11/BS17 probe; a survey alias
/// that still correlates the sync words a little gives 1–2 dB (a live run
/// locked on −760 kHz at 1.46 dB and decoded no TMCC, plan §0.16).
pub const MIN_ACQUIRE_SNR_DB: f64 = 4.0;

/// Live from a radio's FPGA symbol front end: acquire on a short IQ
/// capture (carrier from every candidate, trim), configure the front end
/// and decode its words continuously until `seconds` of signal (or Ctrl-C).
#[allow(clippy::too_many_arguments)]
pub fn run_radio_fpga(
    backend: &'static dyn isdb_radio::Backend,
    cfg: &RadioConfig,
    taps: usize,
    seconds: f64,
    carrier_offset_hz: Option<f64>,
    status: bool,
    stop: &std::sync::Arc<AtomicBool>,
    out: &mut Outputs,
    stats: &mut Stats,
) -> Result<f64> {
    let rs = isdbs3_phy::frontend::system().symbol_rate_hz;
    let mut acq = None;
    // the capture's own stop: set at the end of `seconds`, or on Ctrl-C
    let cap_stop = std::sync::Arc::new(AtomicBool::new(false));
    let stream = crate::symstream::start_radio(backend, cfg, rs, &cap_stop, |x, fs| {
        let p = isdbs3_phy::frontend::acquire(x, fs, carrier_offset_hz, Trim::Auto)?;
        if p.sync_snr_db < MIN_ACQUIRE_SNR_DB {
            warn!(
                target: "isdb::rx",
                carrier_offset_hz = p.carrier_offset_hz,
                sync_snr_db = p.sync_snr_db,
                min_snr_db = MIN_ACQUIRE_SNR_DB,
                "carrier {:+.0} Hz gives a sync Es/N0 of {:.2} dB, below {MIN_ACQUIRE_SNR_DB} dB: no lock",
                p.carrier_offset_hz, p.sync_snr_db
            );
            return Ok(None);
        }
        let mut d = isdbs3_phy::frontend::design(fs, p.carrier_offset_hz);
        d.trim = p.trim;
        d.taps = taps;
        let params = isdb_frontend::Params::design(&d)?;
        acq = Some(p);
        Ok(Some((
            params,
            format!(
                "carrier {:+.0} Hz, trim {:+.3} symbol, sync Es/N0 {:.2} dB there; FPGA front end, {taps} taps",
                p.carrier_offset_hz, p.trim, p.sync_snr_db
            ),
        )))
    })?;
    if let Some(p) = acq {
        stats.carrier_offset_hz = p.carrier_offset_hz;
        stats.trim = p.trim;
    }
    crate::symstream::finish_radio(stream, seconds, stop, &cap_stop, |rx| {
        run_stream(
            rx,
            crate::symstream::Source::Live,
            rs,
            status,
            stop,
            out,
            stats,
        )
    })
}

/// Live: contiguous captures of `block_s` seconds from `spec`.
#[allow(clippy::too_many_arguments)]
pub fn run_radio(
    spec: &DeviceSpec,
    freq_hz: u64,
    channel: Option<Channel>,
    rate: f64,
    bw: f64,
    gains: &[(String, String)],
    block_s: f64,
    seconds: f64,
    carrier_offset_hz: Option<f64>,
    status: bool,
    stop: &AtomicBool,
    out: &mut Outputs,
    stats: &mut Stats,
) -> Result<f64> {
    let backend = isdb_radio::backend(&spec.backend)?;
    let cfg = RadioConfig {
        center_hz: freq_hz,
        nominal_hz: freq_hz,
        rate_num: crate::phyargs::whole_u32(rate, "--rate")?,
        rate_den: 1,
        bandwidth_hz: crate::phyargs::whole_u32(bw, "--bw")?,
        args: spec.args.clone(),
        gains: gains.to_vec(),
    };
    info!(
        target: "isdb::rx",
        backend = backend.name(),
        "{} capture settings{}:\n{}",
        backend.name(),
        channel.map(|c| c.label()).unwrap_or_default(),
        backend.describe_capture(&cfg)?
    );
    let Some((limit, why)) = backend.capture_limit(&cfg)? else {
        bail!(
            "radio backend {:?} cannot capture, which ISDB-S3 reception needs",
            backend.name()
        );
    };
    let want = crate::phyargs::sample_count(block_s, rate);
    let n = want.min(limit);
    if n < want {
        warn!(
            target: "isdb::rx",
            samples = n,
            seconds = n as f64 / rate,
            wanted = want,
            "block shortened to {} samples ({:.3} s): {why}",
            n,
            n as f64 / rate
        );
    }
    // cs16: two bytes per sample
    let block_bytes = usize::try_from(n)
        .ok()
        .and_then(|n| n.checked_mul(2))
        .context("--block-seconds: the capture block does not fit in memory")?;
    let t0 = std::time::Instant::now();
    let mut secs = 0.0;
    pipeline(
        |tx| {
            let mut captured = 0.0;
            while !stop.load(Ordering::Relaxed) && (seconds <= 0.0 || captured < seconds) {
                let mut bytes = Vec::with_capacity(block_bytes);
                let cap = backend.capture(&cfg, n, &mut bytes, stop)?;
                if !cap.gaps.is_empty() {
                    warn!(
                        target: "isdb::rx",
                        gaps = cap.gaps.len(),
                        "the device lost samples {} time(s) inside this block; decoding its contiguous parts",
                        cap.gaps.len()
                    );
                }
                let x = crate::symbols::decode(cap.format, &bytes);
                let fs = cap.sample_rate_hz;
                captured += x.len() as f64 / fs;
                let mut start = 0usize;
                let mut cuts: Vec<usize> = cap
                    .gaps
                    .iter()
                    .map(|g| usize::try_from(g.at))
                    .collect::<Result<_, _>>()
                    .context("capture gap position")?;
                cuts.push(x.len());
                for c in cuts {
                    if c > start
                        && tx
                            .send(Block {
                                x: x[start..c].to_vec(),
                                fs,
                            })
                            .is_err()
                    {
                        return Ok(());
                    }
                    start = c;
                }
            }
            Ok(())
        },
        carrier_offset_hz,
        out,
        stats,
        |st| {
            secs = st.samples as f64 / rate;
            if status {
                info!(
                    target: "isdb::rx",
                    seconds = secs,
                    wall_s = t0.elapsed().as_secs_f64(),
                    blocks = st.blocks,
                    frames = st.frames,
                    slots = st.slots,
                    ldpc_failed = st.ldpc_failed,
                    bch_failed = st.bch_failed,
                    mer_db = st.mer_db().unwrap_or(f64::NAN),
                    "{secs:.2} s of signal after {:.2} s: block {}; {} frames; {} slots, LDPC failed {}, BCH failed {}; MER {}",
                    t0.elapsed().as_secs_f64(),
                    st.blocks,
                    st.frames,
                    st.slots,
                    st.ldpc_failed,
                    st.bch_failed,
                    st.mer_db().map_or("-".into(), |m| format!("{m:.1} dB"))
                );
            }
        },
    )?;
    Ok(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_template_rules() {
        assert!(Outputs::new(Some("x.tlv"), &[]).is_err());
        assert!(Outputs::new(Some("x.tlv"), &[0xB0B0]).is_ok());
        assert!(Outputs::new(Some("bs11-{tlv_id}.tlv"), &[]).is_ok());
        assert!(Outputs::new(Some("bs11-{ts_id}.tlv"), &[]).is_ok());
    }
}
