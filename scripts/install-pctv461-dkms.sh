#!/usr/bin/env bash
# Install PCTV 461e v3 (USB 2013:0462) support via DKMS on stock Ubuntu/Debian
# kernels that do not yet include the linux-media 461e v3 / m88ds3103c patches.
#
# When a full custom kernel rebuild is unnecessary, this overlays:
#   em28xx, em28xx-dvb, m88ds3103
# built from torvalds sources matching the running kernel's major.minor,
# with drivers/pctv461/patches applied.
#
# Verified hosts:
#   - Ubuntu 22.04 HWE 6.8 (lenovo-wifi / T14)
#   - Ubuntu 26.04 7.0 (gediz-lenovo)
#
# Usage:
#   sudo ./scripts/install-pctv461-dkms.sh
#   sudo KERNEL_TAG=v6.8 ./scripts/install-pctv461-dkms.sh
#
# shellcheck disable=SC2086
set -euo pipefail

PKG_NAME="pctv461"
PKG_VER="2026.03.17"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PATCH_DIR="${REPO_ROOT}/drivers/pctv461/patches"
WORK="${WORK_DIR:-/tmp/pctv461-dkms-build}"
SRC_DST="/usr/src/${PKG_NAME}-${PKG_VER}"
KVER="${KVER:-$(uname -r)}"

log() { printf '[pctv461-dkms] %s\n' "$*"; }
die() { printf '[pctv461-dkms] ERROR: %s\n' "$*" >&2; exit 1; }

[[ "$(id -u)" -eq 0 ]] || die "run as root (sudo)"
[[ -d "${PATCH_DIR}" ]] || die "missing patches at ${PATCH_DIR}"
command -v curl >/dev/null || die "curl required"
command -v patch >/dev/null || die "patch required"
command -v dkms >/dev/null || die "dkms required (apt install dkms)"
command -v python3 >/dev/null || die "python3 required"

[[ -d "/lib/modules/${KVER}/build" ]] || die "kernel headers missing for ${KVER} (install linux-headers-${KVER})"

# Map running kernel to a torvalds tag whose media drivers match ABI enough to build.
detect_kernel_tag() {
  local major minor
  major="$(echo "${KVER}" | cut -d. -f1)"
  minor="$(echo "${KVER}" | cut -d. -f2)"
  case "${major}.${minor}" in
    6.8) echo "v6.8" ;;
    6.11|6.12|6.13|6.14|6.15|6.16|6.17) echo "v${major}.${minor}" ;;
    7.0) echo "v7.0" ;;
    *)
      # Prefer exact major.minor tag; caller can override with KERNEL_TAG.
      echo "v${major}.${minor}"
      ;;
  esac
}

KERNEL_TAG="${KERNEL_TAG:-$(detect_kernel_tag)}"
RAW_BASE="https://raw.githubusercontent.com/torvalds/linux/${KERNEL_TAG}"
API_BASE="https://api.github.com/repos/torvalds/linux/contents"

fetch() {
  local url="$1" out="$2"
  mkdir -p "$(dirname "${out}")"
  curl -fsSL "${url}" -o "${out}"
}

fetch_dir_headers() {
  # Download *.h only from a github contents API directory (non-recursive).
  local rel="$1" dest="$2"
  local tmp
  tmp="$(mktemp)"
  curl -fsSL "${API_BASE}/${rel}?ref=${KERNEL_TAG}" -o "${tmp}" \
    || curl -fsSL "${API_BASE}/${rel}?ref=${KERNEL_TAG#v}" -o "${tmp}"
  python3 - "${tmp}" "${dest}" <<'PY'
import json, sys, urllib.request, pathlib
listing = json.load(open(sys.argv[1]))
dest = pathlib.Path(sys.argv[2])
dest.mkdir(parents=True, exist_ok=True)
for ent in listing:
    if ent.get("type") != "file":
        continue
    name = ent["name"]
    if not name.endswith(".h"):
        continue
    url = ent["download_url"]
    data = urllib.request.urlopen(url, timeout=120).read()
    (dest / name).write_bytes(data)
    print(name)
PY
  rm -f "${tmp}"
}

log "kernel=${KVER} tag=${KERNEL_TAG} work=${WORK}"
rm -rf "${WORK}"
mkdir -p "${WORK}/tree/drivers/media/usb/em28xx" \
         "${WORK}/tree/drivers/media/dvb-frontends" \
         "${WORK}/tree/drivers/media/tuners" \
         "${WORK}/pkg"

