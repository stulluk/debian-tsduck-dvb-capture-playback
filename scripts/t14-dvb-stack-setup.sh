#!/usr/bin/env bash
# T14 (Ubuntu 22.04) — DVB stack on stock HWE 6.8 kernel + DKMS drivers + dvbhub.
set -euo pipefail

STAGE="${STAGE_DIR:-/tmp/dvbhub-t14-stage}"
LOG="/tmp/t14-dvb-setup.log"
exec > >(tee -a "$LOG") 2>&1

echo "=== t14-dvb-stack-setup $(date -Is) ==="

# --- 1. Remove legacy DKMS / driver trees ---
echo "--- removing legacy dektec/cxd28xx DKMS ---"
for mod in dektec dektec-2023.10.0 dektec_dtu cxd28xx; do
  while read -r ver _kern _; do
    [[ -n "$ver" ]] || continue
    dkms remove "$mod/$ver" --all 2>/dev/null || true
  done < <(dkms status 2>/dev/null | grep "^${mod}/" || true)
done
sudo rm -rf /usr/src/dektec-2023.10.0 /usr/src/dektec_dtu-* /usr/src/cxd28xx-*

# --- 2. Base packages + Ubuntu HWE kernel (latest jammy 6.8) ---
export DEBIAN_FRONTEND=noninteractive
sudo apt-get update -qq
sudo apt-get install -y \
  dkms build-essential git curl wget ca-certificates gcc-12 \
  flex bison libssl-dev libelf-dev dwarves \
  linux-generic-hwe-22.04 linux-headers-generic-hwe-22.04 \
  v4l-utils dvb-tools \
  mpv \
  gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-libav gstreamer1.0-gl \
  libgstreamer1.0-dev

KVER=""
for d in /lib/modules/6.8.*; do
  [[ -d "$d" ]] || continue
  KVER="${d##*/}"
done
echo "Target kernel: ${KVER:-unknown}"

# --- 3. GRUB: Ubuntu (not Windows), stock HWE kernel ---
sudo sed -i 's/^GRUB_DEFAULT=.*/GRUB_DEFAULT=0/' /etc/default/grub
ROOT_UUID=$(findmnt -no UUID -T /)
sudo grub-set-default "gnulinux-simple-${ROOT_UUID}" 2>/dev/null || true
sudo update-grub
echo "GRUB default: Ubuntu entry 0 (stock HWE after update-grub)"

# --- 4. Firmware (PCTV + TBS) ---
echo "--- firmware ---"
sudo curl -fL -o /lib/firmware/dvb-demod-m88ds3103c.fw \
  https://hauppauge.s3.us-east-1.amazonaws.com/linux/dvb-demod-m88ds3103c.fw
curl -fL -o /tmp/tbs-fw.tar.bz2 \
  http://www.tbsdtv.com/download/document/linux/tbs-tuner-firmwares_v1.0.tar.bz2
rm -rf /tmp/tbs-fw && mkdir -p /tmp/tbs-fw
tar xjf /tmp/tbs-fw.tar.bz2 -C /tmp/tbs-fw
sudo install -m644 /tmp/tbs-fw/dvb-usb-id5530.fw /tmp/tbs-fw/dvb-demod-m88rs6060.fw /lib/firmware/

# --- 5. Dektec LinuxSDK (DTU-315) ---
SDK_TAR="$STAGE/LinuxSDK_v2026.05.0.tar.gz"
if [[ ! -f "$SDK_TAR" ]]; then
  echo "--- downloading Dektec LinuxSDK ---"
  curl -fL -o "$SDK_TAR" \
    https://www.dektec.com/products/SDK/DTAPI/Downloads/LinuxSDK_v2026.05.0.tar.gz
fi
rm -rf /tmp/LinuxSDK
tar xzf "$SDK_TAR" -C /tmp
sudo sed -i 's|\[\[ -d "/lib/modules/\$KERNEL/build/kernel" \]\]|\[\[ -f "/lib/modules/\$KERNEL/build/Makefile" \]\]|' \
  /tmp/LinuxSDK/Drivers/Install
(cd /tmp/LinuxSDK/Drivers && sudo ./Install_Dtu)

# --- 6. TBS5530 cxd28xx DKMS ---
CXD_DIR=/usr/src/cxd28xx-dkms
sudo rm -rf "$CXD_DIR"
sudo git clone --depth 1 https://github.com/koreapyj/cxd28xx.git "$CXD_DIR"
sudo tee "$CXD_DIR/dkms.conf" >/dev/null <<'EOF'
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
if [[ -n "${KVER}" ]]; then
  sudo dkms add "$CXD_DIR"
  sudo dkms build cxd28xx/0.0.1 -k "$KVER"
  sudo dkms install cxd28xx/0.0.1 -k "$KVER"
fi

# --- 7. PCTV 461e v3 DKMS (stock 6.8 has no 2013:0462 / m88ds3103c) ---
# Do NOT install a custom patched 7.0 kernel on T14; overlay via DKMS instead.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -x "${SCRIPT_DIR}/install-pctv461-dkms.sh" ]]; then
  echo "--- PCTV 461e v3 DKMS overlay ---"
  sudo KERNEL_TAG=v6.8 KVER="${KVER}" bash "${SCRIPT_DIR}/install-pctv461-dkms.sh"
else
  echo "WARNING: install-pctv461-dkms.sh missing; PCTV 2013:0462 will not bind"
fi

# --- 8. dvbhub (distro tsp on jammy — sid-built /opt/tsduck needs glibc 2.38) ---
sudo rm -rf /opt/tsduck
if [[ -x "$STAGE/dvbhub" ]]; then
  sudo install -m755 "$STAGE/dvbhub" /usr/local/bin/dvbhub
fi
if [[ -d "$STAGE/gst-plugins" ]]; then
  sudo mkdir -p /usr/local/lib/dvbhub/gst-plugins
  sudo cp -a "$STAGE/gst-plugins/." /usr/local/lib/dvbhub/gst-plugins/
fi

# --- 9. NVIDIA DKMS for HWE kernel ---
if dpkg -l nvidia-dkms-535 &>/dev/null && [[ -n "${KVER}" ]]; then
  echo "--- rebuilding nvidia DKMS for $KVER ---"
  sudo dkms build nvidia/535.309.01 -k "$KVER" || true
  sudo dkms install nvidia/535.309.01 -k "$KVER" || true
fi

# --- 10. Summary ---
echo "=== DKMS status ==="
dkms status
echo "=== kernels ==="
ls /boot/vmlinuz-* | tail -5
echo "=== GRUB saved ==="
sudo grub-editenv list || true
echo "=== tools ==="
tsp --version 2>/dev/null | head -1 || true
/usr/local/bin/dvbhub --version 2>/dev/null || dvbhub --help 2>&1 | head -1 || true
echo "=== done — reboot to activate HWE kernel if upgraded ==="
echo "PCTV check after reboot/plug: dmesg | grep '461e v3'; dvbhub devices"
echo "log: $LOG"
