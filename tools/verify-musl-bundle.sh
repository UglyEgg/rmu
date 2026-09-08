#!/bin/sh

set -eu
umask 022
LC_ALL=C
export LC_ALL

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "Usage: tools/verify-musl-bundle.sh BUNDLE-DIR [SOURCE-ROOT]" >&2
    exit 2
fi

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
default_source_root=$(CDPATH='' cd -- "$script_directory/.." && pwd)
invocation_directory=$(pwd)
bundle_directory=$1
source_root=${2:-$default_source_root}
case $bundle_directory in
    /*) ;;
    *) bundle_directory=$invocation_directory/$bundle_directory ;;
esac
case $source_root in
    /*) ;;
    *) source_root=$invocation_directory/$source_root ;;
esac

if [ -L "$bundle_directory/current" ]; then
    current_target=$(readlink "$bundle_directory/current")
    case $current_target in
        .rmu-generation.*)
            case $current_target in
                */*) echo "unsafe musl current link: $current_target" >&2; exit 1 ;;
                *) bundle_directory=$bundle_directory/$current_target ;;
            esac
            ;;
        *) echo "unsafe musl current link: $current_target" >&2; exit 1 ;;
    esac
fi

expected_artifacts=$(mktemp "${TMPDIR:-/tmp}/rmu-artifacts.XXXXXX")
current_source=$(mktemp "${TMPDIR:-/tmp}/rmu-source.XXXXXX")
current_runtime=$(mktemp "${TMPDIR:-/tmp}/rmu-runtime.XXXXXX")
rmu_identity=$(mktemp "${TMPDIR:-/tmp}/rmu-identity.XXXXXX")
rmuctl_identity=$(mktemp "${TMPDIR:-/tmp}/rmuctl-identity.XXXXXX")
cleanup() {
    rm -f -- "$expected_artifacts" "$current_source" "$current_runtime" \
        "$rmu_identity" "$rmuctl_identity"
}
interrupted() {
    trap - EXIT HUP INT TERM
    cleanup
    exit 1
}
trap cleanup EXIT
trap interrupted HUP INT TERM

for artifact in rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS SHA256SUMS; do
    if [ ! -f "$bundle_directory/$artifact" ] || [ -L "$bundle_directory/$artifact" ]; then
        echo "missing or unsafe musl artifact: $bundle_directory/$artifact" >&2
        exit 1
    fi
done
if [ ! -x "$bundle_directory/rmu" ] || [ ! -x "$bundle_directory/rmuctl" ]; then
    echo "musl binaries are not executable" >&2
    exit 1
fi
for command_name in readelf sha256sum; do
    command -v "$command_name" >/dev/null 2>&1 \
        || { echo "$command_name is required to verify musl bundles" >&2; exit 1; }
done

(cd "$bundle_directory" \
    && sha256sum rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS) \
    > "$expected_artifacts"
cmp "$expected_artifacts" "$bundle_directory/SHA256SUMS" >/dev/null \
    || { echo "musl SHA256SUMS does not contain exactly the expected artifacts" >&2; exit 1; }
(cd "$bundle_directory" && sha256sum -c SHA256SUMS >&2)

sh "$source_root/tools/write-source-manifest.sh" "$source_root" > "$current_source"
cmp "$current_source" "$bundle_directory/SOURCE-SHA256SUMS" >/dev/null \
    || { echo "musl bundle was not built from the current release source" >&2; exit 1; }
source_manifest_sha256=$(sha256sum "$current_source" | cut -d' ' -f1)
sh "$source_root/tools/write-runtime-manifest.sh" "$source_root" > "$current_runtime"
cmp "$current_runtime" "$bundle_directory/RUNTIME-SHA256SUMS" >/dev/null \
    || { echo "musl bundle was not built from the current runtime source" >&2; exit 1; }
runtime_manifest_sha256=$(sha256sum "$current_runtime" | cut -d' ' -f1)

