#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "${repo_root}/Cargo.toml" | head -n 1)"
image_ref="${CARGO_XC_IMAGE_REF:-${1:-ghcr.io/stevepryde/cargo-xc:${version}}}"
docker_output="${DOCKER_OUTPUT:-${2:---push}}"

DOCKER_BUILDKIT=1 docker buildx build \
  --platform linux/arm64 \
  --file "${repo_root}/docker/Dockerfile" \
  --tag "${image_ref}" \
  "${docker_output}" \
  "${repo_root}"
