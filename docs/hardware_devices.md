# Hardware Device Inventory

Detected DVB capture/playback devices on the development host.

- Host kernel: `7.0.0-20260606-sertac-dcn31fix` (custom mainline-based)
- OS: Debian forky/sid
- Detection date: 2026-06-14

## Detected devices

All three devices are currently connected behind a single **VIA Labs USB 2.0 hub**
(`2109:2817`) on `Bus 005`. Every device negotiated **480 Mbps (USB 2.0 High Speed)**.
At detection time none of them had a kernel driver bound and `/dev/dvb` did not exist
yet (drivers/firmware not installed).

| User label      | lsusb product string         | VID:PID     | USB path | Negotiated speed   | Serial          | Driver bound |
| --------------- | ---------------------------- | ----------- | -------- | ------------------ | --------------- | ------------ |
| DTU-315         | DekTec WestBridge (Cypress)  | `1297:030f` | 5-2.3    | 480 Mbps (USB 2.0) | `0000000004BE`  | none         |
| TBS5530         | TBS Technologies China       | `734c:5530` | 5-2.4    | 480 Mbps (USB 2.0) | (none in cold)  | none         |
| "WinTV-Nova-S2" | PCTV Systems "PCTV 461"      | `2013:0462` | 5-2.2    | 480 Mbps (USB 2.0) | `0014237283`    | none         |

> NOTE: the third device enumerates as **"PCTV Systems PCTV 461" (`2013:0462`)**.
> The user confirmed (2026-06-14) that this is the unit referred to as
> "WinTV-Nova-S2" (rebadged hardware; Hauppauge owns the PCTV brand).

## Bus topology (USB 2.0, shared 480 Mbps)

```
Bus 005 root_hub (480M)
  └─ VIA Labs USB2.0 Hub  2109:2817 (480M)
       ├─ 5-2.2  PCTV 461   2013:0462  (480M, drv none)
       ├─ 5-2.3  DekTec      1297:030f  (480M, drv none)
       └─ 5-2.4  TBS 5530    734c:5530  (480M, drv none)
```

Bandwidth note: all three share one USB 2.0 high-speed segment (~480 Mbps theoretical,
~300-400 Mbps practical). A single high-bitrate DVB-S2 transponder (~50-70 Mbps) is
fine, but simultaneous high-bitrate capture + playback through this hub may saturate
the link. The host also has USB 3.x (5000M/10000M/20000M) ports available if higher
throughput or isolation is needed later.

## Per-device USB descriptors

### DTU-315 (DekTec) - `1297:030f`
- bcdUSB 2.00, bcdDevice 1.00
- iManufacturer: Cypress, iProduct: WestBridge
- The Cypress "WestBridge" front-end is the cold/boot state; the DekTec driver
  uploads firmware to bring the device up as a Dektec DTU device.

### TBS5530 - `734c:5530`
- bcdUSB 2.00, bcdDevice 0.01
- No manufacturer/product string descriptors in cold state (iManufacturer=0,
  iProduct=0), consistent with a device waiting for firmware download
  (`dvb-usb-id5530.fw`).

### "PCTV 461" - `2013:0462`
- bcdUSB 2.00, bcdDevice 1.00
- iManufacturer: PCTV, iProduct: PCTV 461, SerialNumber 0014237283

## dmesg evidence (enumeration)

```
usb 5-2:   2109:2817 VIA Labs USB2.0 Hub (high-speed)
usb 5-2.3: 1297:030f Cypress WestBridge  (SerialNumber 0000000004BE)
usb 5-2.2: 2013:0462 PCTV "PCTV 461"     (SerialNumber 0014237283)
usb 5-2.4: 734c:5530 TBS (no strings)
```

## Driver / firmware requirements (to be validated)

| Device    | Driver                                        | Firmware                 | Status        |
| --------- | --------------------------------------------- | ------------------------ | ------------- |
| DTU-315   | Dektec **LinuxSDK** `Dtu.ko` (DKMS)           | uploaded by driver (FX3) | driver OK (L1)|
| TBS5530   | `koreapyj/cxd28xx` (cxd2878 + m88rs6060, DKMS)| `dvb-usb-id5530.fw`      | pending       |
| PCTV 461  | likely mainline `em28xx` (em28178 bridge)?    | demod fw (e.g. m88ds3103)| pending       |

DTU-315 is verified at driver level (see driver_bringup.md). The other two are
candidates and must be verified against the running 7.0.0 kernel.
