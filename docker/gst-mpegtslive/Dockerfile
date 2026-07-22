# Build gst-plugin-mpegtslive (mpegtslivesrc) against host-matching GStreamer (Debian sid).
FROM debian:sid

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
    curl \
    pkg-config \
    build-essential \
    libssl-dev \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-tools \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

RUN cargo install cargo-c

WORKDIR /src
RUN git clone --depth 1 https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git

WORKDIR /src/gst-plugins-rs
RUN cargo cbuild -p gst-plugin-mpegtslive --release
