#!/usr/bin/env bash
#
# Deploy the compiled TSDuck bundle to an isolated prefix on the host.
# The bundle is self-contained (-rpath '$ORIGIN'); removal = rm -rf the prefix.
#
# Usage:
#   ./deploy.sh [prefix]
# Default prefix: /opt/tsduck
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
DIST="${PROJECT_ROOT}/build/tsduck-dist"
PREFIX="${1:-/opt/tsduck}"

if [[ ! -x "${DIST}/tsp" ]]; then
    echo "ERROR: built bundle not found at ${DIST} (run ./indockerbuild.sh first)." >&2
    exit 1
fi

echo "=== Deploying TSDuck bundle to ${PREFIX} ==="
sudo rm -rf "${PREFIX}"
sudo mkdir -p "${PREFIX}"
sudo cp -a "${DIST}/." "${PREFIX}/"

echo "=== Verifying ${PREFIX}/tsp ==="
"${PREFIX}/tsp" --version 2>&1 || true
echo "=== Done. Add ${PREFIX} to PATH or reference ${PREFIX}/tsp directly. ==="
