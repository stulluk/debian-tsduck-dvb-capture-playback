# TSDuck Build & Deploy (with Dektec support)

Step-by-step record of building TSDuck from source in Docker and deploying a
self-contained bundle to the host. Build toolchains stay inside the container
(host-protection policy); only the finished binaries land on the host.

- Date: 2026-06-15
- Host kernel: `7.0.0-20260615-sertac-dcn31fix`, Debian forky/sid
- TSDuck: latest release tag built = **3.44** (commit count 4676)
- Builder base image: `debian:trixie` (older glibc than the host => binaries are
  forward-compatible and run on sid)
- Dektec DTAPI: from `LinuxSDK_v2026.05.0.tar.gz` (same SDK version as the
  installed `Dtu` driver)

## How TSDuck links Dektec / DTAPI (key facts)

TSDuck does not statically bake Dektec into one monolithic binary. Instead:

- The build calls `scripts/dtapi-config.sh --object --download` (from
  `scripts/make-config.sh`). The `--download` step expands a Dektec LinuxSDK
  tarball and links the precompiled `DTAPI64.o` into the Dektec plugin.
- If the env var **`DTAPI_ORIGIN`** points to a LinuxSDK tarball, that tarball is
  used directly — no internet download, and we pin the exact SDK version that
  matches our kernel driver. This is the mechanism we use.
- To disable Dektec entirely you would pass `NODEKTEC=1` / `NODTAPI=1`; we do
  **not**, because we need DTU-315 playout.

## Static vs dynamic — why a relocatable bundle is the right answer

The "static vs dynamic" question does not map cleanly onto TSDuck: it is built as
`libtscore.so` + `libtsduck.so` + a set of tools and `tsplugin_*.so` plugins. The
decisive detail is that on Linux TSDuck links everything with
`-Wl,-rpath,'$ORIGIN'` (see `scripts/make-config.sh`). That means every tool finds
`libtsduck.so`, the plugins, and the `.names`/`.xml` data files **relative to its
own directory**.

Consequence: the build output directory is a fully **relocatable bundle**. We copy
that one directory to `/opt/tsduck` and it just works — no `ldconfig`, no
`LD_LIBRARY_PATH`, no system files touched. Clean removal is `rm -rf /opt/tsduck`.

## Build steps

```
cd docker/tsduck
./dockerbuild.sh            # debian:trixie + TSDuck build prerequisites
./indockerbuild.sh 28       # clone tsduck, checkout latest tag, compile (28 jobs)
./deploy.sh                 # sudo copy build/tsduck-dist -> /opt/tsduck
```

Build flags used (lean build, Dektec kept): `NOSRT=1 NORIST=1 NOVATEK=1 NOJAVA=1
NOPYTHON=1 NODOC=1` with `DTAPI_ORIGIN=/dtapi/LinuxSDK.tar.gz`.

### Prerequisites installed in the image

From `scripts/install-prerequisites.sh` (Debian path) plus the optional libraries
we keep enabled: `git g++ make cmake locales-all flex bison dos2unix curl
ca-certificates tar zip xz-utils linux-libc-dev dpkg-dev python3
libcurl4-openssl-dev libssl-dev zlib1g-dev pcscd libpcsclite-dev libedit-dev`.

> Fix applied: the first image was missing `libedit-dev`, which broke
> `tsEditLine.cpp` (`editline/readline.h: No such file or directory`). Added
> `libedit-dev` and rebuilt the image. (Alternatively `NOEDITLINE=1` disables it.)

## Verification (on the host, from /opt/tsduck)

```
$ /opt/tsduck/tsp --version
tsp: TSDuck - The MPEG Transport Stream Toolkit - version 3.44-4676

$ ldd /opt/tsduck/tsp | grep tsduck
libtsduck.so => /opt/tsduck/libtsduck.so      # resolved via $ORIGIN rpath

$ /opt/tsduck/tsp -O dektec --help | head -1
Send packets to a Dektec DVB-ASI or modulator device   # Dektec plugin compiled in

$ /opt/tsduck/tsdektec --all
0: DTU-315 (Universal modulator)

$ /opt/tsduck/tslsdvb
0: /dev/dvb/adapter0      ("TurboSight TBS 5530 DVB-T/T2/C/...", DVB-T, DVB-T2, ATSC, ISDB-T, DVB-C, ...)
1: /dev/dvb/adapter0:1:0:0 ("TurboSight TBS 5530 DVB-S/S2/S2X, ...", DVB-S, DVB-S2)
2: /dev/dvb/adapter1      ("Montage Technology M88DS3103C", DVB-S, DVB-S2)
```

All three devices are reachable through TSDuck: DTU-315 (Dektec modulator),
TBS5530 (two frontends), PCTV 461 (M88DS3103C). This is the functional baseline
the `dvbhub` application builds on.
