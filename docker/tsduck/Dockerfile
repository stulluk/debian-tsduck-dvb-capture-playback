# Build environment for compiling TSDuck (with Dektec support) from source.
# The image only carries the build toolchain; the actual build runs as the
# invoking host user (see indockerbuild.sh) so output files are not root-owned.
#
# Base: debian:trixie. Its glibc is older than the host (Debian forky/sid), so
# binaries compiled here run on the host (forward-compatible glibc).
FROM debian:trixie

ARG DEBIAN_FRONTEND=noninteractive

# TSDuck build prerequisites for Debian (from scripts/install-prerequisites.sh),
# plus the optional libraries we keep enabled (curl, openssl, zlib, pcsc).
# xz-utils is needed to handle compressed kernel module / archive tooling.
RUN apt-get update && apt-get install -y --no-install-recommends \
        git \
        g++ \
        make \
        cmake \
        locales-all \
        flex \
        bison \
        dos2unix \
        curl \
        ca-certificates \
        tar \
        zip \
        xz-utils \
        linux-libc-dev \
        dpkg-dev \
        python3 \
        libcurl4-openssl-dev \
        libssl-dev \
        zlib1g-dev \
        pcscd \
        libpcsclite-dev \
        libedit-dev \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8 \
    LC_ALL=en_US.UTF-8 \
    LANGUAGE=en_US.UTF-8
