FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive
ARG RUST_VERSION=stable
ARG BAZELISK_VERSION=1.29.0
ARG BUN_VERSION=1.3.14
ARG JUST_VERSION=1.56.0
ARG NEXT_VERSION=0.9.140
ARG INSTA_VERSION=1.48.0
ARG CODEWITH_REPO=https://github.com/hasna/codewith.git
ARG CODEWITH_REF=main

ENV CARGO_HOME=/opt/rust/cargo
ENV RUSTUP_HOME=/opt/rust/rustup
ENV PATH=/opt/rust/cargo/bin:/root/.bun/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin
ENV CARGO_TARGET_DIR=/opt/codewith-target
ENV BAZELISK_HOME=/opt/bazelisk-cache
ENV BAZEL_CXXOPTS=-std=c++20
ENV BAZEL_CXXOPTS_HOST=-std=c++20

RUN /bin/bash -lc 'set -euo pipefail \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        bash-completion \
        build-essential \
        ca-certificates \
        clang \
        curl \
        g++ \
        gcc \
        git \
        gnupg \
        jq \
        libfontconfig1-dev \
        libfreetype6-dev \
        libssl-dev \
        pkg-config \
        protobuf-compiler \
        python3 \
        python3-pip \
        python3-pytest \
        python3-venv \
        ripgrep \
        unzip \
        xz-utils \
        zip \
        zstd \
    && rm -rf /var/lib/apt/lists/*'

RUN /bin/bash -lc 'set -euo pipefail \
    && curl -fsSL https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain "${RUST_VERSION}" --profile minimal \
    && rustup component add clippy rustfmt'

RUN /bin/bash -lc 'set -euo pipefail \
    && arch="$(uname -m)" \
    && case "${arch}" in \
        x86_64) bazelisk_arch=amd64 ;; \
        aarch64|arm64) bazelisk_arch=arm64 ;; \
        *) echo "unsupported arch: ${arch}" >&2; exit 1 ;; \
    esac \
    && curl -fsSL \
        "https://github.com/bazelbuild/bazelisk/releases/download/v${BAZELISK_VERSION}/bazelisk-linux-${bazelisk_arch}" \
        -o /usr/local/bin/bazelisk \
    && chmod +x /usr/local/bin/bazelisk \
    && ln -s /usr/local/bin/bazelisk /usr/local/bin/bazel'

RUN /bin/bash -lc 'set -euo pipefail \
    && curl -fsSL https://bun.sh/install \
        | bash -s "bun-v${BUN_VERSION}" \
    && ln -s /root/.bun/bin/bun /usr/local/bin/bun \
    && ln -s /root/.bun/bin/bunx /usr/local/bin/bunx'

RUN /bin/bash -lc 'set -euo pipefail \
    && cargo install --locked just --version "${JUST_VERSION}" \
    && cargo install --locked cargo-nextest --version "${NEXT_VERSION}" \
    && cargo install --locked cargo-insta --version "${INSTA_VERSION}"'

WORKDIR /opt/codewith

RUN /bin/bash -lc 'set -euo pipefail \
    && git clone --filter=blob:none "${CODEWITH_REPO}" /opt/codewith \
    && git fetch --depth=1 origin "${CODEWITH_REF}" \
    && git checkout FETCH_HEAD'

RUN /bin/bash -lc 'set -euo pipefail \
    && cd /opt/codewith/codex-rs \
    && cargo fetch --locked \
    && bazelisk version \
    && bazelisk fetch //codex-rs/... \
    && just --list >/opt/codewith-just-targets.txt'

RUN /bin/bash -lc 'set -euo pipefail \
    && mkdir -p /workspace /cache/codewith \
    && chmod -R a+rX /opt/codewith /opt/rust /opt/bazelisk-cache /root/.cache \
    && chmod -R a+rwX /workspace /cache/codewith /opt/codewith-target'

WORKDIR /workspace

CMD ["/bin/bash", "-l"]
