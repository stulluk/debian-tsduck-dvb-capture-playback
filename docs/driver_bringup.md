# Driver / Firmware Bring-up Log

Step-by-step record of bringing each DVB device up on the host's mainline kernel.
Each device section records: goal, exact commands, observed output, and the
verification used to confirm success or the reason for failure.

- Host: Debian forky/sid
- Running kernel: `7.0.0-20260606-sertac-dcn31fix` (custom)
- Kernel headers present: `/usr/src/linux-headers-7.0.0-20260606-sertac-dcn31fix`
- Approach (agreed with user): build out-of-tree drivers on the host via DKMS.

Verification is split into two levels:
- **L1 (driver-level)**: module builds, loads, device is recognized in `dmesg`,
  device node appears (`/dev/dvb/adapterN` or Dektec node). Possible now.
- **L2 (functional)**: actual tuning/lock and capture/playback. Requires TSDuck
  (`tslsdvb`, `tsp`), which will be built later. Deferred.

---

## 1. DTU-315 (DekTec, `1297:030f`) - Dtu driver via Dektec LinuxSDK

Goal: load the Dektec USB driver (`Dtu.ko`) so the DTU-315 can be used as a
DVB modulator (playback).

### Important: use the official Dektec LinuxSDK, not `tsduck/dektec-dkms`

The `tsduck/dektec-dkms` GitHub project is **archived/obsolete**. Per its README:
"Starting with version 2024.06.0, the Dektec LinuxSDK includes its own DKMS
installation and this specific project is no longer necessary." Its last release
(`2024.04.0`) does **not** compile on modern kernels.

The correct source is the **Dektec LinuxSDK**, downloaded from the Dektec website:
`https://www.dektec.com/products/SDK/DTAPI/Downloads/LinuxSDK_v2026.05.0.tar.gz`
(version used here: `2026.05.0`). It ships the driver source, the DTAPI C++
library, and its own DKMS install scripts.

### Steps performed (working)

1. Download `LinuxSDK_v2026.05.0.tar.gz` from the Dektec website and extract it:
   ```
   tar xzf LinuxSDK_v2026.05.0.tar.gz   # creates ./LinuxSDK/
   ```
2. Install only the USB (Dtu) driver into DKMS, using the SDK's own script:
   ```
   cd LinuxSDK/Drivers
   sudo ./Install_Dtu
   ```
   `Install_Dtu` runs `Install -d Dtu -r Dtu/Source/Linux/51-dtu.rules
   -n dektec_dtu`, which copies `Dtu` + `DtSal` + `DtDrvCommon` to
   `/usr/src/dektec_dtu-2026.05.0`, generates a Makefile + `dkms.conf`, installs
   the udev rule, and runs `dkms install --force`.

Prerequisites (already satisfied on this host): `dkms` and the running kernel's
headers (`/lib/modules/$(uname -r)/build`).

### One required fix: buggy headers precheck in the SDK Install script

The SDK `Install` script gates on:
```
[[ -d "/lib/modules/$KERNEL/build/kernel" ]] || error "kernel headers are not installed"
```
Standard Debian `linux-headers` packages do **not** ship a `build/kernel/`
subdirectory (only full source trees do), so this precheck wrongly fails even
though the headers are present and modules build fine. Changed the check in our
extracted copy to a correct one:
```
[[ -f "/lib/modules/$KERNEL/build/Makefile" ]] || error "kernel headers are not installed"
```
This is the only modification needed; the driver source itself compiles cleanly
on kernel 7.0.0 with no patches.

### Result: SUCCESS

```
dkms status -> dektec_dtu/2026.05.0, 7.0.0-20260606-sertac-dcn31fix: installed
Building module(s)...... done.
Installing /lib/modules/7.0.0-.../updates/dkms/Dtu.ko.xz
```

### Verification (L1 - driver level): PASS

```
$ sudo modprobe Dtu
$ lsmod | grep Dtu
Dtu                  8884224  0
$ dmesg | grep Dtu
Dtu: The Dtu driver V4.15.5.93 has loaded successfully.
Dtu: < EzUsbInit > FX3 firmware uploaded
usbcore: registered new interface driver Dtu
$ ls -l /dev/Dtu*
crw------- 1 root root 180, 4 ... /dev/Dtu4
```

- Module builds, loads, registers the USB interface driver.
- The DTU-315 (`1297:030f`, Cypress/FX3 "WestBridge") gets its FX3 firmware
  uploaded by the driver and a character device node `/dev/Dtu4` is created.
- (Expected, harmless: "out-of-tree module taints kernel" / unsigned-module
  warning - Secure Boot is off.)

### L2 (functional playback) verification: DEFERRED

Confirming the actual device model (DTU-315) and doing real modulation/playback
requires the DTAPI tools / TSDuck `dektec` plugin, which will be built later.
The DTAPI C++ library ships precompiled in `LinuxSDK/DTAPI/`.

---

## 2. TBS5530 (`734c:5530`) - cxd28xx (out-of-tree DKMS)

Goal: bring up the TBS5530 multi-standard tuner (DVB-T/T2/C/C2 + DVB-S/S2/S2X)
so it appears as a DVB adapter for capture (and modulation).

There is no mainline driver. The out-of-tree driver is
[`koreapyj/cxd28xx`](https://github.com/koreapyj/cxd28xx) (Sony CXD2878 demod +
Montage M88RS6060 satellite demod), plus TBS firmware files.

### Steps performed (working)

