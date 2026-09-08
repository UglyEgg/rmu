#!/bin/sh

set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "Usage: tools/test-musl-verifier.sh BUNDLE-DIR [SOURCE-ROOT]" >&2
    exit 2
fi

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
source_root=${2:-$(CDPATH='' cd -- "$script_directory/.." && pwd)}
bundle=$1
if [ -L "$bundle/current" ]; then
    bundle=$(readlink -f "$bundle/current")
fi
temporary=$(mktemp -d "${TMPDIR:-/tmp}/rmu-musl-verifier.XXXXXX")
fixture=$temporary/bundle
cleanup() {
    rm -rf -- "$temporary"
}
trap cleanup EXIT HUP INT TERM

architecture=$("$script_directory/verify-musl-bundle.sh" "$bundle" "$source_root")
case $architecture in
    x86_64) false_architecture=aarch64 ;;
    aarch64) false_architecture=x86_64 ;;
    *) echo "unexpected verified architecture: $architecture" >&2; exit 1 ;;
esac

mkdir -p "$temporary/unusable-objcopy"
printf '%s\n' '#!/bin/sh' 'exit 99' > "$temporary/unusable-objcopy/objcopy"
chmod 0755 "$temporary/unusable-objcopy/objcopy"
PATH="$temporary/unusable-objcopy:$PATH" \
    "$script_directory/verify-musl-bundle.sh" "$bundle" "$source_root" >/dev/null

reset_fixture() {
    rm -rf -- "$fixture"
    mkdir -p "$fixture"
    for artifact in rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS SHA256SUMS; do
        cp "$bundle/$artifact" "$fixture/$artifact"
    done
}

rechecksum() {
    (cd "$fixture" \
        && sha256sum rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS \
            > SHA256SUMS)
}

expect_rejected() {
    label=$1
    if "$script_directory/verify-musl-bundle.sh" "$fixture" "$source_root" \
        >/dev/null 2>&1; then
        echo "musl verifier accepted $label" >&2
        exit 1
    fi
}

reset_fixture
sed -i "s/^architecture=$architecture$/architecture=$false_architecture/" \
    "$fixture/BUILD-INFO.txt"
rechecksum
expect_rejected "falsified architecture metadata"

reset_fixture
runtime_identity=$(sha256sum "$fixture/RUNTIME-SHA256SUMS" | cut -d' ' -f1)
printf '%s\n%s\n%s\n%s\n' \
    RMU-BUILD-IDENTITY-V2 \
    source-manifest-sha256=0000000000000000000000000000000000000000000000000000000000000000 \
    "runtime-manifest-sha256=$runtime_identity" \
    "architecture=$architecture" > "$temporary/false-identity"
objcopy --update-section ".rmu.build=$temporary/false-identity" "$fixture/rmu"
rechecksum
expect_rejected "a binary with a different embedded source identity"

reset_fixture
sed -i 's/^source-git-worktree=.*/source-git-worktree=unavailable/' \
    "$fixture/BUILD-INFO.txt"
rechecksum
expect_rejected "falsified Git worktree provenance"

reset_fixture
cp -L /bin/sh "$fixture/rmu"
chmod 0755 "$fixture/rmu"
rechecksum
expect_rejected "a dynamic replacement executable"
