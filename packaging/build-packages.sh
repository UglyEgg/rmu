#!/bin/sh

set -eu

: "${RMU_VERSION:?RMU_VERSION is required}"
: "${RMU_RELEASE:?RMU_RELEASE is required}"
: "${RMU_ARCHITECTURE:?RMU_ARCHITECTURE is required}"
: "${RMU_MAINTAINER:?RMU_MAINTAINER is required}"
: "${RUSTC_VERSION:?RUSTC_VERSION is required}"
: "${SOURCE_DATE_EPOCH:?SOURCE_DATE_EPOCH is required}"

case $RMU_VERSION in
    *[!0-9.]*|.*|*.) echo "invalid RMU_VERSION: $RMU_VERSION" >&2; exit 2 ;;
esac
case $RMU_RELEASE in
    *[!0-9]*|"") echo "invalid RMU_RELEASE: $RMU_RELEASE" >&2; exit 2 ;;
esac
case $RUSTC_VERSION in
    *[!0-9.]*|.*|*.) echo "invalid RUSTC_VERSION: $RUSTC_VERSION" >&2; exit 2 ;;
esac

rust_notice_prefix=Rust-$RUSTC_VERSION
for notice in COPYRIGHT COPYRIGHT-library.html LICENSE-MIT; do
    [ -f "/input/licenses/$rust_notice_prefix-$notice" ] \
        || { echo "missing Rust notice for toolchain $RUSTC_VERSION: $notice" >&2; exit 1; }
done

case $RMU_ARCHITECTURE in
    x86_64)
        rpm_arch=x86_64
        deb_arch=amd64
        ;;
    aarch64)
        rpm_arch=aarch64
        deb_arch=arm64
        ;;
    *)
        echo "unsupported package architecture: $RMU_ARCHITECTURE" >&2
        exit 2
        ;;
esac

assert_binary_version() {
    binary=$1
    command_name=$2
    actual=$("$binary" --version) \
        || { echo "cannot execute $binary" >&2; exit 1; }
    expected="$command_name $RMU_VERSION"
    if [ "$actual" != "$expected" ]; then
        echo "$binary reports '$actual'; expected '$expected'" >&2
        exit 1
    fi
}

assert_binary_version /input/rmu rmu
assert_binary_version /input/rmuctl rmuctl

artifact_directory=/artifacts
build_directory=/build
deb_name=rmu_${RMU_VERSION}-${RMU_RELEASE}_${deb_arch}
deb_root=$build_directory/$deb_name
rpm_top=$build_directory/rpm

mkdir -p \
    "$artifact_directory" \
    "$deb_root/DEBIAN" \
    "$rpm_top/BUILD" \
    "$rpm_top/BUILDROOT" \
    "$rpm_top/RPMS" \
    "$rpm_top/SOURCES" \
    "$rpm_top/SPECS" \
    "$rpm_top/SRPMS"

install -D -m 0755 /input/rmu "$deb_root/usr/bin/rmu"
install -D -m 0755 /input/rmuctl "$deb_root/usr/bin/rmuctl"
install -D -m 0644 /input/man/rmu.1 "$deb_root/usr/share/man/man1/rmu.1"
install -D -m 0644 /input/man/rmuctl.1 "$deb_root/usr/share/man/man1/rmuctl.1"
gzip -n -9 "$deb_root/usr/share/man/man1/rmu.1"
gzip -n -9 "$deb_root/usr/share/man/man1/rmuctl.1"
install -D -m 0644 /input/completions/rmu.bash "$deb_root/usr/share/bash-completion/completions/rmu"
install -D -m 0644 /input/completions/rmuctl.bash "$deb_root/usr/share/bash-completion/completions/rmuctl"
install -D -m 0644 /input/completions/_rmu "$deb_root/usr/share/zsh/vendor-completions/_rmu"
install -D -m 0644 /input/completions/_rmuctl "$deb_root/usr/share/zsh/vendor-completions/_rmuctl"
install -D -m 0644 /input/completions/rmu.fish "$deb_root/usr/share/fish/vendor_completions.d/rmu.fish"
install -D -m 0644 /input/completions/rmuctl.fish "$deb_root/usr/share/fish/vendor_completions.d/rmuctl.fish"
install -D -m 0644 /input/LICENSE "$deb_root/usr/share/doc/rmu/copyright"
install -D -m 0644 "/input/licenses/$rust_notice_prefix-COPYRIGHT" "$deb_root/usr/share/doc/rmu/licenses/$rust_notice_prefix-COPYRIGHT"
install -D -m 0644 "/input/licenses/$rust_notice_prefix-COPYRIGHT-library.html" "$deb_root/usr/share/doc/rmu/licenses/$rust_notice_prefix-COPYRIGHT-library.html"
install -D -m 0644 "/input/licenses/$rust_notice_prefix-LICENSE-MIT" "$deb_root/usr/share/doc/rmu/licenses/$rust_notice_prefix-LICENSE-MIT"
install -D -m 0644 /input/licenses/musl-1.2.5-COPYRIGHT "$deb_root/usr/share/doc/rmu/licenses/musl-1.2.5-COPYRIGHT"
install -D -m 0644 /input/README.md "$deb_root/usr/share/doc/rmu/README.md"
install -D -m 0644 /input/UPSTREAM-NOTICES.md "$deb_root/usr/share/doc/rmu/UPSTREAM-NOTICES.md"
gzip -n -9 -c /input/CHANGELOG.md > "$deb_root/usr/share/doc/rmu/changelog.gz"

