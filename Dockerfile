# OpenTrack: one image, one binary; the role is the command (serve, writer, all, ...).
#
# Base images are pinned by digest (docs/security/supply-chain.md says how to
# move them). FIPS 140-3 (docs/security/fips.md): the binary's cryptography
# is the AWS-LC FIPS module (built here, which needs Go and CMake), and
# OpenSSL, which SAML signatures go through, runs with only its validated
# 3.0.9 FIPS provider.
#
# The runtime is distroless Debian 12 (gcr.io/distroless/cc-debian12): glibc,
# OpenSSL 3, CA certificates and nothing else: no shell, no package manager.
# The `runtime-libs` stage adds only the shared libraries the binary loads
# (SAML's libxmlsec1 and libxml2 and what they need), each with its Debian
# package record so image scanners still see them (docs/security/hardening.md).

FROM node:22-bookworm-slim@sha256:43ac6c60b8f89723f746e8a92ce91abd5017e627ce1ddfe4238355d3a30b772c AS ui
WORKDIR /ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY ui/ ./
# The Help page bundles the guides (ui/src/pages/help imports ../docs/guides).
COPY docs/guides /docs/guides
RUN npm run build

# The OpenSSL FIPS provider, built from the release validated under CMVP
# certificate #4282, exactly as its security policy says (enable-fips,
# install_fips). Debian's own OpenSSL 3.0 library loads it.
FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251 AS openssl-fips
ARG OPENSSL_FIPS_VERSION=3.0.9
ARG OPENSSL_FIPS_SHA256=eb1ab04781474360f77c318ab89d8c5a03abc38e63d65a603cabbf1b00a1dc90
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential perl curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
RUN curl -fsSLo openssl.tar.gz \
        "https://github.com/openssl/openssl/releases/download/openssl-${OPENSSL_FIPS_VERSION}/openssl-${OPENSSL_FIPS_VERSION}.tar.gz" \
    && echo "${OPENSSL_FIPS_SHA256}  openssl.tar.gz" | sha256sum -c - \
    && tar xzf openssl.tar.gz \
    && cd "openssl-${OPENSSL_FIPS_VERSION}" \
    && ./Configure enable-fips --prefix=/opt/ossl --openssldir=/opt/ossl/ssl \
    && make -j"$(nproc)" \
    && make install_fips \
    && mkdir -p /out \
    && cp /opt/ossl/lib*/ossl-modules/fips.so /out/fips.so \
    && cp /opt/ossl/ssl/fipsmodule.cnf /out/fipsmodule.cnf

# Only the provider and its configuration: `--target openssl-fips-out
# --output type=local,dest=DIR` gives the release signing script
# (scripts/release/sign-image) the same validated module.
FROM scratch AS openssl-fips-out
COPY --from=openssl-fips /out/ /

