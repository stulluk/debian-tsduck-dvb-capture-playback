#!/usr/bin/env bash
#
# Build the container image for gst-plugin-mpegtslive (the plugin is compiled
# during the image build).
#
# Usage: ./dockerbuild.sh [jobs]
#   jobs  Parallel cargo jobs. Default: 80% of host cores.
#
# A timestamped build log is written to <project>/build/logs/.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
IMAGE_NAME="dvbhub-gst-mpegtslive:latest"

JOBS="${1:-$(( $(nproc) * 8 / 10 ))}"
(( JOBS < 1 )) && JOBS=1
LOG_DIR="${PROJECT_ROOT}/build/logs"
LOG_FILE="${LOG_DIR}/gst-mpegtslive-build-$(date +%Y%m%d-%H%M%S).log"
mkdir -p "${LOG_DIR}"

echo "Building Docker image: ${IMAGE_NAME} (jobs=${JOBS})"
echo "Log: ${LOG_FILE}"
docker build --progress=plain --build-arg JOBS="${JOBS}" \
  -t "${IMAGE_NAME}" "${SCRIPT_DIR}" 2>&1 | tee "${LOG_FILE}"
echo "Done: ${IMAGE_NAME}"