installed_size=$(du -sk "$deb_root/usr" | awk '{print $1}')
{
    echo "Package: rmu"
    echo "Version: $RMU_VERSION-$RMU_RELEASE"
    echo "Section: utils"
    echo "Priority: optional"
    echo "Architecture: $deb_arch"
    echo "Maintainer: $RMU_MAINTAINER"
    echo "Homepage: https://github.com/UglyEgg/rmu"
    echo "Depends: coreutils"
    echo "Installed-Size: $installed_size"
    echo "Description: Recoverable Move Utility with a GNU rm-compatible interface"
    echo " RMU moves files into the FreeDesktop Trash by default. In explicit"
    echo " delegate mode, it hands the invocation to GNU rm. rmuctl manages"
    echo " the same Trash without a daemon or private index and can print"
    echo " shell completion definitions for custom configurations."
} > "$deb_root/DEBIAN/control"

(cd "$deb_root" && find usr -type f -print0 | LC_ALL=C sort -z | xargs -0 md5sum) \
    > "$deb_root/DEBIAN/md5sums"
find "$deb_root" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
dpkg-deb --root-owner-group -Zgzip -z9 --build "$deb_root" \
    "$artifact_directory/${deb_name}.deb"

SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH \
rpmbuild -bb /input/packaging/rmu.spec \
    --target "$rpm_arch" \
    --define "_topdir $rpm_top" \
    --define "_buildhost rmu-build.invalid" \
    --define "use_source_date_epoch_as_buildtime 1" \
    --define "clamp_mtime_to_source_date_epoch 1" \
    --define "rmu_version $RMU_VERSION" \
    --define "rmu_release $RMU_RELEASE" \
    --define "rmu_arch $rpm_arch" \
    --define "rust_version $RUSTC_VERSION"

rpm_path=$(find "$rpm_top/RPMS" -type f -name '*.rpm' -print)
case $rpm_path in
    ""|*'
'*) echo "expected exactly one RPM artifact" >&2; exit 1 ;;
esac
cp "$rpm_path" "$artifact_directory/"

