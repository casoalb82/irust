FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq && \
    apt-get install -y -qq curl build-essential cmake pkg-config git ca-certificates xz-utils && \
    rm -rf /var/lib/apt/lists/*

RUN curl -sSfL https://musl.cc/i686-linux-musl-cross.tgz | tar -C /opt -xz
ENV PATH="/opt/i686-linux-musl-cross/bin:${PATH}"

ENV CARGO_HOME=/root/.cargo
ENV RUSTUP_HOME=/root/.rustup
ENV PATH="/root/.cargo/bin:${PATH}"
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable && \
    rustup target add i686-unknown-linux-musl

# Tell cargo which cross linker to use for this target -- without
# this it reaches for the host's own `cc`, which can't produce i686
# code at all.
ENV CARGO_TARGET_I686_UNKNOWN_LINUX_MUSL_LINKER=i686-linux-musl-gcc
ENV CC_i686_unknown_linux_musl=i686-linux-musl-gcc
