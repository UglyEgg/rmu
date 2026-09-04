#!/bin/sh

set -eu

usage() {
    cat <<'EOF'
Usage: tools/test-packages-container.sh [--engine podman|docker] [--packages-dir DIR]

Install, exercise, and uninstall an RMU RPM and DEB in clean Fedora and Debian
containers. Podman is preferred when both engines are available.

Options:
  --engine ENGINE     use podman or docker instead of automatic detection
  --packages-dir DIR  package artifact directory (default: outputs/packages/<arch>)
  -h, --help          display this help and exit

Environment:
  CONTAINER_ENGINE       same as --engine
  PACKAGE_TEST_FEDORA    digest-pinned Fedora test image
  PACKAGE_TEST_DEBIAN    digest-pinned Debian test image
EOF
}

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH='' cd -- "$script_directory/.." && pwd)
# shellcheck disable=SC1091
. "$script_directory/artifact-identity.sh"
# shellcheck disable=SC1091
. "$script_directory/container-engine.sh"
invocation_directory=$(pwd)
engine=${CONTAINER_ENGINE:-}
packages_directory=
fedora_image=${PACKAGE_TEST_FEDORA:-registry.fedoraproject.org/fedora@sha256:73f8f084d8328edd72d05d86d192234c346678a491ed3f10595ead0e910f394a}
debian_image=${PACKAGE_TEST_DEBIAN:-docker.io/library/debian@sha256:f324c7ff54321e8d9c588493a20244965938ce0aa50bbd1022d38010e9ffc4b1}

while [ "$#" -gt 0 ]; do
    case $1 in
        --engine)
            [ "$#" -ge 2 ] || { echo "--engine requires a value" >&2; exit 2; }
            engine=$2
            shift 2
            ;;
        --packages-dir)
            [ "$#" -ge 2 ] || { echo "--packages-dir requires a value" >&2; exit 2; }
            packages_directory=$2
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

engine=$(select_container_engine "$engine")

for image in "$fedora_image" "$debian_image"; do
    require_pinned_image "$image" "package test image"
done

if [ -z "$packages_directory" ]; then
    architecture=$(uname -m)
    packages_directory=$repository/outputs/packages/$architecture
else
    case $packages_directory in
        /*) ;;
        *) packages_directory=$invocation_directory/$packages_directory ;;
    esac
fi
[ -d "$packages_directory" ] \
    || { echo "package artifact directory is unavailable: $packages_directory" >&2; exit 1; }
[ ! -L "$packages_directory" ] \
    || { echo "refusing a symlink package directory: $packages_directory" >&2; exit 1; }
if [ -L "$packages_directory/current" ]; then
    current_target=$(readlink "$packages_directory/current")
    case $current_target in
        .rmu-generation.*)
            case $current_target in
                */*) echo "unsafe package current link: $current_target" >&2; exit 1 ;;
                *) packages_directory=$packages_directory/$current_target ;;
            esac
            ;;
        *) echo "unsafe package current link: $current_target" >&2; exit 1 ;;
    esac
fi

build_info=$packages_directory/PACKAGE-BUILD-INFO.txt
[ -f "$build_info" ] && [ ! -L "$build_info" ] \
    || { echo "missing or unsafe package build information" >&2; exit 1; }
architecture=$(sed -n 's/^architecture=//p' "$build_info")
version=$(sed -n 's/^version=//p' "$build_info")
case $architecture in
    x86_64)
        rpm_file=rmu-$version-1.x86_64.rpm
        deb_file=rmu_${version}-1_amd64.deb
        ;;
    aarch64)
        rpm_file=rmu-$version-1.aarch64.rpm
        deb_file=rmu_${version}-1_arm64.deb
        ;;
    *) echo "unsupported package architecture: $architecture" >&2; exit 1 ;;
esac
for package in "$rpm_file" "$deb_file"; do
    [ -f "$packages_directory/$package" ] && [ ! -L "$packages_directory/$package" ] \
        || { echo "missing or unsafe package artifact: $package" >&2; exit 1; }
done
(cd "$packages_directory" && sha256sum -c SHA256SUMS)

