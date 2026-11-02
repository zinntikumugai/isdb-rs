#!/usr/bin/env python3
"""Join TLV files decoded from overlapping slices of one recording
(tools/split-rx.sh) without losing or repeating bytes at the seams.

Usage: tools/tlv-join.py [--tail BYTES] [--window BYTES] [--allow-lossy]
                         (--out OUT | --plan) PART0.tlv PART1.tlv ...

Every slice but the last was decoded past its end, so the tail of part k
holds the same packets as the head of part k+1. For each seam the joiner
takes anchors at TLV packet boundaries of part k+1's head (the first
non-null packet, then later ones in case the two decodes differ there, e.g. an LDPC
failure in one of them) and looks for each anchor's --window bytes in the
last --tail bytes of part k. An anchor counts only if its window is found
exactly once there (if it is found more often, e.g. in a run of null
packets, the window is doubled) and parses as whole TLV packets (0x7F,
type, 16-bit length, payload). The first anchor that counts joins the seam: part k up to
the match, then part k+1 from the anchor on.

Only the tail of part k and the head of part k+1 are read for matching; the
output is written by copying byte ranges (copy_file_range where the file
system can), never by modifying a part. With --plan nothing is written and
the ranges are printed.

If a seam has no match the joiner exits 1 without writing OUT, unless
--allow-lossy: then that seam keeps both parts whole (the overlap is
repeated) and a warning is printed.
"""

import argparse
import os
import sys

ANCHOR_STEP = 64 * 1024  # spacing of anchors in part k+1's head
MAX_ANCHORS = 64
NULL_TYPE = 0xFF


def packet_boundaries(buf, limit):
    """Offsets of the TLV packets that start in buf[:limit], walking from 0."""
    out, i = [], 0
    while i < limit and i + 4 <= len(buf) and buf[i] == 0x7F:
        out.append(i)
        i += 4 + int.from_bytes(buf[i + 2:i + 4], "big")
    return out


def parses_as_packets(buf):
    """buf starts with a TLV packet and its packet chain is consistent up to
    its end (the last packet may run past it)."""
    i = 0
    while i + 4 <= len(buf):
        if buf[i] != 0x7F:
            return False
        i += 4 + int.from_bytes(buf[i + 2:i + 4], "big")
    return len(buf) > 0


def find_seam(tail, head, window):
    """(pos in tail, offset in head, anchors that did not match) where the
    seam joins, or None.

    tail: last bytes of part k; head: first bytes of part k+1. Anchors are
    boundaries of packets other than null packets (type 0xFF, identical
    to one another, so found anywhere)."""
    bounds = packet_boundaries(head, max(len(head) - window, 0) + 1)
    anchors, next_at = [], 0
    for b in bounds:
        if b >= next_at and head[b + 1] != NULL_TYPE:
            anchors.append(b)
            next_at = b + ANCHOR_STEP
            if len(anchors) >= MAX_ANCHORS:
                break
    missed = 0
    for a in anchors:
        w = window
        while True:
            probe = head[a:a + w]
            if len(probe) < w or not parses_as_packets(probe):
                break
            pos = tail.find(probe)
            if pos < 0:
                missed += 1  # the decodes differ here: try a later anchor
                break
            if tail.find(probe, pos + 1) < 0:
                return pos, a, missed
            w *= 2  # ambiguous (repeated content): a longer window
    return None


def read_range(path, off, n):
    with open(path, "rb") as f:
        f.seek(off)
        return f.read(n)


def plan(parts, tail_len, window, allow_lossy, log):
    """[(path, start, end)] byte ranges to copy, in order."""
    sizes = [os.path.getsize(p) for p in parts]
    starts = [0] * len(parts)
    ends = list(sizes)
    for k in range(len(parts) - 1):
        t0 = max(sizes[k] - tail_len, starts[k])
        tail = read_range(parts[k], t0, sizes[k] - t0)
        head = read_range(parts[k + 1], 0, min(sizes[k + 1], tail_len))
        seam = find_seam(tail, head, window) if head else None
        if seam is None:
            msg = (f"seam {k}|{k + 1}: no match of {parts[k + 1]}'s head "
                   f"in the last {len(tail)} bytes of {parts[k]}")
            if not allow_lossy:
                raise SystemExit(f"tlv-join: ERROR: {msg}")
            log(f"tlv-join: WARNING: {msg}; both parts kept whole (lossy)")
            continue
        pos, a, missed = seam
        ends[k] = t0 + pos
        starts[k + 1] = a
        log(f"tlv-join: seam {k}|{k + 1}: {parts[k]} cut at {ends[k]} "
            f"(drops its last {sizes[k] - ends[k]} bytes), "
            f"{parts[k + 1]} from {a}")
        if missed:
            log(f"tlv-join: note: seam {k}|{k + 1}: {missed} earlier "
                f"anchor(s) of part {k + 1} not found in part {k}: the "
                f"decodes differ before byte {a} of part {k + 1}; part {k}'s "
                "bytes are kept there")
    for k in range(len(parts)):
        if ends[k] < starts[k]:
            raise SystemExit(f"tlv-join: ERROR: {parts[k]} is used up by "
                             "its neighbours' overlaps; overlap too large?")
    return [(p, s, e) for p, s, e in zip(parts, starts, ends)]


def copy_range(src, dst, off, n):
    with open(src, "rb") as f:
        fi, fo = f.fileno(), dst.fileno()
        try:
            while n > 0:
                done = os.copy_file_range(fi, fo, n, off)
                if done == 0:
                    raise OSError("short copy")
                off += done
                n -= done
            return
        except (AttributeError, OSError):
            pass  # no copy_file_range here: plain copy of what is left
        f.seek(off)
        while n > 0:
            b = f.read(min(n, 8 << 20))
            if not b:
                raise SystemExit(f"tlv-join: ERROR: {src} ended early")
            mv = memoryview(b)
            while mv:
                mv = mv[dst.write(mv):]
            n -= len(b)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--tail", type=int, default=64 << 20,
                    help="bytes at the end of each part searched (default 64 MiB)")
    ap.add_argument("--window", type=int, default=16 << 10,
                    help="anchor length in bytes (default 16 KiB)")
    ap.add_argument("--allow-lossy", action="store_true")
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--out")
    g.add_argument("--plan", action="store_true")
    ap.add_argument("parts", nargs="+")
    o = ap.parse_args()
    if o.window < 4 or o.tail < o.window:
        ap.error("need 4 <= --window <= --tail")
    log = lambda m: print(m, file=sys.stderr)
    ranges = plan(o.parts, o.tail, o.window, o.allow_lossy, log)
    if o.plan:
        for p, s, e in ranges:
            print(f"{p} {s} {e}")
        return
    total = 0
    with open(o.out, "wb", buffering=0) as out:
        for p, s, e in ranges:
            copy_range(p, out, s, e - s)
            total += e - s
        os.fsync(out.fileno())
    if os.path.getsize(o.out) != total:
        raise SystemExit(f"tlv-join: ERROR: {o.out} is not {total} bytes")
    log(f"tlv-join: {len(ranges)} parts -> {o.out} ({total} bytes)")


if __name__ == "__main__":
    main()
