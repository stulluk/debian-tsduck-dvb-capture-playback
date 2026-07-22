#!/usr/bin/env bash
# Extract libgstmpegtslive.so into repo build/gst-plugins/ (for GST_PLUGIN_PATH at runtime).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUT="${REPO_ROOT}/build/gst-plugins"
mkdir -p "${OUT}"

docker run --rm \
  -v "${OUT}:/out" \
  dvbhub-gst-mpegtslive:latest \
  bash -c 'set -e; shopt -s nullglob; libs=(/src/gst-plugins-rs/target/*/release/libgstmpegtslive.so); \
    if ((${#libs[@]}==0)); then echo "libgstmpegtslive.so not found"; exit 1; fi; \
    cp -v "${libs[0]}" /out/; gst-inspect-1.0 --plugin /out/libgstmpegtslive.so mpegtslivesrc | head -5'

echo "Installed: ${OUT}/libgstmpegtslive.so"
