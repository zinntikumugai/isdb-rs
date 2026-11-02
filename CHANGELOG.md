# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
All crates of the workspace share one version (`[workspace.package]`).

## [Unreleased]

Initial feature set, implemented from the published specifications
(ITU-T H.222.0 | ISO/IEC 13818-1, ARIB STD-B10, STD-B24, STD-B31,
STD-B32; editions in `SPECS.md`). ISDB-T is implemented end to end;
ISDB-S (ITU-R BO.1408-1, MIC Ordinance No. 87 of 2011) is received to
transport streams and transmitted from them to IQ files; ISDB-S3 (ARIB
STD-B44) is received to TLV streams.

### Added

- Tools: `tools/split-rx.sh FILE.sym OUT [PARTS] [JOBS]` decodes a long
  ISDB-S3 `Symbols` recording in parallel slices and joins the part TLVs
  losslessly with `tools/tlv-join.py` (every slice but the last decoded
  `OVERLAP_BYTES` past its end; the join finds part k+1's start in part
  k's tail and fails on a seam without a match unless `ALLOW_LOSSY=1`).
  Self-test: `tools/tlv-join-test.py`.

- ISDB-S3 reception from the FPGA receive chain's `Slots` words
  (docs/plans/isdbs3-fpga-rx.md piece 5, host side; no image has the
  chain yet): `isdb rx --slots FILE`, `isdb rx --symbols FILE --chain
  fixed` (the chain's fixed-point CPU model through the same words, the
  bench check), `--slots-out FILE`, and `--offload auto|fpga` taking the
  chain when the radio's ID word announces it; `decoded_by` and `chain`
  in `--json`. `isdb_radio::Offload::RxSlots`, `Backend::capture_slots`
  and `isdb_radio::chain` (the open sequence and the equaliser's slow
  loop); `isdb_frontend::control::chain`, the host's register map of the
  chain (placeholder addresses). Tools: `tools/slots-vs-symbols.sh`,
  `tools/tlv-packet-diff.py`.
- Tools: `tools/isdbs-bench-check.sh --receiver dt295|exact` (default
  `dt295`) and `--receiver-drops PIDS` (the profile's `0x0001`; `""` for
  none): the PIX-DT295's raw output drops the CAT without a null (its PID
  filter, PIX-DT295 docs/stream-investigation.md), drops PMT/PCR packets
  until it has learnt their PIDs, and rewrites the CC of adaptation-only
  packets. With `--placed` the verdict adds "PCR values all present in the
  placed TS", "corrected CBR-fit max ≤ --fit-max-ns" (the dropped packets
  re-inserted) and "content identical modulo receiver drops / CC
  rewrites"; the missing CAT (`tables.mandatory`, now judged) and the
  index-based `pcr.accuracy`, `pcr.rate` and CBR fit are EXPLAINED when
  the drops account for them, and a `tables.frequency` row whose every
  interval is within the criterion and only the capture-edge gap exceeds
  it is EDGE, not FAIL. Exit codes unchanged. `tools/isdbs-pacing-e2e.sh`
  passes `--receiver exact`. Self-test: `tools/isdbs-bench-check-test.sh`
  (an `isdb mux` TS as the placed TS, recordings cut from it the DT295's
  way and four that must FAIL).
- Tools: `tools/ts-align-compare.py --receiver-drops PIDS`,
  `--receiver-learns`, `--ignore-af-only-cc`, `--fit-max-ns`, `--json`:
  packets the receiver dropped without a null are skipped and counted per
  PID, CC rewrites of adaptation-only packets counted apart from real
  mismatches; every recording PCR is looked up in the placed TS (absent,
  ambiguous, out of order counted), and the recording's CBR fit is
  printed index-based, with the dropped packets re-inserted, and for the
  placed TS over the same span.
- `tools/ts-pcr-residuals.py`: the PCR residual shape of a TS (rms, max,
  lag-1 autocorrelation, dominant period); also a module for
  `ts-align-compare.py`.
