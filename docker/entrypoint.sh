#!/bin/sh
set -eu

arch="$(uname -m)"
if [ "$arch" != "aarch64" ] && [ "$arch" != "arm64" ]; then
  echo "error: cargo-xc builder must run natively as ARM64; detected $arch" >&2
  exit 1
fi

if [ -e /proc/sys/fs/binfmt_misc/qemu-x86_64 ]; then
  echo "note: qemu-x86_64 is registered on the host, but cargo-xc does not invoke it" >&2
fi

exec "$@"
