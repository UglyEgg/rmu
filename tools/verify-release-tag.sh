#!/bin/sh

set -eu

if [ "$#" -ne 2 ]; then
    echo "Usage: tools/verify-release-tag.sh TAG EXPECTED-COMMIT" >&2
    exit 2
fi

tag=$1
expected_commit=$2
repository=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}
gh_command=${RMU_GH_COMMAND:-gh}

tag_suffix=${tag#v}
case $tag_suffix in
    "$tag"|""|*[!0-9A-Za-z._-]*)
        echo "unsafe release tag: $tag" >&2
        exit 2
        ;;
esac

repository_owner=${repository%%/*}
repository_name=${repository#*/}
case $repository in
    */*) ;;
    *)
        echo "invalid GITHUB_REPOSITORY: $repository" >&2
        exit 2
        ;;
esac
case $repository_owner in
    ""|*[!0-9A-Za-z._-]*)
        echo "invalid GITHUB_REPOSITORY: $repository" >&2
        exit 2
        ;;
esac
case $repository_name in
    ""|*/*|*[!0-9A-Za-z._-]*)
        echo "invalid GITHUB_REPOSITORY: $repository" >&2
        exit 2
        ;;
esac

local_commit=$(git rev-parse --verify HEAD)
[ "$local_commit" = "$expected_commit" ] \
    || { echo "checkout is $local_commit, expected $expected_commit" >&2; exit 1; }

object=$(
    "$gh_command" api "repos/$repository/git/ref/tags/$tag" \
        --jq '.object.type + " " + .object.sha'
)
depth=0
while :; do
    case $object in
        "commit "*|"tag "*) ;;
        *) echo "GitHub returned malformed tag identity" >&2; exit 1 ;;
    esac
    object_type=${object%% *}
    object_sha=${object#* }
    case $object_sha in
        ""|*' '*) echo "GitHub returned malformed tag identity" >&2; exit 1 ;;
    esac
    case $object_type in
        commit) break ;;
        tag)
            depth=$((depth + 1))
            [ "$depth" -le 8 ] \
                || { echo "annotated tag chain is unexpectedly deep" >&2; exit 1; }
            object=$(
                "$gh_command" api "repos/$repository/git/tags/$object_sha" \
                    --jq '.object.type + " " + .object.sha'
            )
            ;;
        *) echo "release tag resolves to unsupported object type: $object_type" >&2; exit 1 ;;
    esac
done

[ "$object_sha" = "$expected_commit" ] \
    || { echo "release tag resolves to $object_sha, expected $expected_commit" >&2; exit 1; }