inspect_binary() {
    binary=$1
    identity_file=$2
    elf_class=$(readelf -hW "$binary" 2>/dev/null \
        | sed -n 's/^[[:space:]]*Class:[[:space:]]*//p')
    elf_data=$(readelf -hW "$binary" 2>/dev/null \
        | sed -n 's/^[[:space:]]*Data:[[:space:]]*//p')
    elf_machine=$(readelf -hW "$binary" 2>/dev/null \
        | sed -n 's/^[[:space:]]*Machine:[[:space:]]*//p')
    [ "$elf_class" = ELF64 ] \
        || { echo "$binary is not a 64-bit ELF binary" >&2; exit 1; }
    case $elf_data in
        "2's complement, little endian"*) ;;
        *) echo "$binary has unsupported ELF byte order: $elf_data" >&2; exit 1 ;;
    esac
    case $elf_machine in
        "Advanced Micro Devices X86-64") inspected_architecture=x86_64 ;;
        AArch64) inspected_architecture=aarch64 ;;
        *) echo "$binary has unsupported ELF machine: $elf_machine" >&2; exit 1 ;;
    esac
    if readelf -lW "$binary" 2>/dev/null | grep -q '[[:space:]]INTERP[[:space:]]'; then
        echo "$binary contains an ELF interpreter" >&2
        exit 1
    fi
    if readelf -dW "$binary" 2>/dev/null | grep -q '(NEEDED)'; then
        echo "$binary requires a shared library" >&2
        exit 1
    fi
    identity_dump=$(readelf -p .rmu.build "$binary" 2>/dev/null) \
        || { echo "$binary has no readable embedded RMU build identity" >&2; exit 1; }
    printf '%s\n' "$identity_dump" \
        | sed -n \
            -e 's/^  \[[[:space:]0-9a-fA-F][[:space:]0-9a-fA-F]*\]  //p' \
            -e 's/^            //p' \
        | sed 's/\\n$//' > "$identity_file"
    [ "$(wc -l < "$identity_file" | tr -d ' ')" -eq 4 ] \
        || { echo "$binary has a malformed embedded RMU build identity" >&2; exit 1; }
    [ "$(sed -n '1p' "$identity_file")" = RMU-BUILD-IDENTITY-V2 ] \
        || { echo "$binary has an unsupported RMU build identity" >&2; exit 1; }
    inspected_source=$(sed -n 's/^source-manifest-sha256=//p' "$identity_file")
    inspected_runtime=$(sed -n 's/^runtime-manifest-sha256=//p' "$identity_file")
    embedded_architecture=$(sed -n 's/^architecture=//p' "$identity_file")
    [ "$embedded_architecture" = "$inspected_architecture" ] \
        || { echo "$binary embeds architecture $embedded_architecture but is $inspected_architecture" >&2; exit 1; }
    [ "$inspected_source" = "$source_manifest_sha256" ] \
        || { echo "$binary was not compiled from the supplied source manifest" >&2; exit 1; }
    [ "$inspected_runtime" = "$runtime_manifest_sha256" ] \
        || { echo "$binary was not compiled from the supplied runtime manifest" >&2; exit 1; }
}

inspect_binary "$bundle_directory/rmu" "$rmu_identity"
rmu_architecture=$inspected_architecture
rmu_source=$inspected_source
rmu_runtime=$inspected_runtime
inspect_binary "$bundle_directory/rmuctl" "$rmuctl_identity"
rmuctl_architecture=$inspected_architecture
rmuctl_source=$inspected_source
rmuctl_runtime=$inspected_runtime
[ "$rmu_architecture" = "$rmuctl_architecture" ] \
    || { echo "musl binaries have different architectures" >&2; exit 1; }
[ "$rmu_source" = "$rmuctl_source" ] \
    || { echo "musl binaries have different source identities" >&2; exit 1; }
