#!/bin/sh

set -eu

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck disable=SC1091
. "$script_directory/artifact-identity.sh"

valid=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
require_lower_sha256 "$valid" test
require_pinned_image "example.invalid/image@sha256:$valid" test
require_container_image_id "sha256:$valid"

for invalid in short "${valid}0" ABCDEF "${valid}suffix"; do
    if require_lower_sha256 "$invalid" test >/dev/null 2>&1; then
        echo "accepted invalid digest: $invalid" >&2
        exit 1
    fi
done
for invalid in "$valid" "sha256:short" "sha256:${valid}0" "sha256:${valid}suffix"; do
    if require_container_image_id "$invalid" >/dev/null 2>&1; then
        echo "accepted invalid container image ID: $invalid" >&2
        exit 1
    fi
done
