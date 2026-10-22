# The Linux release is built here, not on the release machine: a binary
# links the glibc it was built against, and this one (2.35) is old enough
# for Ubuntu 22.04, Debian 12 and everything newer. Built on Arch (2.43),
# the beta ran on nothing but rolling distributions.
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config curl ca-certificates git \
      autoconf automake libtool cmake clang libclang-dev \
      libopus-dev libasound2-dev libfontconfig1-dev libfreetype6-dev \
      libxkbcommon-dev libwayland-dev libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.98.1 --no-modify-path
