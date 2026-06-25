# Build environment for the dvbhub Rust application.
# Produces a fully static x86_64 musl binary that runs on the host with no
# runtime library dependencies (it drives TSDuck purely via subprocesses).
FROM rust:alpine

# musl-dev provides the C runtime bits some crates need at build time; gcc is
# required to compile C/asm in the rustls TLS stack (ring) for the musl target.
RUN apk add --no-cache musl-dev gcc

# Ensure the musl target is available (default on rust:alpine, explicit here).
RUN rustup target add x86_64-unknown-linux-musl