# --- download media sources matching KERNEL_TAG ---
log "downloading em28xx + m88ds3103 from ${KERNEL_TAG}"
for f in em28xx-core.c em28xx-i2c.c em28xx-cards.c em28xx-camera.c \
         em28xx-dvb.c em28xx.h em28xx-reg.h; do
  fetch "${RAW_BASE}/drivers/media/usb/em28xx/${f}" \
        "${WORK}/tree/drivers/media/usb/em28xx/${f}"
done
for f in m88ds3103.c m88ds3103_priv.h m88ds3103.h; do
  fetch "${RAW_BASE}/drivers/media/dvb-frontends/${f}" \
        "${WORK}/tree/drivers/media/dvb-frontends/${f}"
done

log "downloading frontend/tuner headers + drx39xyj stubs"
fetch_dir_headers "drivers/media/dvb-frontends" "${WORK}/fe-headers"
mkdir -p "${WORK}/tree/drivers/media/dvb-frontends"
cp -a "${WORK}/fe-headers/." "${WORK}/tree/drivers/media/dvb-frontends/"
# Keep the patched m88ds3103.c we already downloaded (headers dir has no .c)
fetch_dir_headers "drivers/media/tuners" "${WORK}/tuner-headers"

mkdir -p "${WORK}/tree/drivers/media/dvb-frontends/drx39xyj"
for f in drx39xxj.h drx_dap_fasi.h drx_driver.h drx_driver_version.h drxj.h drxj_map.h; do
  fetch "${RAW_BASE}/drivers/media/dvb-frontends/drx39xyj/${f}" \
        "${WORK}/tree/drivers/media/dvb-frontends/drx39xyj/${f}" || true
done

# --- assign next free board id for 461E_V3 ---
python3 - "${WORK}/tree/drivers/media/usb/em28xx/em28xx.h" <<'PY'
from pathlib import Path
import re, sys
p = Path(sys.argv[1])
t = p.read_text()
if "EM28178_BOARD_PCTV_461E_V3" in t:
    print("board id already present")
    raise SystemExit(0)
nums = [int(x) for x in re.findall(r"#define\s+EM28\w+_BOARD_\w+\s+(\d+)", t)]
if not nums:
    raise SystemExit("no board ids found in em28xx.h")
nxt = max(nums) + 1
# Prefer anchoring after PCTV_461E_V2 or last EM28xx board define near HAUPPAUGE/MYGICA
anchor = None
for key in (
    "EM28178_BOARD_PCTV_461E_V2",
    "EM2874_BOARD_HAUPPAUGE_USB_QUADHD",
    "EM2860_BOARD_MYGICA_IGRABBER",
):
    m = re.search(rf"(#define\s+{key}\s+\d+\n)", t)
    if m:
        anchor = m.group(1)
        break
line = f"#define EM28178_BOARD_PCTV_461E_V3                {nxt}\n"
if anchor:
    t = t.replace(anchor, anchor + line, 1)
else:
    t += "\n" + line
p.write_text(t)
print(f"added EM28178_BOARD_PCTV_461E_V3 = {nxt}")
PY

# --- apply patches ---
cd "${WORK}/tree"
log "applying m88ds3103c patch"
patch -p1 < "${PATCH_DIR}/0002-m88ds3103-Implement-3103c-chip-support.patch"

log "applying em28xx 461e v3 patch (skip em28xx.h hunk; board id already set)"
python3 - "${PATCH_DIR}/0003-em28xx-Add-Hauppauge-461e-v3.patch" /tmp/em28xx-461e-v3-filtered.patch <<'PY'
from pathlib import Path
import re, sys
src = Path(sys.argv[1]).read_text()
# Drop em28xx.h file section (board number differs per kernel)
parts = src.split("diff --git ")
out = []
for part in parts[1:]:
    first = part.split("\n", 1)[0]
    if "em28xx.h" in first:
        continue
    out.append("diff --git " + part)
Path(sys.argv[2]).write_text("".join(out))
PY
patch -p1 < /tmp/em28xx-461e-v3-filtered.patch

# Ensure MODULE_FIRMWARE for 3103c
python3 - <<'PY'
from pathlib import Path
p = Path("drivers/media/dvb-frontends/m88ds3103.c")
t = p.read_text()
if "MODULE_FIRMWARE(M88DS3103C_FIRMWARE);" not in t:
    t = t.replace(
        "MODULE_FIRMWARE(M88DS3103B_FIRMWARE);\n",
        "MODULE_FIRMWARE(M88DS3103B_FIRMWARE);\nMODULE_FIRMWARE(M88DS3103C_FIRMWARE);\n",
    )
    p.write_text(t)
print("MODULE_FIRMWARE ok")
PY

