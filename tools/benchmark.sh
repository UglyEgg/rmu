#!/bin/sh

set -eu
umask 077
LC_ALL=C
export LC_ALL

usage() {
    cat <<'EOF'
Usage: tools/benchmark.sh [OPTION]...

Compare GNU rm, rmu --delegate, and rmu Trash mode in an isolated directory.
Fixture creation and cleanup are excluded from every measurement.

Options:
  --rmu FILE          rmu binary to test (default: target/release/rmu)
  --gnu-rm FILE       GNU rm binary (default: discovered rm)
  --output DIR        result directory (default: work/benchmark-results)
  --runs COUNT        measured runs per scenario and mode (default: 15)
  --tree-files COUNT  files in the large-tree scenario (default: 10000)
  -h, --help          display this help and exit

The report records wall-clock duration with a nanosecond-resolution clock and
peak resident memory with GNU time. It does not clear kernel filesystem caches
or require elevated privileges.
EOF
}

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH='' cd -- "$script_directory/.." && pwd)
# shellcheck disable=SC1091
. "$script_directory/artifact-publication.sh"
invocation_directory=$(pwd)
rmu_binary=$repository/target/release/rmu
gnu_rm=$(command -v rm || :)
output_directory=$repository/work/benchmark-results
runs=15
tree_files=10000

