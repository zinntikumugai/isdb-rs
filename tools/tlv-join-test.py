#!/usr/bin/env python3
"""Self-test of tools/tlv-join.py on synthetic TLV streams (no receiver).

Checks: overlapping slices of one stream join back to it exactly (2 and 4
parts, a part ending mid-packet, a head that starts with a run of identical
null packets, the copy fallback without copy_file_range), a later anchor is
used when the decodes differ at part k+1's head, a seam without a match
exits 1 and writes nothing, --allow-lossy then concatenates, --plan prints
the ranges and writes nothing.

Run: python3 tools/tlv-join-test.py
"""

import os
import random
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
JOIN = os.path.join(HERE, "tlv-join.py")


def packet(ptype, payload):
    return bytes([0x7F, ptype]) + len(payload).to_bytes(2, "big") + payload


def stream(rng, n):
    """n packets with random payloads and some runs of null packets;
    returns (bytes, packet start offsets)."""
    out, starts, pos = [], [], 0
    while len(starts) < n:
        if rng.random() < 0.05:
            run = [packet(0xFF, b"\xff" * 4093)] * rng.randint(2, 8)
        else:
            run = [packet(0x01, rng.randbytes(rng.randint(20, 1500)))]
        for p in run:
            starts.append(pos)
            out.append(p)
            pos += len(p)
    return b"".join(out), starts


def read(path):
    with open(path, "rb") as f:
        return f.read()


def nulls_then(data):
    return b"".join([packet(0xFF, b"\xff" * 4093)] * 6) + data


class TlvJoin(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = self.tmp.name
        self.rng = random.Random(1)

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, name, data):
        p = os.path.join(self.d, name)
        with open(p, "wb") as f:
            f.write(data)
        return p

    def run_join(self, parts, *args):
        out = os.path.join(self.d, "joined.tlv")
        if os.path.exists(out):
            os.unlink(out)
        r = subprocess.run([sys.executable, JOIN, "--tail", str(1 << 20),
                            "--window", "4096", *args, "--out", out, *parts],
                           capture_output=True)
        data = read(out) if os.path.exists(out) else None
        return r, data

    def slices(self, data, starts, cuts, overlap):
        """Parts that start on packet boundaries near each cut and end
        `overlap` bytes past the next cut (possibly mid-packet)."""
        bounds = [0] + [next(s for s in starts if s >= c) for c in cuts]
        parts = []
        for k, b in enumerate(bounds):
            end = bounds[k + 1] + overlap if k + 1 < len(bounds) else len(data)
            parts.append(self.write(f"p{k}.tlv", data[b:end]))
        return parts

    def test_two_parts_exact(self):
        data, starts = stream(self.rng, 3000)
        parts = self.slices(data, starts, [len(data) // 2], 50_001)
        r, out = self.run_join(parts)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(out, data)
        self.assertIn(b"from 0", r.stderr)

    def test_four_parts_exact(self):
        data, starts = stream(self.rng, 6000)
        n = len(data)
        parts = self.slices(data, starts, [n // 4, n // 2, 3 * n // 4], 30_000)
        r, out = self.run_join(parts)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(out, data)

    def test_head_of_null_packets(self):
        # part 1 starts with a run of null packets, found many times in
        # part 0's tail: the first anchor is the packet after them
        data, starts = stream(self.rng, 2000)
        mid = next(s for s in starts if s >= len(data) // 2)
        data = data[:mid] + nulls_then(data[mid:])
        p0 = self.write("p0.tlv", data[:mid + 80_000])
        p1 = self.write("p1.tlv", data[mid:])
        r, out = self.run_join([p0, p1])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(out, data)
        self.assertIn(b"from 24582", r.stderr)  # 6 null packets of 4097

    def test_repeated_content_grows_window(self):
        # the anchor's 4 KiB occur twice in part 0's tail: the window grows
        data, starts = stream(self.rng, 3000)
        b = next(s for s in starts if s >= len(data) // 2)
        rep = data[b:b + 8192]
        e = next(s for s in starts if s >= b + 8192)
        data = data[:b] + rep + data[b:e] + data[e:]  # rep repeated
        b2 = b + len(rep)  # part 1 starts at the second copy
        p0 = self.write("p0.tlv", data[:b2 + 100_000])
        p1 = self.write("p1.tlv", data[b2:])
        r, out = self.run_join([p0, p1])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(out, data)

    def test_copy_fallback(self):
        data, starts = stream(self.rng, 3000)
        parts = self.slices(data, starts, [len(data) // 3], 40_000)
        # run the joiner in-process with copy_file_range removed
        code = ("import os,runpy,sys; del os.copy_file_range; "
                f"sys.argv={[JOIN, '--tail', str(1 << 20), '--out', os.path.join(self.d, 'fb.tlv'), *parts]!r}; "
                f"runpy.run_path({JOIN!r}, run_name='__main__')")
        r = subprocess.run([sys.executable, "-c", code], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(read(os.path.join(self.d, "fb.tlv")), data)

    def test_decodes_differ_at_head(self):
        # part 1's first packets differ from part 0's copy of them (as after
        # an LDPC failure in one decode): a later anchor joins the seam and
        # part 0's version of the differing bytes is kept
        data, starts = stream(self.rng, 3000)
        b = next(s for s in starts if s >= len(data) // 2)
        p0 = self.write("p0.tlv", data[:b + 200_000])
        bad = bytearray(data[b:])
        bad[100] ^= 0x55  # inside the first packet's payload
        p1 = self.write("p1.tlv", bytes(bad))
        r, out = self.run_join([p0, p1])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(out, data)
        self.assertIn(b"decodes differ", r.stderr)

    def test_no_match_fails(self):
        data, starts = stream(self.rng, 3000)
        b = next(s for s in starts if s >= len(data) // 2)
        p0 = self.write("p0.tlv", data[:b])  # no overlap at all
        # part 1 unrelated to part 0's tail
        other, _ = stream(random.Random(2), 1500)
        p1 = self.write("p1.tlv", other)
        r, out = self.run_join([p0, p1])
        self.assertEqual(r.returncode, 1)
        self.assertIn(b"ERROR", r.stderr)
        self.assertIsNone(out)  # nothing written
        r, out = self.run_join([p0, p1], "--allow-lossy")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(b"WARNING", r.stderr)
        self.assertEqual(out, data[:b] + other)

    def test_plan(self):
        data, starts = stream(self.rng, 3000)
        parts = self.slices(data, starts, [len(data) // 2], 50_000)
        r = subprocess.run([sys.executable, JOIN, "--plan", *parts],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = [line.split() for line in r.stdout.splitlines()]
        self.assertEqual([row[0] for row in rows], parts)
        self.assertEqual(int(rows[1][1]), 0)
        self.assertEqual(int(rows[0][2]) + len(read(parts[1])),
                         len(data))
        self.assertFalse(os.path.exists(os.path.join(self.d, "joined.tlv")))


if __name__ == "__main__":
    unittest.main()
