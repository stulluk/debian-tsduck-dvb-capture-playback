#!/usr/bin/env bash
#
# CI build for dvbhub: builds the Docker builder image, runs the unit tests
# (warnings are errors), builds the release binary and stages it in dist/.
# Used by .github/workflows/build.yml and runnable locally.
#
# Usage: scripts/ci-build.sh [jobs]
#   jobs  Parallel cargo jobs and Docker CPU limit. Default: 80% of host cores.
#
# Output: dist/dvbhub-linux-x86_64 and dist/dvbhub-linux-x86_64.sha256
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMAGE_NAME="dvbhub-builder-docker:latest"
JOBS="${1:-$(( $(nproc) * 8 / 10 ))}"
(( JOBS < 1 )) && JOBS=1
WORK="${REPO_ROOT}/build/ci"
DIST="${REPO_ROOT}/dist"

echo "=== Building image ${IMAGE_NAME} ==="
docker build -t "${IMAGE_NAME}" -f "${REPO_ROOT}/docker/app/Dockerfile" "${REPO_ROOT}/docker/app"

mkdir -p "${WORK}/cargo-home" "${DIST}"
echo "=== cargo test + build (jobs=${JOBS}, warnings are errors) ==="
docker run --rm --cpus="${JOBS}" \
  --user "$(id -u)":"$(id -g)" \
  --volume="${REPO_ROOT}:${REPO_ROOT}" \
  --workdir="${REPO_ROOT}/app" \
  --env CARGO_HOME="${WORK}/cargo-home" \
  --env CARGO_TARGET_DIR="${WORK}/target" \
  --env RUSTFLAGS="-D warnings" \
  "${IMAGE_NAME}" \
  sh -c "cargo test --release -j ${JOBS} && cargo build --release -j ${JOBS}"

cp -f "${WORK}/target/release/dvbhub" "${DIST}/dvbhub-linux-x86_64"
(cd "${DIST}" && sha256sum dvbhub-linux-x86_64 >dvbhub-linux-x86_64.sha256)
"${DIST}/dvbhub-linux-x86_64" --version
echo "=== Done: ${DIST}/dvbhub-linux-x86_64 ==="
