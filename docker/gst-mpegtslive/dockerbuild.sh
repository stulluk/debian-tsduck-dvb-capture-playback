#!/usr/bin/env bash
# Build container image for gst-plugin-mpegtslive.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker build -t dvbhub-gst-mpegtslive:latest "${SCRIPT_DIR}"