# The commands use only disposable paths inside short-lived containers. The
# package uninstall must not remove the already-created FreeDesktop Trash data.
# shellcheck disable=SC2016
"$engine" run --rm --security-opt label=disable \
    --env "RMU_VERSION=$version" --env "RPM_FILE=$rpm_file" \
    --volume "$packages_directory:/packages:ro" \
    "$fedora_image" sh -eu -c '
        dnf --setopt=tsflags= -q -y install zsh "/packages/$RPM_FILE"
        test "$(rmu --version)" = "rmu $RMU_VERSION"
        test "$(rmuctl --version)" = "rmuctl $RMU_VERSION"
        rmuctl completions zsh rmu > /tmp/generated-rmu-completion
        test "$(sha256sum /tmp/generated-rmu-completion | cut -d " " -f 1)" = \
             "$(sha256sum /usr/share/zsh/site-functions/_rmu | cut -d " " -f 1)"
        zsh -fc '\''autoload -Uz compinit; compinit -d /tmp/rmu-zcompdump; whence -w _rmu; whence -w _rmuctl'\'' \
            > /tmp/rmu-zsh-discovery
        grep -qx "_rmu: function" /tmp/rmu-zsh-discovery
        grep -qx "_rmuctl: function" /tmp/rmu-zsh-discovery
        rpm -ql rmu | grep -qx /usr/share/licenses/rmu
        gzip -t /usr/share/man/man1/rmu.1.gz /usr/share/man/man1/rmuctl.1.gz
        mkdir -p /tmp/rmu-smoke/home /tmp/rmu-smoke/data
        : > /tmp/rmu-smoke/operand
        HOME=/tmp/rmu-smoke/home XDG_DATA_HOME=/tmp/rmu-smoke/data rmu /tmp/rmu-smoke/operand
        test ! -e /tmp/rmu-smoke/operand
        HOME=/tmp/rmu-smoke/home XDG_DATA_HOME=/tmp/rmu-smoke/data rmuctl list >/dev/null
        dnf -q -y remove rmu
        test ! -e /usr/bin/rmu
        test ! -e /usr/bin/rmuctl
        test ! -e /usr/share/licenses/rmu
        test -d /tmp/rmu-smoke/data/Trash
    '

# shellcheck disable=SC2016
"$engine" run --rm --security-opt label=disable \
    --env DEBIAN_FRONTEND=noninteractive \
    --env "RMU_VERSION=$version" --env "DEB_FILE=$deb_file" \
    --volume "$packages_directory:/packages:ro" \
    "$debian_image" sh -eu -c '
        apt-get -qq update
        apt-get -qq -y install zsh "/packages/$DEB_FILE"
        test "$(rmu --version)" = "rmu $RMU_VERSION"
        test "$(rmuctl --version)" = "rmuctl $RMU_VERSION"
        rmuctl completions zsh rmu > /tmp/generated-rmu-completion
        test "$(sha256sum /tmp/generated-rmu-completion | cut -d " " -f 1)" = \
             "$(sha256sum /usr/share/zsh/vendor-completions/_rmu | cut -d " " -f 1)"
        zsh -fc '\''autoload -Uz compinit; compinit -d /tmp/rmu-zcompdump; whence -w _rmu; whence -w _rmuctl'\'' \
            > /tmp/rmu-zsh-discovery
        grep -qx "_rmu: function" /tmp/rmu-zsh-discovery
        grep -qx "_rmuctl: function" /tmp/rmu-zsh-discovery
        gzip -t /usr/share/man/man1/rmu.1.gz /usr/share/man/man1/rmuctl.1.gz
        mkdir -p /tmp/rmu-smoke/home /tmp/rmu-smoke/data
        : > /tmp/rmu-smoke/operand
        HOME=/tmp/rmu-smoke/home XDG_DATA_HOME=/tmp/rmu-smoke/data rmu /tmp/rmu-smoke/operand
        test ! -e /tmp/rmu-smoke/operand
        HOME=/tmp/rmu-smoke/home XDG_DATA_HOME=/tmp/rmu-smoke/data rmuctl list >/dev/null
        apt-get -qq -y remove rmu
        test ! -e /usr/bin/rmu
        test ! -e /usr/bin/rmuctl
        test ! -e /usr/share/zsh/vendor-completions/_rmu
        test ! -e /usr/share/zsh/vendor-completions/_rmuctl
        test -d /tmp/rmu-smoke/data/Trash
    '

echo "Fedora and Debian package install smoke tests passed"
