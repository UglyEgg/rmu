#!/bin/sh

prepare_artifact_output_root() {
    _rmu_requested=$1
    case $_rmu_requested in
        /*) ;;
        *) echo "artifact output directory must be absolute: $_rmu_requested" >&2; return 1 ;;
    esac
    _rmu_euid=$(id -u)
    _rmu_system_owner=$(stat -c '%u' -- /)
    _rmu_existing=$_rmu_requested
    while [ ! -e "$_rmu_existing" ] && [ ! -L "$_rmu_existing" ]; do
        _rmu_existing=${_rmu_existing%/*}
        [ -n "$_rmu_existing" ] || _rmu_existing=/
    done
    _rmu_candidate=$_rmu_existing
    while :; do
        [ ! -L "$_rmu_candidate" ] \
            || { echo "refusing a symlink artifact-output ancestor: $_rmu_candidate" >&2; return 1; }
        _rmu_owner=$(stat -c '%u' -- "$_rmu_candidate")
        _rmu_mode=$(stat -c '%a' -- "$_rmu_candidate")
        _rmu_mode_value=$((0$_rmu_mode))
        if [ "$_rmu_owner" != "$_rmu_system_owner" ] \
            && [ "$_rmu_owner" != "$_rmu_euid" ]; then
            echo "refusing artifact output below a foreign-owned directory: $_rmu_candidate" >&2
            return 1
        fi
        if [ $((_rmu_mode_value & 0022)) -ne 0 ] \
            && [ $((_rmu_mode_value & 01000)) -eq 0 ]; then
            echo "refusing artifact output below a non-sticky writable directory: $_rmu_candidate" >&2
            return 1
        fi
        [ "$_rmu_candidate" != / ] || break
        _rmu_candidate=${_rmu_candidate%/*}
        [ -n "$_rmu_candidate" ] || _rmu_candidate=/
    done
    [ ! -L "$_rmu_requested" ] \
        || { echo "refusing a symlink output directory: $_rmu_requested" >&2; return 1; }
    mkdir -p -- "$_rmu_requested"
    _rmu_output=$(CDPATH='' cd -- "$_rmu_requested" && pwd -P) || return 1
    _rmu_candidate=$_rmu_output
    _rmu_first=1
    while :; do
        _rmu_owner=$(stat -c '%u' -- "$_rmu_candidate")
        _rmu_mode=$(stat -c '%a' -- "$_rmu_candidate")
        _rmu_mode_value=$((0$_rmu_mode))
        if [ "$_rmu_owner" != "$_rmu_system_owner" ] \
            && [ "$_rmu_owner" != "$_rmu_euid" ]; then
            echo "refusing artifact output below a foreign-owned directory: $_rmu_candidate" >&2
            return 1
        fi
        if [ "$_rmu_first" -eq 1 ]; then
            [ "$_rmu_owner" = "$_rmu_euid" ] \
                || { echo "artifact output is not owned by the current user: $_rmu_output" >&2; return 1; }
            [ $((_rmu_mode_value & 0022)) -eq 0 ] \
                || { echo "artifact output is writable by other users: $_rmu_output" >&2; return 1; }
            _rmu_first=0
        elif [ $((_rmu_mode_value & 0022)) -ne 0 ] \
            && [ $((_rmu_mode_value & 01000)) -eq 0 ]; then
            echo "refusing artifact output below a non-sticky writable directory: $_rmu_candidate" >&2
            return 1
        fi
        [ "$_rmu_candidate" != / ] || break
        _rmu_candidate=${_rmu_candidate%/*}
        [ -n "$_rmu_candidate" ] || _rmu_candidate=/
    done
    printf '%s\n' "$_rmu_output"
}

validate_legacy_artifact_link() {
    _rmu_output=$1
    _rmu_artifact=$2
    case $_rmu_artifact in
        ""|.|..|*/*) echo "invalid legacy artifact name: $_rmu_artifact" >&2; return 1 ;;
    esac
    _rmu_path=$_rmu_output/$_rmu_artifact
    if [ ! -e "$_rmu_path" ] && [ ! -L "$_rmu_path" ]; then
        return 0
    fi
    [ -L "$_rmu_path" ] \
        || { echo "refusing a non-link legacy artifact: $_rmu_path" >&2; return 1; }
    _rmu_target=$(readlink "$_rmu_path") || return 1
    [ "$_rmu_target" = "current/$_rmu_artifact" ] \
        || { echo "refusing an altered legacy artifact link: $_rmu_path" >&2; return 1; }
}

retire_legacy_artifact_link() {
    _rmu_output=$1
    _rmu_artifact=$2
    validate_legacy_artifact_link "$_rmu_output" "$_rmu_artifact" || return 1
    _rmu_path=$_rmu_output/$_rmu_artifact
    [ ! -L "$_rmu_path" ] || rm -f -- "$_rmu_path"
}

prune_artifact_generations() {
    _rmu_output=$1
    _rmu_current=$2
    _rmu_previous=$3
    for _rmu_candidate in "$_rmu_output"/.rmu-generation.*; do
        [ -e "$_rmu_candidate" ] || continue
        _rmu_name=${_rmu_candidate##*/}
        case $_rmu_name in
            .rmu-generation.*) ;;
            *) echo "refusing an unexpected generation path: $_rmu_candidate" >&2; return 1 ;;
        esac
        if [ -L "$_rmu_candidate" ] || [ ! -d "$_rmu_candidate" ]; then
            echo "refusing an unsafe generation path: $_rmu_candidate" >&2
            return 1
        fi
        if [ "$_rmu_name" != "$_rmu_current" ] && [ "$_rmu_name" != "$_rmu_previous" ]; then
            rm -rf -- "$_rmu_candidate"
        fi
    done
}