FROM rust:1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS server
# SAML single sign-on signs and checks XML with libxmlsec1; the AWS-LC FIPS
# module builds with CMake and Go.
ARG GO_VERSION=1.27.1
ARG GO_SHA256_AMD64=63d339f0da5ab53635a56f2490a7984dfe12dfcff22ad749f63edaf590168445
ARG GO_SHA256_ARM64=3450b45a3f9ee8568792736a5c5e70a1f2e9b36c35a8f74958c03e51d7d92bec
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends libxmlsec1-dev libxml2-dev pkg-config clang libclang-dev cmake \
    && rm -rf /var/lib/apt/lists/*
RUN arch="${TARGETARCH:-$(dpkg --print-architecture)}" \
    && case "$arch" in amd64) sum="$GO_SHA256_AMD64";; arm64) sum="$GO_SHA256_ARM64";; *) echo "no Go for $arch"; exit 1;; esac \
    && curl -fsSLo go.tar.gz "https://go.dev/dl/go${GO_VERSION}.linux-${arch}.tar.gz" \
    && echo "${sum}  go.tar.gz" | sha256sum -c - \
    && tar -C /usr/local -xzf go.tar.gz && rm go.tar.gz
ENV PATH=/usr/local/go/bin:$PATH
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY third_party ./third_party
COPY crates ./crates
# The plugin interface the WebAssembly host is generated from.
COPY wit ./wit
RUN cargo build --release --locked --bin opentrack

# The runtime base: distroless Debian 12, the non-root variant (OpenTrack
# runs as uid 1000, below, so volumes written by earlier images still fit).
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS runtime-base

# Collects what the runtime base lacks, into /out: the shared libraries the
# binary needs (ldd, which follows libxmlsec1-openssl too; Debian builds
# xmlsec with its OpenSSL engine linked, not loaded by name, and it is listed
# explicitly anyway), their dpkg records and copyright files, and the
# opentrack user.
FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251 AS runtime-libs
RUN apt-get update \
    && apt-get install -y --no-install-recommends libxmlsec1 libxmlsec1-openssl libxml2 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=runtime-base / /base/
COPY --from=server /src/target/release/opentrack /usr/local/bin/opentrack
RUN set -eu; \
    engine="$(dpkg -L libxmlsec1-openssl | grep '/libxmlsec1-openssl\.so\.1$')"; \
    mkdir -p /out/etc /out/var/lib/dpkg/status.d; \
    for lib in $(ldd /usr/local/bin/opentrack "$engine" | awk '$2 == "=>" && $3 ~ /^\// { print $3 }' | sort -u); do \
        name="$(basename "$lib")"; triplet="$(basename "$(dirname "$lib")")"; \
        if [ -e "/base/lib/$triplet/$name" ] || [ -e "/base/usr/lib/$triplet/$name" ]; then continue; fi; \
        mkdir -p "/out/usr/lib/$triplet"; \
        cp -L "$lib" "/out/usr/lib/$triplet/$name"; \
        pkg="$(dpkg-query -S "*/$(basename "$(realpath "$lib")")" | head -n1 | cut -d: -f1)"; \
        dpkg-query -s "$pkg" > "/out/var/lib/dpkg/status.d/$pkg"; \
        if [ -f "/usr/share/doc/$pkg/copyright" ]; then \
            mkdir -p "/out/usr/share/doc/$pkg" && cp "/usr/share/doc/$pkg/copyright" "/out/usr/share/doc/$pkg/"; \
        fi; \
        echo "runtime library: $name ($pkg)"; \
    done; \
    cp /base/etc/passwd /out/etc/passwd; cp /base/etc/group /out/etc/group; \
    echo 'opentrack:x:1000:1000:opentrack:/home/opentrack:/sbin/nologin' >> /out/etc/passwd; \
    echo 'opentrack:x:1000:' >> /out/etc/group; \
    mkdir -p /owned/data /owned/home/opentrack

FROM runtime-base
COPY --from=runtime-libs /out/ /
COPY --from=runtime-libs --chown=1000:1000 /owned/ /
COPY --from=openssl-fips /out/fips.so /opt/opentrack/ossl-modules/fips.so
COPY --from=openssl-fips /out/fipsmodule.cnf /etc/opentrack/fipsmodule.cnf
COPY docker/openssl-fips.cnf /etc/opentrack/openssl-fips.cnf
COPY --from=server /src/target/release/opentrack /usr/local/bin/opentrack
COPY --from=ui /ui/dist /opt/opentrack/ui
COPY profiles /opt/opentrack/profiles
COPY LICENSE /opt/opentrack/LICENSE
ENV OT_UI_DIR=/opt/opentrack/ui \
    OT_PROFILES_DIR=/opt/opentrack/profiles/trackers \
    OT_SQLITE_PATH=/data/opentrack.db \
    OPENSSL_CONF=/etc/opentrack/openssl-fips.cnf \
    OPENSSL_MODULES=/opt/opentrack/ossl-modules \
    HOME=/home/opentrack
USER 1000:1000
WORKDIR /home/opentrack
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --retries=3 CMD ["opentrack", "health"]
VOLUME /data
EXPOSE 8090
ENTRYPOINT ["opentrack"]
CMD ["all"]
