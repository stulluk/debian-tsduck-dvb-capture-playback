#!/usr/bin/env bash
# Install (or refresh) dvbhub system-wide from the project build output.
#
# Default source: <repo>/build/dvbhub
# Default destination: /usr/local/bin/dvbhub
#
# Also installs:
#   - bash completion into /etc/bash_completion.d/dvbhub (if writable)
#   - optional GStreamer plugins from build/gst-plugins -> /usr/local/lib/dvbhub/gst-plugins
#
# Usage:
#   sudo ./scripts/install-dvbhub-systemwide.sh
#   sudo ./scripts/install-dvbhub-systemwide.sh /path/to/dvbhub
#
# Called automatically at the end of docker/app/indockerbuild.sh when sudo
# is available (passwordless or already elevated).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SRC="${1:-${REPO_ROOT}/build/dvbhub}"
DEST_BIN="${DVBHUB_SYSTEM_BIN:-/usr/local/bin/dvbhub}"
DEST_GST="${DVBHUB_SYSTEM_GST:-/usr/local/lib/dvbhub/gst-plugins}"
COMPLETION_DIR="${DVBHUB_COMPLETION_DIR:-/etc/bash_completion.d}"

[[ "$(id -u)" -eq 0 ]] || {
  echo "ERROR: run as root (sudo $0 ...)" >&2
  exit 1
}

[[ -x "${SRC}" ]] || {
  echo "ERROR: binary not found or not executable: ${SRC}" >&2
  echo "Build first: ./docker/app/indockerbuild.sh" >&2
  exit 1
}

install -d "$(dirname "${DEST_BIN}")"
install -m755 "${SRC}" "${DEST_BIN}"
echo "Installed: ${DEST_BIN}"
"${DEST_BIN}" --version 2>/dev/null || "${DEST_BIN}" --help 2>&1 | head -2 || true

# Bash completion: prefer generating from the installed binary.
if command -v "${DEST_BIN}" >/dev/null; then
  install -d "${COMPLETION_DIR}"
  if "${DEST_BIN}" completions bash >"${COMPLETION_DIR}/dvbhub" 2>/dev/null; then
    chmod 644 "${COMPLETION_DIR}/dvbhub"
    echo "Installed bash completion: ${COMPLETION_DIR}/dvbhub"
  elif [[ -f "${REPO_ROOT}/completions/dvbhub.bash" ]]; then
    install -m644 "${REPO_ROOT}/completions/dvbhub.bash" "${COMPLETION_DIR}/dvbhub"
    echo "Installed bash completion (vendored): ${COMPLETION_DIR}/dvbhub"
  fi
fi

# Optional GStreamer plugin dir used by watch path
if [[ -d "${REPO_ROOT}/build/gst-plugins" ]]; then
  install -d "${DEST_GST}"
  cp -a "${REPO_ROOT}/build/gst-plugins/." "${DEST_GST}/"
  echo "Installed GStreamer plugins: ${DEST_GST}"
fi

# Make TSDuck tools visible on the default PATH (including sudo secure_path).
if [[ -x /opt/tsduck/tsp ]]; then
  for tool in tsp tsanalyze tslsdvb tsdektec tsversion; do
    if [[ -x "/opt/tsduck/${tool}" ]]; then
      ln -sfn "/opt/tsduck/${tool}" "/usr/local/bin/${tool}"
    fi
  done
  echo "Linked TSDuck tools from /opt/tsduck into /usr/local/bin"
fi

# Shell env hint for interactive sessions
cat >/etc/profile.d/dvbhub.sh <<'EOF'
# dvbhub / TSDuck (installed by scripts/install-dvbhub-systemwide.sh)
export PATH="/usr/local/bin:/opt/tsduck:${PATH}"
# Prefer the Docker-deployed bundle when present.
[ -x /opt/tsduck/tsp ] && export DVBHUB_TSDUCK_DIR="${DVBHUB_TSDUCK_DIR:-/opt/tsduck}"
EOF
chmod 644 /etc/profile.d/dvbhub.sh
echo "Installed /etc/profile.d/dvbhub.sh"

# Runtime dependencies (not installed automatically; report what is missing).
# tool:package:purpose
missing=0
for dep in \
  "dvb-fe-tool:dvb-tools:C/N + post-BER statistics during capture" \
  "mpv:mpv:default player for dvbhub watch" \
  "gst-launch-1.0:gstreamer1.0-tools:GStreamer watch path"; do
  IFS=: read -r tool pkg purpose <<<"${dep}"
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "WARNING: ${tool} not found (${purpose}) — install: apt install ${pkg}" >&2
    missing=1
  fi
done
[[ "${missing}" -eq 0 ]] && echo "Runtime dependencies: OK (dvb-fe-tool, mpv, gst-launch-1.0)"

echo "System-wide dvbhub is ready (PATH should include $(dirname "${DEST_BIN}"))."
