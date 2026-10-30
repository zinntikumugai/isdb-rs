#!/usr/bin/env bash
# Turn a Tezuka XSA into the bitstream file U-Boot loads from the SD card's
# FAT partition, the way Tezuka's board/tezuka/common/postimage-sd.sh does
# for every bitstream/*.xsa (unzip system_top.bit, then bootgen
# -process_bitstream bin), and print its size and SHA-256.
#
# Usage: fpga/tezuka/sdbin.sh XSA OUT.bin
#   then copy OUT.bin next to system_top.bin on the card and select it with
#   bitstream_image=<file> in uEnv.txt (fpga/README.md, "Deploying").
set -euo pipefail
XSA=${1:?usage: sdbin.sh XSA OUT.bin}
OUT=${2:?usage: sdbin.sh XSA OUT.bin}
VIVADO_SETTINGS=${VIVADO_SETTINGS:?"set VIVADO_SETTINGS to the path of Vivado's settings64.sh"}
set +u
# shellcheck disable=SC1090
source "$VIVADO_SETTINGS"
set -u
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
unzip -p "$XSA" system_top.bit > "$T/system_top.bit"
echo "img : {$T/system_top.bit }" > "$T/b.bif"
bootgen -image "$T/b.bif" -process_bitstream bin -arch zynq -w -o i "$T/system_top.bit.bin" >/dev/null
cp "$T/system_top.bit.bin" "$OUT"
echo "$OUT: $(stat -c %s "$OUT") bytes, sha256 $(sha256sum "$OUT" | cut -d' ' -f1)"