# Sanity: USB id present
grep -q '0x0462' drivers/media/usb/em28xx/em28xx-cards.c \
  || die "patch did not add USB id 2013:0462"

# --- assemble flat DKMS tree ---
PKG="${WORK}/pkg"
cp drivers/media/usb/em28xx/em28xx-core.c \
   drivers/media/usb/em28xx/em28xx-i2c.c \
   drivers/media/usb/em28xx/em28xx-cards.c \
   drivers/media/usb/em28xx/em28xx-camera.c \
   drivers/media/usb/em28xx/em28xx-dvb.c \
   drivers/media/usb/em28xx/em28xx.h \
   drivers/media/usb/em28xx/em28xx-reg.h \
   drivers/media/dvb-frontends/m88ds3103.c \
   drivers/media/dvb-frontends/m88ds3103_priv.h \
   "${PKG}/"
cp -a drivers/media/dvb-frontends/*.h "${PKG}/" 2>/dev/null || true
cp -a "${WORK}/tuner-headers/." "${PKG}/"
mkdir -p "${PKG}/drx39xyj"
cp -a drivers/media/dvb-frontends/drx39xyj/*.h "${PKG}/drx39xyj/" 2>/dev/null || true

cat > "${PKG}/Makefile" <<'EOF'
obj-m += em28xx.o
em28xx-y := em28xx-core.o em28xx-i2c.o em28xx-cards.o em28xx-camera.o
obj-m += em28xx-dvb.o
obj-m += m88ds3103.o
ccflags-y += -I$(src) -I$(src)/drx39xyj
EOF

cat > "${PKG}/dkms.conf" <<EOF
PACKAGE_NAME="${PKG_NAME}"
PACKAGE_VERSION="${PKG_VER}"
MAKE="make -C /lib/modules/\${kernelver}/build M=\${dkms_tree}/\${PACKAGE_NAME}/\${PACKAGE_VERSION}/build modules"
CLEAN="make -C /lib/modules/\${kernelver}/build M=\${dkms_tree}/\${PACKAGE_NAME}/\${PACKAGE_VERSION}/build clean"
BUILT_MODULE_NAME[0]="em28xx"
BUILT_MODULE_NAME[1]="em28xx-dvb"
BUILT_MODULE_NAME[2]="m88ds3103"
DEST_MODULE_LOCATION[0]="/updates/dkms"
DEST_MODULE_LOCATION[1]="/updates/dkms"
DEST_MODULE_LOCATION[2]="/updates/dkms"
AUTOINSTALL="yes"
EOF

# --- install firmware ---
if [[ ! -f /lib/firmware/dvb-demod-m88ds3103c.fw ]]; then
  log "installing m88ds3103c firmware"
  curl -fL -o /lib/firmware/dvb-demod-m88ds3103c.fw \
    https://hauppauge.s3.us-east-1.amazonaws.com/linux/dvb-demod-m88ds3103c.fw
fi

# --- DKMS register ---
log "registering DKMS ${PKG_NAME}/${PKG_VER}"
dkms remove "${PKG_NAME}/${PKG_VER}" --all 2>/dev/null || true
rm -rf "${SRC_DST}" "/var/lib/dkms/${PKG_NAME}"
mkdir -p "${SRC_DST}"
cp -a "${PKG}/." "${SRC_DST}/"
dkms add -m "${PKG_NAME}" -v "${PKG_VER}"
dkms build -m "${PKG_NAME}" -v "${PKG_VER}" -k "${KVER}"
# In-tree modules share the same MODULE_VERSION; force install into updates/dkms.
dkms uninstall -m "${PKG_NAME}" -v "${PKG_VER}" -k "${KVER}" 2>/dev/null || true
dkms install -m "${PKG_NAME}" -v "${PKG_VER}" -k "${KVER}" --force

# Softdep so demod is present before bridge probe
cat >/etc/modprobe.d/pctv461-softdep.conf <<'EOF'
softdep em28xx pre: m88ds3103
softdep em28xx post: em28xx-dvb
EOF

depmod -a "${KVER}"

# Reload if possible
modprobe -r em28xx-dvb em28xx-rc em28xx m88ds3103 2>/dev/null || true
modprobe m88ds3103
modprobe em28xx
modprobe em28xx-dvb || true

log "done. dkms status:"
dkms status | grep -E "${PKG_NAME}|em28xx" || dkms status | head -20
log "plug PCTV 461 (2013:0462) and check: dmesg | grep -E '461e v3|M88DS3103C'"
log "expect: /dev/dvb/adapter*/frontend0 and 'Identified as PCTV DVB-S2 Stick (461e v3)'"
