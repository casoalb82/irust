#!/bin/bash
IMAGE_TAG=irust-compat-builder
set -e
if ! docker image inspect "$IMAGE_TAG" >/dev/null 2>&1; then
  echo "compat builder image not found, building it once (this is the slow path)..."
  docker build -t "$IMAGE_TAG" -f build-compat.Dockerfile .
fi
docker run --rm -v "$PWD:/src" -w /src "$IMAGE_TAG" bash -c '
  set -e
  cargo build --release --locked --target-dir target-compat
'