- `isdb tx --system isdbs --mux-config FILE.toml` (and `isdb loopback
  --system isdbs`): the in-process ISDB-S multiplexer (piece J of
  docs/plans/isdbs-next/media-and-margin.md §3). Repeatable, one TS per
  file: a `kind = "bs"` configuration (a terrestrial one is refused, naming
  the fix) with its services, streams and tables on layer "A" (B/C refused,
  naming the fix); its `[transport] transport_stream_id` is the TS_ID,
  taking every sent slot alone, or the slots and place of the `--ts
  ID:SLOTS` of that TS_ID next to other TSs. Each superframe takes the
  multiplexer's `next_period()` (its PCR, PTS/DTS and SI made for those
  slots; its nulls counted as padding), the same packets on the IQ and the
  FPGA code paths. New `--mux-pcr-grid uniform|slot` (default `uniform`:
  `isdbt_mux::Schedule::satellite_uniform`, packet q of N sent slots at
  ⌊q · 1 382 400/(37·N) + ½⌋ ticks, the constant-rate TS a receiver
  outputs and the pacer's `SlotClock::isdbs` grid, to which BS
  transponders stamp their PCRs; `slot`: the true slot times of
  `Schedule::satellite`, whose frame-periodic deviation from that rate
  reaches 2.2 µs in a CBR fit with three PCRs per superframe at 48 slots
  and 231 µs for a 16-slot TS; isdb-cli SPEC-NOTES). `--dry-run`, the
  input line and the status lines name the multiplexer (`TS_ID 0x4017 <-
  in-process multiplexer FILE (PCR grid uniform)`, `0x4017 mux late PES 0
  errors 0`); a `--channel` other than the NIT's warns. `isdb mux --system
  isdbs [--combination …] [--mux-pcr-grid …] --out-ts` writes the same TS
  without modulation (equal to `tx --placed-ts`), `--report` on its
  schedule. Offline: through `tx --iq` → `rx` the received TS equals the
  placed one, validates with `--expect-rate 52170000:1` and the BS
  expectations, CBR fit 18 ns, every PCR exactly on the grid
  (VERIFICATION.md "In-process ISDB-S multiplexer, offline").
  `tools/isdbs-pacing-e2e.sh` takes `MUX=1` (and `GRID=uniform|slot`).
- `isdbt-mux`: `Schedule::satellite_uniform(&tmcc_info, relative_ts)`, the
  ISDB-S TS on its uniform per-TS grid: the TS's own 8 · n_r slots, packet
  q at ⌊q · 1 382 400/(37 · n_r) + ½⌋ ticks, its PCR at that time; rates,
  PCR default and SI system as `Schedule::satellite`.
- Tools: `tools/isdbs-bench-tx.sh --media[=DIR]` sends the BS bench
  service with real video and audio (`video.m2v`, `audio.m2a` of DIR, else
  of `ISDB_RS_MEDIA_DIR`, else of the git-ignored
  `crates/isdbt-mux/examples/media/`; refused when none has both):
  `bs-bench.toml`'s declared streams become `mpeg2-video` / `aac-adts`
  streams (`adts_compliance = "warn"`, which FFmpeg's ADTS needs), written
  by the new `tools/bench-media.py`. The default feed is then `loop`, on a
  TS one media loop long (the whole number of OFDM frames nearest to the
  multiplexer's media loop: 43 frames, 9.947 s, for the 10 s media of
  docs/test-data.md); each pass leaves a gap in the media (offline: video
  137.6 ms, audio 603 ms). `--feed fifo` with `--media` paces a live
  `isdb mux --out-ts -` into the FIFO instead, with no file seam. The
  self-test covers `--media` on stub media. New
  `tools/media-decode-check.py` (run with `python3 -I`; ffmpeg, ffprobe):
  per video / audio stream of a TS, ffmpeg's decode errors, DTS/PTS
  monotonic with step statistics, continuity errors (`isdb ts inspect
  --json`), and with `--source DIR` every decoded picture and audio frame
  checked against the source's (framemd5) and its cyclic order, frames
  not in the source allowed only in short runs at the edges, at order
  jumps (loop seams) and at the source's loop point; PASS/FAIL as the exit
  status. `tools/media-decode-check-test.sh` checks it on a TS as
  multiplexed (PASS), with corrupted video payload and with 1 s cut out
  (FAIL). `tools/isdbs-pacing-e2e.sh` takes `MEDIA=DIR` (those media, the
  TS one media loop long; `FEED=live` then writes `isdb mux --out-ts -`
  into the FIFO) and runs the decode check on the received TS whenever it
  carries media.
- `isdbt-mux`: `Schedule::satellite(&tmcc_info, relative_ts)`, the ISDB-S
  superframe of one TS (piece I of docs/plans/isdbs-next/media-and-margin.md
  §3.4): the sent slots of the 8 frames in transmission order, lane 0 for
  the TS's own and `None` for the other TSs', each slot at the symbol of
  its first on-air byte and its PCR byte (on-air byte 9) from
  `isdbs_core::slots`, 319 488 symbols × 450/481 to ticks, 8 · n_r · 1 504
  bit per superframe (52.17 Mbit/s for 48 TC8PSK slots), one PCR per
  superframe by default (`timing.pcr_per_frame` counts per superframe),
  PAT/NIT/CAT on the lane. `System::Satellite { info, relative_ts }`;
  `Multiplexer::with_schedule` on it refuses a terrestrial network
  (`network.kind`) and a service or table on layer B/C (`InactiveLayer`),
  `next_period()` returns the TS's packets of a superframe; `params()` and
  `pattern()` panic there (`schedule()` instead). isdbt-mux now depends on
  `isdbs-core`. SPEC-NOTES "ISDB-S timing model" (pre-interleave slot
  time; pending B20/B21).
- `isdbt-mux`: the multiplexer runs on a `Schedule` (new `schedule`
  module): one period of slots (`Some(lane)` carried, `None` not), the
  start and PCR-byte time of each in units, the period, units → 27 MHz
  ticks as `num`/`den`, lane rates, the default PCRs per period and the
  control lane, and the `System` (`Terrestrial(Params)`).
  `Schedule::ofdm(&params)` is the OFDM frame model it used before
  (multiplex frame of B31 §3.2.1, 408 clocks per slot, PCR byte at +20,
  × 1701/512); `Multiplexer::with_schedule(cfg, schedule, base)` takes
  any schedule (`Schedule::validate`, `MuxError::Schedule`), `new` is
  `with_schedule` with `Schedule::ofdm`, `next_period()` returns the
  packets of one period's carried slots and `schedule()` the schedule;
  `next_frame*`, `params()`, `pattern()` and `slot_time` are unchanged.
  ISDB-T output is byte-identical (the SHA-256 of `isdb mux` for
  `bench.toml` and `bench-minimal.toml` is pinned by
  `isdb-cli/tests/mux_fixture.rs`). API: `si::SiBuilder`'s `params` field
  is replaced by `system: schedule::System` and `control_lane`.
- `isdbs-core`: the on-air position of a TS packet, shared by the ISDB-S
  pacer and the in-process multiplexer. `slots::packet_symbol(info, r, k)`
  is the symbol of the superframe at which packet k of relative TS r
  starts (its frame ⌊k / n_r⌋ and its own sent slot, the slot's run, the
  first symbol carrying the slot's first bit, past the header and bursts;
  before the inter-frame interleaver), `packet_byte_symbol` the same for
  any on-air byte (byte 9 holds a PCR's last base bit), with
  `packet_slot`, `ts_slots`, `slot_byte_symbol` and `bit_symbol` (the
  punctured codes place a slot at the first pair holding its first bit:
  QPSK 3/4 slot 1 of a run at symbol 1 083). `frame::SUPERFRAME_SYMBOLS`
  (319 488), `SYMBOL_RATE` (28.860 Mbaud) and `TICKS_PER_SYMBOL_NUM` /
  `_DEN` = 450/481 with `symbols_to_ticks` (exact, overflow-free). On the
  bench layout packet 48 f + s starts at 39 936 f + 192 + 828 s.
  Checked against the transmitter's output for ten layouts covering all
  seven combinations (`isdbs-phy/tests/slot_time.rs`).
- `isdbt-mux`: `[network] kind = "bs"` makes the SI of a BS digital
  broadcasting network (ARIB STD-B10 Table 6-1, §6.2.6, Annex N): the NIT
  TS loop carries the service list and a satellite delivery system
  descriptor 0x43 derived from `[network.satellite] channel = "BS1"`…
  `"BS23"` (odd) and `orbital_position` (default `"110.0E"`): frequency
  11 727.48 + 19.18·(n − 1) MHz, circular right, ISDB-S modulation /
  FEC "refer to TMCC", 028.8600 Msym/s; 0xFE carries broadcasting_identifier
  '000010'; 0xCD only when `remote_control_key_id` is given. The
  terrestrial keys (`area_code`, `channels`, `frequencies`,
  `partial_reception_descriptor`) and a BIT broadcaster's `terrestrial`
  are refused with it, and a network_id other than 0x0004 is warned about.
  The loop order and values match the BS NIT on air (checked 2026-10-31).
  `[[bit.broadcasters]] services = [service_id, …]` adds a service list
  descriptor 0x41 to the broadcaster loop (either kind). New
  `examples/bs-bench.toml` (placeholder TS_ID 0x4017, service_id 0x03E7,
  broadcaster_id 0xEE). `kind` defaults to `"terrestrial"`, whose output is
  byte-identical to before (pinned by a fixture test). API: `Network`'s
  `area_code`, `remote_control_key_id` and `partial_reception_descriptor`
  are now `Option`s; a terrestrial configuration without `area_code` or
  `remote_control_key_id` is now `MuxError::Missing` instead of a TOML
  "missing field".
- `isdb-si`: typed view of the satellite delivery system descriptor
  (0x43, ARIB STD-B10 §6.2.6): `values::{Polarization,
  SatelliteModulation, FecInner}` (Tables 6-9…6-11);
  `SatelliteDeliverySystem::{polarization, modulation_type, fec}`; the BCD
  fields in their units, `frequency_10khz()`, `orbital_position_tenths()`
  and `symbol_rate_100sps()` (`None` on a non-BCD character); and
  `SatelliteDeliverySystem::from_values`, which codes them and refuses
  values with too many digits. `values::BroadcastingIdentifier` (Table
  6-36, with `is_satellite()` / `is_terrestrial()`) via
  `SystemManagement::broadcasting_identifier_typed()`. `conformance`
  reports a non-BCD 0x43 field as `Range` and a reserved modulation or
  FEC_inner code as `ReservedValue`. The codec bytes are unchanged; a
  spec vector checks the BS-1 parameters.
- `isdb-analyze`, `isdb-cli`: `isdb ts validate` and `inspect` understand
  satellite (BS) SI. The `cross.nit` warning now accepts either delivery
  descriptor ("no delivery system descriptor (0xFA or 0x43) for the actual
  TS", B10 Appendix §1.1 b)), so a BS NIT no longer warns. A new warning
  `cross.delivery` reports a NIT whose own-TS delivery descriptor (0xFA
  terrestrial, 0x43 satellite), broadcasting_identifier (Table 6-36, NIT
  or PMT) and network_id (Annex N: 0x7880–0x7FE8 terrestrial, 0x0001–0x000C
  satellite) disagree, e.g. a 0x43 under identifier '000011'.
  `--expect-freq-mhz` also matches a 0x43 frequency (10 kHz steps, e.g.
  `11727.48` for BS-1); new `--expect-orbital DDD.DE|W` and
  `--expect-polarization NAME|CODE` check the 0x43's orbital position and
  polarization. `inspect` prints a `satellite delivery:` line (frequency,
  orbital position, polarization, modulation, symbol rate, FEC) and its
  JSON gains `satellite` in each NIT TS entry (`delivery` is unchanged).
- `isdb-cli`: `isdb tx --system isdbs` (and its loopback) warns once per
  TS when the input's first PAT names another transport_stream_id than
  its `--ts` TS_ID, which the TMCC announces: a receiver tuned by TS_ID
  would find a TS that names another (on air they are equal).
- Tools: `tools/isdbs-bench-tx.sh` sends the BS bench service of
  `examples/bs-bench.toml` (BS SI, TS_ID 0x4017, `--ts 0x4017=…`; receive
  with `dt295 tune --isdbs bs1 --tsid 4017`) instead of the terrestrial-SI
  rewrite of `bench-minimal.toml` with TS_ID 0x4010, which
  `--terrestrial-si` still sends; WORKDIR keeps the TOML sent and its TS
  (`bs60.ts`, or `bench60.ts`), made again when the TOML is newer.
  `tools/isdbs-bench-check.sh` expects TS_ID 4017 and the BS NIT
  (`--expect-nid 0x0004 --expect-onid 0x0004 --expect-freq-mhz` of the
  `--transponder`, 11727.48 for bs1; `--terrestrial-si`: TS_ID 4010, no
  network expectations), prints the NIT and SDT, and fails on any
  `cross.*` finding. `tools/isdbs-pacing-e2e.sh` runs `bs-bench.toml`
  (TS_ID 0x4017, `--channel BS1`; `MEDIA=1` turns its declared streams
  into media streams).
- Tools: `tools/isdbs-level-sweep.sh` sweeps the PlutoSky R2's TX
  attenuation under the bench broadcast into the PIX-DT295 (level margin,
  docs/plans/isdbs-next/media-and-margin.md §6): refuses to run without
  `--conducted`, never below 45 dB without `--margin`, writes each level
  live with `iio_attr` (`ad9361-phy` output `voltage0` hardwaregain) and
  reads it back (a write that does not take effect stops the sweep, or with
  `--restart` restarts the broadcast per level), polls `dt295 status
  --isdbs --watch N --json` without a retune (hold), makes a fresh `dt295
  tune` (acquisition) and a `dt295 stream` recording checked with `isdb ts
  inspect` per level, steps 5 → 1 → 0.25 dB towards the loss point, walks
  back down for the hysteresis, and writes `sweep.csv` and the
  VERIFICATION.md table with a raw-C/N fit (`tools/isdbs-level-sweep.py`).
  The broadcast is the operator's (`--tx-pid`, left on air at 89.75 dB on
  exit) or the sweep's own (`--start-tx`: `tools/isdbs-bench-tx.sh`, which
  keeps the `--rf-enable` gate, stopped on every exit path).
  `tools/isdbs-level-sweep-test.sh` checks it with stub `dt295`,
  `iio_attr`, `isdb` and bench script. Needs the PIX-DT295 driver's
  `status --json` (its commit `d4ad6dc`).
- `isdb-cli`: `isdb tx --system isdbs` places a TS file in its slots by its
  PCR: each packet in the first free slot at or after the time its PCRs
  give it, input null packets dropped, nulls in the slots left over, and
  every PCR corrected by the time its packet moved (H.222.0 Annex D.0.4),
  so the TS keeps its own rate (the bench TS no longer plays 1.74× fast)
  and a receiver's output has PCRs on the slot grid (a 3 s CBR file:
  52.17 Mbit/s within 1 ppm, CBR-fit residual below 40 ns). A regular
  file (`--feed file`/`loop`) is probed first and refused when its mean
  rate exceeds its slots' capacity, naming the slots it needs
  (`--allow-overrate` now applies to ISDB-S too); a pipe or stdin is
  paced from its first PCR; files without PCR stay in arrival order.
  `--ts-pacing auto|pcr|arrival` (default `auto`;
  `arrival` is the previous byte-exact rule). `--placed-ts TEMPLATE`
  writes each TS's placed packets, nulls included, as handed to the
  modulator (the same for `--iq` and `--offload fpga`). The plan is shown
  before anything is opened (also by `--dry-run`); the status lines and
  the summary carry each pacer's PCR lag, input rate against capacity,
  dropped nulls and restamped PCRs. `isdb loopback --system isdbs` paces
  its files the same way.
- `isdb-cli`: `isdb tx --system isdbs --feed live` paces a live input
  (a FIFO fed in real time) by its PCR too, with a deadline instead of
  blocking (`WaitMode::Live`, pcr-pacing.md §7.7): a PCR within the first
  second / 65 536 packets selects it (else arrival order, as before), the
  first PCR goes on air 0.3 s after it arrived, and each superframe waits
  for its packets until 0.2 s after that schedule, then goes out as it
  is; later packets take later slots with their PCRs restamped by the lag
  (kept, never re-anchored). `--ts-pacing pcr` now paces a live input
  instead of refusing it. The status lines add `live-input lag`/`lead`
  (each PCR's arrival against the writer's schedule: drift of a writer
  slower than real time shows here first) and `deadline-misses`; PCRs that
  stop are extrapolated after 0.7 s. `tools/isdbs-bench-tx.sh`'s fifo mode
  is therefore paced by PCR; `tools/isdbs-pacing-e2e.sh` with `FEED=live`
  (and `STALL=SECONDS`) checks a `pv`-paced FIFO end to end (10 s of the
  bench TS: no deadline miss, input lag within ±0.08 s, CBR-fit max 19 ns,
  received TS identical to the placed one).
- `isdb-cli`: `isdb tx --system isdbs --feed loop` joins a paced TS's
  loop seams: the reader thread tags each packet with its loop pass, and
  each pass after the first has its PCR, PTS and DTS moved on by the loop
  period (a multiple of 300 ticks, measured by the pacer's clock across
  the seam) and its continuity counters renumbered per PID (H.222.0
  §2.4.3.3, adaptation-only packets included), so a receiver sees one
  continuous TS: no PCR discontinuity, no continuity error
  (`tools/isdbs-pacing-e2e.sh` with `FEED=loop`; the bench TS had a CC
  error on 9 of 11 PIDs at every seam). Pass 0 and `--feed file` change no
  field but the PCR. The status lines add `passes`, `cc-fixed` and
  `pes-shifted`. `tools/ts-loop-seam.py IN.ts PLAYED.ts` checks a looped
  TS (continuity, PCR fit, PES time stamps, bytes changed).
  `tools/isdbs-pacing-e2e.sh` runs the bench TS through `tx --iq` and
  `rx` offline and checks the received TS against the placed one;
  `tools/isdbs-bench-tx.sh --placed TEMPLATE` passes `--placed-ts`.
- `isdb-cli`: `SlotClock` / `SlotPacer` (`src/pace.rs`), PCR-paced
  placement of one TS on an exact slot grid with PCR restamping, the
  core of the above; `isdb-ts`: `retime` (`pcr_field`, `add_to_pcr`
  modulo 2^33·300, `shift_pes_timestamps` with prefixes and markers
  kept, `set_continuity_counter`), the in-place packet corrections it
  uses.
- `isdb-analyze`, `isdb-cli`: `isdb ts validate --expect-rate BPS[:PPM]`
  compares the PCR-implied rate (least-squares slope of PCR time against
  byte position, `Timebase::cbr_fit`) of every PCR_PID with the rate the
  TS is played at: a `pcr.rate` error outside the tolerance (100 ppm by
  default). A TS relayed too fast or too slow, which every other timing
  check passes, is caught. `pcr.accuracy` gives the fit's rms next to its
  max.
- `isdb-cli`: `isdb tx --system isdbs` transmits ISDB-S: `--combination
  WORD:N` (the slot layout, `tc8psk2/3:48` by default; `bpsk1/2` …
  `tc8psk2/3`), `--ts ID[:SLOTS][=PATH]` (up to eight TSs, each a TS
  file with `--feed` or synthetic packets), `--rate` (40M),
  `--lead-superframes`, `--no-flush`. A producer thread runs
  `isdbs_phy::tx::Transmitter` and `isdb_frontend::shape::Shaper` per
  superframe (a pre-roll frame, the lead, the data, a flush frame);
  status and summary lines per TS_ID. `--iq` writes the samples at
  `--backoff-db` below full scale in every format, cf32 included, and a
  SigMF sidecar (`isdb:system "ISDB-S"`, the layout, the channel's IF)
  from which `isdb rx --iq` picks ISDB-S by itself and decodes every TS
  byte-exact. `--radio` goes through the same gate: a BSn / NDn channel
  is tuned at its IF behind `--freq-unrestricted`, at `--rate`, with a
  40 MHz filter by default; a Gigabit-Ethernet Pluto underruns at ISDB-S
  rates and a HackRF refuses them, which `tx` says. ISDB-T-only options
  with `--system isdbs`, and ISDB-S-only ones without, are refused
  naming the system.
- `isdb-cli`: `isdb loopback --system isdbs` (`--superframes`,
  `--cfo-hz`, the layout and inputs of `tx --system isdbs`): the ISDB-S
  transmitter through AWGN, a carrier offset in Hz and a sample offset
  into the ISDB-S receiver, every TS_ID verified byte-exact and every
  TMCC against the one sent, `--json` per TS_ID; PASS from 7 dB SNR on
  the BS layout and 4 dB on the CS layout (VERIFICATION.md "ISDB-S
  transmitter and loopback"). `tools/isdbs-loopback-sweep.sh` sweeps it.
- The ISDB-S transmitter libraries: `isdbs-core` `TmccInfo::assign` (the
  TMCC information of a slot layout, validated), `isdbs-phy` `tx`
  (`Transmitter`: TS packets per TS → the 319 488 symbols of a
  superframe and where each packet went; `frame_symbols`), and
  `isdb-frontend` `shape::Shaper` (symbols → root-raised-cosine IQ at any
  rate ≥ (1 + α)·R, fractional samples per symbol, chunking-invariant).
  Tested as the receiver's inverse: byte-exact symbol-domain loopbacks on
  five combination mixes, and a sample-domain loopback at 40 Msps through
  the shaper and the receive front end (the trim found is 0.000 symbol).
- `isdb-frontend`: the CPU reference of the ISDB-S transmitter in the
  FPGA (docs/plans/isdbs-fpga-tx.md step 2). `shape::fixed`: `TxDesign`
  → `TxParams` (Q0.24 step, shift, two symmetric 18-bit banks of 512
  phases × 24 taps; `validate` refuses an asymmetric bank) and the
  bit-exact `Interpolator` (4-bit symbol codes → 12-bit IQ at the DAC
  rate, no multipliers; a DMA stall enters zero symbols counted in
  MISSED / FIRST_MISS). `txformat`, TX format 1: code 0 the zero symbol,
  8 + k the point e^{jkπ/4} (annexed table 46, `LabelCodes`), eight codes
  per two-channel DAC DMA sample (14.43 MB/s at 28.86 Mbaud).
  `control::tx`: the register map on the DAC core's GPIO pair, the ID
  word 0xB5D1_1918, `Encoder::configure_tx`. `vectors::tx_document` and
  `examples/txvectors.rs` write the HDL's test vectors
  (`fpga/build/tx-*.json`). Measured: ISI MER 54.38 dB through an ideal
  matched filter; in the digital loopback into the receive front end
  8PSK MER 36.96 dB at its 6-bit output (the f32 shaper 36.98 dB),
  `examples/txloop.rs`.
- `isdbs-phy`: `Transmitter::superframe_codes` / `null_superframe_codes`
  (`tx::SuperframeCodes`), `tx::frame_codes`,
  `demod::encode_frame_codes[_with_state]`: the transmitter's symbols as
  `isdb_frontend::txformat` codes, made from the bits by the same encoder
  as the `f64` points (docs/plans/isdbs-fpga-tx.md step 3). Tested: the
  codes are the symbols on every layout and mapping, the symbol-domain
  loopback from the codes' points byte-exact, and the sample-domain
  loopback at 40 Msps through the fixed-point transmit interpolator
  byte-exact; `examples/txvectors.rs` writes the FPGA's vectors with real
  codes.
- `isdb-cli`: `isdb tx --system isdbs --offload auto|fpga|cpu`
  (docs/plans/isdbs-fpga-tx.md step 4): to a radio whose FPGA has the
  ISDB-S transmit block, the producer hands symbol codes
  (`Transmitter::superframe_codes`, 14.43 MB/s) instead of shaped samples,
  with the same pre-roll, lead and flush frame and no shaper; it runs at
  11.1× real time (release; 1.14× with the shaper). `auto` (default)
  takes the block when the radio reports a compatible one (a read-only
  probe with `--rf-enable`), else today's IQ path and its underrun
  warning; `fpga` without a block is an error, as is `fpga` with `--iq`;
  `cpu` is the IQ path. A dry run opens nothing: `fpga` describes the
  block's settings (modulator, link rate, buffers, `kbuffers`, the level
  as its backoff), `auto` both paths. The gate is unchanged. Opt-in
  hardware test `tx_symbols_loopback_on_a_pluto`
  (`ISDB_RS_PLUTO_TX_LOOPBACK_URI`): TX into the AD9361's digital
  loopback and RX through the FPGA front end of the same board, every TS
  byte-exact, 0 uncorrectable, 0 host underruns, 0 RX losses, MISSED 0
  with the block's prefetch (≤ 16 at FIRST_MISS 0 on the image without
  it); passes on the PlutoSky R2 (VERIFICATION.md).
- `isdb-radio`: the transmission of symbol codes through an FPGA transmit
  block. `SymbolSink` (separate from `IqSink`), `Offload::TxSymbols {
  version, phases, taps }` and `Offload::runs_tx`, `Backend::tx_offloads`
  (probed without changing anything), `describe_tx_symbols` /
  `open_tx_symbols(cfg, opts, &TxParams)` (`Unsupported` by default),
  `RadioError::Modulator` / `ModulatorParams`. `QueuedSink<R, E>` is
  generic over its converter (`Encode`: `Quantizer<F>` for samples,
  `CodeEncoder` for codes, eight per two-channel DMA sample), so prefill,
  back-pressure, stall detection, drain, the stop flag and the underrun
  counters are shared and tested for both; it also converts what the
  device takes ahead (`RadioTx::ahead`) before starting, and calls
  `RadioTx::started` / `status` / `summary`. The `pluto` backend's mode:
  the block's ID word probed first, the radio configured as for IQ, r1_mode
  and the TX FIR bit refused, `Encoder::configure_tx`, STEP read back, CTRL
  START|SELECT with STATUS "selected" required, the attenuation last; CTRL
  RUN|SELECT once the stream's first buffer is in the board (so the block
  misses nothing at the start) and the DAC underflow flag cleared; MISSED
  per status line; CONSUMED, MISSED, FIRST_MISS, STATUS and the flag in
  the summary; on stop, drop or error CTRL 0 before the attenuation and
  LO. TX1 only, both channels, `buffer` 65 536 and `kbuffers` 16 by
  default; `kbuffers` is now a TX argument (IQ default 4).
- `fpga`: the transmit block's follow-ups (docs/plans/isdbs-next.md item
  5; not yet in a built image). Prefetch while selected: the block polls
  the DMA from SELECT, drops the underflow answers before RUN and holds
  up to two words, so CTRL RUN|SELECT starts without the 8–16-symbol
  start miss (the deployed host's own sequence gets it too); STATUS bit 3
  PRIMED; CTRL 0 drops the held words; PRE_UNF (0x0A) counts the dropped
  polls. BANK_TAG (0x08): the bitstream's value is the default bank's
  CRC-32C, any bank write clears it, the host writes it after a load; no
  reset. CONSUMED, FIRST_MISS and SAMPLES are 64-bit, the high halves in
  HI (0x09), latched by the low half's READ. FEATURES (0x07) = 7; the ID
  word stays 0xB5D11918 (an older image reads FEATURES 0). Yosys: +80
  LUT, +206 FF (2 793 / 3 110), 0 DSP, 0 BRAM. Nine more simulation tests
  (33). `mise.toml`'s uv tasks default `UV_DEFAULT_INDEX` to PyPI.
- `isdb-frontend`: `Interpolator::prefetched(k)` (the stalls on the k
  words a prefetching block holds at RUN are not seen),
  `vectors::tx_document_prefetched` and the vectors' `"prefetch"` and
  `params.bank_tag`, `examples/txvectors.rs` writes `tx-6-prefetch.json`;
  `control::tx::{bank_tag, features}`, `reg::{FEATURES, BANK_TAG, HI,
  PRE_UNF}`, `status::PRIMED`, `Encoder::configure_tx_registers` (STEP
  and SHIFT alone).
- `isdb-radio` `pluto`: the symbol path reads FEATURES at open and keeps
  the old behaviour at 0. With BANK_TAG a matching tag skips the bank
  ("bank: default (tag match), skipped 13312 commands"), else the bank is
  loaded and tagged; with the prefetch, RUN waits for STATUS PRIMED (up to
  1 s) and PRE_UNF is logged; with 64-bit counters CONSUMED and
  FIRST_MISS are read with their HI words, without them the host extends
  the 32-bit registers (which wrap every 148.8 s) from the time between
  reads at the symbol rate. CTRL 0 before the configuration. The open
  line and the summary name the features in use.
- `isdb-iq`: the SigMF field `isdb:system` (extension 0.4.0).

- `isdb-radio` `pluto`: without `uri`, the device is found by mDNS when
  it is opened (one legacy unicast query for libiio's DNS-SD service
  `_iio._tcp.local` and for `pluto.local`; the one host answering becomes
  `ip:ADDRESS`, none or several is `RadioError::Discovery`). The
  `ISDB_PLUTO_URI` environment variable, or `uri`, gives the device
  instead and sends no query. `ip:pluto.local` is no longer the default.
  `tools/pluto-console.py` takes `--user` / `--password` (default the ADI
  firmware login) and `--tty`, each also from `ISDB_PLUTO_USER`,
  `ISDB_PLUTO_PASSWORD`, `ISDB_PLUTO_TTY`; `tools/pluto-autorun.sh` keeps
  the board's `avahi-daemon` running so that it answers.

- `isdb-cli`: the transmit and tools side emits `tracing` events too
  (`isdb loopback`, `mux`, `ts edit`, the `--relay` builder and the
  inputs of `isdb tx`): every line on stderr is an event under
  `isdb::loopback`, `isdb::mux`, `isdb::relay`, `isdb::ts` or `isdb::tx`
  with its values as fields (`pass`, `frames`, `packets`, `path`, `pid`,
  …); the relay policy's copy-control note carries `source` and `output`.
  Message text unchanged; the `isdb:` / `isdb mux:` / `isdb loopback:`
  prefixes became the `[component]` of the text format.

- `isdb-cli`: the receive side's diagnostics are `tracing` events: `isdb
  rx` (ISDB-T, ISDB-S, ISDB-S3, the continuous symbol stream), `isdb
  symbols`, `isdb capture` and the Ctrl-C notice. Setup, status and
  summary lines are `info` events with the message's numbers as fields
  (`frames`, `valid`, `seconds`, `losses`, `blocks_lost`, `resyncs`,
  `mer_db`, `ldpc_failed`, …); a loss, a resync, a DMA overflow, a block
  shortened or lost samples inside it, a probe without lock and the
  "no ISDB-T signal" diagnoses are `warn`. The text format is unchanged
  except that the `isdb rx:` and `isdb:` prefixes are `[rx]` and
  `[isdb]`.

- `isdb-ts`, `isdb-si`, `isdb-tlv`, `isdb-mmt`, `isdb-text`: the codec
  libraries emit `tracing` events (no subscriber) for what they already
  report as data: every continuity event and section-assembler event, a
  CRC_32 failure, a section_length over 4093 and an SI table on a PID
  Table 5-1 does not assign to it (`debug`, with `pid`, `packet_index`,
  `table_id`, `expected` / `got`); TLV sync losses, MMTP sequence gaps,
  dropped MFUs and messages, rejected tables and version changes
  (`debug`, with `packet_id` / `table_id`); text-decoding issues
  (`trace`). Nothing is emitted for packets that are in order, and the
  returned events, counters and `Vec`s are unchanged.

- `isdbt-mux`, `isdbt-phy`, `isdbt-ofdm`, `isdbt-fec`: the ISDB-T
  libraries emit `tracing` events (no subscriber; `isdb --log-level` /
  `--log-format json` print them). `isdbt-mux`: the relay's dropped
  descriptors and unselected streams, over-capacity, late-PES and ADTS
  compliance warnings at `warn` with `pid`, `tag`, `role` fields; the
  copy-control and conditional-access decisions, the probe summary, relay
  passes and carousel rebuilds at `info`; CC errors, dropped PES and table
  versions at `debug`. `isdbt-ofdm`: lock acquired (`mode`, `guard`,
  `cfo_carriers`, `offset_hz`), lock lost and TMCC parameter switches at
  `info`, acquisition steps and per-frame metrics at `debug`, the
  per-symbol timing step at `trace`. `isdbt-phy`: per-frame RS statistics
  per layer at `debug`. `isdbt-fec`: the multiplex pattern at `debug`.
  Nothing per packet or per symbol above `trace`.

- `isdbs-phy`, `isdbs3-phy`, `isdb-frontend`, `isdb-radio`, `isdb-iio`,
  `isdb-hackrf`, `isdb-iq`: the satellite and radio libraries emit
  `tracing` events (no subscriber). `info`: acquisition (carrier, trim,
  burst MER / sync Es/N0), ISDB-S frame sync acquired and lost, ISDB-S3
  slot sync found or not, the first TMCC and every TMCC change (with the
  modulations and rates in use), a radio opened with its tuning and gains
  (`rate_hz`, `freq_hz`, `bandwidth_hz`, `gain`). `warn`: TX underruns
  since the last status, RX overruns and dropped buffers, ADC overflows
  during a capture, a loss in a `Symbols` stream. `debug`: per frame and
  per superframe TMCC, RS / LDPC / BCH failure counts and MER, the survey,
  library and device details, SigMF metadata read. Nothing is emitted per
  sample, symbol or packet, and the front end's FPGA test vectors are
  unchanged.
- `isdb`: diagnostics are `tracing` events. The global options
  `--log-level` (default `info`; `RUST_LOG` takes an `EnvFilter`
  directive and overrides it) and `--log-format text|json` select what
  reaches stderr and how: `text` keeps the `[tx] …` lines, `json` prints
  one object per line with the event's structured fields (`isdb tx`'s
  status, warning and summary events carry `frames`, `on_air_s`, `speed`,
  `null_fill_pct`, `end`, …). Reports on stdout are unchanged. The
  libraries emit no events yet; `docs/plans/errors-and-logging.md` has
  the plan.
- `isdb-cli`: the left-hand-circular BS and 110° CS channels: `BS2`..`BS24`
  (even) and `ND1`..`ND23` (odd) are accepted by `--freq` / `--channel`
  and tuned at the IF of the left-hand LNB LO, 9.505 GHz (MIC Ordinance
  No. 95 of 2011, Art. 18–19; IF 2224–3224 MHz), e.g. `--freq BS14`
  (NHK BS8K) at 2471.82 MHz; the right-hand channels keep the 10.678 GHz
  LO. Labels read ` (BS14 IF, left-hand)`. Receiving them needs a feed
  that carries the left-hand IF (the lab's cable feed does not).
  `isdb capture` records `isdb:lnb_lo_hz` and `isdb:polarisation`
  (`right` / `left`) next to `isdb:channel_rf_hz` for a satellite
  channel (`isdb` SigMF extension 0.3.0; older recordings, without them,
  read as before). `tools/tlv-nit-polarisation.py` lists the frequency
  and polarisation of each satellite delivery system descriptor in a TLV
  stream file.

- `isdb-mmt` (new): the content of ISDB-S3 TLV streams per ARIB STD-B60:
  MMT-SI descriptors (16-bit tags, the length width of each descriptor's
  structure), the MPT with its assets and locations, PLT, MH-EIT, MH-SDT,
  MH-TOT, MH-BIT, TLV-NIT and AMT (CRC-32 checked, repeats not decoded
  again), divided and aggregated control messages, MPU payloads and MFUs
  reassembled per asset with packet-loss and MPU-continuity counters,
  NTP, and elementary-stream framing (HEVC Annex B, LOAS, caption MFUs).
  MMT-SI text is UTF-8. Scrambled payloads are counted from their clear
  8-byte header and never descrambled.
- `isdb-cli`: `isdb tlv inspect` decodes the tables (network, services,
  events, packages and assets, time; `--json`), `isdb tlv extract
  --out-dir` writes each asset's elementary stream (`--service`,
  `--asset`; scrambled payloads are written as they are, still encrypted),
  `isdb tlv validate` checks the structure. All three stream the file.
- `isdb-tlv`: `tlv::FileReader`, a TLV stream file read a chunk at a
  time.
- `isdb-cli`: `isdb rx --system isdbs|isdbs3 --radio …` receives
  continuously through the radio's FPGA symbol front end when it offers
  one (`--offload auto|fpga|cpu`, default auto): acquisition on a short IQ
  probe, then the front end's `Symbols` words decoded as one stream, cut
  on frame (ISDB-S3) or superframe (ISDB-S) boundaries, with losses and
  lost locks reported as discontinuities. `--symbols FILE` decodes a words
  file (`isdb symbols --out`). Live from a PlutoSky R2: 30 s of gapless TS
  from BS1 (every packet RS-clean, 0 continuity errors) and BS7's TLV
  streams with one reported loss.
- `isdbs3-phy`: `rx::Rx::frame_starts` and `RxOptions::slot_offset` (a
  synchronisation hint for streams cut on a frame start);
  `frontend::acquire`. `isdbs-phy`: `frontend::acquire`.
- `isdb-frontend`: format 4, the arithmetic of an FPGA that takes one
  sample per clock (the PlutoSky R2 clocks its sample path at the sample
  rate): loop delay 24 symbols (was 6), the timing detector's banks `h`
  and `g` non-zero on their centre 8 taps (`TED_TAPS`, enforced by
  `Params::validate`). Same words as format 3; `MIN_ARITH_VERSION` 4, so a
  format-2/3 FPGA is refused. Examples `tedspan` and `loopdiag`.
- `fpga`: the full-rate symbol front end (a sample every clock, fully
  pipelined, 112 DSP48E1) and the PlutoSky R2 image that closes timing
  with Vivado 2026.1 (`fpga/tezuka/`, `isdb_symbols.xdc`; the image leaves
  out Tezuka's DVB-S2 transmitter). `isdb_top.bin` builds; not yet run on
  the board.
- `isdb-radio`: the Pluto symbol path refuses to run when the AD9361 core
  is in r1_mode (the clock case the bitstream is not timed for).
- `isdb-frontend`: AVX2 and AVX-512 builds of the fast front end's dot
  products, chosen at run time (`simd::Isa`; AVX2 by default, AVX-512
  with `ISDB_SIMD=avx512` and Rust ≥ 1.89, `ISDB_SIMD=baseline` for the
  portable path; `FastFrontend::with_isa`). Same output on every
  instruction set (tested per set, with fault injection). ISDB-S3 front
  end on one core 0.83× → 1.19× real time, ISDB-S 1.40×. The crate's one
  `unsafe` is the call into the chosen `#[target_feature]` loop after
  detecting its features (`simd::run`), named in the workspace
  `Cargo.toml`.

