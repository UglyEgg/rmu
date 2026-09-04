#!/bin/sh

set -eu
umask 022
unset TAR_OPTIONS GZIP BZIP BZIP2 XZ_OPT
LC_ALL=C
export LC_ALL

usage() {
    cat <<'EOF'
Usage: tools/build-release-bundles.sh [OPTION]...

Create deterministic binary and source archives from a verified musl build.

Options:
  --musl-dir DIR  musl artifact directory (default: outputs/musl/<arch>)
  --output DIR    export directory (default: outputs/release/<arch>)
  -h, --help      display this help and exit
EOF
}

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH='' cd -- "$script_directory/.." && pwd)
# shellcheck disable=SC1091
. "$script_directory/artifact-identity.sh"
# shellcheck disable=SC1091
. "$script_directory/artifact-publication.sh"
# shellcheck disable=SC1091
. "$script_directory/release-metadata.sh"
release_source_date_epoch=
load_release_metadata "$repository"
invocation_directory=$(pwd)
musl_directory=
output_directory=
source_date_epoch=$release_source_date_epoch

while [ "$#" -gt 0 ]; do
    case $1 in
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

command -v tar >/dev/null 2>&1 || { echo "GNU tar is required" >&2; exit 1; }
tar --version 2>/dev/null | grep -q 'GNU tar' \
    || { echo "GNU tar is required for deterministic archives" >&2; exit 1; }
"$script_directory/verify-release-inventory.sh" "$repository"
version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$repository/Cargo.toml" | sed -n '1p')
case $version in
    ""|*[!0-9.]*|.*|*.) echo "cannot determine a valid package version" >&2; exit 1 ;;
esac

if [ -z "$musl_directory" ]; then
    for candidate in "$repository/outputs/musl/"*; do
        [ -f "$candidate/current/BUILD-INFO.txt" ] \
            || [ -f "$candidate/BUILD-INFO.txt" ] \
            || continue
        [ -z "$musl_directory" ] \
            || { echo "multiple musl architectures found; use --musl-dir" >&2; exit 1; }
        musl_directory=$candidate
    done
    [ -n "$musl_directory" ] || { echo "no musl build found; run tools/build-musl-container.sh" >&2; exit 1; }
