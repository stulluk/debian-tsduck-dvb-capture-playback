# Build environment for dvbhub (plain glibc binary; TSDuck and players are
# run as subprocesses, so no media libraries are linked).
FROM rust:1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*
