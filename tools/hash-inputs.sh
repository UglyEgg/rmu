#!/bin/sh

set -eu

if [ "$#" -lt 2 ]; then
    echo "Usage: tools/hash-inputs.sh SOURCE-ROOT PATH..." >&2
    exit 2
fi

source_root=$1
shift
cd -- "$source_root"
LC_ALL=C
export LC_ALL

temporary=$(mktemp -d "${TMPDIR:-/tmp}/rmu-manifest.XXXXXX")
cleanup() {
    rm -rf -- "$temporary"
}
trap cleanup EXIT HUP INT TERM

unsorted=$temporary/paths.unsorted
sorted=$temporary/paths.sorted
: > "$unsorted"
for source_path do
    if [ ! -e "$source_path" ] && [ ! -L "$source_path" ]; then
        echo "manifest input is missing: $source_path" >&2
        exit 1
    fi
    if ! find "$source_path" ! -type f ! -type d -print -quit > "$temporary/special"; then
        echo "cannot enumerate manifest input: $source_path" >&2
        exit 1
    fi
    if [ -s "$temporary/special" ]; then
        special_path=$(sed -n '1p' "$temporary/special")
        echo "only regular files and nonempty directories are permitted in manifest inputs: $special_path" >&2
        exit 1
    fi
    if ! find "$source_path" -type d -empty -print -quit > "$temporary/empty"; then
        echo "cannot inspect manifest directories: $source_path" >&2
        exit 1
    fi
    if [ -s "$temporary/empty" ]; then
        empty_directory=$(sed -n '1p' "$temporary/empty")
        echo "empty directories are not permitted in manifest inputs: $empty_directory" >&2
        exit 1
    fi
    if ! find "$source_path" -type f -print0 >> "$unsorted"; then
        echo "cannot inventory manifest input: $source_path" >&2
        exit 1
    fi
done

sort -z "$unsorted" > "$sorted"
xargs -0 sha256sum < "$sorted"
