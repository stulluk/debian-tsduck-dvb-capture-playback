# debian-tsduck-dvb-capture-playback

A project to make USB DVB capture/playback devices work on **Debian with a mainline
kernel**, and to build **`dvbhub`** — a hybrid Rust application (single CLI + local
web UI + optional terminal UI) that detects those devices and drives DVB transport-stream
**capture** and **playback** through [TSDuck](https://tsduck.io/).

> Status: TSDuck is built and deployed; `dvbhub` runs on real hardware. Capture
> (DVB-S/S2, DVB-T/T2, DVB-C) works via CLI, web UI, and TUI (`capture --tui`).
> **Playout** to the DTU-315 works via CLI and TUI (`playout --tui`) for satellite,
> terrestrial and cable. Live **watch** (tuner → `tsp` zap → player / GStreamer) is
> available from CLI and TUI. Open items: automated loopback test harness, CI.

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
| "WinTV-Nova-S2" | `2013:0462` | Capture             | `em28xx` (461e v3: patched kernel **or** DKMS) | driver OK (L1)     |

All three verified at driver level on kernel `7.0.0-20260615-sertac-dcn31fix`
(2026-06-15). The same PCTV stick also works on **stock Ubuntu** kernels via a
DKMS overlay (no custom kernel): Ubuntu 22.04 HWE 6.8 (T14) and Ubuntu 26.04 7.0
(gediz-lenovo) — see [`docs/ubuntu_stock_kernel_install.md`](docs/ubuntu_stock_kernel_install.md).
DVB-S capture verified on TBS5530 and PCTV 461 (Turksat 11054).

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

### WinTV-Nova-S2 / PCTV 461 - working (patched kernel **or** DKMS)

This unit (`2013:0462`) is a PCTV 461e v3 (em28178 + **m88ds3103c**).

**Option A — patched custom kernel** (original Debian host):
`7.0.0-20260615-sertac-dcn31fix` with linux-media patches (see
`kernel-deb-package-builder/patches/` and [`docs/driver_bringup.md`](docs/driver_bringup.md)).

**Option B — DKMS overlay on stock Ubuntu/Debian kernels** (preferred when you do
not want a custom kernel). Builds `em28xx` + `m88ds3103` from torvalds sources
matching the running `major.minor`, applies the same two patches, installs as
`pctv461/2026.03.17`:

```bash
sudo ./scripts/install-pctv461-dkms.sh
# Full Ubuntu stack (TBS + Dektec + PCTV DKMS):
sudo ./scripts/ubuntu-dvb-stack-setup.sh
# T14 / jammy HWE 6.8 helper:
sudo ./scripts/t14-dvb-stack-setup.sh
```

Details and host notes (gediz-lenovo, T14): [`docs/ubuntu_stock_kernel_install.md`](docs/ubuntu_stock_kernel_install.md).

## TSDuck build & deploy

TSDuck is built from source in Docker (so no build toolchain lands on the host) with
**Dektec support enabled**, then deployed as a self-contained bundle to `/opt/tsduck`.
The Dektec DTAPI is supplied from the locally downloaded LinuxSDK tarball via the
`DTAPI_ORIGIN` mechanism (same SDK version as the installed driver, no internet
download needed).

```
cd docker/tsduck
./dockerbuild.sh                 # build the builder image (Debian trixie + deps)
./indockerbuild.sh [jobs]        # clone + compile TSDuck (latest release) with Dektec
                                 # jobs default: 80% of host cores (also --cpus limit)
./deploy.sh                      # copy bundle to /opt/tsduck (relocatable, $ORIGIN rpath)
```

Verified: `tsp 3.44`, the `dektec` output plugin and `tsdektec` are present, and all
three devices are visible — `tslsdvb` lists the TBS5530 (two frontends) and PCTV 461,
`tsdektec --all` lists the DTU-315.

## Application (`dvbhub`)

An `x86_64` glibc binary (dynamically linked against GStreamer), built in Docker, that runs on the host
and drives TSDuck via subprocesses. It resolves TSDuck from `$DVBHUB_TSDUCK_DIR`,
then `/opt/tsduck`, then `$PATH`.

```
cd docker/app
./dockerbuild.sh                 # build the Rust builder image (Debian + GStreamer)
./indockerbuild.sh [jobs]        # compile -> build/dvbhub, then install system-wide
                                 # to /usr/local/bin/dvbhub when passwordless sudo works
                                 # jobs default: 80% of host cores; log in build/logs/

# refresh system-wide install without rebuilding:
sudo ./scripts/install-dvbhub-systemwide.sh

# discovery / info:
dvbhub devices                   # discover usable DVB devices (+ USB warnings)
dvbhub devices --json            # machine-readable
dvbhub transponders              # the fixed Turksat 42E reference TPs
dvbhub tsduck                    # show resolved TSDuck dir + version
dvbhub update-satellites-json    # refresh ~/.config/dvbhub/satellites.json (KingOfSat)

# capture (tuner -> file); use --dry-run to print the tsp command:
dvbhub capture --transponder 1 --device-name /dev/dvb/adapter0 \
    --seconds 6 --output ref.ts

# interactive capture wizard (DVB-S; manual setup + catalogue satellites):
dvbhub capture --tui

# playout (file -> DTU-315 modulator, L-band IF):
dvbhub playout --input some.ts --frequency-mhz 1200 \
    --modulation DVB-S2-8PSK --symbol-rate-ksps 30000 --code-rate 3/4 --infinite

# capture one catalogue TP, or a whole satellite into per-TP folders:
dvbhub capture --satellite "Hot Bird 13E" --tp-frequency-mhz 10719 --seconds 600
dvbhub capture --satellite "Hot Bird 13E" --full-satellite --seconds 600 --resume

# interactive playout wizard:
dvbhub playout --tui

# live watch of an FTA service (mpv by default) or the channel-list TUI:
dvbhub watch --tui

# local web UI (browser on localhost):
dvbhub serve

# loopback test (DTU-315 plays out, a tuner re-captures):
dvbhub loopback --input some.ts --if-mhz 1200 \
    --modulation DVB-S2-8PSK --symbol-rate-ksps 30000 --code-rate 3/4 \
    --seconds 15 --output loop.ts
```

### Dependencies

Build: only **Docker** on the host (`docker/app`, `docker/tsduck`,
`docker/gst-mpegtslive`); toolchains and `-dev` packages stay in the images.

Runtime (host):

| Dependency | Debian/Ubuntu package | Used for |
| ---------- | --------------------- | -------- |
| TSDuck bundle (`tsp`, `tsanalyze`, `tslsdvb`, `tsdektec`) | built here, `/opt/tsduck` | all capture / playout / analysis |
| GStreamer 1.x runtime + plugins | `gstreamer1.0-plugins-{base,good,bad}`, `gstreamer1.0-libav` | `dvbhub` is linked against GStreamer; `watch` GStreamer path |
| `dvb-fe-tool` | `dvb-tools` | C/N, signal level and post-BER statistics sampled during capture |
| `mpv` (or `vlc`) | `mpv` | default player for `watch` |
| `ffmpeg` | `ffmpeg` | web UI HLS playback |
| DKMS + kernel headers | `dkms`, `linux-headers-$(uname -r)` | out-of-tree tuner/modulator drivers |

`scripts/ubuntu-dvb-stack-setup.sh` / `t14-dvb-stack-setup.sh` install the distro
packages; `scripts/install-dvbhub-systemwide.sh` warns about missing runtime tools.
Without `dvb-fe-tool`, captures still work but no signal statistics are recorded.

### Signal statistics during capture

TSDuck's own periodic status reports BER only as an integer percentage, too coarse
for DVB-S2. While `tsp` records, `dvbhub` therefore runs `dvb-fe-tool -m` on the
same frontend (read-only, it does not disturb tuning) and samples C/N, level and
post-BER once per second. The first and last locked samples are dropped (settling
/ teardown). The result is printed at the end of each capture:

```
[dvbhub] Signal stats (13 samples): C/N min/avg/max 11.76 / 11.91 / 12.04 dB, level avg -38.5 dBm, postBER avg 1.1e-3 max 4.1e-3
```

and stored as `signal_stats` in the capture's `*_services.json` sidecar.

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

TUI covers Satellite / Terrestrial / Cable capture and playout wizards, plus a
live **watch** path (tuner → tsp zap → player / GStreamer).

Engine modules include `config`, `tsduck`, `usb`, `devices`, `transponders`,
`pipeline`, `satellites`, `sat_capture`, `terr_capture`, `cable_capture`,
`watch*`, `progress`, `signal`, `analyze`, `session`, `web`, `tui`.

> Functional test status (see [`docs/test_log.md`](docs/test_log.md)): DVB-S capture
> and playout verified on hardware (TBS5530 / PCTV 461 capture, DTU-315 playout).
> PCTV lock on stock Ubuntu T14 verified 2026-07-22 (Turksat 11054).

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
│   │   ├── terr_capture.rs, cable_capture.rs, terrestrial.rs, cable.rs
│   │   ├── watch*.rs, channels*.rs  # live watch (tsp zap -> player / GStreamer)
│   │   ├── session.rs, web.rs  # local web UI + session manager
│   │   └── tui/                # ratatui capture/playout/watch wizards
│   └── web/                    # static assets for the browser UI
├── docker/
│   ├── tsduck/                 # Dockerfile + build/deploy scripts for TSDuck
│   ├── app/                    # Dockerfile + build scripts for dvbhub (glibc + GStreamer)
│   └── gst-mpegtslive/         # builds the mpegtslivesrc GStreamer plugin (watch path)
├── completions/                # bash completion (generated by `dvbhub completions bash`)
├── docs/
│   ├── hardware_devices.md     # detected device inventory + evidence
│   ├── driver_bringup.md       # step-by-step driver/firmware bring-up (custom kernel)
│   ├── ubuntu_stock_kernel_install.md  # Ubuntu DKMS path (gediz / T14)
│   ├── design_consideration.md # test methodology + architecture decisions
│   ├── cli_tui_requirements.md # CLI/TUI spec and agreed decisions
│   ├── reference_transponders.md
│   ├── test_log.md
│   └── tsduck_build.md
├── drivers/pctv461/            # vendored 461e v3 patches + DKMS notes
├── scripts/
│   ├── install-dvbhub-systemwide.sh  # /usr/local/bin/dvbhub + completion + TSDuck links
│   ├── install-pctv461-dkms.sh
│   ├── ubuntu-dvb-stack-setup.sh
│   └── t14-dvb-stack-setup.sh
└── build/                      # (gitignored) TSDuck source/bundle, dvbhub binary, logs/
```

## Roadmap

- [x] Detect connected devices, record VID:PID and USB speed
- [x] Confirm the "WinTV-Nova-S2" vs "PCTV 461" identity (confirmed: same unit)
- [x] DTU-315 driver bring-up on 7.0.0 (Dektec LinuxSDK, `Dtu.ko`, L1 verified)
- [x] TBS5530 driver bring-up (cxd28xx + TBS firmware, L1 verified)
- [x] PCTV 461 / Nova-S2 driver bring-up (461e v3 kernel patch, L1 verified)
- [x] PCTV 461 DKMS overlay on stock Ubuntu 6.8 / 7.0 (T14 + gediz-lenovo)
- [x] All 3 devices L1-verified on `7.0.0-20260615-sertac-dcn31fix` (USB hub)
- [x] Build TSDuck in Docker (Dektec enabled), deploy self-contained bundle to `/opt/tsduck`
- [x] `dvbhub` engine + CLI (glibc + GStreamer binary, built in Docker)
- [x] Device discovery + USB diagnostics
- [x] Reference / catalogue transponders (Turksat 42E, KingOfSat refresh)
- [x] DVB-S capture (CLI + web + TUI); manual SAT setup; per-TP output layout
- [x] DVB-T / DVB-C capture + playout TUI paths; live watch
- [x] DTU-315 playout (CLI + TUI)
- [x] Local web UI (`serve`)
- [ ] DVB loopback test harness (L2a/b/c via `tscmp`)
- [ ] CI: Linux `x86_64` release builds (GStreamer linkage rules out a fully static binary)
