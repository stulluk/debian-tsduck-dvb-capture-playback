#!/usr/bin/env bash
# Ubuntu DVB stack setup for stock distro kernels (no custom kernel rebuild).
#
# Covers the hosts brought up in this project:
#   - gediz-lenovo: Ubuntu 26.04, stock 7.0.x — full stack + PCTV DKMS
#   - lenovo-wifi (T14): Ubuntu 22.04 HWE 6.8 — see also t14-dvb-stack-setup.sh
#
# Installs:
#   1. dkms + headers + DVB userspace tools
#   2. TBS5530 firmware + koreapyj/cxd28xx DKMS
#   3. Dektec LinuxSDK Dtu DKMS (DTU-315)
#   4. PCTV 461e v3 DKMS overlay (when stock em28xx lacks 2013:0462)
#   5. optional: copy staged dvbhub binary; install bash completion
#
# Usage:
#   sudo STAGE_DIR=/path/to/stage ./scripts/ubuntu-dvb-stack-setup.sh
#
# Optional env:
#   STAGE_DIR     directory with LinuxSDK tarball and/or dvbhub binary
#   SKIP_DEKTEC=1 SKIP_TBS=1 SKIP_PCTV=1 SKIP_APPS=1
#   KERNEL_TAG=v6.8   (forwarded to install-pctv461-dkms.sh)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
STAGE="${STAGE_DIR:-/tmp/dvbhub-stage}"
LOG="${LOG:-/tmp/ubuntu-dvb-stack-setup.log}"
exec > >(tee -a "$LOG") 2>&1

[[ "$(id -u)" -eq 0 ]] || { echo "run as root (sudo)"; exit 1; }

echo "=== ubuntu-dvb-stack-setup $(date -Is) ==="
echo "host=$(hostname) kernel=$(uname -r) repo=${REPO_ROOT}"

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y \
  dkms build-essential git curl wget ca-certificates \
  linux-headers-"$(uname -r)" \
  v4l-utils dvb-tools \
  mpv \
  gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-libav || true

KVER="$(uname -r)"
echo "Target kernel: ${KVER}"

# --- Firmware (PCTV + TBS) ---
echo "--- firmware ---"
curl -fL -o /lib/firmware/dvb-demod-m88ds3103c.fw \
  https://hauppauge.s3.us-east-1.amazonaws.com/linux/dvb-demod-m88ds3103c.fw
curl -fL -o /tmp/tbs-fw.tar.bz2 \
  http://www.tbsdtv.com/download/document/linux/tbs-tuner-firmwares_v1.0.tar.bz2
rm -rf /tmp/tbs-fw && mkdir -p /tmp/tbs-fw
tar xjf /tmp/tbs-fw.tar.bz2 -C /tmp/tbs-fw
install -m644 /tmp/tbs-fw/dvb-usb-id5530.fw /tmp/tbs-fw/dvb-demod-m88rs6060.fw /lib/firmware/

# --- Dektec DTU-315 ---
if [[ "${SKIP_DEKTEC:-0}" != "1" ]]; then
  echo "--- Dektec LinuxSDK (Dtu) ---"
  SDK_TAR="${STAGE}/LinuxSDK_v2026.05.0.tar.gz"
  if [[ ! -f "${SDK_TAR}" ]]; then
    mkdir -p "${STAGE}"
    curl -fL -o "${SDK_TAR}" \
      https://www.dektec.com/products/SDK/DTAPI/Downloads/LinuxSDK_v2026.05.0.tar.gz
  fi
  rm -rf /tmp/LinuxSDK
  tar xzf "${SDK_TAR}" -C /tmp
  # Debian/Ubuntu linux-headers lack build/kernel/; fix SDK precheck.
  # shellcheck disable=SC2016 # intentional: match literal $KERNEL in SDK script
  sed -i 's|\[\[ -d "/lib/modules/\$KERNEL/build/kernel" \]\]|\[\[ -f "/lib/modules/\$KERNEL/build/Makefile" \]\]|' \
    /tmp/LinuxSDK/Drivers/Install
  (cd /tmp/LinuxSDK/Drivers && ./Install_Dtu)
  # Modern udev: some SDK rules still use obsolete BUS=; rewrite if present.
  if [[ -f /etc/udev/rules.d/51-dtu.rules ]]; then
    sed -i 's/BUS=="usb"/SUBSYSTEM=="usb"/g' /etc/udev/rules.d/51-dtu.rules || true
  fi
fi

# --- TBS5530 cxd28xx ---
if [[ "${SKIP_TBS:-0}" != "1" ]]; then
  echo "--- TBS5530 cxd28xx DKMS ---"
  CXD_DIR=/usr/src/cxd28xx-0.0.1
  rm -rf "${CXD_DIR}"
  git clone --depth 1 https://github.com/koreapyj/cxd28xx.git "${CXD_DIR}"
  if [[ ! -f "${CXD_DIR}/dkms.conf" ]]; then
    cat >"${CXD_DIR}/dkms.conf" <<'EOF'
PACKAGE_NAME="cxd28xx"
PACKAGE_VERSION="0.0.1"
MAKE="make -C /lib/modules/${kernelver}/build M=${dkms_tree}/${PACKAGE_NAME}/${PACKAGE_VERSION}/build modules"
CLEAN="make -C /lib/modules/${kernelver}/build M=${dkms_tree}/${PACKAGE_NAME}/${PACKAGE_VERSION}/build clean"
BUILT_MODULE_NAME[0]="alp"
BUILT_MODULE_NAME[1]="cxd2878"
BUILT_MODULE_NAME[2]="m88rs6060"
BUILT_MODULE_NAME[3]="tbs5530"
BUILT_MODULE_NAME[4]="it930x"
DEST_MODULE_LOCATION[0]="/extra"
DEST_MODULE_LOCATION[1]="/extra"
DEST_MODULE_LOCATION[2]="/extra"
DEST_MODULE_LOCATION[3]="/extra"
DEST_MODULE_LOCATION[4]="/extra"
AUTOINSTALL="yes"
EOF
  fi
  dkms remove cxd28xx/0.0.1 --all 2>/dev/null || true
  dkms add "${CXD_DIR}"
  dkms build cxd28xx/0.0.1 -k "${KVER}"
  dkms install cxd28xx/0.0.1 -k "${KVER}" --force
fi

# --- PCTV 461e v3 DKMS overlay (no custom kernel) ---
if [[ "${SKIP_PCTV:-0}" != "1" ]]; then
  echo "--- PCTV 461e v3 DKMS overlay ---"
  # Stock kernels without 2013:0462 need this. Safe to re-run.
  bash "${SCRIPT_DIR}/install-pctv461-dkms.sh"
fi

# --- Apps (optional staged binary) ---
if [[ "${SKIP_APPS:-0}" != "1" ]]; then
  if [[ -x "${STAGE}/dvbhub" ]]; then
    install -m755 "${STAGE}/dvbhub" /usr/local/bin/dvbhub
  fi
  if [[ -f "${REPO_ROOT}/completions/install-bash-completion.sh" ]]; then
    bash "${REPO_ROOT}/completions/install-bash-completion.sh" || true
  fi
fi

echo "=== DKMS status ==="
dkms status
echo "=== devices (after plug-in) ==="
command -v dvbhub >/dev/null && dvbhub devices || true
echo "=== done ==="
echo "log: ${LOG}"
echo "Verify PCTV: lsusb -d 2013:0462; dmesg | grep '461e v3'; ls /dev/dvb"
echo "Capture smoke: sudo dvbhub capture --transponder 1 --adapter 0 --seconds 10 --output /tmp/tp1.ts"
