#!/usr/bin/env bash
#
# Run the TSDuck build inside the builder container.
# Build output (self-contained bundle) is written to <project>/build/tsduck-dist.
#
# Usage:
#   ./indockerbuild.sh [jobs]
#   jobs  Parallel make jobs and Docker CPU limit. Default: 80% of host cores.
#
# A timestamped build log is written to <project>/build/logs/.
#
# Environment:
#   DTAPI_TARBALL  Path to the Dektec LinuxSDK tarball.
#                  Default: ~/Downloads/LinuxSDK_v2026.05.0.tar.gz
#   TSDUCK_REF     Optional git ref/tag to build (default: latest release tag).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
IMAGE_NAME="tsduck-builder-docker:latest"

JOBS="${1:-$(( $(nproc) * 8 / 10 ))}"
(( JOBS < 1 )) && JOBS=1
DTAPI_TARBALL="${DTAPI_TARBALL:-${HOME}/Downloads/LinuxSDK_v2026.05.0.tar.gz}"
WORKDIR="${PROJECT_ROOT}/build"

if [[ ! -e "${DTAPI_TARBALL}" ]]; then
    echo "ERROR: Dektec LinuxSDK tarball not found: ${DTAPI_TARBALL}" >&2
    echo "Set DTAPI_TARBALL to the LinuxSDK_*.tar.gz path." >&2
    exit 1
fi

if ! docker image inspect "${IMAGE_NAME}" >/dev/null 2>&1; then
    echo "ERROR: image ${IMAGE_NAME} not found. Run ./dockerbuild.sh first." >&2
    exit 1
fi

LOG_DIR="${WORKDIR}/logs"
LOG_FILE="${LOG_DIR}/tsduck-build-$(date +%Y%m%d-%H%M%S).log"
mkdir -p "${WORKDIR}" "${LOG_DIR}"

# Use an interactive TTY only when available (so it can also run in background).
TTY_FLAGS=""
[ -t 0 ] && TTY_FLAGS="-it"

echo "=== Running TSDuck build in container (jobs=${JOBS}) ==="
echo "Log: ${LOG_FILE}"
# shellcheck disable=SC2086  # TTY_FLAGS is intentionally word-split (empty or -it).
docker run --rm ${TTY_FLAGS} \
    --cpus="${JOBS}" \
    --user "$(id -u)":"$(id -g)" \
    --volume="/etc/passwd:/etc/passwd:ro" \
    --volume="/etc/group:/etc/group:ro" \
    --volume="${PROJECT_ROOT}:${PROJECT_ROOT}" \
    --volume="${DTAPI_TARBALL}:/dtapi/LinuxSDK.tar.gz:ro" \
    --env WORKDIR="${WORKDIR}" \
    --env DTAPI_ORIGIN="/dtapi/LinuxSDK.tar.gz" \
    --env JOBS="${JOBS}" \
    --env TSDUCK_REF="${TSDUCK_REF:-}" \
    "${IMAGE_NAME}" \
    /usr/bin/env bash "${SCRIPT_DIR}/build-in-container.sh" 2>&1 | tee "${LOG_FILE}"

echo "=== Build bundle ready at: ${WORKDIR}/tsduck-dist ==="
