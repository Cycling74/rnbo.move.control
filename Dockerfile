FROM debian:bookworm

ENV DEBIAN_FRONTEND=noninteractive

# Enable arm64 architecture for cross-compilation libraries
RUN dpkg --add-architecture arm64

RUN apt-get update && apt-get install -y \
    curl \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    binutils-aarch64-linux-gnu \
    make \
    cmake \
    file \
    python3 \
    python3-pillow \
    libdbus-1-dev:arm64 \
    libsystemd-dev:arm64 \
    libjack-jackd2-dev:arm64 \
    && rm -rf /var/lib/apt/lists/*

#install rust and the target we need
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --target aarch64-unknown-linux-gnu

WORKDIR /build

# Set cross-compilation environment
ENV CROSS_PREFIX=aarch64-linux-gnu-
ENV CC=aarch64-linux-gnu-gcc
ENV CXX=aarch64-linux-gnu-g++
