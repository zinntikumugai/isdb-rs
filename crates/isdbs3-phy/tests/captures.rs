//! Opt-in tests on ISDB-S3 recordings: `ISDB_RS_ISDBS3_TLV_IQ` names one or
//! more SigMF recordings (`:`-separated) with the full 56 MHz analog
//! bandwidth (e.g. `isdb capture --freq BS7 --rate 61.44M --bw 56M`). Each
//! test prints a `SKIP` line and passes when the variable is unset. See
//! docs/test-data.md.

use isdb_iq::Recording;
use isdb_tlv::Assembler;
use isdb_tlv::stats::StreamStats;
use isdbs3_phy::frontend::{Trim, samples_in, symbols};
use isdbs3_phy::rx::{RxOptions, receive};
use std::collections::BTreeMap;

/// Every whole frame's TMCC decodes with a consistent allocation, every
/// valid slot is LDPC- and BCH-clean, the equaliser raises the sync Es/N0
/// by more than 1.5 dB, and every TLV stream reassembles with no
/// head-indication mismatch, no IP error and only MMTP in its UDP
/// payloads.
#[test]
fn isdbs3_tlv_recordings() {
    let Ok(list) = std::env::var("ISDB_RS_ISDBS3_TLV_IQ") else {
        eprintln!("SKIP isdbs3_tlv_recordings: ISDB_RS_ISDBS3_TLV_IQ not set");
        return;
    };
    for path in list.split(':').filter(|p| !p.is_empty()) {
        let mut rec = Recording::open(std::path::Path::new(path), None, None).unwrap();
        let seg = rec.segments()[0].clone();
        let x = rec
            .read(seg.start, usize::try_from(seg.end - seg.start).unwrap())
            .unwrap();
        let s = symbols(&x, rec.sample_rate, None, Trim::Auto).unwrap();
        assert!(s.survey_says_isdbs3, "{path}: survey");
        let r = receive(&s.z, &RxOptions::default());
        assert!(r.frames.len() >= 2, "{path}: {} frames", r.frames.len());
        for f in &r.frames {
            let t = f
                .tmcc
                .as_ref()
                .unwrap_or_else(|e| panic!("{path}: TMCC {e}"));
            assert!(t.allocation_ok(), "{path}: allocation");
        }
        let eq = r.sync_snr_eq_db.expect("equalised");
        assert!(
            eq > r.sync_snr_db + 1.5,
            "{path}: equaliser {} → {eq}",
            r.sync_snr_db
        );
        let bad = r.slots.iter().filter(|s| s.data.is_none()).count();
        assert_eq!(bad, 0, "{path}: {bad} of {} slots failed", r.slots.len());
        let mut asm: BTreeMap<u16, Assembler> = BTreeMap::new();
        let mut st: BTreeMap<u16, StreamStats> = BTreeMap::new();
        for sl in &r.slots {
            let mut out = Vec::new();
            asm.entry(sl.stream_id)
                .or_default()
                .push_slot(sl.data.as_deref(), sl.head, &mut out);
            let e = st.entry(sl.stream_id).or_default();
            out.iter().for_each(|p| e.add(p));
        }
        for (id, a) in &asm {
            let e = &st[id];
            assert_eq!(a.stats.pointer_mismatches, 0, "{path} {id:#06x}");
            assert_eq!((e.ip_errors, e.non_mmtp), (0, 0), "{path} {id:#06x}");
            eprintln!(
                "{path}: stream {id:#06x}: {} TLV packets, MMT-SI tables {:?}",
                a.stats.packets,
                e.mmt_tables.keys().collect::<Vec<_>>()
            );
        }
        eprintln!(
            "{path}: {} frames, {} slots clean, sync Es/N0 {:.2} → {eq:.2} dB",
            r.frames.len(),
            r.slots.len(),
            r.sync_snr_db
        );
    }
}

