//! Where one chunk of `rx::receive` spends its time, stage by stage, on a
//! words file (`isdb symbols --out`): derotation, slot sync, TMCC and
//! equaliser training, the equaliser, sync again, the per-frame
//! preparation, then the demapper and the LDPC decoder apart
//! (development tool, like `chunkprof`).
//!
//! cargo run --release -p isdbs3-phy --example stageprof -- WORDS_FILE [FRAMES]

use isdb_frontend::format::{Event, Reader};
use isdbs3_core::params::FRAME_SYMBOLS;
use isdbs3_core::slots::layout;
use isdbs3_phy::Cplx;
use isdbs3_phy::main_signal::{
    FrameInput, Ldpc, codeword_llrs, decode_frames, learned_points, main_symbols, nominal_points,
    pilots,
};
use isdbs3_phy::rx::{RxOptions, derotate, noise_of, receive};
use isdbs3_phy::sync::{synchronise, synchronise_at};
use isdbs3_phy::{eq, tmcc};
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let frames: usize = a.get(2).map_or(24, |s| s.parse().unwrap());
    let bytes = std::fs::read(&a[1]).expect("read words");
    let tp = Instant::now();
    let mut r = Reader::new();
    let mut ev = Vec::new();
    r.feed_bytes(&bytes, &mut ev);
    let mut z: Vec<Cplx> = Vec::new();
    for e in ev {
        if let Event::Block { symbols, .. } = e {
            z.extend(
                symbols
                    .iter()
                    .map(|s| Cplx::new(f64::from(s[0]), f64::from(s[1]))),
            );
        }
        if z.len() >= (frames + 1) * FRAME_SYMBOLS {
            break;
        }
    }
    z.truncate((frames + 1) * FRAME_SYMBOLS);
    println!("words parsed in {:.3} s", tp.elapsed().as_secs_f64());
    let secs = z.len() as f64 / 33.7561e6;
    println!("{} symbols = {secs:.3} s of signal", z.len());
    let off = synchronise(&z).expect("sync").offset;
    for _ in 0..2 {
        let mut log: Vec<(&str, f64)> = Vec::new();
        let mut t = Instant::now();
        let mut lap = |name: &'static str, log: &mut Vec<(&str, f64)>| {
            log.push((name, t.elapsed().as_secs_f64()));
            t = Instant::now();
        };
        let zd = derotate(&z, 1.0);
        lap("derotate", &mut log);
        let sy0 = synchronise_at(&zd, off).expect("sync at");
        lap("synchronise_at", &mut log);
        let es = 10f64.powf(sy0.sync_snr_db / 10.0);
        let f0 = sy0.frames[0];
        let l = tmcc::llrs(&zd, &sy0.slots, f0, 1.0 / (2.0 * es)).expect("tmcc llrs");
        let lay0 = layout(&tmcc::decode(&l, 50).expect("tmcc").info).expect("layout");
        let w = eq::train(&zd, &sy0.slots, f0, &lay0).expect("train");
        lap("tmcc + eq::train", &mut log);
        let ze = eq::apply(&zd, &w);
        lap("eq::apply", &mut log);
        let sy = synchronise_at(&ze, off).expect("sync at (eq)");
        lap("synchronise_at (eq)", &mut log);
        let es_n0 = 10f64.powf(sy.sync_snr_db / 10.0);
        let preps: Vec<_> = sy
            .frames
            .iter()
            .filter_map(|&f| {
                let l = tmcc::llrs(&ze, &sy.slots, f, 1.0 / (2.0 * es_n0))?;
                let lay = layout(&tmcc::decode(&l, 50).ok()?.info)?;
                Some((
                    lay,
                    main_symbols(&ze, &sy.slots, f)?,
                    pilots(&ze, &sy.slots, f)?,
                ))
            })
            .collect();
        lap("prepare (1 thread)", &mut log);
        let pts: Vec<_> = preps
            .iter()
            .map(|(lay, _, pil)| {
                move |m, r| learned_points(pil, lay, m).unwrap_or_else(|| nominal_points(m, r))
            })
            .collect();
        let n0s: Vec<f64> = preps
            .iter()
            .zip(&pts)
            .map(|((lay, ms, _), p)| noise_of(ms, lay, p))
            .collect();
        lap("noise_of", &mut log);
        let mut nbits = 0usize;
        for (((lay, ms, _), p), &n0) in preps.iter().zip(&pts).zip(&n0s) {
            for s in lay.iter().filter(|s| !s.dummy) {
                nbits += codeword_llrs(ms, s, &p(s.modulation, s.rate), n0).len();
            }
        }
        lap("demap (1 thread)", &mut log);
        let inputs: Vec<FrameInput<'_>> = preps
            .iter()
            .zip(&pts)
            .zip(&n0s)
            .map(|(((lay, ms, _), p), &n0)| FrameInput {
                ms,
                layout: lay,
                n0,
                pts_for: p,
            })
            .collect();
        let dec = decode_frames(&inputs, &Ldpc::FIXED);
        lap("decode_frames (all threads)", &mut log);
        let slots: usize = dec.iter().map(Vec::len).sum();
        println!("{} frames, {slots} slots, {nbits} code bits", preps.len());
        let t = Instant::now();
        let z0 = &z[off..];
        let rx = receive(
            z0,
            &RxOptions {
                slot_offset: Some(0),
                carrier_hz: 1.0,
                ..RxOptions::default()
            },
        );
        log.push(("receive (as the stream calls it)", t.elapsed().as_secs_f64()));
        assert!(!rx.slots.is_empty());
        for (name, s) in &log {
            println!("  {name:30} {s:7.3} s  ({:.2}× real time)", secs / s);
        }
    }
}
