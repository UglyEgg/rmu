#!/bin/sh

set -eu

if [ "$#" -gt 1 ]; then
    echo "Usage: tools/write-runtime-manifest.sh [SOURCE-ROOT]" >&2
    exit 2
fi

source_root=$(CDPATH='' cd -- "${1:-.}" && pwd)
exec "$source_root/tools/hash-inputs.sh" "$source_root" \
    ./Cargo.lock \
    ./Cargo.toml \
    ./build.rs \
    ./rust-toolchain.toml \
    ./src \
    ./tools/hash-inputs.sh \
    ./tools/write-runtime-manifest.sh
