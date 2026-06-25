#!/usr/bin/env bash
#
# Build the Docker image used to compile TSDuck.
# Usage: ./dockerbuild.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE_NAME="tsduck-builder-docker:latest"

echo "Building Docker image: ${IMAGE_NAME}"
docker build -t "${IMAGE_NAME}" -f "${SCRIPT_DIR}/Dockerfile" "${SCRIPT_DIR}"
echo "Done: ${IMAGE_NAME}"
