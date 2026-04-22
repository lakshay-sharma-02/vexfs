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
RESULTS_FILE="/tmp/vexfs_bench_results.txt"

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
    for cmd in cargo fusermount awk; do
        if ! command -v "$cmd" &>/dev/null; then
            echo -e "${RED}✗ Missing: $cmd${RST}"
            missing=1
        fi
    done
    [ $missing -ne 0 ] && { echo -e "${YLW}Install missing tools and retry.${RST}"; exit 1; }
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

# Run bench and extract a single metric by label pattern
extract_metric() {
    local file="$1"
    local pattern="$2"
    grep "$pattern" "$file" | awk '{print $(NF-1)}' | head -1
}

print_comparison() {
    local vexfs_file="$1"
    local tmpfs_file="$2"

    echo ""
    echo -e "${BOLD}${BLU}╔══════════════════════════════════════════════════════════════╗${RST}"
    echo -e "${BOLD}${BLU}║              SIDE-BY-SIDE COMPARISON                        ║${RST}"
    echo -e "${BOLD}${BLU}╚══════════════════════════════════════════════════════════════╝${RST}"
    printf "\n  %-38s %12s %12s %10s\n" "Metric" "VexFS" "tmpfs" "Ratio"
    echo "  ──────────────────────────────────────────────────────────────────"

    compare_row() {
        local label="$1"
        local pattern="$2"
        local unit="$3"
        local higher_is_better="${4:-1}"  # 1 = higher is better (throughput), 0 = lower is better (latency)

        local vval
        local tval
        vval=$(extract_metric "$vexfs_file" "$pattern")
        tval=$(extract_metric "$tmpfs_file" "$pattern")

        if [ -z "$vval" ] || [ -z "$tval" ]; then
            printf "  %-38s %12s %12s %10s\n" "$label" "${vval:-n/a}" "${tval:-n/a}" "n/a"
            return
        fi

        local ratio
        ratio=$(awk "BEGIN {
            if ($tval != 0) printf \"%.2f\", $vval / $tval;
            else print \"inf\"
        }")

        # Color: green if VexFS is within 2x of tmpfs, yellow within 5x, red beyond 5x
        local color="$GRN"
        local ratio_num
        ratio_num=$(awk "BEGIN { printf \"%.2f\", $ratio }" 2>/dev/null || echo "0")
        if [ "$higher_is_better" -eq 1 ]; then
            # throughput — ratio < 0.5 is bad
            awk "BEGIN { exit ($ratio_num < 0.2) ? 0 : 1 }" && color="$RED" || \
            awk "BEGIN { exit ($ratio_num < 0.5) ? 0 : 1 }" && color="$YLW" || true
        else
            # latency — ratio > 5 is bad
            awk "BEGIN { exit ($ratio_num > 5.0) ? 0 : 1 }" && color="$RED" || \
            awk "BEGIN { exit ($ratio_num > 2.0) ? 0 : 1 }" && color="$YLW" || true
        fi

        printf "  %-38s %12s %12s ${color}%10sx${RST}\n" \
            "$label" "${vval} ${unit}" "${tval} ${unit}" "$ratio"
    }

    compare_row "Seq write 16MB"       "sequential write"  "MB/s" 1
    compare_row "Seq read 16MB"        "sequential read"   "MB/s" 1
    compare_row "File create (200)"    "file creates"      "ms"   0
    compare_row "Random reads (1000)"  "random reads"      "µs"   0
    compare_row "Overwrites (100)"     "overwrites"        "ms"   0
    compare_row "Renames (100)"        "renames"           "ms"   0

    echo ""
    echo -e "  ${YLW}Note: VexFS overhead = AI indexing + entropy check + ARC cache + FUSE${RST}"
    echo -e "  ${YLW}      tmpfs is in-memory — this is the theoretical maximum${RST}"
    echo -e "  ${CYN}      Green = within 2x  Yellow = 2-5x slower  Red = >5x slower${RST}"
    echo ""
}

# ─────────────────────── MAIN ───────────────────────
banner
check_deps
build_release
ensure_image
ensure_mount

mkdir -p "$TMP_BASELINE"

section "VexFS Benchmark"
VEXFS_RESULTS=$(mktemp)
"$BINARY" "$VEXFS_MNT" 2>/dev/null | tee "$VEXFS_RESULTS"

section "tmpfs Baseline"
TMPFS_RESULTS=$(mktemp)
"$BINARY" "$TMP_BASELINE" 2>/dev/null | tee "$TMPFS_RESULTS"

print_comparison "$VEXFS_RESULTS" "$TMPFS_RESULTS"

rm -f "$VEXFS_RESULTS" "$TMPFS_RESULTS"

section "Entropy Detection Demo"
echo -e "  ${BLU}Writing encrypted-looking data to VexFS...${RST}"
python3 -c "
import sys
data = bytes(range(256)) * 256
sys.stdout.buffer.write(data)
" > "$VEXFS_MNT/suspicious.enc" 2>/dev/null && \
    echo -e "  ${RED}🚨 Check VexFS logs — entropy alert should have fired!${RST}" || \
    echo -e "  ${YLW}Skipped${RST}"

section "Live Search Demo"
echo "benchmark performance test" > "$VEXFS_MNT/test_doc.txt" 2>/dev/null || true
echo "authentication login credentials" > "$VEXFS_MNT/auth.txt" 2>/dev/null || true
sleep 0.2
echo "authentication" > "$VEXFS_MNT/.vexfs-search" 2>/dev/null || true
sleep 0.3
echo -e "  ${GRN}Search results:${RST}"
cat "$VEXFS_MNT/.vexfs-search" 2>/dev/null | head -10 | sed 's/^/  /' || \
    echo -e "  ${YLW}(search query filed — cat .vexfs-search to read results)${RST}"

section "Filesystem Stats"
df -h "$VEXFS_MNT" 2>/dev/null | sed 's/^/  /' || true

echo ""
echo -e "${GRN}${BOLD}  ✓ Benchmark complete.${RST}"
echo ""
echo -e "  Re-run:  ${BLU}./bench.sh ~/mnt/vexfs ~/vexfs.img${RST}"
echo ""
