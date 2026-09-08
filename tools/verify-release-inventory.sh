#!/bin/sh

set -eu

if [ "$#" -gt 1 ]; then
    echo "Usage: tools/verify-release-inventory.sh [SOURCE-ROOT]" >&2
    exit 2
fi

source_root=${1:-.}
cd -- "$source_root"
LC_ALL=C
export LC_ALL

if [ ! -f release-files.txt ] || [ -L release-files.txt ]; then
    echo "release-files.txt is missing or unsafe" >&2
    exit 1
fi

temporary=$(mktemp -d "${TMPDIR:-/tmp}/rmu-release-inventory.XXXXXX")
cleanup() {
    rm -rf -- "$temporary"
}
trap cleanup EXIT HUP INT TERM

: > "$temporary/listed.unsorted"
: > "$temporary/docker.expected"
set --
while IFS= read -r path || [ -n "$path" ]; do
    case $path in
        ""|.|..|/*|*/*) echo "invalid release inventory entry: $path" >&2; exit 1 ;;
    esac
    if [ ! -e "$path" ] && [ ! -L "$path" ]; then
        echo "release inventory entry is missing: $path" >&2
        exit 1
    fi
    if [ -L "$path" ]; then
        echo "release inventory entries may not be symlinks: $path" >&2
        exit 1
    fi
    printf '!/%s\n' "$path" >> "$temporary/docker.expected"
    if [ -d "$path" ]; then
        printf '!/%s/**\n' "$path" >> "$temporary/docker.expected"
    fi
    if ! find "$path" -type f -print0 >> "$temporary/listed.unsorted"; then
        echo "cannot enumerate release inventory entry: $path" >&2
        exit 1
    fi
done < release-files.txt

line_count=$(wc -l < release-files.txt | tr -d ' ')
unique_count=$(sort -u release-files.txt | wc -l | tr -d ' ')
[ "$line_count" = "$unique_count" ] \
    || { echo "release-files.txt contains duplicate entries" >&2; exit 1; }

sort -z "$temporary/listed.unsorted" > "$temporary/listed"
git ls-files --cached --others --exclude-standard -z > "$temporary/tracked.unsorted"
git ls-files --deleted -z > "$temporary/deleted.unsorted"
sort -z "$temporary/tracked.unsorted" > "$temporary/tracked"
sort -z "$temporary/deleted.unsorted" > "$temporary/deleted"
comm -z -23 "$temporary/tracked" "$temporary/deleted" > "$temporary/present"
cmp "$temporary/listed" "$temporary/present" >/dev/null \
    || { echo "release-files.txt does not cover the exact tracked file set" >&2; exit 1; }

sed -n '/^!/p' .dockerignore > "$temporary/docker.actual"
cmp "$temporary/docker.expected" "$temporary/docker.actual" >/dev/null \
    || { echo ".dockerignore release allow-list does not match release-files.txt" >&2; exit 1; }
