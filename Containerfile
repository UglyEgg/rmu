ARG RUST_IMAGE=docker.io/library/rust@sha256:7bae7c67364dad5ebbd4060923b34d734fbed66d7c1cf3af72aa2f062af93eb6
FROM ${RUST_IMAGE} AS builder

ARG RUST_IMAGE
ARG RUSTC_VERSION
ARG MUSL_VERSION=1.2.5-r11
ARG BINUTILS_VERSION=2.43.1-r3
ARG COREUTILS_VERSION=9.5-r2
ARG FILE_VERSION=5.46-r2
RUN apk add --no-cache \
        "acl-libs=2.3.2-r1" \
        "binutils=${BINUTILS_VERSION}" \
        "coreutils=${COREUTILS_VERSION}" \
        "coreutils-env=9.5-r2" \
        "coreutils-fmt=9.5-r2" \
        "coreutils-sha512sum=9.5-r2" \
        "file=${FILE_VERSION}" \
        "libattr=2.5.2-r2" \
        "libmagic=5.46-r2" \
        "skalibs-libs=2.14.3.0-r0" \
        "utmps-libs=0.1.2.3-r2" \
    && adduser -D -u 1000 builder \
    && mkdir -p /src /artifacts \
    && chown builder:builder /src /artifacts \
    && test -n "$RUSTC_VERSION" \
    && test "$(rustc --version | awk '{print $2}')" = "$RUSTC_VERSION" \
    && apk info -e "musl=$MUSL_VERSION"

USER builder
WORKDIR /src
COPY --chown=builder:builder . .

ENV CARGO_INCREMENTAL=0
RUN tools/write-source-manifest.sh > /artifacts/SOURCE-SHA256SUMS \
    && tools/write-runtime-manifest.sh > /artifacts/RUNTIME-SHA256SUMS
RUN source_manifest_sha256=$(sha256sum /artifacts/SOURCE-SHA256SUMS | cut -d' ' -f1) \
    && runtime_manifest_sha256=$(sha256sum /artifacts/RUNTIME-SHA256SUMS | cut -d' ' -f1) \
    && export RMU_SOURCE_MANIFEST_SHA256="$source_manifest_sha256" \
    && export RMU_RUNTIME_MANIFEST_SHA256="$runtime_manifest_sha256" \
    && cargo test --all-targets --release --locked --offline \
    && cargo build --release --locked --offline \
    && cp target/release/rmu target/release/rmuctl /artifacts/

RUN tools/write-source-manifest.sh > /tmp/SOURCE-SHA256SUMS.after \
    && tools/write-runtime-manifest.sh > /tmp/RUNTIME-SHA256SUMS.after \
    && cmp /artifacts/SOURCE-SHA256SUMS /tmp/SOURCE-SHA256SUMS.after \
    && cmp /artifacts/RUNTIME-SHA256SUMS /tmp/RUNTIME-SHA256SUMS.after

RUN set -eu; \
    for binary in /artifacts/rmu /artifacts/rmuctl; do \
        file "$binary" | grep -Eq 'statically linked|static-pie linked'; \
        if readelf -l "$binary" | grep -q INTERP; then \
            echo "$binary unexpectedly contains an ELF interpreter" >&2; \
            exit 1; \
        fi; \
        if readelf -d "$binary" 2>/dev/null | grep -q NEEDED; then \
            echo "$binary unexpectedly needs a shared library" >&2; \
            exit 1; \
        fi; \
        "$binary" --version; \
    done

RUN { \
        echo "base-image=$RUST_IMAGE"; \
        printf 'alpine='; cat /etc/alpine-release; \
        printf 'architecture='; uname -m; \
        printf 'rmu-version='; /artifacts/rmu --version; \
        printf 'rmuctl-version='; /artifacts/rmuctl --version; \
        rustc --version --verbose; \
        cargo --version; \
        printf 'source-manifest-sha256='; \
        sha256sum /artifacts/SOURCE-SHA256SUMS | cut -d' ' -f1; \
        printf 'runtime-manifest-sha256='; \
        sha256sum /artifacts/RUNTIME-SHA256SUMS | cut -d' ' -f1; \
        echo '[apk-packages]'; \
        apk info -v | sort; \
    } > /artifacts/BUILD-INFO.txt \
    && cd /artifacts \
    && sha256sum rmu rmuctl BUILD-INFO.txt SOURCE-SHA256SUMS RUNTIME-SHA256SUMS > SHA256SUMS

RUN tools/write-source-manifest.sh > /tmp/SOURCE-SHA256SUMS.final \
    && tools/write-runtime-manifest.sh > /tmp/RUNTIME-SHA256SUMS.final \
    && cmp /artifacts/SOURCE-SHA256SUMS /tmp/SOURCE-SHA256SUMS.final \
    && cmp /artifacts/RUNTIME-SHA256SUMS /tmp/RUNTIME-SHA256SUMS.final

FROM scratch AS artifact
COPY --from=builder /artifacts /artifacts
ENTRYPOINT ["/artifacts/rmu"]
CMD ["--version"]
