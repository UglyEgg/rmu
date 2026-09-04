#!/bin/sh

select_container_engine() {
    _rmu_requested_engine=$1
    case $_rmu_requested_engine in
        "")
            if command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
                printf '%s\n' podman
            elif command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
                printf '%s\n' docker
            else
                echo "neither podman nor docker is installed and operational" >&2
                return 1
            fi
            ;;
        podman|docker)
            if ! command -v "$_rmu_requested_engine" >/dev/null 2>&1; then
                echo "requested container engine is unavailable: $_rmu_requested_engine" >&2
                return 1
            fi
            if ! "$_rmu_requested_engine" info >/dev/null 2>&1; then
                echo "requested container engine is not operational: $_rmu_requested_engine" >&2
                return 1
            fi
            printf '%s\n' "$_rmu_requested_engine"
            ;;
        *)
            echo "unsupported container engine: $_rmu_requested_engine (expected podman or docker)" >&2
            return 2
            ;;
    esac
}
