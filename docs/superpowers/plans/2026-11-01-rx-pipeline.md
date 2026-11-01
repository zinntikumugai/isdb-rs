# isdb rx pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Overlap the one-thread front of each 24-frame chunk with the previous chunk's LDPC so a single `isdb rx --symbols` process keeps more cores busy, with byte-identical TLV output.

**Architecture:** `rx::receive` splits into `receive_front` (sync … prepare, owns its results) and `receive_back` (noise, LDPC, `Rx`). `rxs3::run_stream` runs the front on the `symstream::run` thread and the back + TLV emission on a second thread fed through a depth-1 channel that also carries end-of-stretch markers.

**Tech Stack:** Rust 1.99, std threads/mpsc only (no new deps).

**Spec:** `docs/superpowers/specs/2026-11-01-rx-pipeline-design.md`

## Global Constraints

- TLV output byte-identical to the pre-change binary on the golden files; summary lines equal except timing.
- Live (`--radio`) path semantics unchanged: chunk order, loss handling (`ended` closes assemblers in order), Ctrl-C.
- Memory growth ≤ 1 GB (one extra chunk in flight). No f32.
- Build: `PATH=$HOME/.cargo/bin:$PATH RUSTFLAGS="-L /home/user/.local/lib/iio-link" cargo build --release -p isdb-cli --no-default-features --features pluto`. Tests the same flags with `cargo test -p isdbs3-phy` / `-p isdb-cli`.
- Commit messages: conventional (`perf(...)`, `feat(...)`), English, `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, **no Claude-Session line**.
- Golden: `/mnt/ssd/tv-rec-test/golden/` (1m from the merged pre-change binary; 10m likewise).

## Review Focus

- A loss mid-stream: the `End` marker must reach the back after the chunk before it and before the next chunk, or packets cross a discontinuity (golden files have 0 losses — pin with the existing symstream loss tests + a run_stream ordering test).
- Back-stage write error: must surface as the run's `Err`, not hang the front on a full channel.
- Ctrl-C while the back is busy: the front stops, the back drains what was sent and the run returns.
- `keep_last` with no whole frame (`upto == None`) returns 0 without sending a chunk to the back.
- Status lines read counters the back updates: no deadlock holding the stats lock across a send.

---

### Task 1: Stage timing

**Files:** Modify `crates/isdbs3-phy/src/rx.rs` (`Rx`, `receive`), `crates/isdb-cli/src/rxs3.rs` (`Stats`, `decode`, `run_stream`, `report`).

**Interfaces:** Produces `Rx.front_s: f64`, `Rx.back_s: f64`; `Stats.front_s`, `Stats.ldpc_s`, `Stats.emit_s` (f64 seconds, summed).

- [ ] Add the fields; time the front part (sync → preps loop) and the back part (pts → end) inside `receive`; `decode` adds them and times its emission loop as `emit_s`.
- [ ] `report`: after the `time:` line, `info!` one line `stages: front {:.2} s, LDPC {:.2} s, TLV {:.2} s` when `stats.words` is set.
- [ ] Build, run the 1m golden: TLV `cmp` identical, new line present. Commit `perf(isdb-cli): rx reports the time of the receiver's front, LDPC and TLV stages`.

### Task 2: receive_front / receive_back

**Files:** Modify `crates/isdbs3-phy/src/rx.rs`; Test: `crates/isdbs3-phy/tests/captures.rs` (opt-in) and a unit test in `rx.rs`.

**Interfaces:** Produces `pub struct Front` (opaque fields: `rx: Rx` partial, `order`, `ready`, `opt`-independent data); `pub fn receive_front(z: &[Cplx], opt: &RxOptions) -> Front`; `pub fn receive_back(f: Front, opt: &RxOptions) -> Rx`; `impl Front { pub fn frame_starts(&self) -> &[usize]; pub fn residual_hz(&self) -> Option<f64>; pub fn sync_snr_db(&self) -> f64; pub fn sync_snr_eq_db(&self) -> Option<f64> }`. `receive` = `receive_back(receive_front(z, opt), opt)`. Early returns (no sync, re-sync failed) yield a `Front` whose back returns that `Rx` unchanged.

- [ ] Test (unit, no data): `receive_back(receive_front(noise))` equals `receive(noise)` on 3 frames of seeded noise (no sync → default Rx) — compare via `format!("{:?}")` of both `Rx`.
- [ ] Test (opt-in, `ISDB_RS_ISDBS3_WORDS`=words file): first 25 frames; `Debug` strings of `receive` and front→back equal; `frames.len() >= 24`.
- [ ] Implement by moving code, not rewriting it; move `front_s`/`back_s` timing to the two functions.
- [ ] `cargo test -p isdbs3-phy` passes; opt-in test passes with the 1m words file. Commit `refactor(isdbs3-phy): rx::receive is receive_front then receive_back`.

### Task 3: Pipelined run_stream

**Files:** Modify `crates/isdb-cli/src/rxs3.rs` (`decode` split into `cut_front` and `emit`, `run_stream`).

**Interfaces:** Consumes Task 2. Produces `fn emit(r: &Rx, upto: Option<usize>, asm, out, stats)` (current `decode` second half) and a back thread inside `run_stream`; message `enum Back { Chunk { front: Front, upto: Option<usize> }, End }`, channel `sync_channel(1)`.

- [ ] Front closure: `receive_front`, set `residual_hz`/`sync_snr*`/`tracked`/`aligned`, compute `upto` exactly as `decode` does (return 0 without sending when `keep_last && upto.is_none()`), send `Chunk`, return `upto.unwrap_or(z.len())`. `ended` sends `End`. A send error → `bail!` (the back failed).
- [ ] Back thread: owns `asm` and `out`; on `Chunk` runs `receive_back` then `emit`; on `End` runs `close`. Stats shared as `Mutex<&mut Stats>`-equivalent (scoped thread so borrows work: `std::thread::scope`); never hold the lock while sending/receiving.
- [ ] After `symstream::run` returns, drop the sender, join the back; the back's error wins over a front `bail!`; re-raise a back panic.
- [ ] Tests: `cargo test -p isdb-cli` (existing symstream/live_feed tests). Golden 1m and 10m TLV `cmp` identical; summary lines equal except time. Measure wall/CPU% on 4 cores vs golden. Commit `perf(isdb-cli): rx overlaps a chunk's sync and preparation with the previous chunk's LDPC`.