while [ "$#" -gt 0 ]; do
    case $1 in
        --rmu)
            [ "$#" -ge 2 ] || { echo "--rmu requires a value" >&2; exit 2; }
            rmu_binary=$2
            shift 2
            ;;
        --gnu-rm)
            [ "$#" -ge 2 ] || { echo "--gnu-rm requires a value" >&2; exit 2; }
            gnu_rm=$2
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || { echo "--output requires a value" >&2; exit 2; }
            output_directory=$2
            shift 2
            ;;
        --runs)
            [ "$#" -ge 2 ] || { echo "--runs requires a value" >&2; exit 2; }
            runs=$2
            shift 2
            ;;
        --tree-files)
            [ "$#" -ge 2 ] || { echo "--tree-files requires a value" >&2; exit 2; }
            tree_files=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case $rmu_binary in /*) ;; *) rmu_binary=$invocation_directory/$rmu_binary ;; esac
case $gnu_rm in /*) ;; *) gnu_rm=$invocation_directory/$gnu_rm ;; esac
case $output_directory in /*) ;; *) output_directory=$invocation_directory/$output_directory ;; esac
case $runs in ""|*[!0-9]*|0) echo "--runs must be a positive integer" >&2; exit 2 ;; esac
case $tree_files in ""|*[!0-9]*|0) echo "--tree-files must be a positive integer" >&2; exit 2 ;; esac

[ -x "$rmu_binary" ] || { echo "rmu is not executable: $rmu_binary" >&2; exit 1; }
[ -x "$gnu_rm" ] || { echo "GNU rm is not executable: $gnu_rm" >&2; exit 1; }
"$gnu_rm" --version 2>/dev/null | sed -n '1p' | grep -q 'GNU coreutils' \
    || { echo "--gnu-rm must identify as GNU coreutils rm" >&2; exit 1; }
[ -x /usr/bin/time ] || { echo "GNU /usr/bin/time is required" >&2; exit 1; }
command -v objcopy >/dev/null 2>&1 || { echo "objcopy is required" >&2; exit 1; }
date +%s%N | grep -Eq '^[0-9]{12,}$' \
    || { echo "date must support nanosecond timestamps" >&2; exit 1; }

output_directory=$(prepare_artifact_output_root "$output_directory")
benchmark_root=$(mktemp -d "$output_directory/.benchmark-work.XXXXXX")
cleanup() {
    "$gnu_rm" -rf -- "$benchmark_root"
}
trap cleanup EXIT HUP INT TERM

fixture_root=$benchmark_root/fixtures
run_root=$benchmark_root/run
home_root=$benchmark_root/home
data_root=$benchmark_root/data
mkdir -p "$fixture_root/single" "$fixture_root/tree/payload"
dd if=/dev/zero of="$fixture_root/single/payload" bs=4096 count=1 status=none

index=0
while [ "$index" -lt "$tree_files" ]; do
    directory_number=$((index / 100))
    directory=$fixture_root/tree/payload/d$directory_number
    [ -d "$directory" ] || mkdir "$directory"
    : > "$directory/f$index"
    index=$((index + 1))
done

raw_file=$benchmark_root/raw.tsv
report_file=$benchmark_root/REPORT.md
time_file=$benchmark_root/time.txt
identity_file=$benchmark_root/rmu-build-identity
objcopy --dump-section ".rmu.build=$identity_file" "$rmu_binary" 2>/dev/null \
    || { echo "rmu has no readable embedded build identity" >&2; exit 1; }
[ "$(sed -n '1p' "$identity_file")" = RMU-BUILD-IDENTITY-V2 ] \
    || { echo "rmu has an unsupported embedded build identity" >&2; exit 1; }
runtime_identity=$(sed -n 's/^runtime-manifest-sha256=//p' "$identity_file")
case $runtime_identity in
    ""|*[!0-9a-f]*) echo "rmu has no bound runtime source identity" >&2; exit 1 ;;
esac
[ "${#runtime_identity}" -eq 64 ] \
    || { echo "rmu has no bound runtime source identity" >&2; exit 1; }
printf 'scenario\tmode\trun\telapsed_ns\tmax_rss_kib\n' > "$raw_file"

prepare_fixture() {
    scenario=$1
    "$gnu_rm" -rf -- "$run_root" "$home_root" "$data_root"
    mkdir -p "$run_root" "$home_root" "$data_root"
    cp -a "$fixture_root/$scenario/payload" "$run_root/payload"
}

measure() {
    scenario=$1
    mode=$2
    run_number=$3
    prepare_fixture "$scenario"
    start_ns=$(date +%s%N)
    case $mode in
        gnu-rm)
            HOME="$home_root" XDG_DATA_HOME="$data_root" \
                RMU_MODE=trash RMU_PROGRESS=never \
                /usr/bin/time -f '%M' -o "$time_file" \
                "$gnu_rm" -r -- "$run_root/payload"
            ;;
        delegate)
            HOME="$home_root" XDG_DATA_HOME="$data_root" \
                RMU_GNU_RM="$gnu_rm" RMU_MODE=trash RMU_PROGRESS=never \
                /usr/bin/time -f '%M' -o "$time_file" \
                "$rmu_binary" --delegate -r -- "$run_root/payload"
            ;;
        trash)
            HOME="$home_root" XDG_DATA_HOME="$data_root" \
                RMU_MODE=trash RMU_PROGRESS=never \
                /usr/bin/time -f '%M' -o "$time_file" \
                "$rmu_binary" --trash -r -- "$run_root/payload"
            ;;
        *)
            echo "internal error: unknown mode $mode" >&2
            exit 1
            ;;
    esac
    end_ns=$(date +%s%N)
    max_rss=$(sed -n '1p' "$time_file")
    case $max_rss in ""|*[!0-9]*) echo "invalid GNU time result" >&2; exit 1 ;; esac
    printf '%s\t%s\t%s\t%s\t%s\n' \
        "$scenario" "$mode" "$run_number" "$((end_ns - start_ns))" "$max_rss" \
        >> "$raw_file"
}

# Warm every code path once. These rows are intentionally not recorded.
for scenario in single tree; do
    for mode in gnu-rm delegate trash; do
        prepare_fixture "$scenario"
        case $mode in
            gnu-rm) "$gnu_rm" -r -- "$run_root/payload" ;;
            delegate) RMU_GNU_RM="$gnu_rm" RMU_PROGRESS=never "$rmu_binary" --delegate -r -- "$run_root/payload" ;;
            trash) HOME="$home_root" XDG_DATA_HOME="$data_root" RMU_PROGRESS=never "$rmu_binary" --trash -r -- "$run_root/payload" ;;
        esac
    done
done

for scenario in single tree; do
    run_number=1
    while [ "$run_number" -le "$runs" ]; do
        # Rotate order so one mode does not systematically receive warmer state.
        case $((run_number % 3)) in
            1) modes='gnu-rm delegate trash' ;;
            2) modes='trash gnu-rm delegate' ;;
            0) modes='delegate trash gnu-rm' ;;
        esac
        for mode in $modes; do
            measure "$scenario" "$mode" "$run_number"
        done
        run_number=$((run_number + 1))
    done
done

rmu_version=$($rmu_binary --version)
gnu_version=$($gnu_rm --version | sed -n '1p')
kernel=$(uname -srmo)
cpu=$(sed -n 's/^model name[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | sed -n '1p')
filesystem=$(stat -f -c %T "$benchmark_root")
rmu_sha=$(sha256sum "$rmu_binary" | awk '{print $1}')
gnu_sha=$(sha256sum "$gnu_rm" | awk '{print $1}')
raw_sha=$(sha256sum "$raw_file" | awk '{print $1}')

{
    echo '# RMU benchmark report'
    echo
    echo "- Date (UTC): $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo "- System: $kernel"
    echo "- CPU: $cpu"
    echo "- Temporary filesystem: $filesystem"
    echo "- RMU: $rmu_version (SHA-256 $rmu_sha)"
    echo "- Runtime source identity: \`$runtime_identity\`"
    echo "- GNU rm: $gnu_version (SHA-256 $gnu_sha)"
    echo "- Measured runs: $runs per mode and scenario, after one warm-up"
    echo "- Tree fixture: $tree_files empty files in groups of 100"
    echo "- Raw sample SHA-256: \`$raw_sha\`"
    echo
    echo '| Scenario | Mode | Median time (ms) | 5th-95th percentile (ms) | Median peak RSS (MiB) | Max RSS (MiB) | Time vs GNU rm |'
    echo '|---|---:|---:|---:|---:|---:|---:|'
    awk -f "$script_directory/benchmark-table.awk" "$raw_file"
    echo
    echo 'The single fixture is one 4 KiB file. The tree fixture measures recursive handling.'
    echo 'Fixture copying, cleanup, and report generation are outside the timed region.'
    echo 'Runs use normal warm filesystem caches; no privileged cache dropping is performed.'
    echo
    echo "GNU rm and \`rmu --delegate\` perform the same unlink operation. Trash mode instead"
    echo 'scans the tree, writes FreeDesktop metadata, and atomically renames the recoverable'
    echo 'payload; it does not reclaim the payload blocks. Compare it as user-observed command'
    echo 'cost, not as identical storage work. Results are specific to this machine and filesystem.'
} > "$report_file"

published_report=$output_directory/REPORT.md
published_raw=$output_directory/raw.tsv
mv -fT "$raw_file" "$published_raw"
mv -fT "$report_file" "$published_report"
printf 'Wrote %s and %s\n' "$published_report" "$published_raw"
