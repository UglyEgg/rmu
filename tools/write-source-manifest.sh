#!/bin/sh

set -eu

if [ "$#" -gt 1 ]; then
    echo "Usage: tools/write-source-manifest.sh [SOURCE-ROOT]" >&2
    exit 2
fi

source_root=$(CDPATH='' cd -- "${1:-.}" && pwd)
set --
while IFS= read -r path || [ -n "$path" ]; do
    set -- "$@" "./$path"
done < "$source_root/release-files.txt"

exec "$source_root/tools/hash-inputs.sh" "$source_root" "$@"