/// On the same recordings: the fixed-point LDPC decoder gives every slot
/// the same iterations, convergence and data with 16 codewords per lane
/// group (SIMD) as one at a time (the scalar reference), and decodes at
/// least as many slots as the floating-point reference.
#[test]
fn isdbs3_ldpc_simd_equals_scalar_on_recordings() {
    use isdbs3_phy::main_signal::Ldpc;
    let Ok(list) = std::env::var("ISDB_RS_ISDBS3_TLV_IQ") else {
        eprintln!(
            "SKIP isdbs3_ldpc_simd_equals_scalar_on_recordings: ISDB_RS_ISDBS3_TLV_IQ not set"
        );
        return;
    };
    for path in list.split(':').filter(|p| !p.is_empty()) {
        let mut rec = Recording::open(std::path::Path::new(path), None, None).unwrap();
        let seg = rec.segments()[0].clone();
        let x = rec
            .read(seg.start, usize::try_from(seg.end - seg.start).unwrap())
            .unwrap();
        let s = symbols(&x, rec.sample_rate, None, Trim::Auto).unwrap();
        let run = |ldpc| {
            receive(
                &s.z,
                &RxOptions {
                    ldpc,
                    ..RxOptions::default()
                },
            )
        };
        let (simd, scalar, float) = (run(Ldpc::FIXED), run(Ldpc::SCALAR), run(Ldpc::FLOAT));
        assert_eq!(simd.slots.len(), scalar.slots.len(), "{path}");
        assert!(!simd.slots.is_empty(), "{path}: no slots");
        for (a, b) in simd.slots.iter().zip(&scalar.slots) {
            assert_eq!(
                (a.frame, a.slot, a.ldpc_iterations, a.bch_corrected, &a.data),
                (b.frame, b.slot, b.ldpc_iterations, b.bch_corrected, &b.data),
                "{path}"
            );
        }
        let ok = |r: &isdbs3_phy::rx::Rx| {
            r.slots
                .iter()
                .filter(|s| s.ldpc_iterations.is_some())
                .count()
        };
        assert!(
            ok(&simd) >= ok(&float),
            "{path}: fixed {} < float {}",
            ok(&simd),
            ok(&float)
        );
        eprintln!(
            "{path}: {} slots, SIMD = scalar; LDPC-clean fixed {} / float {}",
            simd.slots.len(),
            ok(&simd),
            ok(&float)
        );
    }
}

/// `ISDB_RS_ISDBS3_LIVE_IQ`: recordings of several separately captured
/// blocks (e.g. `isdb capture --radio …,recorder=iio,bits=8 --segments 2`).
/// Every segment acquired on its own decodes the TMCC of every frame (a
/// candidate carrier a few tens of kHz off once won the carrier choice on
/// its sync Es/N0 and decoded none, plan §0.10), and so does every segment
/// after the first when it reuses the first's acquisition.
#[test]
fn isdbs3_every_segment_acquires() {
    use isdbs3_phy::frontend::symbols_with_prior;
    let Ok(list) = std::env::var("ISDB_RS_ISDBS3_LIVE_IQ") else {
        eprintln!("SKIP isdbs3_every_segment_acquires: ISDB_RS_ISDBS3_LIVE_IQ not set");
        return;
    };
    for path in list.split(':').filter(|p| !p.is_empty()) {
        let mut rec = Recording::open(std::path::Path::new(path), None, None).unwrap();
        let fs = rec.sample_rate;
        let segs = rec.segments().clone();
        let mut prior = None;
        for (i, seg) in segs.iter().enumerate() {
            let x = rec
                .read(seg.start, usize::try_from(seg.end - seg.start).unwrap())
                .unwrap();
            let fresh = symbols_with_prior(&x, fs, None, Trim::Auto, None).unwrap();
            let mut tries = vec![("fresh", fresh.0)];
            if let Some(p) = &prior {
                tries.push((
                    "prior",
                    symbols_with_prior(&x, fs, None, Trim::Auto, Some(p))
                        .unwrap()
                        .0,
                ));
            }
            for (label, s) in tries {
                let r = receive(&s.z, &RxOptions::default());
                let ok = r.frames.iter().filter(|f| f.tmcc.is_ok()).count();
                assert!(
                    !r.frames.is_empty() && ok == r.frames.len(),
                    "{path} segment {i} ({label}, carrier {:.0} Hz): TMCC {ok}/{}",
                    s.carrier_offset_hz,
                    r.frames.len()
                );
                eprintln!(
                    "{path} segment {i} ({label}): carrier {:.0} Hz, trim {:.3}, TMCC {ok}/{}",
                    s.carrier_offset_hz,
                    s.trim,
                    r.frames.len()
                );
            }
            prior.get_or_insert(fresh.1);
        }
    }
}

