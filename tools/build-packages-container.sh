#!/bin/sh

set -eu
umask 022

usage() {
    cat <<'EOF'
Usage: tools/build-packages-container.sh [OPTION]...

Build validated RPM and DEB packages around the static musl RMU binaries.
The static builder runs first unless --musl-dir supplies an existing bundle.

Options:
  --engine ENGINE       use podman or docker instead of automatic detection
  --musl-dir DIR        reuse a verified build-musl-container.sh output
  --output DIR          export directory (default: outputs/packages/<arch>)
  --maintainer VALUE    Debian Maintainer field
  --no-cache            rebuild every requested container layer
  -h, --help            display this help and exit

Environment:
  CONTAINER_ENGINE       same as --engine
  MUSL_DIR               same as --musl-dir
  OUTPUT_DIR             same as --output
  CONTAINER_NO_CACHE     set to 1 to rebuild every requested container layer
  PACKAGE_MAINTAINER     same as --maintainer
  PACKAGE_IMAGE          override the pinned package-builder image
  RMU_PACKAGE_IMAGE_TAG  override the local package image tag
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
release_source_date_epoch=
load_release_metadata "$repository"
invocation_directory=$(pwd)
engine=${CONTAINER_ENGINE:-}
musl_directory=${MUSL_DIR:-}
output_directory=${OUTPUT_DIR:-}
maintainer=${PACKAGE_MAINTAINER:-UglyEgg <UglyEgg@users.noreply.github.com>}
package_image=${PACKAGE_IMAGE:-docker.io/library/rust@sha256:7bae7c67364dad5ebbd4060923b34d734fbed66d7c1cf3af72aa2f062af93eb6}
image_tag=${RMU_PACKAGE_IMAGE_TAG:-localhost/rmu-package-builder:local}
no_cache=${CONTAINER_NO_CACHE:-0}

while [ "$#" -gt 0 ]; do
    case $1 in
        --engine)
            [ "$#" -ge 2 ] || { echo "--engine requires a value" >&2; exit 2; }
            engine=$2
            shift 2
            ;;
        --musl-dir)
            [ "$#" -ge 2 ] || { echo "--musl-dir requires a value" >&2; exit 2; }
            musl_directory=$2
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || { echo "--output requires a value" >&2; exit 2; }
            output_directory=$2
            shift 2
            ;;
        --maintainer)
            [ "$#" -ge 2 ] || { echo "--maintainer requires a value" >&2; exit 2; }
            maintainer=$2
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

case $maintainer in
    ""|*'
'*|*''*) echo "maintainer must be one nonempty line" >&2; exit 2 ;;
esac

require_pinned_image "$package_image" PACKAGE_IMAGE

if ! mv --help 2>&1 | grep -q -- '--no-target-directory'; then
    echo "GNU mv with --no-target-directory is required for atomic publication" >&2
    exit 1
fi

engine=$(select_container_engine "$engine")

staging_directory=$(mktemp -d "${TMPDIR:-/tmp}/rmu-packages.XXXXXX")
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

if [ -z "$musl_directory" ]; then
    musl_directory=$staging_directory/musl
    if [ "$no_cache" -eq 1 ]; then
        "$script_directory/build-musl-container.sh" \
            --engine "$engine" \
            --output "$musl_directory" \
            --no-cache
    else
        "$script_directory/build-musl-container.sh" \
            --engine "$engine" \
            --output "$musl_directory"
    fi
