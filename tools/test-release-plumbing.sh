#!/bin/sh

set -eu

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
temporary=$(mktemp -d "${TMPDIR:-/tmp}/rmu-release-plumbing.XXXXXX")
cleanup() {
    rm -rf -- "$temporary"
}
trap cleanup EXIT HUP INT TERM

# shellcheck disable=SC1091
. "$script_directory/artifact-publication.sh"
# shellcheck disable=SC1091
. "$script_directory/artifact-identity.sh"
output=$(prepare_artifact_output_root "$temporary/output")
mkdir -p \
    "$output/.rmu-generation.old" \
    "$output/.rmu-generation.orphan" \
    "$output/.rmu-generation.new"
printf old > "$output/.rmu-generation.old/artifact"
printf new > "$output/.rmu-generation.new/artifact"
ln -s .rmu-generation.old "$output/current"
ln -s .rmu-generation.new "$output/.rmu-generation.new/.current-link"
mv -fT "$output/.rmu-generation.new/.current-link" "$output/current"
prune_artifact_generations "$output" .rmu-generation.new .rmu-generation.old
[ ! -e "$output/.rmu-generation.orphan" ]
[ "$(cat "$output/current/artifact")" = new ]
[ ! -e "$output/artifact" ]

ln -s current/artifact "$output/artifact"
validate_legacy_artifact_link "$output" artifact
retire_legacy_artifact_link "$output" artifact
[ ! -L "$output/artifact" ]
printf unexpected > "$output/unexpected-file"
if validate_legacy_artifact_link "$output" unexpected-file >/dev/null 2>&1; then
    echo "legacy artifact validation accepted a regular file" >&2
    exit 1
fi
ln -s current/different "$output/altered-link"
if validate_legacy_artifact_link "$output" altered-link >/dev/null 2>&1; then
    echo "legacy artifact validation accepted an altered link" >&2
    exit 1
fi

mkdir -m 0777 "$temporary/writable-parent"
chmod 0777 "$temporary/writable-parent"
mkdir "$temporary/writable-parent/output"
if prepare_artifact_output_root "$temporary/writable-parent/output" >/dev/null 2>&1; then
    echo "artifact publication accepted a non-sticky writable parent" >&2
    exit 1
fi
mkdir "$temporary/real-output-parent"
ln -s real-output-parent "$temporary/linked-output-parent"
if prepare_artifact_output_root "$temporary/linked-output-parent/output" >/dev/null 2>&1; then
    echo "artifact publication accepted a symlink ancestor" >&2
    exit 1
fi

musl_source=$temporary/musl-source
musl_generation=$musl_source/.rmu-generation.test
mkdir -p "$musl_generation"
printf 'sealed bytes' > "$musl_generation/rmu"
ln -s .rmu-generation.test "$musl_source/current"
fake_verifier=$temporary/fake-verifier
cat > "$fake_verifier" <<'EOF'
#!/bin/sh
printf 'changed bytes' > "$ORIGINAL/.rmu-generation.test/rmu"
printf 'x86_64\n'
EOF
chmod 0755 "$fake_verifier"
ORIGINAL=$musl_source
export ORIGINAL
sealed_musl=$temporary/sealed-musl
[ "$(stage_verified_musl_bundle "$musl_source" "$sealed_musl" "$temporary" "$fake_verifier")" = x86_64 ]
[ "$(cat "$sealed_musl/rmu")" = 'sealed bytes' ]

mkdir "$temporary/input"
mkfifo "$temporary/input/not-regular"
if "$script_directory/hash-inputs.sh" "$temporary" ./input >/dev/null 2>&1; then
    echo "manifest hashing accepted a FIFO" >&2
    exit 1
fi

inventory_repo=$temporary/inventory-repository
mkdir "$inventory_repo"
printf keep > "$inventory_repo/keep"
printf removed > "$inventory_repo/removed"
printf '.dockerignore\nkeep\nrelease-files.txt\n' > "$inventory_repo/release-files.txt"
printf '**\n!/.dockerignore\n!/keep\n!/release-files.txt\n' > "$inventory_repo/.dockerignore"
git -C "$inventory_repo" init -q
git -C "$inventory_repo" add .
git -C "$inventory_repo" \
    -c user.name=RMU -c user.email=rmu.invalid@example.invalid \
    commit -qm inventory
rm "$inventory_repo/removed"
"$script_directory/verify-release-inventory.sh" "$inventory_repo"

fake_bin=$temporary/bin
mkdir "$fake_bin"
cat > "$fake_bin/podman" <<'EOF'
#!/bin/sh
[ "$1" = info ] || exit 2
exit "${PODMAN_STATUS:-0}"
EOF
cat > "$fake_bin/docker" <<'EOF'
#!/bin/sh
[ "$1" = info ] || exit 2
exit "${DOCKER_STATUS:-0}"
EOF
chmod 0755 "$fake_bin/podman" "$fake_bin/docker"
# shellcheck disable=SC1091
. "$script_directory/container-engine.sh"
PATH=$fake_bin:$PATH PODMAN_STATUS=0 DOCKER_STATUS=0
export PATH PODMAN_STATUS DOCKER_STATUS
[ "$(select_container_engine '')" = podman ]
PODMAN_STATUS=1
export PODMAN_STATUS
[ "$(select_container_engine '')" = docker ]
if select_container_engine invalid >/dev/null 2>&1; then
    echo "container selection accepted an invalid engine" >&2
    exit 1
fi