/// `ISDB_RS_ISDBS3_LIVE_IQ`: the acquisition's carrier is the candidate
/// with the best sync-symbol Es/N0, whatever the survey's strongest line
/// says. At 40 Msps the ±R/2 lines alias, and on two of seven live
/// recordings (BS7 and BS11, 2026-10-26) the strongest line was an alias
/// (Es/N0 −3 and −8 dB) while the carrier was the 2nd and 4th of the
/// candidates (plan §0.16): `isdb symbols` took the strongest line, and
/// `pick_carrier` tried only the first four.
#[test]
fn isdbs3_carrier_beats_the_survey_aliases() {
    use isdbs3_phy::frontend::acquire;
    let Ok(list) = std::env::var("ISDB_RS_ISDBS3_LIVE_IQ") else {
        eprintln!("SKIP isdbs3_carrier_beats_the_survey_aliases: ISDB_RS_ISDBS3_LIVE_IQ not set");
        return;
    };
    for path in list.split(':').filter(|p| !p.is_empty()) {
        let mut rec = Recording::open(std::path::Path::new(path), None, None).unwrap();
        let fs = rec.sample_rate;
        let seg = rec.segments()[0].clone();
        let n = usize::try_from(seg.end - seg.start)
            .unwrap()
            .min(samples_in(0.2, fs));
        let x = rec.read(seg.start, n).unwrap();
        let p = acquire(&x, fs, None, Trim::Auto).unwrap();
        // a wrong carrier leaves the sync words uncorrelated (≤ 0 dB);
        // the right one gives the signal's Es/N0 (≈ 9 dB on 0.05 s untrimmed,
        // more after the trim)
        assert!(
            p.sync_snr_db > 6.0,
            "{path}: carrier {:+.0} Hz, sync Es/N0 {:.2} dB",
            p.carrier_offset_hz,
            p.sync_snr_db
        );
    }
}

/// `rx::receive` is `receive_front` then `receive_back` on a live stream's
/// symbols: `ISDB_RS_ISDBS3_WORDS` names a words file (`isdb symbols
/// --out`), of which the first 25 frames are received both ways and must
/// give the same result (timings aside).
#[test]
fn isdbs3_front_then_back_is_receive_on_words() {
    use isdb_frontend::format::{Event, Reader};
    use isdbs3_core::params::FRAME_SYMBOLS;
    use isdbs3_phy::Cplx;
    use isdbs3_phy::rx::{receive_back, receive_front};
    use std::io::Read;
    let Ok(path) = std::env::var("ISDB_RS_ISDBS3_WORDS") else {
        eprintln!("SKIP isdbs3_front_then_back_is_receive_on_words: ISDB_RS_ISDBS3_WORDS not set");
        return;
    };
    let want = 25 * FRAME_SYMBOLS;
    let mut f = std::fs::File::open(&path).unwrap();
    let (mut r, mut ev, mut z) = (Reader::new(), Vec::new(), Vec::<Cplx>::new());
    let mut buf = vec![0u8; 1 << 20];
    while z.len() < want {
        let n = f.read(&mut buf).unwrap();
        assert!(n > 0, "{path}: fewer than 25 frames");
        r.feed_bytes(&buf[..n], &mut ev);
        for e in ev.drain(..) {
            if let Event::Block { symbols, .. } = e {
                z.extend(
                    symbols
                        .iter()
                        .map(|s| Cplx::new(f64::from(s[0]), f64::from(s[1]))),
                );
            }
        }
    }
    z.truncate(want);
    let untimed = |mut r: isdbs3_phy::rx::Rx| {
        r.prep_s = 0.0;
        r.ldpc_s = 0.0;
        format!("{r:?}")
    };
    let opt = RxOptions::default();
    let whole = receive(&z, &opt);
    assert!(
        whole.frames.len() >= 23,
        "{path}: {} frames",
        whole.frames.len()
    );
    assert!(
        whole.slots.iter().all(|s| s.data.is_some()),
        "{path}: failed slots"
    );
    let halves = receive_back(receive_front(&z, &opt), &opt);
    assert_eq!(untimed(halves), untimed(whole), "{path}");
}
