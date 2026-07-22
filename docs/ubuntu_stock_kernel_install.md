# Ubuntu / Debian stock-kernel host installs

How to bring up the DVB stack when a **full custom kernel rebuild is not
required**: keep the distro kernel (Ubuntu HWE / generic) and install drivers
via **DKMS**, including a PCTV 461e v3 (`2013:0462`) overlay.

For the original Debian forky host with a patched custom kernel, see
[`driver_bringup.md`](driver_bringup.md).

## When to use DKMS vs a patched kernel

| Situation | Approach |
| --------- | -------- |
| Distro kernel already has `2013:0462` + `m88ds3103c` in-tree | Firmware only; no PCTV DKMS |
| Distro kernel lacks 461e v3 (Ubuntu 22.04 HWE **6.8**, Ubuntu 26.04 **7.0** as of 2026-07) | **`scripts/install-pctv461-dkms.sh`** |
| You already maintain a custom kernel (e.g. `*-sertac-dcn31fix`) | Apply patches at kernel build time (see `kernel-deb-package-builder/patches/`) |

DKMS overlays `em28xx`, `em28xx-dvb`, and `m88ds3103` into
`/lib/modules/$(uname -r)/updates/dkms/`, which take precedence over in-tree
modules. No GRUB / initramfs custom kernel entry is required.

## Hosts documented here

### gediz-lenovo (Ubuntu 26.04, stock `7.0.0-*-generic`)

Full stack installed 2026-07 without a custom kernel:

1. Distro packages: `dkms`, matching `linux-headers-$(uname -r)`, `dvb-tools`, GStreamer/mpv as needed.
2. Firmware: `dvb-demod-m88ds3103c.fw`, TBS `dvb-usb-id5530.fw` + `dvb-demod-m88rs6060.fw`.
3. Dektec LinuxSDK `2026.05.0` → `dektec_dtu` DKMS (`Install_Dtu`; headers precheck patched to look for `build/Makefile`).
4. TBS5530 → `koreapyj/cxd28xx` DKMS.
5. PCTV 461 → **`install-pctv461-dkms.sh`** (KERNEL_TAG `v7.0`).
6. Userspace: TSDuck bundle under `/opt/tsduck` (or distro `tsp` if ABI allows) + `dvbhub` in `/usr/local/bin`.

One-shot:

```bash
sudo ./scripts/ubuntu-dvb-stack-setup.sh
# or skip pieces:
sudo SKIP_DEKTEC=1 ./scripts/ubuntu-dvb-stack-setup.sh
```

After reboot / plug-in, `dvbhub devices` listed TBS5530 and PCTV 461.

### lenovo-wifi / T14 (Ubuntu 22.04, HWE `6.8.0-*-generic`)

Earlier bring-up used stock HWE 6.8 + NVIDIA DKMS + TBS + Dektec + `dvbhub`,
but **intentionally omitted** PCTV DKMS (stock `em28xx` does not claim
`2013:0462`). A temporary custom 7.0 kernel was tried and **removed** at the
user’s request — T14 stays on Ubuntu 6.8.

PCTV was added later with the same DKMS overlay against **v6.8** sources:

```bash
# Full T14 path (HWE kernel + stack + PCTV DKMS):
sudo STAGE_DIR=/tmp/dvbhub-t14-stage ./scripts/t14-dvb-stack-setup.sh

# PCTV-only (stack already present):
sudo KERNEL_TAG=v6.8 ./scripts/install-pctv461-dkms.sh
```

Verified 2026-07-22 on T14: `Identified as PCTV DVB-S2 Stick (461e v3)`,
`/dev/dvb/adapter0`, Turksat TP#1 **11054 V 30000** lock + 15 s capture (~125 MB)
with TRT HD services.

## PCTV 461 DKMS overlay (how it works)

Script: [`scripts/install-pctv461-dkms.sh`](../scripts/install-pctv461-dkms.sh)

Patches (vendored): [`drivers/pctv461/patches/`](../drivers/pctv461/patches/)

1. Detect running kernel (`uname -r`) and map to a torvalds tag (`v6.8`, `v7.0`, …), overridable via `KERNEL_TAG`.
2. Download matching `em28xx` + `m88ds3103` sources and the frontend/tuner headers needed to compile out-of-tree.
3. Assign the next free `EM28178_BOARD_PCTV_461E_V3` board id in `em28xx.h` (differs by kernel: e.g. **107** on v6.8, **108** on adapted v7.0).
4. Apply:
   - `0002-m88ds3103-Implement-3103c-chip-support.patch`
   - `0003-em28xx-Add-Hauppauge-461e-v3.patch` (`.h` hunk skipped; board id already set)
5. Assemble a flat DKMS package `pctv461/2026.03.17` and `dkms install --force` (in-tree modules share the same `MODULE_VERSION`, so force is required).
6. Install firmware + `/etc/modprobe.d/pctv461-softdep.conf` (`em28xx` soft-depends on `m88ds3103` / `em28xx-dvb`).

### Verify

```bash
dkms status | grep pctv461
lsusb -d 2013:0462
dmesg | grep -E '461e v3|M88DS3103C|a8293|ts2020'
ls -la /dev/dvb/adapter*/frontend0
modinfo em28xx | grep filename   # expect .../updates/dkms/em28xx.ko
sudo dvbhub capture --transponder 1 --adapter 0 --seconds 10 --output /tmp/tp1.ts
```

### Persistence

`AUTOINSTALL=yes` in `dkms.conf` rebuilds the overlay after kernel upgrades.
Re-run `install-pctv461-dkms.sh` if a new `major.minor` needs a different
`KERNEL_TAG` or if the upstream board-id table moved.

## Related scripts

| Script | Role |
| ------ | ---- |
| `scripts/install-pctv461-dkms.sh` | PCTV-only DKMS overlay |
| `scripts/ubuntu-dvb-stack-setup.sh` | Generic Ubuntu stock-kernel full stack (gediz-style) |
| `scripts/t14-dvb-stack-setup.sh` | T14/jammy HWE 6.8 path (legacy cleanup + NVIDIA + PCTV) |
