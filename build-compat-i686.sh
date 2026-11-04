#!/bin/bash
IMAGE_TAG=irust-compat-builder-i686
set -e
if ! docker image inspect "$IMAGE_TAG" >/dev/null 2>&1; then
  echo "compat builder image (i686) not found, building it once (this is the slow path)..."
  docker build -t "$IMAGE_TAG" -f build-compat-i686.Dockerfile .
fi
docker run --rm -v "$PWD:/src" -w /src "$IMAGE_TAG" bash -c '
  set -e
  cargo build --release --locked --target i686-unknown-linux-musl --target-dir target-compat-i686
'
# Output lands under target-compat-i686/i686-unknown-linux-musl/release/irust
# (the --target flag adds a target-triple subdir), not
# target-compat-i686/release/irust like a hostless build would.
