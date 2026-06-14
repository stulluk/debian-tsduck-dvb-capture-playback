# debian-tsduck-dvb-capture-playback

A project to make USB DVB capture/playback devices work on **Debian with a mainline
kernel**, and to build a **terminal UI (TUI)** that detects those devices and drives
DVB transport-stream **capture** and **playback** through [TSDuck](https://tsduck.io/).

> Status: early bring-up / design phase. Hardware has been detected; driver and TUI
> work is being planned step by step.

## Background

This continues earlier work documented on the author's blog and repositories:

- [Dektec DTU-315 and Linux - Intro to TSduck](https://www.drejo.com/blog/dtu-315-linux/)
- [Capture and Playback with TSduck under linux](https://www.drejo.com/blog/tsduck-capture-playback/)
- [gitlab.com/stulluk/tsduck-capture-playback](https://gitlab.com/stulluk/tsduck-capture-playback) (existing bash scripts)
- DTU-315 driver notes: [tsduck/dektec-dkms#6](https://github.com/tsduck/dektec-dkms/issues/6)
  (NOTE: the `tsduck/dektec-dkms` project is now archived/obsolete; use the
  official Dektec LinuxSDK instead, see Drivers below)
- TBS5530 out-of-tree driver: [koreapyj/cxd28xx](https://github.com/koreapyj/cxd28xx)

## Hardware

Three USB devices are targeted (see [`docs/hardware_devices.md`](docs/hardware_devices.md)
for the full inventory and detection evidence):

| Device          | VID:PID     | Role                | Driver path                        | Status        |
| --------------- | ----------- | ------------------- | ---------------------------------- | ------------- |
| DTU-315         | `1297:030f` | Playback (modulate) | Dektec **LinuxSDK** `Dtu` (DKMS)   | driver OK (L1)|
| TBS5530         | `734c:5530` | Capture + Playback  | `koreapyj/cxd28xx` (DKMS)          | pending       |
| "WinTV-Nova-S2" | `2013:0462` | Capture             | mainline `em28xx`? (to confirm)    | pending       |

All three are currently connected at **USB 2.0 High Speed (480 Mbps)** through a shared
hub. The "WinTV-Nova-S2" is rebadged hardware that reports as "PCTV 461" (`2013:0462`),
confirmed by the user.

## Goals

1. Get each device working on the host's mainline kernel (`7.0.0`), documenting the
   driver + firmware steps reproducibly.
2. Build `tsduck-tui`: a Rust/ratatui terminal application that
   - finds DVB devices via `lsusb` and the Linux DVB API,
   - prompts the user to plug in a device if none is present,
   - checks whether the right kernel driver is loaded and loads/installs it if not,
   - reports a clear diagnostic when a device fails to come up,
   - activates **capture** and/or **playback** workflows depending on the device type,
   - for capture, asks DVB-S / DVB-T / DVB-C and shows the relevant tuning options,
   - for playback, reads modulation parameters from the TS file when possible, or
     asks the user otherwise.

## Design decisions

These were decided together with the user and are intentionally recorded here.

- **TUI language: Rust + [ratatui](https://ratatui.rs/).** Modern TUI ergonomics,
  static cross-platform binaries (aligns with the CI release strategy).
- **TSDuck integration: subprocess, not library linking.** The TUI orchestrates the
  `tsp` / `tsanalyze` / `tslsdvb` command-line tools and parses their **`--json`**
  output (via `serde_json`). The TUI never touches transport-stream packets itself;
  `tsp` does all the heavy lifting. This mirrors the existing bash scripts, avoids a
  fragile Rust-to-C++ FFI against libtsduck (which has no stable C/Rust binding), and
  decouples the TUI from TSDuck's ABI.
- **TSDuck deployment: build in Docker, copy a portable binary to the host.** `tsp`
  and its plugins must run on the host (hardware access), but build toolchains stay in
  the container per the host-protection policy.
- **Kernel module build: on the host via DKMS** (the user explicitly allowed this for
  this project, since out-of-tree DVB drivers must be built against the running host
  kernel whose headers are already installed at
  `/usr/src/linux-headers-7.0.0-20260606-sertac-dcn31fix`).

## Architecture (planned)

```
+---------------------------------------------------+
|                 tsduck-tui (Rust)                 |
|  device detect | driver check | workflow wizard   |
+------------------------+--------------------------+
                         | spawns / monitors
                         v
            tsp / tsanalyze / tslsdvb  (--json)
                         |
        +----------------+-----------------+
        |                                  |
   tsplugin_dvb.so                   tsplugin_dektec.so
   (Linux DVB API)                   (Dektec DTAPI)
        |                                  |
   TBS5530 / PCTV 461                    DTU-315
   (cxd28xx / em28xx)                  (dektec-dkms)
```

## Drivers (bring-up)

Full step-by-step bring-up notes with verification are in
[`docs/driver_bringup.md`](docs/driver_bringup.md).

### DTU-315 (Dektec, USB) - working

Do **not** use the archived `tsduck/dektec-dkms` project. Instead download the
official **Dektec LinuxSDK** from the Dektec website (DTAPI / SDK downloads):

```
# 1. Download (version used here: 2026.05.0)
#    https://www.dektec.com/products/SDK/DTAPI/Downloads/LinuxSDK_v2026.05.0.tar.gz
tar xzf LinuxSDK_v2026.05.0.tar.gz

# 2. Install only the USB (Dtu) driver into DKMS (needs `dkms` + kernel headers)
cd LinuxSDK/Drivers
sudo ./Install_Dtu
```

The SDK ships its own DKMS install scripts (`Install`, `Install_Dtu`, ...), the
driver source, and the precompiled DTAPI C++ library under `LinuxSDK/DTAPI/`.
On this host one small fix was needed: the SDK `Install` script's kernel-headers
precheck looks for a `build/kernel/` dir that Debian `linux-headers` packages do
not ship; it was changed to check `build/Makefile` instead. The driver source
itself compiles cleanly on the 7.0.0 kernel without patches.

Verified: `Dtu` module loads, the DTU-315 firmware is uploaded, and `/dev/Dtu4`
is created. Functional (playback) verification is deferred to the TSDuck stage.

## Repository layout

```
.
├── README.md
├── docs/
│   ├── hardware_devices.md     # detected device inventory + evidence
│   └── driver_bringup.md       # step-by-step driver/firmware bring-up + verification
└── (tsduck-tui/, docker/  -- to be added)
```

## Roadmap

- [x] Detect connected devices, record VID:PID and USB speed
- [x] Confirm the "WinTV-Nova-S2" vs "PCTV 461" identity (confirmed: same unit)
- [x] DTU-315 driver bring-up on 7.0.0 (Dektec LinuxSDK, `Dtu.ko`, L1 verified)
- [ ] TBS5530 driver bring-up (cxd28xx)
- [ ] PCTV 461 / Nova-S2 driver bring-up (em28xx)
- [ ] Build TSDuck in Docker, deploy portable `tsp` to host
- [ ] Scaffold `tsduck-tui` (Rust + ratatui)
- [ ] Implement device detection + driver-state diagnostics
- [ ] Implement capture wizard (DVB-S/T/C)
- [ ] Implement playback wizard (auto-detect modulation params)
- [ ] CI: static cross-platform release builds
