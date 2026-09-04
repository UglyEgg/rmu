#!/bin/sh

set -eu
umask 022

usage() {
    cat <<'EOF'
Usage: tools/build-musl-container.sh [--engine podman|docker] [--output DIR] [--no-cache]

Build and test statically linked musl binaries in the pinned Alpine Rust
container, then export them to DIR. Podman is preferred when both engines are
available.

Options:
  --engine ENGINE  use podman or docker instead of automatic detection
  --output DIR     export directory (default: outputs/musl/<container-arch>)
  --no-cache       rebuild every container layer
  -h, --help       display this help and exit

Environment:
  CONTAINER_ENGINE       same as --engine
  OUTPUT_DIR             same as --output
  CONTAINER_NO_CACHE     set to 1 to rebuild every container layer
  RUST_IMAGE             override the pinned official Rust Alpine image
  RMU_MUSL_IMAGE_TAG     override the local builder image tag
EOF
}

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH='' cd -- "$script_directory/.." && pwd)
# shellcheck disable=SC1091
. "$script_directory/artifact-identity.sh"
# shellcheck disable=SC1091
. "$script_directory/container-engine.sh"
# shellcheck disable=SC1091
. "$script_directory/artifact-publication.sh"
# shellcheck disable=SC1091
. "$script_directory/release-metadata.sh"
release_rust_version=
load_release_metadata "$repository"
invocation_directory=$(pwd)
engine=${CONTAINER_ENGINE:-}
output_directory=${OUTPUT_DIR:-}
rust_image=${RUST_IMAGE:-docker.io/library/rust@sha256:7bae7c67364dad5ebbd4060923b34d734fbed66d7c1cf3af72aa2f062af93eb6}
image_tag=${RMU_MUSL_IMAGE_TAG:-localhost/rmu-musl-builder:local}
no_cache=${CONTAINER_NO_CACHE:-0}

while [ "$#" -gt 0 ]; do
    case $1 in
        --engine)
            [ "$#" -ge 2 ] || { echo "--engine requires a value" >&2; exit 2; }
            engine=$2
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || { echo "--output requires a value" >&2; exit 2; }
            output_directory=$2
            shift 2
            ;;
        --no-cache)
            no_cache=1
            shift
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

case $no_cache in
    0|1) ;;
    *) echo "CONTAINER_NO_CACHE must be 0 or 1" >&2; exit 2 ;;
esac

require_pinned_image "$rust_image" RUST_IMAGE
"$script_directory/verify-release-inventory.sh" "$repository"

if ! mv --help 2>&1 | grep -q -- '--no-target-directory'; then
    echo "GNU mv with --no-target-directory is required for atomic publication" >&2
    exit 1
fi

engine=$(select_container_engine "$engine")

staging_directory=$(mktemp -d "${TMPDIR:-/tmp}/rmu-musl.XXXXXX")
container_id=
publication_directory=
publication_lock=

cleanup() {
    if [ -n "$container_id" ]; then
        "$engine" rm -f "$container_id" >/dev/null 2>&1 || :
    fi
    if [ -n "$publication_directory" ]; then
        rm -rf -- "$publication_directory"
    fi
    if [ -n "$publication_lock" ]; then
        rmdir "$publication_lock" >/dev/null 2>&1 || :
    fi
    rm -rf -- "$staging_directory"
}
interrupted() {
    trap - EXIT HUP INT TERM
    cleanup
    exit 1
}
trap cleanup EXIT
trap interrupted HUP INT TERM

echo "Building with $engine using $rust_image"
version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' \
    "$repository/Cargo.toml" | sed -n '1p')
case $version in
    ""|*[!0-9.]*|.*|*.) echo "cannot determine a valid source version" >&2; exit 1 ;;
esac
image_id_file=$staging_directory/image-id
build_container() {
    "$engine" build "$@" \
        --file "$repository/Containerfile" \
        --build-arg "RUST_IMAGE=$rust_image" \
        --build-arg "RUSTC_VERSION=$release_rust_version" \
        --iidfile "$image_id_file" \
        --tag "$image_tag" \
        "$repository"
}
if [ "$no_cache" -eq 1 ]; then
    build_container --no-cache
else
    build_container
fi

image_id=$(sed -n '1p' "$image_id_file")
require_container_image_id "$image_id"

