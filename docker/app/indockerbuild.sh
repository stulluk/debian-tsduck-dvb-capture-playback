#!/usr/bin/env bash
#
# Compile the dvbhub Rust binary inside the builder container and copy the
# resulting static binary to <project>/build/dvbhub.
#
# Usage: ./indockerbuild.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
IMAGE_NAME="dvbhub-builder-docker:latest"

APP_DIR="${PROJECT_ROOT}/app"
WORKDIR="${PROJECT_ROOT}/build"
TARGET="x86_64-unknown-linux-musl"

if ! docker image inspect "${IMAGE_NAME}" >/dev/null 2>&1; then
    echo "ERROR: image ${IMAGE_NAME} not found. Run ./dockerbuild.sh first." >&2
    exit 1
fi

mkdir -p "${WORKDIR}/cargo-home"

TTY_FLAGS=""
[ -t 0 ] && TTY_FLAGS="-it"

echo "=== Compiling dvbhub (target ${TARGET}) ==="
docker run --rm ${TTY_FLAGS} \
    --user "$(id -u)":"$(id -g)" \
    --volume="/etc/passwd:/etc/passwd:ro" \
    --volume="/etc/group:/etc/group:ro" \
    --volume="${PROJECT_ROOT}:${PROJECT_ROOT}" \
    --workdir="${APP_DIR}" \
    --env CARGO_HOME="${WORKDIR}/cargo-home" \
    --env CARGO_TARGET_DIR="${WORKDIR}/target" \
    "${IMAGE_NAME}" \
    cargo build --release --target "${TARGET}"

BIN="${WORKDIR}/target/${TARGET}/release/dvbhub"
if [[ ! -x "${BIN}" ]]; then
    echo "ERROR: built binary not found at ${BIN}" >&2
    exit 1
fi

cp -f "${BIN}" "${WORKDIR}/dvbhub"
echo "=== Done. Static binary: ${WORKDIR}/dvbhub ==="
file "${WORKDIR}/dvbhub" || true
