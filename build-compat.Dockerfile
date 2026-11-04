FROM ubuntu:20.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq && \
    apt-get install -y -qq curl build-essential cmake pkg-config git ca-certificates && \
    rm -rf /var/lib/apt/lists/*

ENV CARGO_HOME=/root/.cargo
ENV RUSTUP_HOME=/root/.rustup
ENV PATH="/root/.cargo/bin:${PATH}"
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
