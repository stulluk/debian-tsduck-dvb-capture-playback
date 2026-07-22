#!/usr/bin/env bash
# Install bash completion for dvbhub.
#
# Usage:
#   ./completions/install-bash-completion.sh [/path/to/dvbhub]
#
# Default dvbhub path: ./build/dvbhub (project static binary).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DVBHUB="${1:-${PROJECT_ROOT}/build/dvbhub}"
OUT="${SCRIPT_DIR}/dvbhub.bash"

if [[ ! -x "${DVBHUB}" ]]; then
  echo "ERROR: dvbhub not found or not executable: ${DVBHUB}" >&2
  echo "Build first: ./docker/app/indockerbuild.sh" >&2
  exit 1
fi

echo "Generating ${OUT} from ${DVBHUB}…"
"${DVBHUB}" completions bash > "${OUT}"

TARGET="${HOME}/.local/share/bash-completion/completions/dvbhub"
mkdir -p "$(dirname "${TARGET}")"
cp -f "${OUT}" "${TARGET}"
echo "Installed: ${TARGET}"
echo "Reload shell or: source ${TARGET}"
