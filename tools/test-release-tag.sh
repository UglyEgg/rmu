#!/bin/sh

set -eu

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH='' cd -- "$script_directory/.." && pwd)
temporary=$(mktemp -d "${TMPDIR:-/tmp}/rmu-release-tag.XXXXXX")
cleanup() {
    rm -rf -- "$temporary"
}
trap cleanup EXIT HUP INT TERM

cat > "$temporary/fake-gh" <<'EOF'
#!/bin/sh
case $2 in
    */git/ref/tags/*)
        case ${FAKE_TAG_KIND:-commit} in
            commit) printf 'commit %s\n' "$FAKE_COMMIT" ;;
            tag) printf 'tag annotation-object\n' ;;
        esac
        ;;
    */git/tags/annotation-object) printf 'commit %s\n' "$FAKE_COMMIT" ;;
    *) exit 1 ;;
esac
EOF
chmod 0755 "$temporary/fake-gh"

commit=$(git -C "$repository" rev-parse --verify HEAD)
GITHUB_REPOSITORY=example/rmu RMU_GH_COMMAND="$temporary/fake-gh" \
    FAKE_COMMIT=$commit "$script_directory/verify-release-tag.sh" v0.7.1 "$commit"
GITHUB_REPOSITORY=example/rmu RMU_GH_COMMAND="$temporary/fake-gh" \
    FAKE_TAG_KIND=tag FAKE_COMMIT=$commit \
    "$script_directory/verify-release-tag.sh" v0.7.1 "$commit"
if GITHUB_REPOSITORY=example/rmu RMU_GH_COMMAND="$temporary/fake-gh" \
    FAKE_COMMIT=0000000000000000000000000000000000000000 \
    "$script_directory/verify-release-tag.sh" v0.7.1 "$commit" >/dev/null 2>&1; then
    echo "accepted a release tag targeting the wrong commit" >&2
    exit 1
fi

for unsafe_tag in v0.7.1/other v0.7.1@other v0.7.1:other; do
    if GITHUB_REPOSITORY=example/rmu RMU_GH_COMMAND="$temporary/fake-gh" \
        FAKE_COMMIT=$commit \
        "$script_directory/verify-release-tag.sh" "$unsafe_tag" "$commit" \
        >/dev/null 2>&1; then
        echo "accepted unsafe release tag: $unsafe_tag" >&2
        exit 1
    fi
done

for invalid_repository in example example/rmu/other example+owner/rmu example/; do
    if GITHUB_REPOSITORY=$invalid_repository RMU_GH_COMMAND="$temporary/fake-gh" \
        FAKE_COMMIT=$commit \
        "$script_directory/verify-release-tag.sh" v0.7.1 "$commit" \
        >/dev/null 2>&1; then
        echo "accepted invalid repository: $invalid_repository" >&2
        exit 1
    fi
done