- `isdb-frontend`: `Symbols` format version 3. The block trailer is a
  CRC-32C (two words: CRC-32/ISCSI of the 32-bit sequence number and
  the payload, little endian) instead of a CRC-16, and a version-3
  block is accepted on it alone: no framing check, so neither the
  intact block before a loss is dropped nor delivery held back. Versions
  1 and 2 still read (CRC-16 and framing check); `Packer::with_version`
  writes them. Slicing-by-8 CRC-32C in safe Rust (2.8 GB/s per core).
  The FPGA packer computes it one word per clock (+279 LUT, +17 FF);
  the host drives front ends of version 2 or 3. `examples/crcspeed`.
- `tools/sigmf-repeat.py`: repeat a SigMF recording into a long
  multi-block one, for pipeline throughput measurements.
- `isdb-frontend`: `fast::FastFrontend`, the bit-exact front end 2–2.4×
  faster (linear delay line, fused banks, 16-bit products in `i32` on
  split coefficients, an `i64` kernel while a larger input is in the
  line), equal to `fixed::Frontend` symbol for symbol (property tests,
  recordings); the receivers run it, the FPGA vectors stay made with the
  reference. `process_each` hands symbols to a closure.
- `isdb-frontend` `s3sync::measure_at`; the slot search on every core,
  bit for bit as one thread.
