#!/usr/bin/env bash
#
# Compile dvbhub (GStreamer-linked glibc binary) inside the builder container.
#
# Usage: ./indockerbuild.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
IMAGE_NAME="dvbhub-builder-docker:latest"

APP_DIR="${PROJECT_ROOT}/app"
WORKDIR="${PROJECT_ROOT}/build"

if ! docker image inspect "${IMAGE_NAME}" >/dev/null 2>&1; then
    echo "ERROR: image ${IMAGE_NAME} not found. Run ./dockerbuild.sh first." >&2
    exit 1
fi

mkdir -p "${WORKDIR}/cargo-home"

TTY_FLAGS=""
[ -t 0 ] && TTY_FLAGS="-it"

echo "=== Compiling dvbhub (glibc + GStreamer) ==="
docker run --rm ${TTY_FLAGS} \
    --user "$(id -u)":"$(id -g)" \
    --volume="/etc/passwd:/etc/passwd:ro" \
    --volume="/etc/group:/etc/group:ro" \
    --volume="${PROJECT_ROOT}:${PROJECT_ROOT}" \
    --workdir="${APP_DIR}" \
    --env CARGO_HOME="${WORKDIR}/cargo-home" \
    --env CARGO_TARGET_DIR="${WORKDIR}/target" \
    "${IMAGE_NAME}" \
    cargo build --release

BIN="${WORKDIR}/target/release/dvbhub"
if [[ ! -x "${BIN}" ]]; then
    echo "ERROR: built binary not found at ${BIN}" >&2
    exit 1
fi

cp -f "${BIN}" "${WORKDIR}/dvbhub"
echo "=== Done. Binary: ${WORKDIR}/dvbhub ==="
file "${WORKDIR}/dvbhub" || true
ldd "${WORKDIR}/dvbhub" 2>/dev/null | head -20 || true

# Refresh the system-wide install so we do not keep copying from build/ by hand.
INSTALL_SCRIPT="${PROJECT_ROOT}/scripts/install-dvbhub-systemwide.sh"
if [[ -x "${INSTALL_SCRIPT}" ]]; then
  if [[ "$(id -u)" -eq 0 ]]; then
    "${INSTALL_SCRIPT}" "${WORKDIR}/dvbhub"
  elif sudo -n true 2>/dev/null; then
    echo "=== Installing system-wide via sudo -n ==="
    sudo -n "${INSTALL_SCRIPT}" "${WORKDIR}/dvbhub"
  else
    echo "NOTE: system-wide install skipped (sudo needs a password)."
    echo "      Run: sudo ${INSTALL_SCRIPT}"
  fi
fi