rmu_version=$("$engine" run --rm "$image_id" --version)
rmuctl_version=$(
    "$engine" run --rm --entrypoint /artifacts/rmuctl "$image_id" --version
)
[ "$rmu_version" = "rmu $version" ] \
    || { echo "container rmu reports an unexpected version: $rmu_version" >&2; exit 1; }
[ "$rmuctl_version" = "rmuctl $version" ] \
    || { echo "container rmuctl reports an unexpected version: $rmuctl_version" >&2; exit 1; }
printf '%s\n%s\n' "$rmu_version" "$rmuctl_version"

container_id=$("$engine" create "$image_id")
"$engine" cp "$container_id:/artifacts/." "$staging_directory"
"$engine" rm "$container_id" >/dev/null
container_id=

for artifact in rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS SHA256SUMS; do
    if [ ! -f "$staging_directory/$artifact" ]; then
        echo "container did not produce $artifact" >&2
        exit 1
    fi
done

{
    echo "container-engine=$engine"
    echo "artifact-image-id=$image_id"
    if command -v git >/dev/null 2>&1 \
        && source_head=$(git -C "$repository" rev-parse --verify HEAD 2>/dev/null); then
        echo "source-git-head=$source_head"
        if [ -n "$(git -C "$repository" status --porcelain --untracked-files=all)" ]; then
            echo "source-git-worktree=modified"
        else
            echo "source-git-worktree=clean"
        fi
    else
        echo "source-git-head=unavailable"
        echo "source-git-worktree=unavailable"
    fi
} >> "$staging_directory/BUILD-INFO.txt"
(cd "$staging_directory" \
    && sha256sum rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS > SHA256SUMS \
    && sha256sum -c SHA256SUMS)
architecture=$(
    "$script_directory/verify-musl-bundle.sh" "$staging_directory" "$repository"
)

if [ -z "$output_directory" ]; then
    output_directory=$repository/outputs/musl/$architecture
else
    case $output_directory in
        /*) ;;
        *) output_directory=$invocation_directory/$output_directory ;;
    esac
fi

output_directory=$(prepare_artifact_output_root "$output_directory")
publication_lock=$output_directory/.rmu-publish.lock
if ! mkdir -- "$publication_lock" 2>/dev/null; then
    echo "another publication may be active; remove stale lock $publication_lock if necessary" >&2
    exit 1
fi
if [ -e "$output_directory/current" ] && [ ! -L "$output_directory/current" ]; then
    echo "refusing a non-link current generation: $output_directory/current" >&2
    exit 1
fi
previous_generation=$(readlink "$output_directory/current" 2>/dev/null || :)
case $previous_generation in
    ""|.rmu-generation.*) ;;
    *) echo "refusing an unsafe current generation: $previous_generation" >&2; exit 1 ;;
esac
case $previous_generation in
    */*) echo "refusing an unsafe current generation: $previous_generation" >&2; exit 1 ;;
esac
legacy_artifacts="rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS SHA256SUMS rtrash rtrashctl"
for artifact in $legacy_artifacts; do
    validate_legacy_artifact_link "$output_directory" "$artifact"
done
publication_directory=$(mktemp -d "$output_directory/.rmu-generation.XXXXXX")
for artifact in rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS SHA256SUMS; do
    cp "$staging_directory/$artifact" "$publication_directory/$artifact"
done
chmod 0755 "$publication_directory/rmu" "$publication_directory/rmuctl"
chmod 0644 \
    "$publication_directory/BUILD-INFO.txt" \
    "$publication_directory/SOURCE-SHA256SUMS" \
    "$publication_directory/RUNTIME-SHA256SUMS" \
    "$publication_directory/SHA256SUMS"
chmod 0755 "$publication_directory"
(cd "$publication_directory" && sha256sum -c SHA256SUMS)

generation_name=${publication_directory##*/}
ln -s "$generation_name" "$publication_directory/.current-link"
published_directory=$publication_directory
mv -fT "$published_directory/.current-link" "$output_directory/current"
publication_directory=
for artifact in $legacy_artifacts; do
    retire_legacy_artifact_link "$output_directory" "$artifact"
done
prune_artifact_generations "$output_directory" "$generation_name" "$previous_generation"
rmdir -- "$publication_lock" \
    || echo "warning: artifacts were published, but the publication lock could not be removed: $publication_lock" >&2
publication_lock=

echo "Static musl artifacts exported to $output_directory"
