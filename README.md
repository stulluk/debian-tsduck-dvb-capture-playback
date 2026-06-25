# debian-tsduck-dvb-capture-playback

A project to make USB DVB capture/playback devices work on **Debian with a mainline
kernel**, and to build **`dvbhub`** — a hybrid Rust application (single CLI + local
web UI + optional terminal UI) that detects those devices and drives DVB transport-stream
**capture** and **playback** through [TSDuck](https://tsduck.io/).

> Status: TSDuck is built and deployed; `dvbhub` runs on real hardware. **DVB-S/S2
> capture** works via CLI, web UI, and TUI (`capture --tui`). **Playout** to the
> DTU-315 works via CLI and TUI (`playout --tui`). DVB-T and DVB-C TUI flows are
> next (web UI has basic terrestrial/cable capture already).

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

| Device          | VID:PID     | Role                | Driver path                        | Status                 |
| --------------- | ----------- | ------------------- | ---------------------------------- | ---------------------- |
| DTU-315         | `1297:030f` | Playback (modulate) | Dektec **LinuxSDK** `Dtu` (DKMS)   | driver OK (L1)         |
| TBS5530         | `734c:5530` | Capture + Playback  | `koreapyj/cxd28xx` (DKMS) + TBS fw | driver OK (L1)         |
| "WinTV-Nova-S2" | `2013:0462` | Capture             | `em28xx` (461e v3 patch in kernel) | driver OK (L1)         |

All three verified at driver level on kernel `7.0.0-20260615-sertac-dcn31fix`
(2026-06-15). DVB-S capture verified on TBS5530; PCTV 461 lock depends on cabling.

All three are currently connected at **USB 2.0 High Speed (480 Mbps)** through a shared
hub. The "WinTV-Nova-S2" is rebadged hardware that reports as "PCTV 461" (`2013:0462`),
confirmed by the user.

## Goals

1. Get each device working on the host's mainline kernel (`7.0.0`), documenting the
   driver + firmware steps reproducibly.
2. Build `dvbhub`: a hybrid Rust application (one binary exposing CLI, local web UI,
   and optional ratatui TUI) that
   - finds DVB devices via `lsusb`, `tslsdvb` and `tsdektec` (the Linux DVB API and
     the Dektec DTAPI, surfaced through TSDuck),
   - prompts the user to plug in a device if none is present,
   - reports a clear diagnostic when a device is present on USB but not usable
     (missing driver/firmware),
   - activates **capture** and/or **playback** workflows depending on the device type,
   - for capture, asks DVB-S / DVB-T / DVB-C and shows the relevant tuning options,
   - for playback, reads modulation parameters from the TS file when possible, or
     asks the user otherwise.

## Design decisions

These were decided together with the user and are intentionally recorded here.

- **Frontend: hybrid Rust backend with CLI + local web UI + optional TUI.** The engine
  is scriptable from the CLI (automation, CI) and exposed as a `localhost` web UI.
  A ratatui TUI (`capture --tui`, `playout --tui`) was added for interactive capture
  and playout on the terminal. See [`docs/design_consideration.md`](docs/design_consideration.md)
  and [`docs/cli_tui_requirements.md`](docs/cli_tui_requirements.md).
- **TSDuck integration: subprocess, not library linking.** `dvbhub` orchestrates the
  `tsp` / `tsanalyze` / `tslsdvb` / `tsdektec` command-line tools and parses their
  output (JSON where available). The app never touches transport-stream packets
  itself; `tsp` does all the heavy lifting. This avoids a fragile Rust-to-C++ FFI
  against libtsduck and decouples the app from TSDuck's ABI. `tsp` stops gracefully
  on `SIGTERM`, so start/stop is clean (not a hard kill).
- **TSDuck deployment: build in Docker, copy a self-contained bundle to the host.**
  TSDuck binaries are linked with `-rpath '$ORIGIN'`, so the build output directory
  (tools + `libtsduck.so` + plugins + data files) is relocatable. It is deployed to
  an isolated prefix `/opt/tsduck` (clean removal = `rm -rf`). Build toolchains stay
  in the container per the host-protection policy.
- **Kernel module build: on the host via DKMS** (the user explicitly allowed this for
  this project, since out-of-tree DVB drivers must be built against the running host
  kernel whose headers are already installed).

## Architecture

```
+---------------------------------------------------+
|                  dvbhub (Rust)                    |
|   CLI  <->  engine  <->  web UI (localhost)       |
|              |                                    |
|              +---- capture/playout TUI (ratatui)  |
|   device discovery | transponders | workflows     |
+------------------------+--------------------------+
                         | spawns / monitors (subprocess)
                         v
       tsp / tsanalyze / tslsdvb / tsdektec  (JSON)
                         |
        +----------------+-----------------+
        |                                  |
   Linux DVB plugin                  Dektec plugin (DTAPI)
        |                                  |
   TBS5530 / PCTV 461                    DTU-315
   (cxd28xx / em28xx)                  (Dektec LinuxSDK)
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
is created. Playout verified via `dvbhub playout`.

### TBS5530 (USB) - working

No mainline driver; uses the out-of-tree [`koreapyj/cxd28xx`](https://github.com/koreapyj/cxd28xx)
driver via DKMS plus TBS firmware:

```
git clone --depth 1 https://github.com/koreapyj/cxd28xx.git
cd cxd28xx && make && sudo dkms add . && sudo dkms install cxd28xx/0.0.1 --force
# firmware (required):
curl -fL http://www.tbsdtv.com/download/document/linux/tbs-tuner-firmwares_v1.0.tar.bz2 \
  | sudo tar -C /lib/firmware/ -jxf -
sudo modprobe tbs5530
```

Verified: builds clean on 7.0.0, registers `/dev/dvb/adapter0` with frontend0
(DVB-T/T2/C/ATSC) and frontend1 (DVB-S/S2/S2X). DVB-S capture verified with `dvbhub`.

### WinTV-Nova-S2 / PCTV 461 - working (patched kernel)

This unit (`2013:0462`) is a PCTV 461e v3 (em28178 + **m88ds3103c**). Support was
added via patched custom kernel `7.0.0-20260615-sertac-dcn31fix` (see
`kernel-deb-package-builder/patches/`). Verified: em28xx binds the device as
"461e v3 (card=108)", m88ds3103c + ts2022 + a8293 chain up, `/dev/dvb/adapter1`.

## TSDuck build & deploy

TSDuck is built from source in Docker (so no build toolchain lands on the host) with
**Dektec support enabled**, then deployed as a self-contained bundle to `/opt/tsduck`.
The Dektec DTAPI is supplied from the locally downloaded LinuxSDK tarball via the
`DTAPI_ORIGIN` mechanism (same SDK version as the installed driver, no internet
download needed).

```
cd docker/tsduck
./dockerbuild.sh                 # build the builder image (Debian trixie + deps)
./indockerbuild.sh 28            # clone + compile TSDuck (latest release) with Dektec
./deploy.sh                      # copy bundle to /opt/tsduck (relocatable, $ORIGIN rpath)
```

Verified: `tsp 3.44`, the `dektec` output plugin and `tsdektec` are present, and all
three devices are visible — `tslsdvb` lists the TBS5530 (two frontends) and PCTV 461,
`tsdektec --all` lists the DTU-315.

## Application (`dvbhub`)

A static `x86_64` musl binary, built in Docker, that runs on the host and drives
TSDuck via subprocesses. It resolves TSDuck from `$DVBHUB_TSDUCK_DIR`, then
`/opt/tsduck`, then `$PATH`.

```
cd docker/app
./dockerbuild.sh                 # build the Rust builder image (rust:alpine, musl)
./indockerbuild.sh               # compile -> build/dvbhub (static binary)

# discovery / info:
build/dvbhub devices             # discover usable DVB devices (+ USB warnings)
build/dvbhub devices --json      # machine-readable
build/dvbhub transponders        # the fixed Turksat 42E reference TPs
build/dvbhub tsduck              # show resolved TSDuck dir + version
build/dvbhub update-satellites-json   # refresh ~/.config/dvbhub/satellites.json (KingOfSat)

# capture (tuner -> file); use --dry-run to print the tsp command:
build/dvbhub capture --transponder 1 --device-name /dev/dvb/adapter1 \
    --seconds 6 --output ref.ts

# interactive capture wizard (DVB-S; manual setup + catalogue satellites):
build/dvbhub capture --tui

# playout (file -> DTU-315 modulator, L-band IF):
build/dvbhub playout --input some.ts --frequency-mhz 1200 \
    --modulation DVB-S2-8PSK --symbol-rate-ksps 30000 --code-rate 3/4 --infinite

# interactive playout wizard:
build/dvbhub playout --tui

# local web UI (browser on localhost):
build/dvbhub serve

# loopback test (DTU-315 plays out, a tuner re-captures):
build/dvbhub loopback --input some.ts --if-mhz 1200 \
    --modulation DVB-S2-8PSK --symbol-rate-ksps 30000 --code-rate 3/4 \
    --seconds 15 --output loop.ts
```

### Capture TUI highlights

Wizard: device → delivery (Satellite / Terrestrial / Cable) → satellite catalogue or
**MANUAL SETUP** → full-sat or single-TP mode → three-panel run view (TP queue,
services, live signal metrics).

- **MANUAL SETUP:** single-page form (frequency, SR, polarity, modulation, FEC,
  DiSEqC 1.0/1.1 via ←→, LNB presets).
- **Run screen:** Quality (SNR), Strength, BER, Bitrate in the top-right panel.
- **Keys:** `Esc` back one step; during capture `Esc`/`q` asks Y/N to stop `tsp`.
- Satellite catalogue: `~/.config/dvbhub/satellites.json` (refresh with
  `update-satellites-json`).

Terrestrial and Cable entries in the delivery picker currently show “coming soon” in
the TUI; the web UI already exposes basic DVB-T/T2 and DVB-C capture forms.

Engine modules: `config`, `tsduck`, `usb`, `devices`, `transponders`, `pipeline`,
`satellites`, `sat_capture`, `progress`, `signal`, `analyze`, `session`, `web`, `tui`.

> Functional test status (see [`docs/test_log.md`](docs/test_log.md)): DVB-S capture
> and playout command structures verified on hardware (TBS5530 capture, DTU-315
> playout). PCTV 461 satellite lock depends on cabling; loopback L2 tests pending.

## Repository layout

```
.
├── README.md
├── app/                        # dvbhub Rust application
│   ├── Cargo.toml
│   ├── data/kingofsat/         # KingOfSat INI sources for satellite catalogue
│   ├── src/
│   │   ├── main.rs             # CLI entry point
│   │   ├── config.rs, tsduck.rs, usb.rs, devices.rs
│   │   ├── pipeline.rs, progress.rs, signal.rs, analyze.rs
│   │   ├── satellites.rs, sat_capture.rs, transponders.rs
│   │   ├── session.rs, web.rs  # local web UI + session manager
│   │   └── tui/                # ratatui capture/playout wizards
│   └── web/                    # static assets for the browser UI
├── docker/
│   ├── tsduck/                 # Dockerfile + build/deploy scripts for TSDuck
│   └── app/                    # Dockerfile + build scripts for dvbhub (musl static)
├── docs/
│   ├── hardware_devices.md     # detected device inventory + evidence
│   ├── driver_bringup.md       # step-by-step driver/firmware bring-up
│   ├── design_consideration.md # test methodology + architecture decisions
│   ├── cli_tui_requirements.md # CLI/TUI spec and agreed decisions
│   ├── reference_transponders.md
│   ├── test_log.md
│   └── tsduck_build.md
└── build/                      # (gitignored) cloned TSDuck source, bundle, dvbhub binary
```

## Roadmap

- [x] Detect connected devices, record VID:PID and USB speed
- [x] Confirm the "WinTV-Nova-S2" vs "PCTV 461" identity (confirmed: same unit)
- [x] DTU-315 driver bring-up on 7.0.0 (Dektec LinuxSDK, `Dtu.ko`, L1 verified)
- [x] TBS5530 driver bring-up (cxd28xx + TBS firmware, L1 verified)
- [x] PCTV 461 / Nova-S2 driver bring-up (461e v3 kernel patch, L1 verified)
- [x] All 3 devices L1-verified on `7.0.0-20260615-sertac-dcn31fix` (USB hub)
- [x] Build TSDuck in Docker (Dektec enabled), deploy self-contained bundle to `/opt/tsduck`
- [x] `dvbhub` engine + CLI (static musl binary, built in Docker)
- [x] Device discovery + USB diagnostics
- [x] Reference / catalogue transponders (Turksat 42E, KingOfSat refresh)
- [x] DVB-S capture (CLI + web + TUI); manual SAT setup; per-TP output layout
- [x] DTU-315 playout (CLI + TUI)
- [x] Local web UI (`serve`)
- [ ] DVB-T / DVB-C capture TUI (web forms exist; TUI “coming soon” today)
- [ ] DVB loopback test harness (L2a/b/c via `tscmp`)
- [ ] CI: static cross-platform release builds
