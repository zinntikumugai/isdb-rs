#!/usr/bin/env bash
# Build the PlutoSky R2 bitstream (XSA) of Tezuka v0.3.24, stock or with the
# isdb-rs symbol front end, from the pinned upstream revisions.
#
# Usage: fpga/tezuka/build.sh WORKDIR [stock|isdb]
#   WORKDIR  a directory outside this repository with room for the sources
#            and the Vivado run (several GB; an NFS path is fine)
#   stock    the unmodified design, to check the toolchain (default)
#   isdb     with SymbolsRx (fpga/build/isdb_symbols_rx.v) on the RX path
#
# Needs: Vivado (VIVADO_SETTINGS: the path of its settings64.sh)
# with Zynq-7000 devices and a licence that covers xc7z020 (the free BASIC
# tier does), git, uv, make. Nothing from the upstream projects is copied
# into this repository: they are cloned into WORKDIR at the revisions below.
#
# Upstream revisions (the bitstream Tezuka v0.3.24 ships for plutoskyr2 was
# built on 2026-10-06 from F5OEO/maia-sdr "refactor" at f7092ba, the last
# plutoskyr2 change before it; the XSA records Vivado 2023.1):
#   tezuka_fw  https://github.com/F5OEO/tezuka_fw    v0.3.24 = a304a1f7ecba49dc4cd9d57a7935ec52504c7334
#   maia-sdr   https://github.com/F5OEO/maia-sdr     f7092ba (refactor branch)
#     adi-hdl              cf81ab15cdb1376982b53827a9aa3db01297eecc (submodule)
#     dvb_fpga             297a5b7e73d9aa3f07140e08dea7cf27a72432e8 (submodule, with its own
#                          third_party submodules, fetched recursively)
#     XilinxUnisimLibrary  1c8e05fd1e9a79ceb8b996a0996674122eed086f (submodule)
set -euo pipefail

WORK=${1:?usage: fpga/tezuka/build.sh WORKDIR [stock|isdb]}
MODE=${2:-stock}
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
VIVADO_SETTINGS=${VIVADO_SETTINGS:?"set VIVADO_SETTINGS to the path of Vivado's settings64.sh"}
TEZUKA_REV=a304a1f7ecba49dc4cd9d57a7935ec52504c7334
MAIA_REV=f7092ba

case "$MODE" in stock|isdb) ;; *) echo "mode: stock or isdb" >&2; exit 2 ;; esac
case "$WORK" in "$REPO"|"$REPO"/*) echo "WORKDIR must be outside the repository" >&2; exit 2 ;; esac
mkdir -p "$WORK"
cd "$WORK"

if [ ! -d tezuka_fw ]; then
    git clone -q https://github.com/F5OEO/tezuka_fw tezuka_fw
fi
git -C tezuka_fw checkout -q "$TEZUKA_REV"

if [ ! -d maia-sdr ]; then
    git clone -q https://github.com/F5OEO/maia-sdr maia-sdr
fi
git -C maia-sdr checkout -q "$MAIA_REV"
git -C maia-sdr submodule update -q --init --depth 1 \
    maia-hdl/adi-hdl maia-hdl/dvb_fpga maia-hdl/XilinxUnisimLibrary
# dvb_fpga has its own submodules (third_party/hdl_string_format, fpga_cores)
git -C maia-sdr/maia-hdl/dvb_fpga submodule update -q --init --recursive --depth 1

# maia-hdl generates its IP cores with Amaranth 0.5 (MIT), in its own venv;
# Yosys comes from the amaranth-yosys wheel (upstream uses oss-cad-suite).
if [ ! -x venv/bin/python ]; then
    UV_DEFAULT_INDEX=${UV_DEFAULT_INDEX:-https://pypi.org/simple} uv venv -q --python 3.12 venv
    UV_DEFAULT_INDEX=${UV_DEFAULT_INDEX:-https://pypi.org/simple} VIRTUAL_ENV="$WORK/venv" \
        uv pip install -q 'amaranth>=0.5,<0.6' amaranth-yosys numpy scipy
fi

PROJ=maia-sdr/maia-hdl/projects
# Start from the pinned files every time, so stock and isdb builds don't mix.
git -C maia-sdr checkout -q -- maia-hdl/projects
# -x: the Vivado project of a previous build (plutoskyr2.xpr, .runs, .sdk) is
# gitignored upstream and must not be reused by the other mode. The IP
# libraries (adi-hdl/library, maia-hdl/ip) are the same for both and kept.
git -C maia-sdr clean -q -fdx -- maia-hdl/projects/tezuka maia-hdl/projects/common maia-hdl/projects/boards
if [ "$MODE" = isdb ]; then
    "$HERE/apply.sh" "$WORK/maia-sdr" "$REPO"
fi

# The install target copies to a hardcoded path of the upstream author's
# machine; build and keep the XSA here instead.
set +u
# shellcheck disable=SC1090
source "$VIVADO_SETTINGS"
set -u
export ADI_IGNORE_VERSION_CHECK=1
export PATH="$WORK/venv/bin:$PATH"
cd "$PROJ/tezuka"
make clean >/dev/null 2>&1 || true
# The Makefile's build_project target names a rule that ADI's
# project-xilinx.mk does not define, and `make all` ends in an install to
# the upstream author's path: build the XSA target directly. PROJECT_NAME
# goes in the environment, not on the command line, where ADI's makefile
# would turn it into a project subdirectory.
export PROJECT_NAME=plutoskyr2
make ip_cores
make plutoskyr2.sdk/system_top.xsa
OUT="$WORK/out/$MODE"
mkdir -p "$OUT"
cp -f plutoskyr2.sdk/system_top.xsa "$OUT/plutoskyr2.xsa"
cp -f plutoskyr2.runs/impl_1/*_timing_summary_routed.rpt "$OUT/" 2>/dev/null || true
cp -f plutoskyr2.runs/impl_1/*_utilization_placed.rpt "$OUT/" 2>/dev/null || true
echo "XSA: $OUT/plutoskyr2.xsa"
