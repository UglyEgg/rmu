#!/bin/sh

set -eu
LC_ALL=C
export LC_ALL

if [ "$#" -gt 1 ]; then
    echo "Usage: tools/verify-benchmark-evidence.sh [SOURCE-ROOT]" >&2
    exit 2
fi

source_root=${1:-.}
script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
source_root=$(CDPATH='' cd -- "$source_root" && pwd)
report=$source_root/docs/benchmarks.md
raw=$source_root/docs/benchmark-data/0.7.1/raw.tsv
temporary=$(mktemp -d "${TMPDIR:-/tmp}/rmu-benchmark-evidence.XXXXXX")
cleanup() {
    rm -rf -- "$temporary"
}
trap cleanup EXIT HUP INT TERM

runtime_manifest=$temporary/runtime-manifest
"$script_directory/write-runtime-manifest.sh" "$source_root" > "$runtime_manifest"
runtime_digest=$(sha256sum "$runtime_manifest" | cut -d' ' -f1)
# shellcheck disable=SC2016
reported_runtime=$(sed -n 's/^- Runtime source identity: `\([0-9a-f]\{64\}\)`$/\1/p' "$report")
[ "$reported_runtime" = "$runtime_digest" ] \
    || { echo "benchmark runtime source identity is stale or malformed" >&2; exit 1; }

raw_digest=$(sha256sum "$raw" | cut -d' ' -f1)
# shellcheck disable=SC2016
reported_raw=$(sed -n 's/^- Raw sample SHA-256: `\([0-9a-f]\{64\}\)`$/\1/p' "$report")
[ "$reported_raw" = "$raw_digest" ] \
    || { echo "benchmark raw sample digest is stale or malformed" >&2; exit 1; }

runs=$(sed -n 's/^- Measured runs: \([0-9][0-9]*\) per mode and scenario, after one warm-up$/\1/p' "$report")
case $runs in
    ""|*[!0-9]*|0) echo "benchmark run count is missing or malformed" >&2; exit 1 ;;
esac
awk -F '\t' -v runs="$runs" '
    NR == 1 {
        if ($0 != "scenario\tmode\trun\telapsed_ns\tmax_rss_kib") exit 1
        next
    }
    $1 !~ /^(single|tree)$/ || $2 !~ /^(gnu-rm|delegate|trash)$/ ||
        $3 !~ /^[0-9]+$/ || $4 !~ /^[0-9]+$/ || $5 !~ /^[0-9]+$/ { exit 1 }
    {
        key = $1 SUBSEP $2 SUBSEP $3
        if (seen[key]++) exit 1
        count[$1 SUBSEP $2]++
    }
    END {
        scenarios[1] = "single"; scenarios[2] = "tree"
        modes[1] = "gnu-rm"; modes[2] = "delegate"; modes[3] = "trash"
        for (s = 1; s <= 2; s++)
            for (m = 1; m <= 3; m++)
                if (count[scenarios[s] SUBSEP modes[m]] != runs) exit 1
    }
' "$raw" || { echo "benchmark raw samples are incomplete or malformed" >&2; exit 1; }

awk -f "$script_directory/benchmark-table.awk" "$raw" > "$temporary/expected-table"
sed -n '/^| single |/p; /^| tree |/p' "$report" > "$temporary/reported-table"
cmp "$temporary/expected-table" "$temporary/reported-table" >/dev/null \
    || { echo "benchmark summary table does not match the raw samples" >&2; exit 1; }
