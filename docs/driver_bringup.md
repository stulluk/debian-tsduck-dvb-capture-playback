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

## 2. TBS5530 (`734c:5530`) - cxd28xx

Pending (will start after DTU-315 direction is decided).

---

## 3. WinTV-Nova-S2 / PCTV 461 (`2013:0462`) - em28xx (mainline)

Pending. Note: `em28xx` modules are already present in the running kernel
(`em28xx.ko.xz`, `em28xx-dvb.ko.xz`), so this device may work with the in-tree
driver plus the correct demod firmware - no out-of-tree build expected.