else
    case $musl_directory in
        /*) ;;
        *) musl_directory=$invocation_directory/$musl_directory ;;
    esac
fi

verified_musl=$staging_directory/verified-musl
architecture=$(stage_verified_musl_bundle \
    "$musl_directory" \
    "$verified_musl" \
    "$repository" \
    "$script_directory/verify-musl-bundle.sh")
musl_directory=$verified_musl
version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$repository/Cargo.toml" | sed -n '1p')
case $version in
    ""|*[!0-9.]*|.*|*.) echo "cannot determine a valid package version" >&2; exit 1 ;;
esac

context=$staging_directory/context
mkdir -p "$context/man" "$context/completions" "$context/licenses" "$context/packaging"
cp "$musl_directory/rmu" "$musl_directory/rmuctl" "$context/"
cp "$repository/LICENSE" "$repository/README.md" "$context/"
cp "$repository/docs/CHANGELOG.md" "$context/CHANGELOG.md"
cp "$repository/docs/upstream-notices.md" "$context/UPSTREAM-NOTICES.md"
cp "$repository/licenses/"* "$context/licenses/"
cp "$repository/man/rmu.1" "$repository/man/rmuctl.1" "$context/man/"
cp "$repository/completions/"* "$context/completions/"
cp "$repository/packaging/rmu.spec" "$repository/packaging/build-packages.sh" "$context/packaging/"
cp "$repository/Containerfile.packages" "$context/"

image_id_file=$staging_directory/image-id
build_container() {
    "$engine" build "$@" \
        --file "$context/Containerfile.packages" \
        --build-arg "PACKAGE_IMAGE=$package_image" \
        --build-arg "RUSTC_VERSION=$release_rust_version" \
        --build-arg "RMU_VERSION=$version" \
        --build-arg "RMU_RELEASE=1" \
        --build-arg "RMU_ARCHITECTURE=$architecture" \
        --build-arg "RMU_MAINTAINER=$maintainer" \
        --build-arg "SOURCE_DATE_EPOCH=$release_source_date_epoch" \
        --iidfile "$image_id_file" \
        --tag "$image_tag" \
        "$context"
}
if [ "$no_cache" -eq 1 ]; then
    build_container --no-cache
else
    build_container
fi

image_id=$(sed -n '1p' "$image_id_file")
require_container_image_id "$image_id"

container_id=$("$engine" create "$image_id")
"$engine" cp "$container_id:/artifacts/." "$staging_directory"
"$engine" rm "$container_id" >/dev/null
container_id=

for metadata in PACKAGE-FILES PACKAGE-BUILD-INFO.txt SHA256SUMS; do
    [ -f "$staging_directory/$metadata" ] \
        || { echo "package container did not produce $metadata" >&2; exit 1; }
done

case $architecture in
    x86_64)
        rpm_artifact=rmu-$version-1.x86_64.rpm
        deb_artifact=rmu_${version}-1_amd64.deb
        ;;
    aarch64)
        rpm_artifact=rmu-$version-1.aarch64.rpm
        deb_artifact=rmu_${version}-1_arm64.deb
        ;;
esac
for artifact in "$rpm_artifact" "$deb_artifact"; do
    [ -f "$staging_directory/$artifact" ] \
        || { echo "package container did not produce $artifact" >&2; exit 1; }
done
expected_package_files=$staging_directory/EXPECTED-PACKAGE-FILES
{
    echo "$rpm_artifact"
    echo "$deb_artifact"
} > "$expected_package_files"
cmp "$expected_package_files" "$staging_directory/PACKAGE-FILES" \
    || { echo "package container reported unexpected artifacts" >&2; exit 1; }

(cd "$staging_directory" && sha256sum -c SHA256SUMS)
{
    echo "container-engine=$engine"
    echo "artifact-image-id=$image_id"
    echo "musl-rmu-sha256=$(sha256sum "$musl_directory/rmu" | awk '{print $1}')"
    echo "musl-rmuctl-sha256=$(sha256sum "$musl_directory/rmuctl" | awk '{print $1}')"
} >> "$staging_directory/PACKAGE-BUILD-INFO.txt"
(cd "$staging_directory" \
    && sha256sum "$rpm_artifact" "$deb_artifact" \
        PACKAGE-BUILD-INFO.txt PACKAGE-FILES > SHA256SUMS \
    && sha256sum -c SHA256SUMS)

if [ -z "$output_directory" ]; then
    output_directory=$repository/outputs/packages/$architecture
else
    case $output_directory in
        /*) ;;
        *) output_directory=$invocation_directory/$output_directory ;;
    esac
fi
output_directory=$(prepare_artifact_output_root "$output_directory")
publication_lock=$output_directory/.rmu-publish.lock
mkdir -- "$publication_lock" 2>/dev/null \
    || { echo "another publication may be active: $publication_lock" >&2; exit 1; }

if [ -e "$output_directory/current" ] && [ ! -L "$output_directory/current" ]; then
    echo "refusing a non-link current generation: $output_directory/current" >&2
    exit 1
fi
current_target=$(readlink "$output_directory/current" 2>/dev/null || :)
case $current_target in
    "") ;;
    .rmu-generation.*)
        case $current_target in
            */*) echo "unsafe package current link: $current_target" >&2; exit 1 ;;
            *) ;;
        esac
        ;;
    *) echo "unsafe package current link: $current_target" >&2; exit 1 ;;
esac

legacy_artifacts="PACKAGE-FILES PACKAGE-BUILD-INFO.txt SHA256SUMS"
for legacy_path in "$output_directory"/rmu-*.rpm "$output_directory"/rmu_*.deb; do
    if [ -e "$legacy_path" ] || [ -L "$legacy_path" ]; then
        legacy_artifacts="$legacy_artifacts ${legacy_path##*/}"
    fi
done
for artifact in $legacy_artifacts; do
    validate_legacy_artifact_link "$output_directory" "$artifact"
done

publication_directory=$(mktemp -d "$output_directory/.rmu-generation.XXXXXX")
for artifact in \
    "$rpm_artifact" "$deb_artifact" \
    PACKAGE-FILES PACKAGE-BUILD-INFO.txt SHA256SUMS; do
    cp "$staging_directory/$artifact" "$publication_directory/$artifact"
done
chmod 0644 "$publication_directory/"*
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
prune_artifact_generations "$output_directory" "$generation_name" "$current_target"
rmdir -- "$publication_lock" \
    || echo "warning: packages were published, but the publication lock could not be removed: $publication_lock" >&2
publication_lock=

echo "RPM and DEB artifacts exported to $output_directory"