```
# 1. Clone and build the driver
git clone --depth 1 https://github.com/koreapyj/cxd28xx.git
cd cxd28xx
make            # builds alp/cxd2878/m88rs6060/tbs5530/it930x .ko (warnings only)

# 2. Install via DKMS (persists across kernel upgrades)
sudo dkms add .
sudo dkms build  cxd28xx/0.0.1
sudo dkms install cxd28xx/0.0.1 --force

# 3. Install TBS firmware (driver fails without it)
curl -fL -o tbs-fw.tar.bz2 \
  http://www.tbsdtv.com/download/document/linux/tbs-tuner-firmwares_v1.0.tar.bz2
tar xjf tbs-fw.tar.bz2 -C /tmp/tbs-fw
sudo install -m644 /tmp/tbs-fw/dvb-usb-id5530.fw \
                   /tmp/tbs-fw/dvb-demod-m88rs6060.fw /lib/firmware/

# 4. (Re)load to re-probe the device
sudo modprobe -r tbs5530; sudo modprobe tbs5530
```

The driver compiles cleanly on kernel 7.0.0 (modern `ccflags-y += -I$(src)`
Makefile, no patching needed). Without the firmware the probe fails with:
`Direct firmware load for dvb-usb-id5530.fw failed with error -2`.

### Result + Verification (L1 - driver level): PASS

```
dkms status -> cxd28xx/0.0.1, 7.0.0-...: installed
dmesg:
  tbs5530: start downloading tbs5530 firmware
  dvbdev: DVB: registering new adapter (TurboSight TBS 5530)
  i2c: Detect CXD2878/CXD6802(SiP) chip.
  DVB: registering adapter 0 frontend 0 (... DVB-T/T2/C/C2,ISDB-T/C,ATSC,J83B,ATSC3)
  i2c: found a 'Montage m88rs6060' ... downloading 'dvb-demod-m88rs6060.fw' ... warm state, firmware version:30
  DVB: registering adapter 0 frontend 0 (TurboSight TBS 5530 DVB-S/S2/S2X)
  tbs5530: TurboSight TBS 5530 attached
```

Device nodes created under `/dev/dvb/adapter0`:
`frontend0` (DVB-T/T2/C/ATSC), `frontend1` (DVB-S/S2/S2X), `demux0`, `dvr0`, `net0`.

(Note: the cold->warm firmware cycle re-enumerates the USB device; the first
probe logs a transient `cxd2878: Can not detect the chip` before the warm-state
retry succeeds. Final state is fully attached.)

### L2 (functional) verification: DEFERRED

Real tuning/lock needs a live antenna feed + TSDuck (`tslsdvb`/`tsp`). The user
has live DVB-S and DVB-T feeds in Istanbul; this will be tested later.

---

## 3. WinTV-Nova-S2 / PCTV 461 (`2013:0462`) - em28xx (mainline) - BLOCKED

Goal: bring up the DVB-S2 USB stick the user calls "Nova-S2" (box label:
PCTV 461) for capture.

### Device identification (confirmed)

`2013:0462 PCTV Systems PCTV 461` is the **Hauppauge WinTV-NOVA-S2 / PCTV 461e
v3 (Rev 3 "BBH9")**: Empia **em28178** USB bridge + Montage **m88ds3103c**
demodulator (the newer "C" silicon revision). It identifies in the driver as
"PCTV DVB-S2 Stick (461e) (card=92)".

### Why it does not work yet (root cause)

This kernel already ships the `em28xx` family and `m88ds3103`/`ts2020` modules,
but **the USB ID `2013:0462` is NOT in the `em28xx` ID table**, and the
`m88ds3103c` (C revision) demod is not supported by the in-tree `m88ds3103`
driver. Evidence:

```
$ lsusb -d 2013:0462
Bus 005 Device 004: ID 2013:0462 PCTV Systems PCTV 461
# interface 5-2.2:1.0 -> driver: (none)

$ modinfo .../em28xx.ko -> no '2013p0462' alias  (USB id not claimed)
$ sudo modprobe em28xx em28xx_dvb
  em28xx: Registered (Em28xx dvb Extension) extension
# em28xx loads fine but does NOT bind 2013:0462 (still no driver on the interface)
```

Support for this exact hardware (USB id `2013:0462` + `m88ds3103c`) was only
submitted to linux-media on **2026-03-17** by Bradford Love
(<https://lwn.net/Articles/1063513/>), as two patches:
- `m88ds3103: Implement 3103c chip support`
- `em28xx: Add Hauppauge 461e v3`

These are newer than / not present in this kernel build (2026-06-06).

### Done so far (safe, prerequisite)

The required demod firmware is already downloaded and installed for when the
patched driver is built:
```
curl -fL -o dvb-demod-m88ds3103c.fw \
  https://hauppauge.s3.us-east-1.amazonaws.com/linux/dvb-demod-m88ds3103c.fw
sudo install -m644 dvb-demod-m88ds3103c.fw /lib/firmware/   # 16 KB, installed
```

### Status: BLOCKED - needs patched em28xx + m88ds3103 (decision required)

Stopping here (not attempting an unattended out-of-tree media build overnight).
Options to discuss/execute next session:
1. Build only patched `em28xx` + `m88ds3103` modules out-of-tree against this
   kernel's headers, applying the "461e v3 / m88ds3103c" patches
   (sources: linux-media patch series; `b-rad-NDi/Ubuntu-media-tree-kernel-builder`;
   edisionnano's patch).
2. Rebuild the kernel with the patches applied in-tree.
3. Use the full `media_build` (tbsdtv or linuxtv) tree with the patches.

Recommended: option 1 (smallest, reversible, fits the host-DKMS approach).
