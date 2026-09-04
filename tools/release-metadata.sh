#!/bin/sh

load_release_metadata() {
    _rmu_repository=$1
    # shellcheck disable=SC1090,SC1091
    . "$_rmu_repository/release-metadata.env"
    release_source_date_epoch=${SOURCE_DATE_EPOCH:-}
    release_rust_version=$(sed -n 's/^channel = "\([0-9][0-9.]*\)"$/\1/p' \
        "$_rmu_repository/rust-toolchain.toml" | sed -n '1p')
    case $release_source_date_epoch in
        ""|*[!0-9]*) echo "invalid release SOURCE_DATE_EPOCH" >&2; return 1 ;;
    esac
    case $release_rust_version in
        ""|*[!0-9.]*|.*|*.) echo "invalid release Rust version" >&2; return 1 ;;
    esac
    for _rmu_notice in COPYRIGHT COPYRIGHT-library.html LICENSE-MIT; do
        [ -f "$_rmu_repository/licenses/Rust-$release_rust_version-$_rmu_notice" ] \
            || { echo "missing Rust notice for release toolchain: $_rmu_notice" >&2; return 1; }
    done
}
