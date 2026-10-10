# syntax=docker/dockerfile:1.7
ARG RUST_IMAGE="rust:1.96-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663"
FROM --platform=linux/arm64 ${RUST_IMAGE} AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --locked --release

FROM --platform=linux/arm64 ${RUST_IMAGE}

RUN test "$(dpkg --print-architecture)" = arm64 \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
       binutils-x86-64-linux-gnu \
       ca-certificates \
       clang \
       cmake \
       crossbuild-essential-amd64 \
       g++-x86-64-linux-gnu \
       git \
       libclang-dev \
       ninja-build \
       openssh-client \
       pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-unknown-linux-gnu

COPY --from=builder /src/target/release/cargo-xc /usr/local/bin/cargo-xc
COPY docker/entrypoint.sh /usr/local/bin/cargo-xc-container

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    PATH=/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/cargo-xc-container"]
CMD ["cargo", "xc", "--help"]
