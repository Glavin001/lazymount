# Stage 1: Build chisel (TCP tunnel tool)
FROM golang:latest AS chisel-builder
RUN CGO_ENABLED=0 go install github.com/jpillora/chisel@latest

# Stage 2: Build lazymount for Linux from local source
FROM rust:1.85-bookworm AS lazymount-builder
WORKDIR /build
COPY . .
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    pkg-config \
    libssl-dev && \
    rm -rf /var/lib/apt/lists/*
RUN cargo build --release -p lazymount-cli

# Stage 3: Runtime image (the "remote server" in the demo)
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    unzip \
    fuse3 && \
    rm -rf /var/lib/apt/lists/*

# Install rclone (used on the server side for rclone mount)
RUN curl -fsSL https://rclone.org/install.sh | bash

# Copy binaries built above
COPY --from=chisel-builder /go/bin/chisel /usr/local/bin/chisel
COPY --from=lazymount-builder /build/target/release/lazymount /usr/local/bin/lazymount

# rclone expects 'fusermount'; Ubuntu 22.04 ships fusermount3
RUN ln -sf /usr/bin/fusermount3 /usr/local/bin/fusermount

# Smoke-test binaries at build time
RUN chisel --version && rclone version && lazymount --help >/dev/null

EXPOSE 8090

# The server side: start chisel server + wait for client manifest + rclone mount
CMD ["lazymount", "server", "start", "--port", "8090", "--auth", "demo:secret"]