else
    case $musl_directory in
        /*) ;;
        *) musl_directory=$invocation_directory/$musl_directory ;;
    esac
fi

staging_directory=$(mktemp -d "${TMPDIR:-/tmp}/rmu-release.XXXXXX")
publication_directory=
publication_lock=
cleanup() {
    [ -z "$publication_directory" ] || rm -rf -- "$publication_directory"
    [ -z "$publication_lock" ] || rmdir "$publication_lock" >/dev/null 2>&1 || :
    rm -rf -- "$staging_directory"
}
interrupted() {
    trap - EXIT HUP INT TERM
    cleanup
    exit 1
}
trap cleanup EXIT
trap interrupted HUP INT TERM

verified_musl=$staging_directory/verified-musl
architecture=$(stage_verified_musl_bundle \
    "$musl_directory" \
    "$verified_musl" \
    "$repository" \
    "$script_directory/verify-musl-bundle.sh")
musl_directory=$verified_musl

if [ -z "$output_directory" ]; then
    output_directory=$repository/outputs/release/$architecture
else
    case $output_directory in
        /*) ;;
        *) output_directory=$invocation_directory/$output_directory ;;
    esac
fi
output_directory=$(prepare_artifact_output_root "$output_directory")

binary_root=rmu-$version-$architecture-linux-musl
source_root=rmu-$version-source
mkdir -p \
    "$staging_directory/$binary_root/bin" \
    "$staging_directory/$binary_root/share/man/man1" \
    "$staging_directory/$binary_root/share/bash-completion/completions" \
    "$staging_directory/$binary_root/share/zsh/site-functions" \
    "$staging_directory/$binary_root/share/fish/vendor_completions.d" \
    "$staging_directory/$binary_root/licenses" \
    "$staging_directory/$source_root"
install -m 0755 "$musl_directory/rmu" "$musl_directory/rmuctl" \
    "$staging_directory/$binary_root/bin/"
install -m 0644 "$repository/man/rmu.1" "$repository/man/rmuctl.1" \
    "$staging_directory/$binary_root/share/man/man1/"
install -m 0644 "$repository/completions/rmu.bash" \
    "$staging_directory/$binary_root/share/bash-completion/completions/rmu"
install -m 0644 "$repository/completions/rmuctl.bash" \
    "$staging_directory/$binary_root/share/bash-completion/completions/rmuctl"
install -m 0644 "$repository/completions/_rmu" "$repository/completions/_rmuctl" \
    "$staging_directory/$binary_root/share/zsh/site-functions/"
install -m 0644 "$repository/completions/rmu.fish" "$repository/completions/rmuctl.fish" \
    "$staging_directory/$binary_root/share/fish/vendor_completions.d/"
install -m 0644 "$repository/README.md" "$repository/docs/CHANGELOG.md" \
    "$repository/LICENSE" "$musl_directory/SOURCE-SHA256SUMS" \
    "$musl_directory/RUNTIME-SHA256SUMS" \
    "$staging_directory/$binary_root/"
install -m 0644 "$repository/licenses/"* "$staging_directory/$binary_root/licenses/"
install -m 0644 "$repository/docs/upstream-notices.md" \
    "$staging_directory/$binary_root/UPSTREAM-NOTICES.md"
sed '/^container-engine=/d; /^artifact-image-id=/d' \
    "$musl_directory/BUILD-INFO.txt" \
    > "$staging_directory/$binary_root/BUILD-INFO.txt"

while IFS= read -r path || [ -n "$path" ]; do
    cp -R "$repository/$path" "$staging_directory/$source_root/"
done < "$repository/release-files.txt"

staged_source_manifest=$staging_directory/STAGED-SOURCE-SHA256SUMS
sh "$script_directory/write-source-manifest.sh" \
    "$staging_directory/$source_root" > "$staged_source_manifest"
cmp "$staged_source_manifest" "$musl_directory/SOURCE-SHA256SUMS" >/dev/null \
    || { echo "staged source does not match the verified musl source" >&2; exit 1; }

find "$staging_directory/$binary_root" "$staging_directory/$source_root" \
    -type d -exec chmod 0755 {} +
find "$staging_directory/$binary_root" "$staging_directory/$source_root" \
    -type f -exec chmod 0644 {} +
chmod 0755 \
    "$staging_directory/$binary_root/bin/rmu" \
    "$staging_directory/$binary_root/bin/rmuctl" \
    "$staging_directory/$source_root"/tools/*.sh \
    "$staging_directory/$source_root/packaging/build-packages.sh"
find "$staging_directory/$binary_root" "$staging_directory/$source_root" \
    -exec touch -h -d "@$source_date_epoch" {} +

binary_archive=$binary_root.tar.gz
source_archive=$source_root.tar.gz
(cd "$staging_directory" && tar \
    --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$source_date_epoch" -czf "$binary_archive" "$binary_root")
(cd "$staging_directory" && tar \
    --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$source_date_epoch" -czf "$source_archive" "$source_root")

archive_check=$staging_directory/archive-check
mkdir -p "$archive_check/binary" "$archive_check/source"
tar -xzf "$staging_directory/$binary_archive" -C "$archive_check/binary"
tar -xzf "$staging_directory/$source_archive" -C "$archive_check/source"
tree_manifest() {
    root=$1
    (cd "$root" && find . -printf '%P\t%y\t%m\n' | sort)
}
tree_manifest "$staging_directory/$binary_root" > "$staging_directory/BINARY-TREE"
tree_manifest "$archive_check/binary/$binary_root" > "$staging_directory/BINARY-TREE-CHECK"
cmp "$staging_directory/BINARY-TREE" "$staging_directory/BINARY-TREE-CHECK" >/dev/null \
    || { echo "binary archive member set or modes do not match staging" >&2; exit 1; }
diff -r "$staging_directory/$binary_root" "$archive_check/binary/$binary_root" >/dev/null \
    || { echo "binary archive content does not match staging" >&2; exit 1; }
tree_manifest "$staging_directory/$source_root" > "$staging_directory/SOURCE-TREE"
tree_manifest "$archive_check/source/$source_root" > "$staging_directory/SOURCE-TREE-CHECK"
cmp "$staging_directory/SOURCE-TREE" "$staging_directory/SOURCE-TREE-CHECK" >/dev/null \
    || { echo "source archive member set or modes do not match staging" >&2; exit 1; }
diff -r "$staging_directory/$source_root" "$archive_check/source/$source_root" >/dev/null \
    || { echo "source archive content does not match staging" >&2; exit 1; }
(cd "$staging_directory" && sha256sum "$binary_archive" "$source_archive" > RELEASE-SHA256SUMS)

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
            */*) echo "unsafe release current link: $current_target" >&2; exit 1 ;;
            *) ;;
        esac
        ;;
    *) echo "unsafe release current link: $current_target" >&2; exit 1 ;;
esac
legacy_artifacts=RELEASE-SHA256SUMS
for legacy_path in \
    "$output_directory"/rmu-*-linux-musl.tar.gz \
    "$output_directory"/rmu-*-source.tar.gz; do
    if [ -e "$legacy_path" ] || [ -L "$legacy_path" ]; then
        legacy_artifacts="$legacy_artifacts ${legacy_path##*/}"
    fi
done
for artifact in $legacy_artifacts; do
    validate_legacy_artifact_link "$output_directory" "$artifact"
done
publication_directory=$(mktemp -d "$output_directory/.rmu-generation.XXXXXX")
for artifact in "$binary_archive" "$source_archive" RELEASE-SHA256SUMS; do
    cp "$staging_directory/$artifact" "$publication_directory/$artifact"
done
chmod 0644 "$publication_directory/"*
chmod 0755 "$publication_directory"
(cd "$publication_directory" && sha256sum -c RELEASE-SHA256SUMS)
generation_name=${publication_directory##*/}
ln -s "$generation_name" "$publication_directory/.current-link"
published_directory=$publication_directory
mv -fT "$published_directory/.current-link" "$output_directory/current"
publication_directory=
for artifact in $legacy_artifacts; do
    retire_legacy_artifact_link "$output_directory" "$artifact"
done
prune_artifact_generations "$output_directory" "$generation_name" "$current_target"
rmdir "$publication_lock" \
    || echo "warning: archives were published, but the publication lock could not be removed: $publication_lock" >&2
publication_lock=
echo "Release archives exported to $output_directory"
