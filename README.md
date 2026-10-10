# cargo-xc

`cargo-xc` is a Cargo subcommand that cross-compiles Rust from ARM64 (Apple Silicon or ARM64 Linux) to x86-64 GNU/Linux using native ARM64-hosted GNU cross tools. It doesn't use emulation, and it never runs the target binaries during a build.

## Install

```console
cargo install cargo-xc
```

Docker is required on macOS.

## Usage

On macOS, build inside a native `linux/arm64` container:

```console
cargo xc docker build --release --target x86_64-unknown-linux-gnu.2.36
```

On ARM64 Linux with Debian's `crossbuild-essential-amd64`, `g++-x86-64-linux-gnu` and `binutils-x86-64-linux-gnu` installed, build directly:

```console
cargo xc build --release --target x86_64-unknown-linux-gnu.2.36
```

The output is written to `target/x86_64-unknown-linux-gnu/release/`. After each build, `cargo-xc` checks that every new binary is x86-64 ELF and imports no glibc symbol newer than the requested baseline.

`build`, `check`, `rustc`, `test`, `bench`, `run`, `clippy` and `doc` are supported, and other Cargo options are passed through. `test` and `bench` compile with `--no-run` unless `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER` is set. `run` requires that runner.

## Targets

Only `x86_64-unknown-linux-gnu.2.36` is supported. The suffix is the newest glibc the binary may require, so it runs on glibc 2.36 or later (Debian 12, Ubuntu 23.04+). Plain `x86_64-unknown-linux-gnu` means 2.36. Any other suffix is rejected.

C and C++ dependencies built with `cc-rs`, CMake, pkg-config or bindgen are configured for the target automatically.

## Private Git dependencies

`cargo xc docker` runs `cargo fetch` on the host before starting the container, using your normal Git credentials (SSH agent or Git credential helper). The container gets the downloaded sources but no credentials.

## Using the builder image in a Dockerfile

The builder is published as `ghcr.io/stevepryde/cargo-xc:<version>` and can be used as a normal BuildKit build stage:

```dockerfile
FROM --platform=linux/arm64 ghcr.io/stevepryde/cargo-xc:0.1.0 AS builder
WORKDIR /src
COPY . .
RUN cargo xc build --locked --release --target x86_64-unknown-linux-gnu.2.36

FROM --platform=linux/amd64 debian:bookworm-slim
COPY --from=builder /src/target/x86_64-unknown-linux-gnu/release/my-app /usr/local/bin/my-app
CMD ["my-app"]
```

To build and push the builder image yourself, run `scripts/publish-builder-image.sh [IMAGE_REF]`. Set `DOCKER_OUTPUT=--load` to load it locally instead of pushing.

## Commands

```console
cargo xc targets                 # list supported targets
cargo xc doctor --verbose        # diagnose the toolchain
cargo xc toolchain install       # register the installed cross tools
cargo xc toolchain list|inspect|remove
cargo xc init                    # create cargo-xc.toml
cargo xc lock [--update]         # pin toolchain versions and checksums in cargo-xc.lock
cargo xc verify [FILE]           # check the lock file, or check a built binary
cargo xc cache clean
```

Global options: `--trace` prints the commands that `cargo-xc` runs, and `--offline`/`--frozen` stop it downloading anything (they're also passed to Cargo). Set `CARGO_XC_CACHE_DIR` to move the cache.

## License

MIT
