# debian-tsduck-dvb-capture-playback

A project to make USB DVB capture/playback devices work on **Debian with a mainline
kernel**, and to build **`dvbhub`** — a hybrid Rust application (single CLI + local
web UI + optional terminal UI) that detects those devices and drives DVB transport-stream
**capture** and **playback** through [TSDuck](https://tsduck.io/).

> Status (2026-10-20): TSDuck is built and deployed; `dvbhub` runs on real hardware.
> Capture, channel scan and live watch (DVB-S/S2, DVB-T/T2, DVB-C) work from the CLI,
> the TUIs and the browser UI; DTU-315 playout (S/T/C) works from all three. Live TV
> plays in the browser (HLS, first frame ≈ 3–4 s). A loopback harness checks L2a/b/c.
> See [`docs/work_plan.md`](docs/work_plan.md) for the verification matrix.

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

An `x86_64` glibc binary built in Docker. No media libraries are linked: TSDuck,
the players and ffmpeg run as subprocesses. TSDuck is resolved from
`$DVBHUB_TSDUCK_DIR`, then `/opt/tsduck`, then `$PATH`.

```
cd docker/app
./dockerbuild.sh                 # build the Rust builder image (Debian + Rust)
./indockerbuild.sh [jobs]        # compile -> build/dvbhub, then install system-wide
                                 # to /usr/local/bin/dvbhub when passwordless sudo works
                                 # jobs default: 80% of host cores; log in build/logs/
sudo ./scripts/install-dvbhub-systemwide.sh   # refresh the system-wide install only
scripts/ci-build.sh              # what CI runs: tests (warnings = errors) + dist/ binary
```

### Command line

```
# discovery / info
dvbhub devices [--json]          # usable DVB devices (+ USB-present-but-unusable warnings)
dvbhub tsduck                    # resolved TSDuck dir + version
dvbhub update-satellites-json    # refresh ~/.config/dvbhub/satellites.json (KingOfSat)

# capture (tuner -> file); --dry-run prints the tsp command
dvbhub capture --satellite "Hot Bird 13E" --tp-frequency-mhz 10719 --seconds 600
dvbhub capture --satellite "Hot Bird 13E" --full-satellite --seconds 60 --resume
dvbhub capture --tui             # wizard: tuner -> S/T/C -> catalogue or MANUAL SETUP

# channel scan (saves FTA TV + radio with --save)
dvbhub scan --satellite "Hot Bird 13E" --save            # every TP of the satellite
dvbhub scan --satellite "Hot Bird 13E" --tp 10719v --tp 10930h
dvbhub scan --terrestrial [--frequency-mhz 618] --save   # empty = UHF 21-69, T2 then T
dvbhub scan --cable --frequency-mhz 722 --symbol-rate-ksps 6875 --modulation 256-QAM --save

# watch (tuner -> tsp zap -> mpv, else vlc, or --player "…")
dvbhub watch --tui               # channel list: / filter, Enter watch, s scan
dvbhub watch --channel "TVP INFO HD"
dvbhub watch --channel "TVP INFO HD" --test --duration 15   # headless: ffmpeg decodes, PASS/FAIL

# playout (file -> DTU-315)
dvbhub playout --tui             # picks parameters from the capture's _services.json
dvbhub playout --input some.ts --frequency-mhz 1200 \
    --modulation DVB-S2-8PSK --symbol-rate-ksps 30000 --code-rate 3/4 --infinite

# loopback (DTU-315 plays, a tuner re-captures, then L2a/L2b/L2c comparison)
dvbhub loopback --input some.ts --if-mhz 1200 --modulation DVB-S2-8PSK \
    --symbol-rate-ksps 30000 --code-rate 3/4 --seconds 15 --output loop.ts
dvbhub loopback --delivery terrestrial --input t2.ts --frequency-mhz 618 \
    --modulation DVB-T2 --seconds 15 --output loop_t2.ts
dvbhub compare --source some.ts --capture loop.ts       # comparison only

# browser UI
dvbhub serve                     # http://127.0.0.1:8080 (localhost only)
```

### Web UI (`dvbhub serve`)

Landing page with four tiles (devices, capture, playback, watch). A hot-plug
monitor (netlink uevents, no polling) keeps the top-right badges current — one badge
per physical device — and greys out tiles the connected hardware cannot serve. The
header shows the running job (recording / scanning / watching / playout) with a
stop button; the activity log at the bottom streams every tool line (also written
to `~/.config/dvbhub/dvbhub.log`).

- **TS capture:** tuner → S / T / C tabs (from the tuner's delivery systems).
  Satellite: single TP (reference list or satellite catalogue), whole satellite, or
  manual TP ("save as reference transponder"); satellite and TP add / edit / remove,
  KingOfSat import. Live countdown, signal metrics and saved file list.
- **TS playback:** DTU-315 playout for satellite, terrestrial and cable. Selecting a
  capture fills the form from its `_services.json`; "save as reference capture".
- **Watch TV:** channel list per delivery, grouped by satellite, with a filter; click
  plays in the browser. Scan tab for S / T / C with live per-frequency results.
- **Settings:** capture directory.

Tuner jobs (monitor / capture / scan / watch) share the tuner and replace each other;
DTU-315 playout runs independently, so a loopback (play out + capture or scan) works
from the browser.

**Browser playback:** `tsp -P zap` → ffmpeg → HLS (fMP4, 1 s segments) on tmpfs
(`$XDG_RUNTIME_DIR/dvbhub-hls`). Video is re-encoded to 8-bit H.264 because
broadcast video is often interlaced open-GOP H.264 (no IDR frames, rejected by
browser decoders), MPEG-2 (not playable in browsers at all) or 10-bit HEVC. The
quality selector next to the player offers **720p** (default, x264, ≈ 0.5 core) or
**Source** — the native resolution. With a VAAPI GPU (`/dev/dri/renderD128`,
checked at start-up) Source decodes, deinterlaces and encodes entirely on the GPU
at 25 fps: UHD 3840×2160 plays with ffmpeg at ≈ 5 % of one core instead of 6–10
cores with x264 (which remains the fallback without VAAPI). An integrated GPU
encodes UHD H.264 at only ~45–55 fps, hence 25 fps. Audio becomes AAC.

### Channel database

`~/.config/dvbhub/channels.json`. Each channel stores the full tuning (satellite,
LNB, DiSEqC, frequency, polarity, symbol rate, modulation …) and the model of the
tuner that found it. Playback picks a tuner by delivery and model, never by the
`/dev/dvb/adapterN` number (which follows plug order). Scans keep only services
that carry video (TV) or audio (radio): data services labelled "TV" in the SDT and
FTA entries that are off air are skipped.

### Dependencies

Build: only **Docker** on the host (`docker/app`, `docker/tsduck`); toolchains and
`-dev` packages stay in the images.

Runtime (host):

| Dependency | Debian/Ubuntu package | Used for |
| ---------- | --------------------- | -------- |
| TSDuck bundle (`tsp`, `tsanalyze`, `tslsdvb`, `tsdektec`) | built here, `/opt/tsduck` | all capture / playout / scan / analysis |
| `dvb-fe-tool` | `dvb-tools` | C/N, signal level and post-BER statistics sampled during capture |
| `mpv` (or `vlc`) | `mpv` | player for `watch` (mpv preferred, vlc fallback, or `--player`) |
| `ffmpeg` (with libx264) | `ffmpeg` | browser playback (HLS), `watch --test` |
| DKMS + kernel headers | `dkms`, `linux-headers-$(uname -r)` | out-of-tree tuner/modulator drivers |

`scripts/ubuntu-dvb-stack-setup.sh` / `t14-dvb-stack-setup.sh` install the distro
packages; `scripts/install-dvbhub-systemwide.sh` warns about missing runtime tools.
Without `dvb-fe-tool`, captures still work but no signal statistics are recorded.

### Signal statistics during capture

TSDuck's own periodic status reports BER only as an integer percentage, too coarse
for DVB-S2. While `tsp` records, `dvbhub` runs `dvb-fe-tool -m` on the same frontend
(read-only, it does not disturb tuning) and samples C/N, level and post-BER once per
second. The first and last locked samples are dropped (settling / teardown):

```
[dvbhub] Signal stats (13 samples): C/N min/avg/max 11.76 / 11.91 / 12.04 dB, level avg -38.5 dBm, postBER avg 1.1e-3 max 4.1e-3
```

The same values are stored as `signal_stats` in the capture's `*_services.json`.

### Loopback comparison (L2a / L2b / L2c)

A loopback re-capture is never byte-identical to its source (random start offset,
null-packet stuffing, loop seams), so `dvbhub compare` checks layers:

- **L2a link** — no continuity errors after the lead-in (first 2 % of packets);
- **L2b structure** — same transport-stream id and service ids;
- **L2c ES** — every video payload packet of the re-capture exists bit-for-bit in the
  source (≥ 99.9 %).

### TUI highlights

- Capture: device → delivery → satellite catalogue or **MANUAL SETUP** (one-page
  form, DiSEqC 1.0/1.1, LNB presets) → full satellite or single TP → three-panel run
  view (TP queue, services, Quality / Strength / BER / Bitrate). Settings (`s`):
  capture directory and seconds per TP (←/→, accelerates when held, up to 1 h).
- Watch: channel list per delivery, `/` filter, Enter watches in mpv / vlc, closing
  the player returns to the list; `s` scans (satellite catalogue, UHF, cable).
- Playout: file picker; parameters from the `_services.json` sidecar or S/T/C forms.
- `Esc` goes back one screen; while recording / playing out / scanning it asks Y/N.

## Repository layout

```
.
├── README.md
├── .github/workflows/build.yml # CI: tests + Linux x86_64 binary (release on v* tags)
├── app/                        # dvbhub Rust application
│   ├── Cargo.toml
│   ├── data/kingofsat/         # KingOfSat INI seeds for the satellite catalogue
│   ├── src/
│   │   ├── main.rs             # CLI entry point
│   │   ├── config.rs, tsduck.rs, usb.rs, devices.rs, monitor.rs (hot-plug)
│   │   ├── pipeline.rs, progress.rs, signal.rs, fe_stats.rs, analyze.rs
│   │   ├── satellites.rs, sat_capture.rs, terr_capture.rs, cable_capture.rs
│   │   ├── channel_scan.rs, channels.rs, channels_import.rs
│   │   ├── watch.rs            # tsp zap | mpv / vlc (+ --test via ffmpeg)
│   │   ├── loopback_compare.rs # L2a / L2b / L2c
│   │   ├── session.rs, web.rs  # web UI server and job manager
│   │   └── tui/                # ratatui capture / playout / watch / scan screens
│   └── web/                    # browser UI: index.html, app.css, js/*.js, hls.min.js
├── docker/
│   ├── tsduck/                 # Dockerfile + build/deploy scripts for TSDuck
│   └── app/                    # Dockerfile + build scripts for dvbhub
├── completions/                # bash completion (generated by `dvbhub completions bash`)
├── docs/                       # design, requirements, work plan, test log, bring-up notes
├── drivers/pctv461/            # vendored 461e v3 patches + DKMS notes
├── scripts/
│   ├── ci-build.sh             # CI build (also runnable locally)
│   ├── install-dvbhub-systemwide.sh  # /usr/local/bin/dvbhub + completion + TSDuck links
│   ├── install-pctv461-dkms.sh
│   ├── ubuntu-dvb-stack-setup.sh
│   └── t14-dvb-stack-setup.sh
└── build/, dist/               # (gitignored) TSDuck source/bundle, binaries, logs
```

## Roadmap

- [x] Detect connected devices, record VID:PID and USB speed
- [x] Confirm the "WinTV-Nova-S2" vs "PCTV 461" identity (confirmed: same unit)
- [x] DTU-315 driver bring-up on 7.0.0 (Dektec LinuxSDK, `Dtu.ko`, L1 verified)
- [x] TBS5530 driver bring-up (cxd28xx + TBS firmware, L1 verified)
- [x] PCTV 461 / Nova-S2 driver bring-up (461e v3 kernel patch, L1 verified)
- [x] PCTV 461 DKMS overlay on stock Ubuntu 6.8 / 7.0 (T14 + gediz-lenovo)
- [x] Build TSDuck in Docker (Dektec enabled), deploy self-contained bundle to `/opt/tsduck`
- [x] `dvbhub` engine + CLI, TUIs and web UI
- [x] Device discovery, hot-plug monitor, USB diagnostics
- [x] Satellite catalogue (KingOfSat), reference TPs, per-TP output layout + sidecars
- [x] Capture S/T/C (CLI, TUI, web) with signal statistics
- [x] Channel scan S/T/C (CLI, TUI, web); channel DB with satellite / LNB / tuner model
- [x] Live watch: mpv / vlc (CLI, TUI) and in the browser (HLS)
- [x] DTU-315 playout S/T/C (CLI, TUI, web)
- [x] Loopback harness with L2a / L2b / L2c comparison (`loopback`, `compare`)
- [x] CI: Linux `x86_64` build (`.github/workflows/build.yml`)
