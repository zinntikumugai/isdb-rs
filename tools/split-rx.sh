#!/usr/bin/env bash
# Usage: tools/split-rx.sh FILE.sym PARTS JOBS [OUTDIR]
#
# Decode an ISDB-S3 Symbols words file in PARTS slices, JOBS of them at a
# time, and join the part TLVs in order into OUTDIR/<name>-<tlv_id>.tlv.
#
# - Slices start on 4096-byte boundaries (L = size / PARTS rounded down to
#   4096; the last part takes the remainder) and are fed through process
#   substitution, so there is no sidecar: --system isdbs3 is passed.
# - Each slice needs a receiver that acquires mid-recording (isdb-rs
#   fix/mid-file-start: the carrier alias is resolved from the TMCC).
# - Parts and logs go to OUTDIR (default: next to FILE). Nothing existing is
#   overwritten: the script refuses to start if a joined TLV or any part
#   file is already there, and checks again before each join.
# - Every part must end with "TMCC ok N" with N > 0, else the script fails
#   without joining.
# - Each seam loses the frame that straddles it (~1 frame, 33 ms).
#
# Env: ISDB (receiver binary, default target/release/isdb of this checkout),
#      ISDB_RX_ARGS (extra rx arguments).
set -euo pipefail

usage() { echo "usage: $0 FILE.sym PARTS JOBS [OUTDIR]" >&2; exit 2; }
[ $# -ge 3 ] && [ $# -le 4 ] || usage
F=$1 N=$2 J=$3
OUT=${4:-$(dirname -- "$F")}
ISDB=${ISDB:-$(cd -- "$(dirname -- "$0")/.." && pwd)/target/release/isdb}
[[ $N =~ ^[1-9][0-9]*$ && $J =~ ^[1-9][0-9]*$ ]] || usage
[ -r "$F" ] || { echo "error: cannot read $F" >&2; exit 1; }
[ -x "$ISDB" ] || { echo "error: receiver $ISDB not executable" >&2; exit 1; }
mkdir -p -- "$OUT"
B=$OUT/$(basename -- "$F" .sym)

shopt -s nullglob
existing=( "$B"-[0-9a-f][0-9a-f][0-9a-f][0-9a-f].tlv "$B"-part*-*.tlv "$B"-part*.rx.log "$B"-part*.status )
if [ ${#existing[@]} -gt 0 ]; then
    echo "error: refusing to overwrite existing output:" >&2
    printf '  %s\n' "${existing[@]}" >&2
    exit 1
fi

size=$(stat -c %s -- "$F")
L=$(( size / N / 4096 * 4096 ))
[ "$L" -gt 0 ] || { echo "error: $F too small for $N parts" >&2; exit 1; }
echo "[split] $F: $size bytes, $N parts of $L bytes (last: $(( size - (N - 1) * L ))), $J at a time -> $OUT"

part() {
    local i=$1 off=$(( $1 * L )) cnt=$L
    [ "$i" -eq $(( N - 1 )) ] && cnt=$(( size - off ))
    # shellcheck disable=SC2086
    "$ISDB" rx --system isdbs3 ${ISDB_RX_ARGS:-} \
        --symbols <(dd if="$F" bs=4M iflag=skip_bytes,count_bytes skip="$off" count="$cnt" status=none) \
        --out "$B-part$i-{tlv_id}.tlv" > "$B-part$i.rx.log" 2>&1
}

for (( i = 0; i < N; i++ )); do
    while [ "$(jobs -rp | wc -l)" -ge "$J" ]; do wait -n || true; done
    echo "[split] part $i starts (offset $(( i * L )))"
    ( if part "$i"; then echo 0; else echo $?; fi > "$B-part$i.status" ) &
done
wait
fail=0
for (( i = 0; i < N; i++ )); do
    rc=$(cat -- "$B-part$i.status" 2>/dev/null || echo missing)
    if [ "$rc" != 0 ]; then
        echo "[split] part $i: receiver exit status $rc (see $B-part$i.rx.log)" >&2
        fail=1
    fi
done

for (( i = 0; i < N; i++ )); do
    log=$B-part$i.rx.log
    done_line=$(grep -m1 '^\[rx\] done:' "$log" || true)
    echo "part $i: ${done_line:-NO [rx] done LINE}"
    ok=$(sed -n 's/.*TMCC ok \([0-9]*\).*/\1/p' <<<"$done_line")
    if [ -z "$ok" ] || [ "$ok" -eq 0 ]; then
        echo "[split] ERROR: part $i decoded no TMCC (TMCC ok ${ok:-?}); see $log" >&2
        fail=1
    fi
done
[ "$fail" -eq 0 ] || { echo "[split] FAILED: parts kept, nothing joined" >&2; exit 1; }

# the stream ids any part wrote
ids=$(for p in "$B"-part*-*.tlv; do p=${p##*-}; echo "${p%.tlv}"; done | sort -u)
[ -n "$ids" ] || { echo "[split] ERROR: no part TLVs" >&2; exit 1; }
for id in $ids; do
    target=$B-$id.tlv
    if [ -e "$target" ]; then
        echo "[split] ERROR: $target exists; not overwritten" >&2
        exit 1
    fi
    list=()
    for (( i = 0; i < N; i++ )); do
        p=$B-part$i-$id.tlv
        if [ -e "$p" ]; then list+=( "$p" ); else echo "[split] warning: part $i has no stream $id" >&2; fi
    done
    tmp=$(mktemp -- "$target.joining.XXXXXX")
    cat -- "${list[@]}" > "$tmp"
    chmod --reference="${list[0]}" -- "$tmp"
    # no-clobber rename: fails if the target appeared meanwhile
    if ! ln -- "$tmp" "$target" 2>/dev/null; then
        rm -f -- "$tmp"
        echo "[split] ERROR: $target appeared while joining; not overwritten" >&2
        exit 1
    fi
    rm -f -- "$tmp"
    echo "[split] joined ${#list[@]} parts -> $target ($(stat -c %s -- "$target") bytes)"
done
