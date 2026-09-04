#!/bin/sh

require_lower_sha256() {
    _rmu_digest=$1
    _rmu_label=$2
    if [ "${#_rmu_digest}" -ne 64 ]; then
        echo "$_rmu_label must contain exactly 64 hexadecimal characters" >&2
        return 2
    fi
    case $_rmu_digest in
        *[!0-9a-f]*)
            echo "$_rmu_label must contain only lowercase hexadecimal characters" >&2
            return 2
            ;;
    esac
}

require_pinned_image() {
    _rmu_image=$1
    _rmu_label=$2
    case $_rmu_image in
        ?*@sha256:*) _rmu_digest=${_rmu_image##*@sha256:} ;;
        *)
            echo "$_rmu_label must use an immutable @sha256: digest" >&2
            return 2
            ;;
    esac
    require_lower_sha256 "$_rmu_digest" "$_rmu_label digest"
}

require_container_image_id() {
    _rmu_image_id=$1
    case $_rmu_image_id in
        sha256:*) _rmu_digest=${_rmu_image_id#sha256:} ;;
        *)
            echo "container engine did not report a sha256 image ID" >&2
            return 1
            ;;
    esac
    require_lower_sha256 "$_rmu_digest" "container image ID" || return 1
}

stage_verified_musl_bundle() {
    _rmu_source=$1
    _rmu_destination=$2
    _rmu_repository=$3
    _rmu_verifier=$4
    _rmu_selected=$_rmu_source
    if [ -L "$_rmu_source/current" ]; then
        _rmu_current=$(readlink "$_rmu_source/current")
        case $_rmu_current in
            .rmu-generation.*)
                case $_rmu_current in
                    */*) echo "unsafe musl current link: $_rmu_current" >&2; return 1 ;;
                    *) _rmu_selected=$_rmu_source/$_rmu_current ;;
                esac
                ;;
            *) echo "unsafe musl current link: $_rmu_current" >&2; return 1 ;;
        esac
    elif [ -e "$_rmu_source/current" ]; then
        echo "refusing a non-link musl current generation" >&2
        return 1
    fi
    [ ! -e "$_rmu_destination" ] && [ ! -L "$_rmu_destination" ] \
        || { echo "verified musl staging destination already exists" >&2; return 1; }
    mkdir -m 0700 -- "$_rmu_destination"
    cp -a "$_rmu_selected/." "$_rmu_destination/"
    "$_rmu_verifier" "$_rmu_destination" "$_rmu_repository"
}