[ "$rmu_runtime" = "$rmuctl_runtime" ] \
    || { echo "musl binaries have different runtime identities" >&2; exit 1; }

metadata_architecture=$(sed -n 's/^architecture=//p' "$bundle_directory/BUILD-INFO.txt")
[ "$metadata_architecture" = "$rmu_architecture" ] \
    || { echo "musl metadata architecture does not match the binaries" >&2; exit 1; }
metadata_source=$(sed -n 's/^source-manifest-sha256=//p' "$bundle_directory/BUILD-INFO.txt")
[ "$metadata_source" = "$source_manifest_sha256" ] \
    || { echo "musl metadata source identity does not match the binaries" >&2; exit 1; }
metadata_runtime=$(sed -n 's/^runtime-manifest-sha256=//p' "$bundle_directory/BUILD-INFO.txt")
[ "$metadata_runtime" = "$runtime_manifest_sha256" ] \
    || { echo "musl metadata runtime identity does not match the binaries" >&2; exit 1; }

[ "$(grep -c '^source-git-head=' "$bundle_directory/BUILD-INFO.txt")" -eq 1 ] \
    || { echo "musl metadata must contain exactly one source Git head" >&2; exit 1; }
[ "$(grep -c '^source-git-worktree=' "$bundle_directory/BUILD-INFO.txt")" -eq 1 ] \
    || { echo "musl metadata must contain exactly one source Git worktree state" >&2; exit 1; }
metadata_git_head=$(sed -n 's/^source-git-head=//p' "$bundle_directory/BUILD-INFO.txt")
metadata_git_state=$(sed -n 's/^source-git-worktree=//p' "$bundle_directory/BUILD-INFO.txt")
case $metadata_git_state in
    clean|modified|unavailable) ;;
    *) echo "musl metadata has an invalid source Git worktree state" >&2; exit 1 ;;
esac
case $metadata_git_head in
    unavailable) ;;
    *[!0-9a-f]*) echo "musl metadata has an invalid source Git head" >&2; exit 1 ;;
    *)
        [ "${#metadata_git_head}" -eq 40 ] || [ "${#metadata_git_head}" -eq 64 ] \
            || { echo "musl metadata has an invalid source Git head" >&2; exit 1; }
        ;;
esac
if command -v git >/dev/null 2>&1 \
    && current_git_head=$(git -C "$source_root" rev-parse --verify HEAD 2>/dev/null); then
    [ "$metadata_git_head" = "$current_git_head" ] \
        || { echo "musl metadata Git head does not match the supplied source" >&2; exit 1; }
    if [ -n "$(git -C "$source_root" status --porcelain --untracked-files=all)" ]; then
        current_git_state=modified
    else
        current_git_state=clean
    fi
    [ "$metadata_git_state" = "$current_git_state" ] \
        || { echo "musl metadata Git worktree state does not match the supplied source" >&2; exit 1; }
elif [ "$metadata_git_head" != unavailable ] && [ "$metadata_git_state" = unavailable ]; then
    echo "musl metadata has inconsistent unavailable Git provenance" >&2
    exit 1
fi

version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' \
    "$source_root/Cargo.toml" | sed -n '1p')
case $version in
    ""|*[!0-9.]*|.*|*.) echo "cannot determine a valid source version" >&2; exit 1 ;;
esac

rmu_version=$(sed -n 's/^rmu-version=//p' "$bundle_directory/BUILD-INFO.txt")
rmuctl_version=$(sed -n 's/^rmuctl-version=//p' "$bundle_directory/BUILD-INFO.txt")
[ "$rmu_version" = "rmu $version" ] \
    || { echo "musl metadata does not bind rmu to source version $version" >&2; exit 1; }
[ "$rmuctl_version" = "rmuctl $version" ] \
    || { echo "musl metadata does not bind rmuctl to source version $version" >&2; exit 1; }

printf '%s\n' "$rmu_architecture"
