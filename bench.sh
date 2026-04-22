#!/usr/bin/env bash
# ============================================================
# bench.sh — VexFS vs ext4/tmpfs benchmark runner
# Usage: ./bench.sh [vexfs_mountpoint] [image_file]
# ============================================================

set -euo pipefail

VEXFS_MNT="${1:-$HOME/mnt/vexfs}"
IMAGE="${2:-$HOME/vexfs.img}"
TMP_BASELINE="/tmp/vexfs_bench_baseline"
BINARY="./target/release/vexfs_bench"

RED='\033[0;31m'
GRN='\033[0;32m'
YLW='\033[1;33m'
BLU='\033[0;34m'
CYN='\033[0;36m'
BOLD='\033[1m'
RST='\033[0m'

banner() {
    echo ""
    echo -e "${BLU}╔══════════════════════════════════════════════════════════════╗${RST}"
    echo -e "${BLU}║${BOLD}          VexFS Benchmark Suite — Phase 3                     ${BLU}║${RST}"
    echo -e "${BLU}╚══════════════════════════════════════════════════════════════╝${RST}"
    echo ""
}

section() {
    echo ""
    echo -e "${CYN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RST}"
    echo -e "${BOLD}  $1${RST}"
    echo -e "${CYN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RST}"
}

check_deps() {
    local missing=0
    for cmd in cargo fusermount; do
        if ! command -v "$cmd" &>/dev/null; then
            echo -e "${RED}✗ Missing: $cmd${RST}"
            missing=1
        fi
    done
    if [ $missing -ne 0 ]; then
        echo -e "${YLW}Install missing tools and retry.${RST}"
        exit 1
    fi
}

build_release() {
    section "Building VexFS (release)"
    if [ ! -f "$BINARY" ]; then
        cargo build --release --bin vexfs_bench 2>&1 | tail -5
    else
        echo -e "  ${GRN}✓ Binary already built${RST}"
    fi
}

ensure_image() {
    if [ ! -f "$IMAGE" ]; then
        section "Creating VexFS image (128 MB)"
        cargo build --release --bin mkfs_vexfs 2>&1 | tail -3
        ./target/release/mkfs_vexfs "$IMAGE" 128
        echo -e "  ${GRN}✓ Image created: $IMAGE${RST}"
    else
        echo -e "  ${GRN}✓ Image exists: $IMAGE${RST}"
    fi
}

ensure_mount() {
    mkdir -p "$VEXFS_MNT"
    if ! mountpoint -q "$VEXFS_MNT" 2>/dev/null; then
        section "Mounting VexFS at $VEXFS_MNT"
        cargo build --release --bin vexfs 2>&1 | tail -3
        ./target/release/vexfs "$IMAGE" "$VEXFS_MNT" &
        VEXFS_PID=$!
        sleep 1
        if mountpoint -q "$VEXFS_MNT"; then
            echo -e "  ${GRN}✓ Mounted (pid=$VEXFS_PID)${RST}"
            MOUNTED_HERE=1
        else
            echo -e "  ${RED}✗ Mount failed${RST}"
            exit 1
        fi
    else
        echo -e "  ${GRN}✓ Already mounted${RST}"
        VEXFS_PID=0
        MOUNTED_HERE=0
    fi
}

cleanup() {
    if [ "${MOUNTED_HERE:-0}" -eq 1 ] && [ "${VEXFS_PID:-0}" -ne 0 ]; then
        echo ""
        echo -e "  ${YLW}Unmounting VexFS...${RST}"
        fusermount -u "$VEXFS_MNT" 2>/dev/null || true
        wait "$VEXFS_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_BASELINE"
}
trap cleanup EXIT

run_bench() {
    local label="$1"
    local target="$2"
    echo ""
    echo -e "  ${BOLD}Target: $label ($target)${RST}"
    "$BINARY" "$target" 2>/dev/null
}

compare_heading() {
    echo ""
    echo -e "${BOLD}${BLU}  COMPARISON SUMMARY${RST}"
    echo -e "  ${CYN}VexFS = AI-augmented FUSE filesystem${RST}"
    echo -e "  ${CYN}tmpfs = in-memory baseline (theoretical max throughput)${RST}"
    echo ""
    echo -e "  ${YLW}Note: VexFS overhead = AI indexing + entropy check + ARC cache${RST}"
    echo -e "  ${YLW}      This trades raw throughput for semantic search + safety${RST}"
}

# ─────────────────────── MAIN ───────────────────────
banner
check_deps
build_release
ensure_image
ensure_mount

mkdir -p "$TMP_BASELINE"

section "VexFS Benchmark"
run_bench "VexFS (AI-augmented FUSE)" "$VEXFS_MNT"

section "tmpfs Baseline"
run_bench "tmpfs (in-memory baseline)" "$TMP_BASELINE"

compare_heading

section "Entropy Detection Demo"
echo -e "  ${BLU}Writing encrypted-looking data to VexFS...${RST}"
python3 -c "
import sys, random
# Generate high-entropy data (looks like ransomware payload)
data = bytes(range(256)) * 256
sys.stdout.buffer.write(data)
" > "$VEXFS_MNT/suspicious.enc" 2>/dev/null && \
    echo -e "  ${RED}🚨 Check VexFS logs — entropy alert should have fired!${RST}" || \
    echo -e "  ${YLW}Skipped (VexFS logs printed to FUSE daemon output)${RST}"

section "Live Search Demo"
echo "benchmark performance test" > "$VEXFS_MNT/test_doc.txt" 2>/dev/null || true
echo "authentication login credentials" > "$VEXFS_MNT/auth.txt" 2>/dev/null || true
sleep 0.2

echo "authentication" > "$VEXFS_MNT/.vexfs-search" 2>/dev/null && {
    echo -e "  ${GRN}Search results:${RST}"
    cat "$VEXFS_MNT/.vexfs-search" 2>/dev/null | head -10 | sed 's/^/  /'
} || echo -e "  ${YLW}Search query filed (VexFS returns results via cat)${RST}"

section "Filesystem Stats"
df -h "$VEXFS_MNT" 2>/dev/null | sed 's/^/  /' || true

echo ""
echo -e "${GRN}${BOLD}  ✓ Benchmark complete.${RST}"
echo ""
echo -e "  Run again with a custom mountpoint:"
echo -e "    ${BLU}./bench.sh ~/mnt/vexfs ~/vexfs.img${RST}"
echo ""
