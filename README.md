# cargo-xc

`cargo-xc` is a Cargo subcommand that builds Rust for x86-64 and ARM64 GNU/Linux from an ARM64 machine (Apple Silicon or ARM64 Linux). It doesn't use emulation, and it never runs the target binaries during a build.

## Install

```console
cargo install cargo-xc
```

Docker is required on macOS. The first `cargo xc docker` run for each target builds a local builder image.

## Targets

| Target | Runs on | Builder |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu.2.36` | x86-64 Linux with glibc 2.36+ (Debian 12, Ubuntu 23.04+) | Debian 12 with GNU cross tools |
| `aarch64-unknown-linux-gnu.2.34` | ARM64 Linux with glibc 2.34+ (Amazon Linux 2023, Debian 12, Ubuntu 22.04+, AWS Lambda) | Amazon Linux 2023, native |

The suffix is the newest glibc the binary may require. A plain triple uses that target's default, and any other suffix is rejected. Run `cargo xc targets` to list them.

## Usage

On macOS, build inside a native `linux/arm64` container:

```console
cargo xc docker build --release --target x86_64-unknown-linux-gnu
cargo xc docker build --release --target aarch64-unknown-linux-gnu
```

On ARM64 Linux, build directly. The x86-64 target needs Debian's `crossbuild-essential-amd64`, `g++-x86-64-linux-gnu` and `binutils-x86-64-linux-gnu`. The ARM64 target needs Amazon Linux 2023 with `gcc`, `gcc-c++` and `binutils`.

```console
cargo xc build --release --target x86_64-unknown-linux-gnu
```

The output is written to `target/<triple>/release/`. After each build, `cargo-xc` checks that every new binary has the target's architecture and dynamic loader, and reports the newest glibc version it imports.

`build`, `check`, `clippy` and `doc` are supported, and other Cargo options are passed through. C and C++ dependencies built with `cc-rs`, CMake, pkg-config or bindgen are configured for the target automatically.

## AWS Lambda

`cargo xc lambda` builds a release binary for the ARM64 `provided.al2023` runtime and packages it as a `bootstrap` zip:

```console
cargo xc docker lambda
cargo xc docker lambda --bin my-function --zip dist/my-function.zip
```

The default package path is `target/lambda/<binary>/bootstrap.zip`.

## Private Git dependencies

`cargo xc docker` runs `cargo fetch` on the host before starting the container, using your normal Git credentials (SSH agent or Git credential helper). The container gets the downloaded sources but no credentials.

## Commands

```console
cargo xc targets                 # list supported targets
cargo xc doctor --verbose        # diagnose the toolchain
cargo xc toolchain install       # register the installed build tools
cargo xc toolchain list|inspect|remove
cargo xc init                    # create cargo-xc.toml
cargo xc lock [--update]         # pin toolchain versions and checksums in cargo-xc.lock
cargo xc verify [FILE]           # check the lock file, or check a built binary
cargo xc cache clean
```

Commands that take a target default to `x86_64-unknown-linux-gnu`; pass `--target` to choose another. Global options: `--trace` prints the commands that `cargo-xc` runs, and `--offline`/`--frozen` stop it downloading anything (they're also passed to Cargo). Set `CARGO_XC_CACHE_DIR` to move the cache.

## License

MIT