- `isdbs3-phy`: `main_signal::decode_frames` (every codeword of several
  frames in one pool), `sync::synchronise_at`, `frontend::symbols_with_prior`
  (reuse a block's carrier and trim after checking them on 0.05 s);
  `rx::receive` prepares frames in parallel: 5.85 → 0.60 s per 0.83 s
  block, output unchanged.
- `isdbs-phy`: `frontend::symbols_with_prior`, the trim candidates run at
  once (7.6 → 0.65 s), frame acquisition and derotation on every core,
  `par` (ordered parallel map, first-maximum search).
- `isdb-cli`: `isdb rx --system isdbs` / `isdbs3` read or capture, run the
  front end and receive at once, up to three blocks' front ends at a time
  (`satpipe`); the summary's `[rx] time:` line and `time` in `--json`.
  ISDB-S3 recordings decode 3–5× faster, ISDB-S 5–8×, with the same
  output on single-block recordings; live from a PlutoSky the decoder
  keeps up with the radio's captures.
- `tools/rx-compare.sh`: decode recordings with two builds and compare
  the outputs byte for byte.

- `isdb-fec`: `ldpc::fixed`, a fixed-point layered offset min-sum
  decoder for the quasi-cyclic IRA codes, decoding 16 codewords at once
  in SIMD lanes (refilled as each stops) with the same bits and
  iterations as its one-lane scalar form; `simd`, its kernels, written
  for auto-vectorisation on stable compilers, or on `std::simd` when the
  build script finds a matching nightly. ISDB-S3 LDPC: 166 → 4 678
  codewords/s per thread, 11.4× real time on 32 threads.
- `isdb rx --system isdbs3 --ldpc fixed|scalar|float`: the main signal's
  LDPC decoder (fixed by default).
- `isdb-fec`: `bch` (binary BCH over GF(2¹⁶), t = 12, any shortening:
  ARIB STD-B44 Table 3.4-1), `ldpc` (quasi-cyclic IRA LDPC from a
  374-bit-group address table: encoder and layered normalised min-sum
  decoder) and `prbs::B44Prbs` (the ISDB-S3 slot, TMCC and pilot energy
  dispersals, output from the last stage).
- `isdbs3-core` (new): ISDB-S3 parameters from ARIB STD-B44 v2.1: frame
  and slot geometry, the eleven code rates and their LDPC codes (address
  tables transcribed by `tools/b44-ldpc-tables.py`), π/2-BPSK to 32APSK
  constellations with γ, the bit interleaver, energy dispersal, the TMCC
  information codec and signal coding, slot allocation and codeword
  placement. Readings and their confirmation on live signals in its
  SPEC-NOTES.
- `isdbs3-phy` (new): ISDB-S3 receive chain after the symbol front end:
  slot and frame sync, carrier phase per slot, an 11-tap least-squares
  equaliser trained on each frame's sync and pilot symbols, TMCC
  decoding, the pilot-learned constellation and the main signal (max-log
  demapping, de-interleaving, LDPC, dispersal, BCH) to each slot's data;
  `rx::receive`.
- `isdb-tlv` (new): TLV packets reassembled from a stream's slots with the
  head-TLV indication, IPv4 / IPv6 / header-compressed IP, MMTP headers
  (with the B60 scramble-information extension), MMT-SI messages and
  table ids, TLV-SI sections by CRC, per-stream statistics.
- `isdb-cli`: `isdb rx --system isdbs3` (and `auto` from a recording's
  survey) writes one TLV stream file per stream id (`{tlv_id}` in
  `--out`, or `--ts-id`), offline or block-wise from a radio; `isdb iq
  s3tmcc` (ISDB-S3 TMCC of a recording); `isdb tlv inspect` (content of
  a TLV stream file).

- `isdbs-core` (new): ISDB-S parameters from ITU-R BO.1408-1 and MIC
  Ordinance No. 87 of 2011 annexed tables 45–51: frame geometry (192-symbol
  header, 192 × (203 + 4)), W1/W2/W3 and their coded tails, the seven
  modulation-code combinations with slot units and puncturing, slot runs,
  the 384-bit TMCC information codec with validation and the TMCC signal
  of a superframe (RS(64,48), energy dispersal), the burst bits.
- `isdbs-phy` (new): ISDB-S reception after the symbol front end: frame
  sync on the frame's 808 known symbols with slip detection, burst-aided
  forward-backward carrier loop, TMCC decoding, and the main signal to TS
  packets (`demod`: BPSK / punctured QPSK / TC8PSK soft demapping with
  set-partition metrics, one Viterbi decoder per frame from the W2/W3
  state to the next W1; `ts`: de-interleaving, energy dispersal, sync-byte
  restoration, RS(204,188), TS_ID per packet, superframes decoded in
  parallel). Examples `mapprobe` and `throughput`.
- `isdbs-core::mainsig`: the slots sent (dummy slots after the valid ones
  of each unit), the superframe energy dispersal, the depth-8 inter-frame
  interleaver of BO.1408-1 Fig. 6, a superframe's RS codewords and their
  transmit-side inverse.
- `isdb-fec`: `ViterbiDecoder::with_state` (known start state),
  `push_known` (known trailing bits end the trellis) and `push_metrics`
  (arbitrary branch metrics, for trellis-coded 8PSK).
- `isdb-cli`: `isdb rx --system auto|isdbt|isdbs` receives ISDB-S from a
  SigMF recording or a radio, one TS per TS_ID (`--out` with `{ts_id}`,
  `--ts-id`), per-TS RS counters and JSON; from a radio it decodes one
  contiguous capture after another (`--rate`, `--bw`, `--block-seconds`).
  `tools/isdbs-ts-acceptance.sh` receives a capture directory and checks
  each TS. 19 of the 21 lab transponders give RS-clean TS (311 520
  packets, 0 uncorrectable, 0 continuity errors; VERIFICATION.md).
- `isdb-cli`: `isdb iq tmcc REC [--summary|--json]` decodes the frames and
  the TMCC of an ISDB-S recording: combinations, slots and TS_IDs per
  superframe, slips, pilot and main MER. `tools/isdbs-tmcc-acceptance.sh`
  runs it over a capture directory. The TMCC decodes on every superframe
  of the 9 BS and 12 110° CS transponders recorded from a cable feed
  (VERIFICATION.md).
- `isdb-ts`: MPEG-2 transport stream layer — bit I/O, CRC_32, TS packets,
  adaptation field and PCR, PES, section assembly and packetisation,
  PAT/PMT/CAT, a TS reader for 188/192/204-byte framings.
- `isdb-text`: ARIB STD-B24 8-unit character coding, decode and encode.
- `isdb-si`: ARIB STD-B10 service information tables and descriptors with
  lossless typed codecs (reserved bits kept), MJD/BCD time and separate
  conformance checks.
- `isdb-analyze`: TS inspection, spec validation with clause citations,
  table/field diff of two streams and experiment editing.
- `isdbt-mux`: service multiplexer — PSI/SI generation, MPEG-2 video and
  AAC (ADTS, STD-B32 Part 2 rules) packetisation, rebroadcast of a
  recorded program (`ts-program`), CBR output paced to the OFDM frame,
  layer capacities.
- `isdbt-core`: ISDB-T transmission parameters (modes, guard intervals,
  layer parameters, derived rates; STD-B31 tables).
- `isdbt-fec`: STD-B31 §3.2–3.8 channel coding — TS re-multiplex, RS(204,188),
  energy dispersal, delay adjustment, byte interleaving, convolutional
  code with puncturing and a Viterbi decoder.
- `isdbt-ofdm`: STD-B31 §3.9–3.16 — bit interleaving and mapping,
  layer combining, time/frequency interleaving, OFDM frame (SP/CP/TMCC/AC),
  IFFT/guard interval, and receiver synchronisation and equalisation.
- `isdbt-phy`: TX/RX pipelines between TS packets and complex baseband,
  IQ file formats (cf32/cs8).
- `isdb-hackrf`: minimal safe wrapper over the system libhackrf for TX/RX
  streaming.
- `isdb-radio`: device-neutral SDR layer between `isdbt-phy` and a radio —
  the `IqSink` / `IqSource` traits, `DeviceSpec`
  (`NAME[:key=value,...]`), `RadioConfig`, `TxOptions` (digital backoff,
  checked to 0–40 dB, and transmit-queue sizing) and `RxOptions`
  (receive-queue length in seconds), the `Backend` trait with a registry
  of compiled-in backends (an unknown or left-out backend is reported
  with the `isdb-radio` feature enabling it and the `isdb-cli` feature
  forwarding it), the `Quantizer` for integer sample formats, the
  `QueuedSink` transmit path shared by backends with integer samples
  (`RadioTx`), and an in-memory sink and radio for tests (feature
  `mock`). The HackRF backend (feature `hackrf`, default on) moved here
  from `isdb-cli`; it validates its device arguments, gains, the
  1 MHz–6 GHz / 2–20 Msps range and the baseband filter request without
  opening the device.
- `isdb-iio`: minimal safe wrapper over the system libiio (0.x API) —
  contexts by URI (USB, Ethernet, local), device, channel and debug
  attributes, and TX/RX streams of interleaved `i16` on their own threads
  with bounded queues, underrun/overrun counters, a drain on TX end and a
  stop that cancels a blocked buffer only as a fallback.
- `isdb-radio`: the `pluto` backend (feature `pluto`, default on) for
  AD936x radios with ADI's Pluto firmware (ADALM-Pluto, PlutoSky R2, …)
  over libiio, e.g. `--radio pluto:uri=ip:192.0.2.30` over Ethernet. Device
  arguments `uri`, `channel` (TX1/RX1 or TX2/RX2), `buffer` and, on TX,
  `loopback` (digital TX→RX loopback with the TX LO powered down, for
  tests without radiating); TX gain `att` (attenuation, default the
  maximum 89.75 dB), RX gains `agc` and `gain`. The programmed LO, the
  sample rate the clock chain runs at (512/63 MHz: −0.02 ppm) and the RF
  bandwidth are read back and reported; a rate more than 10 ppm off is
  refused. Opening the TX sets the maximum attenuation and switches the
  DDS off first; closing it restores the maximum attenuation and powers
  the TX LO down. The `Cs12` sample format (12-bit, left-aligned in
  `i16`) is the DAC format of its `QueuedSink`. A TV received its
  `isdb tx` output from a PlutoSky R2's TX2 over a conducted link
  (VERIFICATION.md "Real receivers").
- `isdb-cli`: the `pluto` feature (default on) forwarding `isdb-radio/pluto`.
- `isdb-iq`: IQ recordings independent of the broadcast system — sample
  formats (`cs8`, `cs16` with its full scale, `cf32`), SigMF 1.2 metadata
  with an optional `isdb` extension (full scale, LO and bandwidth read
  back, gain, channel, capture method, gap marker between capture
  segments and the samples discarded around a loss), `Recording` (opened by data, `.sigmf-meta` or base name), and
  a single-carrier survey: power spectrum and band edges, the |z|²
  symbol-rate line (aliases above fs/2 included), z², z⁴, z⁸ and
  π/2-BPSK carrier lines, and the system (ISDB-S, ISDB-S3) whose symbol
  rate matches.
- `isdb-iio`: `Context::capture`, one buffer refilled once (contiguous
  samples without a stream thread, returned as the device's raw bytes);
  `Context::start_rx_raw`, an RX stream of raw bytes for any channel
  layout that waits for its consumer instead of dropping; register access
  (`Device::reg_read` / `reg_write`).
- `isdb-radio`: raw captures — `Backend::describe_capture`,
  `capture_limit` and `capture`, which writes the samples to an
  `io::Write` as they arrive and returns a `Capture` (`CaptureFormat`
  cs8 or cs16 with its full scale, samples written, `CaptureGap`s, rate /
  LO / bandwidth / gain read back, device identity, method, host time);
  `hackrf` cannot capture. The `pluto` backend captures through one
  libiio buffer (`recorder=iio`, up to 63 MiB), Tezuka's maia-sdr
  recorder over HTTP (`recorder=maia`, its 128 MiB buffer, modes
  `IQ16bit` / `IQ12bit` / `IQ8bit`) or a stream of libiio buffers
  (`recorder=stream`) whose losses it finds from the ADC core's overflow
  bit; `bits=8` selects 8-bit samples (Tezuka's packing through libiio).
- `isdb-cli`: `isdb capture` (receive only: one contiguous block per
  segment, or a stream, at any rate the radio supports; cs16 or cs8 +
  SigMF metadata; `--samples` or `--seconds`, `--segments` and losses
  inside a stream as SigMF segments with gap markers, `--discard` to test
  a link without writing, progress every 5 s, Ctrl-C ends a stream and
  keeps what it wrote, `--dry-run`, `--survey`, no overwrite without
  `--overwrite`) and `isdb iq survey` (text or `--json`). `--freq` of
  `capture` takes channel names: `T<n>` (UHF), `BS<n>` and `ND<n>` (BS
  and 110° CS at their satellite IF).
- `isdb-frontend`: single-carrier receive front end for ISDB-S and
  ISDB-S3 (IQ to symbols at one per symbol):
  - a bit-exact fixed-point model of format version 2: 32-bit NCO mixer,
    64-phase polyphase root-raised-cosine matched filter with its
    derivative and an output filter (the matched filter a trim later),
    non-data-aided timing recovery on the first two with a PI loop applied
    six symbols late, AGC and n-bit output on the third;
  - a floating-point reference;
  - the `Symbols` boundary: 16-bit words in blocks with sync, sequence
    number, gain, period, header check and a CRC-16 trailer, and a
    `Reader` that reports every loss;
  - `Design::for_system` with the settings measured on the satellite IF
    (32 taps, 6 bits for both systems) and `Design::trim`;
  - ring / PSK MER and SQNR metrics;
  - `s3sync`: ISDB-S3 slot synchronisation on the STD-B44 sync words, the
    SNR on those known symbols, and `best_trim`, the trim search that the
    host runs before configuring the front end;
  - the FPGA's register map and command words over the AD9361 core's
    general purpose registers (`control`);
  - JSON test vectors for the HDL.
- `fpga/`: the same front end in Amaranth HDL (`SymbolsRx`: core,
  command decoder on `up_adc_gpio_out`, ID word on `up_adc_gpio_in`,
  output selection before the RX DMA's packer), format 2 with the
  output bank. It matches the model bit for bit in simulation, trimmed
  and untrimmed, and Yosys estimates 64 DSP48E1 and 3.9k LUT for 32 taps.
  `mise run fpga:test`, `fpga:estimate` and `fpga:verilog`. Not yet built
  into a bitstream.
- `isdb-radio`: `Offload` (stages a radio runs itself), probed by
  `Backend::offloads` without changing the device, and
  `Backend::capture_symbols`. The Pluto backend reads the front end's ID
  word, configures it through the command register (checked by a
  readback), streams the 16-bit words and switches back to IQ afterwards.
- `isdb-cli`: `isdb symbols` (receive only). It uses the radio's FPGA
  front end when its bitstream has one (`--offload auto|fpga|cpu`,
  `--probe`). Otherwise it runs the CPU front end on one contiguous IQ
  capture (40 Msps and 56 MHz analog bandwidth by default). Also `isdb iq
  symbols`: the CPU front end on a recording, or `--read` a words file
  (blocks, losses, ring MER).
- `isdb-cli`: the `isdb` binary — `tx` (to an IQ file or a radio; radiates
  only with `--rf-enable`), `rx`, `loopback` (software channel with
  impairments), `mux` (including `--relay` of a recording) and `ts`
  (inspect, validate, diff, edit).
- `isdb-cli`: `isdb completions SHELL` prints a shell completion script
  (bash, zsh; also fish, elvish, PowerShell) generated with
  `clap_complete` from the binary's own command line.
- `isdb tx` playback keys (when stdin is a terminal; `--no-keys`,
  `--seek-step S`): space pauses / resumes, ←/→ seek, ↑/↓ change the play
  rate (0.25–16×) of `--relay` and `ts-program` relays. The engine is
  `isdbt_mux::playback` with `Multiplexer::enable_playback_control`,
  `pause`, `resume`, `seek`, `set_rate` and `playback_status`: PSI/SI and
  PCR continue in every state, fast play skips video pictures and audio
  frames within the source's own rate, slow play repeats audio frames, and
  seeks use a 0.5 s index from the whole-file scan. Uncontrolled relay
  output is unchanged (byte-identical).
- `isdb tx --iq-in FILE [--iq-in-format cf32|cs8]`: replay a baseband
  file to a radio or file; the receiver finds its parameters and frame
  start, and the playback keys pause (null frames with the same
  parameters) and seek it.
- `isdbt-phy`: `broadcast_ts` — the 204-byte broadcasting TS of ARIB
  STD-B31 Part 2 §5.5 (studio-to-transmitter interface): lossless codecs
  of `ISDB-T_information` in the dummy bytes of each TSP (Table 5-6), the
  optional shortened RS(204,196) parity, and the IIP (Table 5-8) with
  `modulation_control_configuration_information` (Table 5-10, carrying the
  TMCC information) and `network_synchronization_information` (Table
  5-12, SFN delay control); `BroadcastTsWriter` wraps multiplex frames
  with one IIP per frame, `BroadcastTsReader` delimits frames, splits them
  by layer and recovers the transmission parameters from the IIP, so a
  `Transmitter` can be configured from the stream. `isdbt-phy` now
  depends on `isdb-ts` (CRC_32).
- `isdb-si`: `dsmcc` (DSM-CC data carousel DII / DDB sections and the DII
  module descriptors, ARIB STD-B24 Vol. 3 Ch. 6), `ait` (the Application
  Information Table and its descriptors, B24 Vol. 4 Ch. 5, tags per B10
  Part 1 Table 5-5) and `bxml` (additional_arib_bxml_info,
  arib_bxml_info, ait_identifier_info, the BML Control descriptor data;
  B24 Vol. 2 §9.3, Vol. 3 Annex C, Vol. 4 Table 7-1), all lossless, with
  the `DocumentResolution`, `ApplicationControlCode` and
  `ApplicationVisibility` value tables.
- `isdbt-mux`: BML data broadcasting — `[[service.stream]] kind =
  "bml-carousel"` sends the files of a directory as a data carousel
  (module 0x0000 as a `multipart/mixed` entity with the
  `application/X-arib-resourceList`, or explicit `modules` with direct
  mapping), signalled with stream_type 0x0D, a stream_identifier
  (component_tag 0x40) and a data component descriptor carrying
  additional_arib_bxml_info in the PMT and a data content descriptor in
  the EIT; DII / cycle / rate options, `source_encoding = "utf-8"`
  (BML / CSS / script sources transcoded to EUC-JP on load) and `watch`
  (files edited while on air are re-sent with moduleVersion + 1 and a new
  DII transaction, so pages subscribed to `ModuleUpdated` reload) under
  `[service.stream.bml]` (`docs/mux-config.md`). The example pages
  (`examples/bml/startup.bml`, `info.bml`) show the usual layout — picture
  in the upper-right quarter, menu, text strip — with remote-key
  navigation (arrows, Enter, Back, the four colour keys) and a page
  transition, modelled on live stations' documents. The layouts and the default operational values
  (bml version 3.0, data_event_id 0xF in the PMT, the multipart boundary
  and media type spellings, DII repetition) follow a live terrestrial
  recording. Example `examples/bml-data.toml` with
  `examples/bml/startup.bml` (the terrestrial document prologue, the
  broadcast video / audio kept on screen). Shown on a Sony BDZ-ZW1500
  under a local station's identity (`docs/transmitting.md`).
- `isdbt-mux`: Hybridcast — `[service.hybridcast]` with `url`,
  `organization_id` and `application_id` builds an AIT (application_type
  0x0010, AUTOSTART, profile 0x0002 / 1.1.1, HTTP transport protocol,
  simple application location, boundary and permission descriptors per
  IPTVFJ STD-0013) and carries it as a resource of the service's
  bml-carousel (module 0x9FFF / `app1.ait`, with a generated `startup.bml`
  launcher calling `startAITControlledApp` unless the files have one) or,
  with `carriage = "es"`, on its own PID with ait_identifier_info in the
  PMT. Example `examples/hybridcast.toml`.
- Verification: spec-vector tests, property-tested lossless round trips,
  TX→RX loopbacks, opt-in tests on real captures (`ISDB_RS_*` environment
  variables) and a differential test against gr-isdbt (`oracle/`,
  `VERIFICATION.md`).
- Repository harness: pinned toolchain, rustfmt/clippy/cargo-deny
  configuration, git hooks (lefthook, commitlint), mise tasks as the single
  check entry point, CI workflows (lint, test, doc, cargo-deny, MSRV) and
  Renovate.
- `oracle/Dockerfile`: a reproducible gr-isdbt oracle container (Ubuntu
  24.04, GNU Radio 3.10, gr-isdbt built from upstream at a pinned commit),
  and `oracle/README.md` with the licensing of the oracle scripts.
- `README.ja.md`: a Japanese translation of the root README, linked from
  the first line of `README.md` (the documents it links to stay in
  English).

### Changed

- `isdb rx` (ISDB-S3, streamed `Symbols`): each chunk's sync, equaliser,
  TMCC and symbols run on the stream's thread while the calling thread
  decodes the previous chunk's LDPC/BCH and writes its TLV packets, in
  order (`isdbs3_phy::rx::receive_front` / `receive_back`); the summary
  gains a `stages` line. With the unrolled max-log demapper, one
  interleaver table per mode, a byte-wise codeword check before the BCH
  syndromes and the per-slot phase step taken once per slot, 1 minute of
  BS-14 decodes in 244.5 s instead of 297.6 s on 4 cores, byte-identical.

- `isdb-cli`: the ISDB-T `isdb tx --mux-config` and `isdb loopback
  --mux-config` refuse a `[network] kind = "bs"` configuration (a
  satellite NIT on an ISDB-T signal), naming `isdb tx --system isdbs
  --channel … --mux-config` and `isdb mux` + `isdb tx --system isdbs
  --channel … --ts <its TS_ID>=…` instead. `--mux-config` is repeatable
  (ISDB-S, one TS each); the ISDB-T commands refuse it given twice.
  `--auto-layout` is refused with `--system isdbs` (ISDB-T only).
- `isdbs-phy`: `Transmitter::new` refuses a dummy combination entry with
  slots (`TxError::DummySlots`; BO.1408-1 §6.1.2 requires 0), which left
  0 + 0j symbols in the main signal; `TmccInfo::assign` never builds one.

- Workspace: the dev profile builds `isdb-frontend`, `isdbs-core` and
  `isdbs-phy` with `opt-level = 3`, as the ISDB-T PHY crates (the ISDB-S
  transmit chain ran at 0.02× real time unoptimised, now 0.8×).
- `isdb-cli`: `tx` and `loopback` take `--system` (default `isdbt`, so
  ISDB-T command lines are unchanged); `isdb tx --channel BSn` without
  `--system isdbs` is refused pointing at it, no longer saying that
  there is no satellite transmitter; `isdb loopback --snr-db` is
  documented as the ratio over the sample rate's bandwidth;
  `Channel::new` is `Channel::with_cfo_hz` with a carrier spacing of
  (512/63 MHz) / FFT size.
- `isdbt-ofdm`, `isdbt-phy`, `isdbt-fec`, `isdbt-core`: typed errors and
  panic triage (errors-and-logging plan, group W4).
  `isdbt_ofdm::Error::Tmcc(String)` is `Tmcc { field: TmccField, code }`
  (the Table 3-23 field and its code as received; the message names both
  and the table: "invalid TMCC: reserved code-rate code 110 in a used
  layer (Table 3-29)"); a TMCC whose layers are each valid but together
  break §3.11.2 is `TmccParams(ParamError)` with the `ParamError` as its
  source; `Error::Invalid(String)` (its one producer, the AC carrier
  count) is `AcCarriers { expected, got }`. `isdbt_phy::Error::Input` is
  `UnusedLayer { layer, got }` and `MultiplexFrameLength { expected, got }`;
  `Error::BroadcastTs(String)` wraps the new
  `broadcast_ts::BroadcastTsError` (CRC, reserved mode, field width,
  limit, SFN length, header, missing / unusable IIP variants; transparent,
  so the message no longer starts with "broadcast TS: "), the codecs of
  `broadcast_ts` return it, and `BroadcastFrame::iip_error` is an
  `Option<BroadcastTsError>`. `ChannelEstimator::estimate` takes the
  current spectrum explicitly. New: `Params::layer_segments` /
  `segment_layer`, `TimeInterleave::adjustment_symbols` /
  `delayed_frames` (Table 3-12 by code), `dsc::DIFFERENCE_SET` (the
  Singer set as a constant; the tests re-derive it), `delay::Fifo::
  from_vec`. The library `expect` / `unreachable!` / `panic!` sites go
  from 26 / 1 / 3 to 1 (test helper) / 0 / 2 (documented contracts): the
  types carry the invariants (fixed-size bit writers, ring FIFOs, the
  layer-segment iterator, a modulation-indexed Gray mapper, Table 3-8 /
  3-12 indexed by the closed enums) and the receiver's state steps take
  their `Option`s with `let … else`. Output is bit-identical (transmitter
  IQ, demodulator LLRs, receiver TS; new ignored probe
  `isdbt-phy/tests/bitexact_hashes.rs`); the transmitter is ~17% faster,
  the receiver unchanged.


- `isdb-radio`, `isdb-iio`, `isdb-hackrf`: typed errors, no panics in
  library code. `isdb-radio` drops `anyhow`: every call returns
  `isdb_radio::RadioError` (`isdb_radio::Result`), with a variant per
  kind of failure and its facts as fields: `Spec` / `KeyValue` (device
  selection syntax), `UnknownBackend`, `UnknownKey`, `RepeatedKey`,
  `Argument` (with the `Rule` broken), `Conflict`, `Backoff`,
  `Duration`, `Tuning`, `Readback`, `CaptureLength`, `Unsupported`,
  `Firmware`, `FrontEnd`, `FrontEndParams`, `Stream`, `StillStreaming`,
  `Reply`, `Device` (backend and operation named, the library's error as
  `source`) and `Io`. `IqSink` / `IqSource` return it; the `isdb-cli`
  file sinks report `Io` naming the file, and `isdb-cli` adds
  `open radio SPEC for TX/RX` context. `isdb_iio::Error` becomes an enum
  (`Call`, `NoDevice`, `NoChannel`, `Nul`, `EmptyStream`, `SampleSize`;
  `code()` gives the errno) and `isdb_hackrf::Error` too (`Call`,
  `OldLibrary`, `NulInSerial`; `code()`). Messages are the same
  sentences except: HackRF frequency ranges read `1..6000 MHz`; Pluto
  `kbuffers` and RX `gain` ranges read `outside 2..64` / `outside
  -10..77 dB`; `agc` lists `manual, slow_attack, fast_attack or hybrid`;
  the RX1-only refusals read `bits=8` / `recorder=maia records RX1 only,
  not channel=2`; an RX that still streams after stopping also says
  `power-cycle the device`. Panic census 2 unwrap / 6 expect → 0.

- `isdb-frontend`, `isdb-iq`, `isdbs-phy`, `isdbs3-phy`, `isdbs-core`,
  `isdbs3-core`, `isdb-fec`: typed errors and panic triage
  (`docs/plans/errors-and-logging.md` §5, group W5). **Breaking:**
  `isdb_frontend::params::ParamError` is an enum naming the violated rule
  with its values (`SamplesPerSymbol`, `Taps`, `Trim`, `LoopGain`,
  `Range { what, value, min, max }`, `Phases`, `RaggedBank`,
  `Coefficient`, `CoefficientSum`, `DetectorTaps`, `PeriodBounds`,
  `NoTrimCandidates`) instead of a `String`; `isdb_iq::Error::Format`
  is replaced by `FullScale`, `Datatype`, `FormatName`, `Meta { path,
  source }`, `NoMeta { missing, .. }`, `Json`, `MissingField`,
  `SampleRate`, `CaptureNotObject`, `CaptureOrder`;
  `isdb_frontend::vectors::document` returns `Result`;
  `isdbs3_phy::tmcc::Error` is `#[non_exhaustive]`. New:
  `isdbs_core::Combination::rank`, `isdbs3_core::CodeRate::TMCC` and
  `CodeRate::index`. 42 of the 43 library `expect`s and the
  `unreachable!` are gone: slice-to-array conversions take arrays
  (`as_chunks`, array patterns), the fast front end's kernels take
  `&[[i16; W]; 6]` / `&[i16; W]`, worker threads' panics are resumed
  instead of re-raised under a new message, and lookups use the enum or
  newtype. `isdb_fec::ldpc::Code::decode` with `max_iter` 0 returns
  `NotConverged` instead of panicking; `isdb_frontend::format::unpack`
  reads the words in one pass and states its `bits` contract (1..=8;
  0 divided by zero). Messages: the
  `ParamError` texts name the bank and phase, `bits 9: 2..=8` replaces
  `9 bits: 2..=8`, and a recording without metadata and sample rate now
  names the metadata path it looked for.

- `isdbt-mux`: configuration errors are typed. `MuxError::Config` is
  `Config { key, problem }`, `key` being the dotted TOML path of the value
  (`service[0].stream[1].bml.block_size`, indices from 0 in file order),
  and the rules with several producers have their own variants: `Toml`
  (parser message), `Hex`, `Format` (times, durations, offsets, versions),
  `Range`, `TimeRange` (outside the 16-bit MJD), `Reserved`, `PidRange`,
  `PidInUse`, `InactiveLayer`, `Missing`, `NotApplicable`, `Conflict`,
  `Duplicate`; `CallOrder` (playback control after the first frame) and
  `Internal` (an invariant of the multiplexer, reported instead of a
  panic). `MuxError::key()` returns the path. The helpers that build these
  errors take the key: `config::parse_hex`, `jst::parse` / `parse_hms` /
  `parse_hm` / `to_mjd_time` / `bcd_duration`, `si::channel_frequency_code`,
  `si::validate_targets`, `si::additional_bxml_info` / `bxml_info`,
  `hybridcast::split_url` / `check` / `ait_section`,
  `HybridcastCfg::profile_version`, `carousel::check_resource_name` /
  `multipart_entity` / `bml_versions` / `load` (and `CarouselSource::key`);
  `carousel::transcode_utf8_to_euc_jp` reports `MuxError::Media` with the
  file path. Messages now start with the key instead of
  `service 0x0400 stream 0x0111`. A `[[raw_table]]` with `sections = []`
  is refused at load (it reached an `unreachable!`); `analysis::analyze`
  asserts one arrival time per packet. The library's runtime `unwrap` /
  `expect` / `unreachable!` sites went from 13 / 17 / 1 to 10 / 3 / 0 (the
  10 unwraps are `const` PIDs, checked at compile time).

- `isdb-ts`, `isdb-si`, `isdb-tlv`, `isdb-mmt`, `isdb-text`,
  `isdb-analyze`: typed errors and panic triage (wave 2, group W3, of
  `docs/plans/errors-and-logging.md`). `isdb_ts::Error` gains `Range {
  what, value, min, max }`, `Reserved { what, value }`, `Mismatch { what,
  value, expected }` (a field the syntax fixes to one value) and
  `NotAccepted { what, value }` (the table_id of another table); 18 of the
  60 `Invalid` producers moved to them (section_length minima,
  adaptation_field and extension lengths, section_syntax_indicator,
  packet_start_code_prefix, the DSM-CC protocolDiscriminator / dsmccType
  / messageId, table_id 0xFF and table_ids a decoder does not take,
  blockSize 0, JST_time / duration / BCD fields), and `u8_to_bcd` reports
  `Range` instead of `Overflow`; `Invalid` stays for inconsistent lengths
  and contradicting fields. `isdb_analyze::edit` and `mutate` no longer
  use `anyhow`: `EditError` and `MutateError` with the facts as fields
  (`anyhow` is a dev-dependency only); `isdb ts edit` adds which
  `--remove` / `--replace` / `--set` / `--insert` argument failed. New
  `isdb_ts::packet::NULL_PACKET`. `isdb_tlv::IpError` and
  `isdb_mmt::table::SectionError` are `#[non_exhaustive]`; `SectionError`
  implements `Display` and `Error`. Library panic sites (census of
  `tools/panic-census.py`, non-test code) 12 unwrap / 12 expect / 4
  unreachable → 1 / 0 / 0 (the remaining `unwrap` and one `assert!` are
  const-evaluated); the 188-byte slice-to-array `expect`s of the readers
  are gone (`split_first_chunk`).
- Every library crate denies printing: `#![warn(clippy::print_stdout,
  clippy::print_stderr)]` in its `lib.rs` (CI runs clippy with `-D
  warnings`), with `allow-print-in-tests` in the new `clippy.toml` for
  unit tests. Libraries report through `tracing` events and return
  values; only `isdb-cli`, examples and tests print.
- `isdb-ts`: `Pid::of_packet` and the new `Header::of_packet` take the
  packet as `&[u8; 188]` and cannot fail (the length is the type);
  `Pid::of_bytes` is the fallible form for a slice. The seven
  `expect("188-byte packet")` calls in `isdb-analyze`, `isdbt-mux`,
  `isdb-ts` and `isdb-cli` are gone. Pilot of
  `docs/plans/errors-and-logging.md` item E4.
- `clippy::cast_possible_truncation` is `"warn"` in
  `[workspace.lints.clippy]`, so every crate, example and integration test
  is held to it by CI; the per-crate `#![warn]` attributes that turned it
  on crate by crate are removed. The last three crates are done here:
  `isdbs-core` and `isdbs3-core` read TMCC fields through width-asserted
  `take_u8` / `take_u16`, `isdbs3-core`'s interleaver builds its `u32`
  tables through one bounded `bit_index`, and `isdbt-fec`'s multiplex
  pattern does its slot arithmetic in `usize`. No behaviour change.
- `isdbs-phy`, `isdbs3-phy`: `clippy::cast_possible_truncation` is on at
  crate level and in every example and integration test
  (`docs/plans/type-safety.md` §2), with three `#[allow]`s per crate, each
  on a one-line private helper: the 12-bit input quantiser (rounded and
  clamped before the cast), the f64 → f32 soft metric handed to the
  Viterbi / LDPC decoder, and the f64 → usize sample count (no fallible
  conversion in std). Both `frontend` modules gain `samples_in(seconds,
  fs)`; `isdbs_phy::frontend::to_input` is now public, so the examples
  stop repeating the quantiser. Recording segment lengths in the examples
  and tests go through `usize::try_from`. Behaviour is unchanged.
- `isdb-analyze`, `isdb-iq`: `clippy::cast_possible_truncation` is on at
  crate level (`docs/plans/type-safety.md` §2, item H3), with 3 and 8
  `#[allow]`s. The ADTS header parse uses the width-typed `BitReader`
  readers; `edit` / `mutate` values given on the command line (table_id,
  table_id_extension, section_number, stream_type, program_number,
  descriptor tag, BCD duration / offset, `frequency_mhz`) are range
  checked and an out-of-range value is an error naming the field instead
  of being wrapped; the remaining allows are float-to-integer and
  f64-to-f32 conversions (PCR interpolation, timestamps, spectrum bins,
  window and full scale) with the invariant stated. Report output is
  unchanged.
- `isdb-cli`, `isdb-fec`: `clippy::cast_possible_truncation` is on at
  crate level (`docs/plans/type-safety.md` §2, item H3), with 23 and 8
  `#[allow]`s. The radio `--rate` / `--bw` (and the ppm-corrected
  tuning) now fail with the flag name when out of the `u32` range
  instead of saturating, as does an oversized `loopback
  --sample-offset`; the RX float-to-12-bit conversion is
  `isdb_frontend::input_sample`.
- `isdb-si`: `clippy::cast_possible_truncation` is on at crate level
  (`docs/plans/type-safety.md` §2, item H3), with no `#[allow]`: the 149
  narrowing casts are gone. Bit fields go through the width-typed
  `BitReader::read_u8` / `read_u16` / `read_u32` / `read_usize`; the
  descriptor and DSM-CC `descriptor_length`, the DDB block count and the
  Annex C MJD / time-of-day arithmetic narrow through `try_from`; byte
  extraction (`MjdTime::from_u40`, the BCD digit pairs, the DII / DDB
  header fields that carry the low bits of a wider value) uses
  `to_be_bytes`. `CodedValue` gains the provided constant `MASK`.
  Behaviour is unchanged.
- `clippy::cast_possible_truncation` is on (`#![warn]`) in `isdb-ts` and
  `isdb-mmt` (`docs/plans/type-safety.md` §2), with no new `#[allow]`:
  the codec field reads go through `BitReader::read_u8/u16/u32/usize`,
  the length fields of `Descriptor::encode`, `PesPacket::encode` and
  `packetize_sections` are `try_from` into the `Error::Overflow` they
  already returned, and `isdb_mmt::es::loas_frame` takes its 13-bit
  length from `u16::to_be_bytes`. No public signature changed.
- `clippy::cast_possible_truncation` is on (`#![warn]`) in `isdbt-mux`
  (`docs/plans/type-safety.md` §2), with 8 `#[allow]`s (float → integer
  tick and packet counts, two clamped values, one bounded PES length).
  Section builders report a value that does not fit its wire field
  instead of wrapping it: the BML resource list (`resourceCount`, a
  `resourceNameLength` over 127 bytes), `moduleSize` / `component_size`
  and the EIT schedule section numbers are errors; a `service:` / `event:`
  / `broadcaster:` loop selector whose number exceeds the identifier
  width selects nothing. `SilentFrame::stereo_48k` and `build` panic
  (documented) on a `len` past the 13-bit `aac_frame_length`.
- `clippy::cast_possible_truncation` is on (crate-level `#![warn]`) in
  `isdbt-ofdm` and `isdbt-phy` (`docs/plans/type-safety.md` §2). The
  TMCC, AC and IIP decoders read narrow fields through width-typed
  `get_u8` / `get_u16` readers, the IIP encoder's loop and length bytes
  are `try_from` into `Error::BroadcastTs`, and the remaining narrowings
  (DSP quantisation, `f64` sample positions, carrier-table indices) sit
  under 19 `#[allow]`s in library code, each with its invariant. No
  behaviour change; no public signature changed.
- `isdb-frontend`: `clippy::cast_possible_truncation` is on (crate-level
  `#![warn]` in `lib.rs`, every example and test; `docs/plans/type-safety.md`
  §3 H3). Of its 130 casts, 54 are restructured (the `Symbols` words
  through `to_le_bytes`, the filter phase as a masked `u8`, index
  windows through `usize::try_from`, the float-to-12-bit input through the
  new `isdb_frontend::input_sample`), 14 are `try_from` in examples and
  tests, and 62 are DSP quantisation under 31 `#[allow]`s, each with its
  invariant (the loop's clamps in `fixed`, shared with `fast` through
  `pub(crate)` helpers, the bounded `Params::design` quantities, the f64
  sample positions of the measurement tools). The FPGA vectors are
  bit-identical.
- The reserved payloads of the lossless coded enums are canonical by
  construction (`docs/plans/type-safety.md` §3 H1): `Reserved(u8)` /
  `UserDefined(u8)` become `Reserved(ReservedCode)` /
  `UserDefined(UserDefinedCode)`, a private-field newtype per crate with
  `get()` (`const`), `Display`, `Binary` and `LowerHex`, whose only
  constructor is `pub(crate)` and called by `from_code` after it has
  excluded every assigned and unused code. A hand-built `Reserved(0xF6)`
  that was `!=` `Reserved(0x06)` yet encoded the same, or a
  `Reserved(0b111)` spelled where `Unused` is canonical, can no longer be
  written: `==` is wire equality on every value, not only on decoded ones.
  Affects `isdb_ts::packet::TransportScramblingControl`,
  `isdbt_ofdm::tmcc::{ModulationCode, RateCode, InterleaveCode,
  SegmentsCode}`, `isdbs_core::CombinationWord`,
  `isdbs3_core::{ModulationCode, RateCode, StreamType}`,
  `isdb_tlv::mmtp::PayloadType` and `isdb_mmt::mpu::FragmentType`.
  `code()` no longer masks (the payload is canonical); `from_code` still
  does. A `compile_fail` doctest per crate proves the constructor is
  private, and an exhaustive test per enum that `from_code(c) ==
  from_code(c & mask)` and `from_code(c).code() == c & mask` over every
  `u8`. Callers that matched `Reserved(c)` read `c.get()`; tests and
  READMEs that spelled a reserved value use `from_code`.
- `isdb-ts`: the continuity_counter is a value type, `ContinuityCounter`,
  instead of a `u8`: 4 bits by construction (`ContinuityCounter::new(u8)
  -> Option<_>`, `get()`, `ZERO`, `MAX`), with the H.222.0 §2.4.3.3
  arithmetic on it (`next()` wraps 15 → 0, `is_next_of()` is "differs by
  a positive value of one" modulo 16) and `Display` as the decimal value.
  `Header.continuity_counter`, `Header::new`, `Packet::with_payload` /
  `with_adaptation` / `pcr_only`, `cc::Counter::new` / `next` / `peek`
  and the `expected` / `got` / `previous` payloads of `cc::Continuity`
  and `cc::CcEventKind` take it, so the `& 0x0F` masks and `+ 1` of the
  checker (`Tracker`), the transmit `Counter`, `packetize`, the muxer's
  relay (`last_cc`) and `isdbt-phy`'s IIP writer are gone; `Counter::new(0)`
  is `Counter::default()`. Decoders build it with
  `ContinuityCounter::from_wire` (a mask on the 4 bits already read, so
  the codec stays lossless; `prop_packet_roundtrip` covers 0..=15). The
  `isdbt_phy::broadcast_ts::Iip.continuity_counter` field follows, and
  `Iip::to_packet` / `from_packet` build and read the TS header through
  `isdb_ts::packet::Header`. In the same change the raw-byte readers of
  transport_scrambling_control and adaptation_field_control outside
  `isdb-ts` (`p[3] >> 6`, `(p[3] >> 4) & 3` in `isdb-analyze`'s scan,
  `isdb-cli`'s packet source, the muxer's relay, PCR clock and rate
  probe) go through `Header::decode` / `packet::View` and the Table 2-4 /
  2-5 enums (`is_scrambled()`, `has_payload()`); the relay policy's
  refusal of scrambled input and its messages are unchanged. Item H2 of
  `docs/plans/type-safety.md`; `tools/newtype-wrap.py` did the literal
  sites.
- `clippy::cast_possible_truncation` is on (`#![warn]`) in `isdbt-core`,
  `isdb-hackrf`, `isdb-iio`, `isdb-text`, `isdb-radio` and `isdb-tlv`
  (`docs/plans/type-safety.md` §2): the remaining narrowing casts there
  are restructured to produce the narrow type, made fallible, or carry an
  `#[allow]` with the invariant. `isdb_tlv::tlv::Packet::encode` returns
  `isdb_ts::Error::Overflow` for data longer than the 16-bit `data_length`
  field and replaces `Packet::bytes`, which wrapped the length silently
  (`isdb rx` for ISDB-S3 propagates the error).
- `isdbt-core`: `LayerConfig.segments` is `Segments`, a `1..=13` newtype
  of ARIB STD-B31 Table 3-31 ('0001'–'1101'; Table 3-2 Ns = 13), instead
  of `usize`. `Segments::new(n)` is the only constructor (`None` outside
  the range; `const`), with `get()` (the `u8` count / code), `count()`
  (`usize` for carrier and TSP arithmetic), `MAX` (13), `ALL` and
  `Display`. `Params::new` no longer checks for an empty layer and
  `ParamError::EmptyLayer` is gone (the value cannot be built); the
  sum-to-13 and partial-reception checks stay.
  `ofdm_params::bit_delay_adjustment_bits` takes `Segments`. In
  `isdbt-ofdm`, `tmcc::SegmentsCode::Segments` carries a `Segments`
  (not a `u8`), so `TmccLayer::from_config` has no cast and
  `from_code` goes through `Segments::new`; the CLI layer spec refuses
  a count outside 1..=13 at the parser. Item G (segments) of
  `docs/plans/type-safety.md`.
- `isdb-ts`: the PID is a value type, `Pid`, instead of a `u16`: 13 bits
  by construction (`Pid::new(u16) -> Option<Pid>`, `TryFrom<u16>`,
  `get()`), with H.222.0 Table 2-3 on it (`Pid::PAT`, `CAT`, `TSDT`,
  `IPMP`, `NULL`; `is_reserved()` for 0x0000–0x000F and 0x1FFF,
  `is_assignable()` for 0x0010–0x1FFE, `may_carry_pcr()` for the table's
  NOTE) and `Display` / `Debug` as `0x1fff`. Every `pid` field, parameter
  and map key of the crate takes it: `Header.pid`, `PatEntry.pid`,
  `PmtStream.elementary_pid`, `Pmt.pcr_pid`, `Packet::with_payload` /
  `with_adaptation` / `pcr_only`, `SectionAssembler::new`,
  `SectionDemux::add_pid` / `has_pid`, `PesAssembler::new`,
  `pes::packetize`, `packetize_sections`, `CcEvent.pid`, `Stats.pids`.
  The `packet::PID_*` constants are gone (use the `Pid` constants);
  `BitReader::pid()` reads the 13-bit field. Decoders build it with
  `Pid::from_wire` (a mask on the 13 bits already read, so the codec stays
  lossless; `prop_pid_round_trip` covers 0..=0x1FFF). Behind the new
  `serde` feature it serialises as the integer and deserialises with the
  range check, so a PID above 8191 in an `isdbt-mux` TOML now fails at
  load, naming the key, instead of at encode time; the TOML and the
  `isdb ts` JSON reports are byte-identical. `isdb-si` follows
  (`AnyTable::decode`, `conformance::check`, `table_allowed_on_pid`,
  `ConditionalAccess.ca_pid`, `AccessControl.pid`,
  `HierarchicalTransmission.reference_pid`), as do `isdb-analyze`,
  `isdbt-mux` (`Config` PID keys, `PidMap`, `PidList`, `si::pid`
  constants, `Probe`, `RelayedEs`), `isdbt-phy` (`Iip.pid`, `IIP_PID`;
  `BroadcastTsWriter::with_iip_pid` is infallible) and `isdb-mmt`
  (`Location::Mpeg2Ts`). Item G of `docs/plans/type-safety.md`;
  `tools/newtype-wrap.py` did the literal sites.

- Workspace: `clippy::cast_lossless` is enabled (`warn`, so an error in
  CI). The 632 `x as T` casts where `T: From<X>` (`u8`/`u16`/`u32` →
  wider integers, `bool` → integer, `f32`/`u32`/`i32` → `f64`) are written
  `T::from(x)`, so every `as` left is a lossy one. No behaviour change.

- `isdb-ts`: `Header.transport_scrambling_control` and
  `Header.adaptation_field_control` are lossless coded enums of H.222.0
  Tables 2-4 and 2-5 instead of raw `u8` codes:
  `TransportScramblingControl { NotScrambled, UserDefined(u8) }` (the
  user-defined codes '01'–'11' keep their value) and
  `AdaptationFieldControl { Reserved, PayloadOnly, AdaptationOnly,
  AdaptationAndPayload }`, each with a total `from_code()` (masked to the
  2-bit field) / `code()` pair and `Display` as two binary digits.
  `has_payload()` / `has_adaptation_field()` live on
  `AdaptationFieldControl` (the `Header` methods forward to them),
  `TransportScramblingControl::is_scrambled()` replaces `!= 0`,
  `Header::new` takes the enum and the `packet::afc` constants are gone.
  Decoding stays bit-exact (`prop_*` round trips unchanged). Item A of
  `docs/plans/type-safety.md`.
- `isdbs-core`: the TMCC combination word is a lossless coded enum of
  ITU-R BO.1408-1 Fig. 8 / Table 1, `CombinationWord { Assigned(Combination),
  Dummy, Reserved(u8) }` (`from_word()` total over the 4-bit code,
  `word()` its inverse), instead of `CombinationEntry { word: u8 }`.
  `TmccInfo::validate` and `slot_combinations` match on the variants
  (no table lookup, no `DUMMY_WORD` comparison); `TmccError::ReservedWord`
  is still what `validate` returns for `Reserved(_)`, and the codec stays
  bit-exact (`prop_tmcc_info_lossless` unchanged). `CombinationEntry::DUMMY`
  is the §6.1.2 unused entry (1111, zero slots); `CombinationEntry::combination`
  is `const`. `isdb iq tmcc --json` prints the same `word` number as
  before. Second step of `docs/plans/type-safety.md` (item C).
- `isdbt-ofdm`: the four fields of `tmcc::TmccLayer` are lossless coded
  enums of ARIB STD-B31 Tables 3-28..3-31 (`ModulationCode`, `RateCode`,
  `InterleaveCode`, `SegmentsCode`: `Assigned(..)` over the `isdbt-core`
  type, `Reserved(code)`, `Unused`; total `from_code()` / `code()`)
  instead of raw `u8` codes. A received TMCC word still re-encodes bit
  for bit (reserved codes keep their value), and `TmccLayer::config()`
  (was `to_config`) matches variants, not integers: the unused layer is
  `None`, a reserved code is `Error::Tmcc` as before. Table 3-30 '100'
  ("not used", terrestrial sound) is `InterleaveCode::Reserved(0b100)`.
  Second item of `docs/plans/type-safety.md`.
- `isdbs3-core`: the TMCC code fields of ARIB STD-B44 §3.11 are lossless
  coded enums instead of raw integers: `Mode.modulation_code:
  ModulationCode { Assigned(Modulation), Unused, Reserved(u8) }` (Table
  3.11-1), `Mode.rate_code: RateCode { Assigned(CodeRate), Unused,
  Reserved(u8) }` (Table 3.11-2; `1010` still reads as 105/120 per
  SPEC-NOTES "TMCC code-rate table") and `TmccInfo.stream_type:
  [StreamType; 16]` with `StreamType { Ts, Tlv, None, Reserved(u8) }`
  (Table 3.11-4), each with a total `from_code()` (masked to the field
  width) and its inverse `code()`, so the TMCC word still round-trips
  bit-exactly. `Mode::modulation()` / `rate()` / `is_used()` are
  `const` pattern matches; the `STREAM_TS` / `STREAM_TLV` constants are
  gone; `isdbs3_phy::rx::RxSlot.stream_type` is a `StreamType`. The
  `isdb iq s3tmcc` / `isdb rx` JSON keeps the numeric codes. Item D of
  `docs/plans/type-safety.md`.
- `isdbt-core`: `TimeInterleave` is a closed enum of the four TMCC codes
  of ARIB STD-B31 Table 3-12 (`K0`..`K3`, `code()` / `from_code()`,
  `ALL`) instead of `TimeInterleave(pub u8)`. `length(mode)` is
  infallible and `const`; `Params::new` no longer checks the index and
  `ParamError::TimeInterleaveIndex` is gone; the reserved codes
  '100'–'111' are refused in `from_code` alone (TMCC decoder, CLI layer
  spec). `Mode::number` / `Mode::scale` are `const fn`. First step of
  the type-safety workflow in `docs/plans/type-safety.md`;
  `tools/type-census.sh` counts what is left.
- `isdb-tlv`, `isdb-mmt`: the coded fields of the MMTP and MPU headers
  are lossless coded enums of their ARIB STD-B60 §6 tables instead of
  `u8`: `mmtp::Header { fec_type: FecType, payload_type: PayloadType }`
  (Tables 6-5, 6-6; the loose consts `mmtp::MPU` / `mmtp::MESSAGES` are
  gone, use `PayloadType::Mpu` / `PayloadType::Messages`) and
  `mpu::PayloadHeader { fragment_type: FragmentType, fragmentation:
  FragmentationIndicator }` (Tables 6-2, 6-3). Each has `from_code()`
  (total, masked to the field width) and `code()`, names every assigned
  code and keeps the reserved ones in `Reserved(u8)`, so headers still
  re-encode bit-exactly; the MFU reassembler matches on
  `FragmentationIndicator::{Complete, Head, Middle, Last}` instead of
  integers. `StreamStats::mmtp` keeps a `PayloadType`. Item E of the
  type-safety workflow.
- `isdb-fec`: the fixed-point LDPC iteration (every check node, then the
  syndrome) runs in a function of its own, 14 % more codewords per thread
  (4 615 → 5 380 on live ISDB-S3 codewords), same results. Runtime SIMD
  dispatch was measured and not adopted (needs `unsafe` or gains under
  6 %; VERIFICATION.md "Satellite receive speed").
- `isdb-frontend` format 2 (`Symbols` words keep their layout; the
  reader accepts versions 1 and 2): the front end's timing loop locks a
  fixed fraction of a symbol away from the best sampling instant on the
  lab's satellite signals (0.15–0.18 on ISDB-S3, 5–6 dB of SNR; 0.05–0.10
  on ISDB-S), so the output now comes from a separate filter bank shifted
  by a trim the host measures on known symbols. `isdb symbols` and `isdb
  iq symbols` measure and apply it (`--trim auto|T`, default auto) and
  record it in the sidecar; ISDB-S3 output goes from 5 to 6 bits; the
  ISDB-S3 report gives the sync-symbol Es/N0. With the trim: ISDB-S3
  sync-symbol Es/N0 8.8–9.5 → 14.6–14.7 dB on BS7/11/17; ISDB-S burst
  MER +1.3 dB (median) on 21 transponders, TS still RS-clean.
  `isdbs_phy::frontend::symbols` measures the trim too (`Trim::Auto`).
- `isdb-cli`: `isdb iq s3sync`, the ISDB-S3 slot sync and sync-symbol
  Es/N0 of a recording (untrimmed and trimmed) or of a `Symbols` words
  file.

- `isdb rx`: a SigMF recording given to `--iq` supplies its format and
  must be at 512/63 MHz (others are refused with a pointer to `isdb iq
  survey` / `isdb iq tmcc` instead of being misread); `--iq-format` is now
  optional (default cf32, or the recording's). When a file holds no
  ISDB-T frame, `rx` surveys it and names the single-carrier system found.
- `isdb-fec` (new): the RS(255,239) shortened codes (now any length, for
  RS(204,188) and the ISDB-S TMCC's RS(64,48)), the K = 7 171/133
  convolutional code and Viterbi decoder, and the X¹⁵+X¹⁴+1 PRBS moved
  out of `isdbt-fec`, joined by the ISDB-S burst PRBS X⁹+X⁴+1.
  `isdbt-fec::{rs, conv, energy}` keep their public items and delegate;
  every ISDB-T test is unchanged and Viterbi throughput is the same.
- `isdb-cli`: channels are written `T<n>` (terrestrial UHF, T13–T62),
  `BS<n>` (odd 1–23) and `ND<n>` (even 2–24), parsed case-insensitively.
  `--channel` of `tx` / `rx` takes these names, and a bare number still
  means a terrestrial channel (`--channel 44` = T44). Labels and messages
  print ` (T27)`, ` (BS1 IF)`, ` (ND2 IF)` (formerly ` (UHF ch27)`,
  ` (BS-1 IF)`); `isdb capture` writes `isdb:channel` as `BS1` / `ND2` /
  `T27`. The `chN` / `uhfN` / `bs-N` / `nd-N` spellings are no longer
  accepted. `tx` / `rx` refuse satellite channels (ISDB-T only).
- `isdb-cli`: the UHF channel table moved from `phyargs` to the new
  `bands` module (`channel_hz` → `bands::uhf_hz`), next to the BS and
  110° CS satellite IF tables; `tx` / `rx` behave as before.
- `isdb-radio`: the `pluto` device arguments are parsed per use (TX, RX,
  capture): `recorder` is a capture argument, `buffer` a streaming one.
- `.gitignore`: `*.cs16`, `*.sigmf-data` and `*.sigmf-meta` recordings.

- Crates and binary renamed: `arib-ts` → `isdb-ts`, `arib-text` →
  `isdb-text`, `arib-si` → `isdb-si`, `arib-analyze` → `isdb-analyze`,
  `arib-mux` → `isdbt-mux`, `hackrf` → `isdb-hackrf`, `arib-cli` →
  `isdb-cli`; the binary `arib` is now `isdb`. The `isdbt-*` PHY crates keep
  their names. The default oracle container name is now `isdb-oracle`
  (`ISDB_RS_ORACLE_CONTAINER` still overrides it).
- `ts-program` relay, conditional access: input must be unscrambled
  (transport_scrambling_control '00') and scrambled input is refused; the
  tool never descrambles. The source's CA signalling is now kept by default
  (`ca = "keep"`, CLI `--ca keep`): the PMT's conditional_access_descriptors
  (program and relayed ES loops) and the ECM PIDs, and the CAT's
  CA_descriptors with the EMM PIDs present in the source, following the PID
  mapping. `ca = "drop"` (`--ca drop`) removes them on explicit request.
- `ts-program` relay, copy control: the source value is detected from the
  PMT program loop, the relayed ES loops, the SDT and EIT p/f (0xC1 and
  0xDE; precedence PMT > ES > SDT > EIT, per-component ES-loop 0xC1 kept)
  and preserved by default. An explicit value is used as given, and an
  `info` line prints the source value next to what is relayed. Raw
  descriptors, byte patches or section replacements that can set a relayed
  service's copy control are reported with the value actually sent.
- `isdb tx`/`rx`: `--freq` and `--channel` are limited to 470–710 MHz (UHF
  channels 13–52) unless `--freq-unrestricted` is given.
- `isdb-cli`: HackRF support is the optional `hackrf` feature (default on),
  which forwards to `isdb-radio/hackrf`; `isdb-cli` no longer depends on
  `isdb-hackrf` directly and contains no device code.
  `cargo build -p isdb-cli --no-default-features` builds without libhackrf
  (all file-based commands work). CI has a `no-hackrf` job, which also
  checks `isdb-radio` without any backend.
- `isdb-hackrf`: minimum libhackrf 2022.09.1; older libraries are refused
  at open.
- Neutral default identifiers in examples, the relay template and tests:
  network_id / transport_stream_id 0x79EE (placeholder; choose an ID not
  used where your receiver is), area_code 0x5A5.
- **Breaking:**
  - `isdb tx` / `isdb rx` radio options, with no aliases for the old
    spellings: `--hackrf` is now `--radio SPEC` with SPEC
    `NAME[:key=value,...]` (`--radio hackrf`); `--serial S` is now the
    device argument `--radio hackrf:serial=S`; `--txvga N`, `--amp`,
    `--lna N` and `--vga N` are now the repeatable
    `--gain NAME=VALUE` (TX `txvga=0..47`, `amp=on|off`; RX `lna=0..40`
    in 8 dB steps, `vga=0..62` in 2 dB steps, `amp=on|off`), validated by
    the backend. `--bb-filter-hz` is an analog bandwidth request that the
    backend rounds to a supported value. The `--dry-run` text starts with
    `<backend> TX settings:` (` (UHF chN)` with `--channel`), and the
    tuning line no longer repeats the channel. `--backoff-db`, `--rf-enable`,
    `--dry-run`, `--prefill-frames`, `--channel`, `--freq`,
    `--freq-unrestricted` and `--freq-corr-ppm` are unchanged.
  - `isdb tx --dry-run` / `isdb rx` with `--radio` in a build without the
    backend (e.g. `--no-default-features`) fail at the backend lookup and
    no longer print the settings first.
  - The device's frequency range (HackRF 1 MHz–6 GHz) is checked by the
    backend on the programmed, `--freq-corr-ppm`-corrected frequency
    instead of the nominal one. `--bb-filter-hz` above 28 MHz is refused
    by the HackRF backend (libhackrf would program 0 Hz).
  - `isdb rx --json`: the `input` string of a radio reception is
    `radio SPEC[ (UHF chN)], centre … MHz, programmed … Hz; sample rate …`
    instead of `HackRF …`.
  - The header of a relay `--print-config` suggests
    `isdb tx --mux-config THIS.toml … --radio SPEC --channel N --rf-enable`
    (no `--hackrf`, no `--txvga 20` hint).
  - The radio TX status lines count `buffers` instead of `usb buffers`, and
    the `--dry-run` refusal reads `dry run, not transmitting: use
    --rf-enable instead of --dry-run …`.
  - `isdbt_core::Params` and `LayerConfig` fields are private, with
    accessors (construction is validated); `ParamError` is an enum.
  - `isdbt_phy::PipelinedReceiver::new` returns a `Result`.
  - Public error enums and growing configuration enums are
    `#[non_exhaustive]`.
  - `isdbt_mux::relay::Probe::source_copy_control` is a
    `SourceCopyControl` with findings (no `Option`);
    `relay::expanded_stream` takes `keep_copy_control`; stream
    configurations have a `ca` field; `[eit_schedule] reference` is a
    validated enum (`"today"` | `"month"`).
  - `isdbt_mux::relay::Probe` has a `rate` field (the whole-file
    `relay_rate::RateProfile`), and the `ts-program` rate check uses its
    `required_bps` instead of the probe's `mean_bps`: a layer accepted
    before may now be refused (it could not carry the recording).
- Repository harness: mise is the single task runner. The Makefile is
  replaced by mise tasks (`mise.toml`) with the same names and commands
  (`mise run check`, `mise run lint`, `mise run test`, …; `mise tasks`
  replaces `make help`), and the git hooks and CI call them. The `CARGO`
  override of the Makefile is gone.

### Fixed

- `isdbs3-phy`: a stream acquired far from the front end's NCO (a slice
  from the middle of a long recording whose carrier had drifted by
  kilohertz) never decoded a TMCC: the residual carrier from the SSync
  phase step is unambiguous only within ±908 Hz, and the sync took an
  alias 1815.6 Hz apart and tracked it. An acquisition now estimates the
  carrier coarsely from the squared π/2-BPSK TMCC symbols (±60 kHz) and
  takes the residual's alias nearest it. Opt-in test
  `isdbs3_mid_file_slice_acquires` (`ISDB_RS_ISDBS3_MIDFILE_WORDS`).

- `tools/ts-align-compare.py` aligned on the first recording packet whose
  bytes occur once in the reference, which with a looped placed TS could
  be a section packet of another pass (sections repeat unchanged, and a
  CC-rewritten PCR packet never matches): on the 2026-10-31 DT295
  recording it paired an SDT packet and reported 1600 packets displaced.
  It now aligns on PCR values (restamped every pass, so unique), with or
  without `--ignore-nulls`, and checks the stretch between every two
  PCRs. `tools/ts-pcr-residuals.py`: the dominant period came from a DFT
  over the PCR number (as if PCRs were evenly spaced in packets), from
  one cycle per span (a trend, not a period) at whole-cycle resolution,
  and the docstring said 2..m/2 cycles; now a periodogram over the actual
  packet indices from two cycles, refined to 0.01 cycle, with the
  fundamental's share of the residual power. A backward PCR step is
  unwrapped only when it is the 2^33 wrap, and the PCR PID is found by a
  header search (a multi-GB placed TS in seconds).
- `isdb rx --symbols-file` replayed a recording through the live-capture
  queue, which drops blocks when the decoder falls four chunks behind the
  parser: a decoder slower than the disk lost blocks silently (reported
  as losses, differently on every run). A file source now waits for the
  decoder (`symstream::Source::File`); a radio still never waits.
- `isdbs3-phy` / `isdb-cli`: streamed ISDB-S3 reception lost every frame
  ≈ 30 s after the FPGA front end was configured. The front end has no
  carrier loop and the carrier drifted (−97 → −830 Hz in 30 s); at
  ≈ 908 Hz the two SSync neighbours that tell the frame marker from
  !FSync cancel. `RxOptions::carrier_hz` / `Rx::residual_hz`: the
  streaming receiver tracks the residual and removes it; a loss now costs
  about one frame. The block-wise path is unchanged.
- `isdb-radio`: the FPGA symbol stream overflowed the device's DMA every
  10.2 s at 51 MB/s (iiod near a full Pluto core, four kernel buffers).
  New device argument `kbuffers` (default 16 × 1 Mi words); `buffer` also
  applies to the symbol stream. The cause is the board's TCP congestion
  control: Tezuka sets BBR, whose PROBE_RTT phase pauses the sender for
  ≥ 200 ms every 10 s (5 of 5 BBR runs lost blocks at the same stream
  position, 2 of 2 `cubic` runs lost none); 16 buffers (660 ms) absorb
  it.
- `isdb-cli`: the streaming receiver's queue is bounded (four chunks);
  beyond it whole blocks are dropped and reported (`blocks_dropped`)
  instead of memory growing while the decoder is behind.
- `isdbs3-phy` / `isdb-cli`: the ISDB-S3 carrier at 40 Msps. The survey's
  strongest line was an alias on two of seven live recordings (sync
  Es/N0 −3 / −8 dB against +9 dB for the carrier): `isdb symbols` took it,
  and `pick_carrier` tried only the first four candidates while the
  carrier was the 4th of 5. Every candidate is now tried, and `isdb
  symbols` uses the same choice; the FPGA path of `isdb rx` also refuses a
  lock below 4 dB and probes again.
- `isdb-frontend`: the `Symbols` reader could deliver a damaged block.
  A loss glued block 1's intact header to later payload, and a payload
  word that landed in the trailer position equalled the CRC-16 of that
  spliced payload (property-test seed `bits = 5, cut_at = 208, cut_len =
  282`). A block is now accepted only when it is also framed: the next
  block's header starts right after its trailer, or the stream ends
  there. Cost: the intact block just before a loss is counted as lost
  too, and delivery lags by at most one block. No format change; the
  FPGA vectors are unchanged.
- `isdbs3-phy`: the carrier choice took a candidate 48 kHz off on a live
  BS11 block (its sync Es/N0 0.6 dB higher, no TMCC decoded): candidates
  whose carrier phase turns more than 0.5 rad between SSync slots now
  rank below the others.
- `isdb-cli`: the live-feed stall test no longer fails when the rest of
  the workspace tests load every core.

- `isdbs3-phy`: the ISDB-S3 carrier was the survey's strongest line, and
  an unrelated line in one of the ±R/2 windows could be stronger than the
  π/2-BPSK line: a live block then decoded no TMCC at all. The carrier is
  now the candidate the front end locks on best.
- `isdb-analyze`: scrambled packets (transport_scrambling_control ≠ 00)
  were parsed as sections, PES and ADTS, giving hundreds of false CRC,
  pointer and PES-start errors on scrambled services; their payloads are
  now left alone (an all-scrambled elementary stream is an Info finding).
  Mandatory tables, CAT and PMTs missing from a capture shorter than
  their repetition interval are Info instead of Error.
- `ts-program` relay: the rate check and `--auto-layout` use the rate the
  whole recording needs (`relay_rate`: the smallest constant rate that
  delivers every relayed packet by its PES's decoding time, read once over
  the whole file) instead of the mean over the probed first 30 s. A
  recording that started quieter than it went on was put on a layer below
  its sustained rate; the relay then fell up to seconds behind its PCR
  schedule and receivers dropped its late audio after a few minutes while
  the video kept playing. The probe report adds a `whole file:` line, and
  the 1-s peak warning is gone (the need covers the peaks).
- `ts-program` relay: PES that leave after their decoding time raise a
  run-time warning (`Multiplexer::take_alerts`, printed by `isdb tx`,
  `loopback` and `mux`; at most once a minute per relay), not only a
  counter in the status line.
- `isdb-ts`: `StreamReader` no longer zero-fills a 64 KiB scratch buffer
  for every packet; reading is about ten times faster.
- `ts-program` relay: a due packet whose T-STD buffer would overflow now
  waits up to 2 ms (`relay::RETIME_HOLD`), so output slot jitter no longer
  overflows MBn for sources multiplexed without buffer headroom.
- `ts-program` relay: read errors on the recording are reported instead of
  being treated as end of file.
- `isdb-hackrf`: the callback context is kept alive when
  `hackrf_stop_tx`/`hackrf_stop_rx` fails, closing a possible
  use-after-free.
- `isdb ts validate --expect-services` parses its argument without
  panicking.

### Removed

- `isdb tx` / `isdb rx` flags `--serial`, `--txvga`, `--lna`, `--vga` and
  `--amp` (and `--hackrf`): use `--radio hackrf:serial=S` and
  `--gain NAME=VALUE` (see Changed).
- Insertion of AAC crc_check into unprotected ADTS input. Computing the
  CRC needs a raw_data_block parser written from ISO/IEC 13818-7, which
  the project does not have, so the default `adts_compliance = "conform"`
  refuses such input with the reason, and `"warn"` sends it unchanged.
  crc_check values of protected input are neither verified nor rewritten.

[Unreleased]: https://github.com/a1678991/isdb-rs/commits/main