rpm_file=${rpm_path##*/}
deb_file=${deb_name}.deb
rpm -qpi "$artifact_directory/$rpm_file"
rpm -qpl "$artifact_directory/$rpm_file"
dpkg-deb --info "$artifact_directory/$deb_file"
dpkg-deb --contents "$artifact_directory/$deb_file"
rpm -qp --requires "$artifact_directory/$rpm_file" | grep -qx coreutils \
    || { echo "RPM does not require coreutils" >&2; exit 1; }
[ "$(dpkg-deb -f "$artifact_directory/$deb_file" Depends)" = coreutils ] \
    || { echo "DEB does not depend on coreutils" >&2; exit 1; }
[ "$(rpm -qp --qf '%{URL}\n' "$artifact_directory/$rpm_file")" = https://github.com/UglyEgg/rmu ] \
    || { echo "RPM has an unexpected project URL" >&2; exit 1; }
[ "$(dpkg-deb -f "$artifact_directory/$deb_file" Homepage)" = https://github.com/UglyEgg/rmu ] \
    || { echo "DEB has an unexpected project homepage" >&2; exit 1; }

mkdir -p "$build_directory/rpm-extract" "$build_directory/deb-extract"
(cd "$build_directory/rpm-extract" && rpm2cpio "$artifact_directory/$rpm_file" | cpio -id)
dpkg-deb --extract "$artifact_directory/$deb_file" "$build_directory/deb-extract"
cmp /input/rmu "$build_directory/rpm-extract/usr/bin/rmu"
cmp /input/rmuctl "$build_directory/rpm-extract/usr/bin/rmuctl"
cmp /input/rmu "$build_directory/deb-extract/usr/bin/rmu"
cmp /input/rmuctl "$build_directory/deb-extract/usr/bin/rmuctl"
for package_root in "$build_directory/rpm-extract" "$build_directory/deb-extract"; do
    cmp /input/completions/rmu.bash "$package_root/usr/share/bash-completion/completions/rmu"
    cmp /input/completions/rmuctl.bash "$package_root/usr/share/bash-completion/completions/rmuctl"
    cmp /input/completions/rmu.fish "$package_root/usr/share/fish/vendor_completions.d/rmu.fish"
    cmp /input/completions/rmuctl.fish "$package_root/usr/share/fish/vendor_completions.d/rmuctl.fish"
    gzip -dc "$package_root/usr/share/man/man1/rmu.1.gz" | cmp /input/man/rmu.1 -
    gzip -dc "$package_root/usr/share/man/man1/rmuctl.1.gz" | cmp /input/man/rmuctl.1 -
done
cmp /input/completions/_rmu "$build_directory/rpm-extract/usr/share/zsh/site-functions/_rmu"
cmp /input/completions/_rmuctl "$build_directory/rpm-extract/usr/share/zsh/site-functions/_rmuctl"
cmp /input/completions/_rmu "$build_directory/deb-extract/usr/share/zsh/vendor-completions/_rmu"
cmp /input/completions/_rmuctl "$build_directory/deb-extract/usr/share/zsh/vendor-completions/_rmuctl"
for notice in "$rust_notice_prefix-COPYRIGHT" \
    "$rust_notice_prefix-COPYRIGHT-library.html" \
    "$rust_notice_prefix-LICENSE-MIT" musl-1.2.5-COPYRIGHT; do
    cmp "/input/licenses/$notice" "$build_directory/rpm-extract/usr/share/licenses/rmu/$notice"
    cmp "/input/licenses/$notice" "$build_directory/deb-extract/usr/share/doc/rmu/licenses/$notice"
done
cmp /input/UPSTREAM-NOTICES.md "$build_directory/rpm-extract/usr/share/doc/rmu/UPSTREAM-NOTICES.md"
cmp /input/UPSTREAM-NOTICES.md "$build_directory/deb-extract/usr/share/doc/rmu/UPSTREAM-NOTICES.md"
assert_binary_version "$build_directory/rpm-extract/usr/bin/rmu" rmu
assert_binary_version "$build_directory/rpm-extract/usr/bin/rmuctl" rmuctl
assert_binary_version "$build_directory/deb-extract/usr/bin/rmu" rmu
assert_binary_version "$build_directory/deb-extract/usr/bin/rmuctl" rmuctl

{
    echo "$rpm_file"
    echo "$deb_file"
} > "$artifact_directory/PACKAGE-FILES"

{
    echo "package-image=$PACKAGE_IMAGE"
    echo "architecture=$RMU_ARCHITECTURE"
    echo "rpm-architecture=$rpm_arch"
    echo "deb-architecture=$deb_arch"
    echo "version=$RMU_VERSION"
    echo "release=$RMU_RELEASE"
    echo "rust-version=$RUSTC_VERSION"
    echo "source-date-epoch=$SOURCE_DATE_EPOCH"
    echo "maintainer=$RMU_MAINTAINER"
    rustc --version --verbose
    cargo --version
    rpm --version
    dpkg-deb --version | sed -n '1p'
    echo '[apk-packages]'
    apk info -v | sort
} > "$artifact_directory/PACKAGE-BUILD-INFO.txt"

(cd "$artifact_directory" \
    && sha256sum "$rpm_file" "$deb_file" PACKAGE-BUILD-INFO.txt PACKAGE-FILES > SHA256SUMS)
