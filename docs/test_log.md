# Capture / Playback / Loopback Test Log

Step-by-step record of the first functional (L2) test attempts with the real
hardware, the verified TSDuck command set, and the two hardware blockers found.

- Date: 2026-06-15
- TSDuck: 3.44 at `/opt/tsduck`
- Devices (L1 OK): DTU-315 (Dektec modulator), TBS5530 (tuner), PCTV 461 (tuner)
- Cabling reported by user: live satellite -> PCTV 461; DTU-315 RF out -> TBS5530 RF in

## Verified TSDuck command set

DVB capture (`tsp -I dvb`) accepted values match the reference transponders
exactly: `--delivery-system DVB-S2`, `--modulation 8-PSK|QPSK`,
`--fec-inner 3/4|5/6`, `--polarity vertical|horizontal`. Useful options:
`--device-name /dev/dvb/adapterA[:F]` (frontend select), `--signal-timeout S`,
`--lnb low[,high,switch]` (use `--lnb 0` for direct L-band injection).

Dektec playout (`tsp -O dektec`): `-m DVB-S2-8PSK|DVB-S2-QPSK`, `--symbol-rate`,
`-r 3/4` (convolutional rate), `-f <Hz>` (intermediate/L-band frequency,
950-2150 MHz range), `--instant-detach`, optional `--level` (0.1 dBm units).

These are produced verbatim by `dvbhub capture|play|loopback --dry-run`.

## Test 1a - Live capture from PCTV 461 (DVB-S) - BLOCKED (no signal lock)

(Initial attempts, hub attached, adapter1.)

Tried all four reference transponders on `/dev/dvb/adapter1` (PCTV 461), covering
both bands and both polarities (13 V / 18 V, tone off/on):

| TP | Params                          | Result                         |
| -- | ------------------------------- | ------------------------------ |
| 1  | 11054 V 30000 8-PSK 3/4 (low)   | no signal lock (12 s timeout)  |
| 2  | 11096 H 30000 QPSK 5/6 (low)    | no signal lock                 |
| 3  | 12245 H 27500 8-PSK 5/6 (high)  | no signal lock                 |
| 4  | 12380 V 27500 QPSK 3/4 (high)   | no signal lock                 |

The demod is healthy: `tsp` tuned correctly and `dmesg` shows the m88ds3103c
loading firmware and going cold->warm on each attempt. A quick check on the
TBS5530 DVB-S/S2 frontend (`/dev/dvb/adapter0:1`) also did not lock. Conclusion:
no usable satellite signal is reaching either tuner right now.

**Action needed (user):** verify the satellite cable is connected to PCTV 461 and
that the LNB is powered (check the F-connector / multiswitch / dish). Once a
transponder locks, `dvbhub capture --transponder 1 --device-name /dev/dvb/adapter1
--seconds 6 --output ref.ts` will record it.

## Test 2 - Loopback DTU-315 -> TBS5530 - BLOCKED (DTU-315 USB power)

Played `Turksat_11054_v_30000_*.ts` out of the DTU-315 (DVB-S2 8-PSK 3/4, 30 MS/s,
L-band 1200 MHz) while capturing on the TBS5530 S/S2 frontend with `--lnb 0`.

The DTU-315 failed as soon as modulation started:

```
* Error: dektec: error getting output fifo load: DTAPI_E_EVENT_POWER (DTAPI status 4230)
dmesg: usb 8-2.2: USB disconnect, device number 4
dmesg: Dtu: ... powerdown occured!
```

After this the DTU-315 was no longer present on USB (`lsusb` lost `1297:030f`,
`tsdektec --all` empty). The device draws more current when actively modulating
than the current hub provides, browns out, and disconnects.

**Action needed (user):** power the DTU-315 from a **powered USB hub** or a direct
motherboard port (not a bus-powered hub), then replug it. Re-check with
`tsdektec --all`. Once stable, run:

```
dvbhub loopback \
  --input /media/G550/TSFILES/Turksat_11054_v_30000_18_Oct_2023.ts \
  --if-mhz 1200 --modulation DVB-S2-8PSK --symbol-rate-ksps 30000 --code-rate 3/4 \
  --seconds 15 --output loop.ts
```

## Comparison plan (once Test 2 produces a capture)

Per `docs/design_consideration.md`, the loopback capture is **not** byte-identical
to the source (re-modulation + re-demodulation reorders/stuffs at the TS layer).
Compare semantically:

- **L2a** link health: `tsanalyze loop.ts` (continuity errors, bitrate, lock).
- **L2b** structure: compare PAT/PMT/service list and PIDs (`tstables`, `tsanalyze`).
- **L2c** elementary-stream bit-exactness: extract a PID from both source and
  capture and compare payloads with `tscmp` / `tspacketize`.

## Test 1b - Cross-check: same antenna on TBS5530 DVB-S/S2 - PASS (2026-06-15)

User verified the same satellite cable on an STB: TP lock at ~98% quality. USB hub
removed; all three devices on separate host ports. Adapter mapping changed:
PCTV 461 = `adapter0`, TBS5530 = `adapter1` (S/S2 = `adapter1:1`).

Antenna connected to TBS5530 DVB-S/S2 input:

```
dvbhub capture --transponder 1 --device-name /dev/dvb/adapter1:1 --seconds 6 \
  --output ref_tbs_tp1.ts   # PASS, 57.6 MB
dvbhub capture --transponder 3 --device-name /dev/dvb/adapter1:1 --seconds 4 \
  --output ref_tbs_tp3.ts   # PASS, 42.6 MB
```

`tsanalyze ref_tbs_tp1.ts`: TS id 0x7919, 10 services (TRT HD group), ~65.3 Mbps.
Conclusion: satellite signal and tuning parameters are correct; the PCTV 461 lock
failure is **device-specific** (likely LNB power / a8293 SEC on the 461e v3 patch).

PCTV 461 on `adapter0` still does not lock with the same cable (not retested after
moving cable to TBS; prior direct-port attempts also failed).

## Test 2b - Loopback DTU-315 -> TBS5530 - PASS (2026-06-15, direct USB)

After moving DTU-315 to a direct host USB port (no hub), loopback succeeded:

```
dvbhub loopback --input Turksat_11054_v_30000_*.ts --if-mhz 1200 \
  --modulation DVB-S2-8PSK --symbol-rate-ksps 30000 --code-rate 3/4 \
  --tuner-device /dev/dvb/adapter1:1 --seconds 10 --output loop_capture.ts
```

Result: 92 MB capture; DTU-315 still present after test (`tsdektec --all` OK).

L2a: bitrate ~65.3 Mbps, some transport errors (1238 packets) - acceptable for first
pass; L2b: same TS id and 10 service names as source; L2c: raw `tscmp` fails at
packet 0 (expected after re-modulation).

## Test 1c - PCTV 461 full diagnosis - PASS (2026-06-15, all 4 TPs)

Systematic root-cause hunt for the 461 "no lock" problem. Installed DVB diag
tools (`dvbv5-zap`, `dvb-fe-tool`, `femon`) and `i2c-tools`.

**Step 1 - LNB power path.** Direct LNB on 461: signal pinned at the noise floor
(-75/-79 dBm, status 0x00) -> 461 was not feeding the LNB. Routing the antenna
through an externally-powered multiswitch made the signal vary (down to -38 dBm
bursts) but still no lock. This proved the LNB-power dependency.

**Step 2 - driver vs PPA.** Compared our em28xx / m88ds3103 / a8293 sources
against the b-rad-NDi media-tree PPA (the Ubuntu 22.04 driver that worked for the
user). They are **functionally identical**: `461e v3` init with `lnb_hv_pol=1`,
`lnb_en_pol=1`, m88ds3103c, a8293 SEC; the 3103c `0xea=0xcb` DVB-S quirk is
already present; `chiptype` is correctly set to `M88DS3103_CHIPTYPE_3103C` via the
i2c id table. No driver bug.

**Step 3 - SEC voltage (multimeter).** Held 13 V then 18 V on the F-connector via
`dvbv5-zap`; the a8293 received the commands cleanly (`set_voltage_noslew`, no i2c
error). Measured output:

| Commanded | Measured | Verdict |
| --------- | -------- | ------- |
| 13 V (V)  | 12.69 V  | healthy |
| 18 V (H)  | 17.65 V  | healthy |

So the 461 LNB-power path is fully functional. The earlier "no feed" was the
multiswitch/DiSEqC layer, not the 461.

**Step 4 - direct LNB, correct delivery system.** With the antenna direct on 461
and the LNB now powered, lock succeeded - but only on the 8PSK TPs (#1, #3). The
QPSK TPs (#2, #4) refused to lock as DVB-S2 (status stuck at 0x03, never 0x8f),
which initially matched the community "QPSK stuck at 8PSK" reports
(b-rad-NDi issue #177). **Root cause was simpler:** TP #2/#4 are **DVB-S**, not
DVB-S2. Retuned as DVB-S and they lock immediately:

| TP | System | Mod  | Result (dvbv5-zap / dvbhub)            |
| -- | ------ | ---- | -------------------------------------- |
| 1  | DVB-S2 | 8PSK | LOCK, C/N 13.4 dB, 56 MB / 6 s         |
| 2  | DVB-S  | QPSK | LOCK, C/N 10.8 dB, 42 MB / 6 s; 27 svc |
| 3  | DVB-S2 | 8PSK | LOCK, C/N 17.9 dB, 58 MB / 6 s         |
| 4  | DVB-S  | QPSK | LOCK, C/N 11.1 dB, 36 MB / 6 s; 20 svc |

`tsanalyze` on the TP #2 capture: TS id 0x003D, 27 services, Original Network
0x002A (42 = Turksat) - valid stream. Fixed `app/src/transponders.rs` and
`docs/reference_transponders.md` to mark TP #2/#4 as DVB-S.

**Conclusion:** PCTV 461 works for all four reference transponders with the
antenna connected **directly** (universal LNB). See Test 1d for multiswitch.

## Test 1d - PCTV 461 via multiswitch (DiSEqC port A) - PASS (2026-06-15)

After correcting TP #2/#4 to DVB-S (not DVB-S2), retested with the antenna on a
multiswitch (Turksat on DiSEqC port A = `SAT_NUMBER` / `--satellite-number` 0).

`dvbv5-zap` (10 s each, universal LNB):

| TP | System | Result              |
| -- | ------ | ------------------- |
| 1  | DVB-S2 | LOCK, C/N 9.5 dB    |
| 2  | DVB-S  | LOCK, C/N 13.0 dB   |
| 3  | DVB-S2 | LOCK, C/N 18.8 dB   |
| 4  | DVB-S  | LOCK, C/N 12.3 dB   |

`dvbhub capture` (5 s each, default `--satellite-number` 0):

| TP | Output size |
| -- | ----------- |
| 1  | 48 MB       |
| 2  | 36 MB       |
| 3  | 50 MB       |
| 4  | 31 MB       |

Earlier multiswitch failures were caused by wrong delivery system on TP #2/#4
(DVB-S2 instead of DVB-S), not by DiSEqC or LNB power. `dvbhub capture` now
accepts `--satellite-number` for explicit DiSEqC port selection (0-63).

## Direct LNB vs multiswitch — signal quality comparison (2026-06-15)

Same PCTV 461, same four reference TPs, correct delivery systems (TP #2/#4 as
DVB-S). Measurements from `dvbv5-zap` during lock (10 s tune window).

| TP | Path        | System | C/N (dB) | RF level (dBm)     | Lock |
| -- | ----------- | ------ | -------- | ------------------ | ---- |
| 1  | Direct LNB  | DVB-S2 | 13.4     | ~-79               | yes  |
| 1  | Multiswitch | DVB-S2 | 9.5      | ~-76               | yes  |
| 2  | Direct LNB  | DVB-S  | 10.8     | (not logged)       | yes  |
| 2  | Multiswitch | DVB-S  | 13.0     | -69 to -22 (bursts)| yes  |
| 3  | Direct LNB  | DVB-S2 | 17.9     | ~-79               | yes  |
| 3  | Multiswitch | DVB-S2 | 18.8     | ~-66               | yes  |
| 4  | Direct LNB  | DVB-S  | 11.1     | (not logged)       | yes  |
| 4  | Multiswitch | DVB-S  | 12.3     | ~-76               | yes  |

Notes:
- **C/N:** comparable on both paths; multiswitch is slightly better on TP #2–#4,
  slightly worse on TP #1. Both are well above the demod threshold.
- **RF level (dBm):** `dvbv5-zap` reports demod AGC-related values, not calibrated
  dBm; useful for relative comparison only. Multiswitch shows stronger bursts on
  TP #2 (-22 dBm peaks) but also more variation. Direct LNB on TP #1/#3 reported
  ~-79 dBm while locked — the 461 LNB feed is working but the absolute dBm
  reading is not a reliable field-strength meter.
- **Practical:** both cabling modes work for capture once delivery system is
  correct. Use multiswitch when DiSEqC port selection is needed (`--satellite-number`).

## Test 3 - Full chain: PCTV 461 capture -> DTU-315 play -> TBS5530 re-capture - PASS (2026-06-15)

End-to-end "live signal in, re-modulated signal out, captured again" with L2a/b/c
comparison. All three devices on direct USB ports (no hub).

Steps:
1. PCTV 461 (`adapter0`) captured live TP#1 (11054 V, DVB-S2 8PSK 3/4) for 20 s
   -> `captures/tp1_461.ts` (172 MB, ~65.3 Mbps, TS id 0x7919, 10 TRT services).
2. DTU-315 played it out at 1200 MHz L-band IF (DVB-S2-8PSK, 30000 kS/s, 3/4),
   `--infinite`, **attached** (no `--instant-detach`).
3. TBS5530 S/S2 frontend (`adapter1:1:0:0`) re-captured the DTU-315 carrier with
   `--lnb 0` (direct L-band) for 20 s -> `captures/tp1_recap_5530.ts` (175 MB).

Comparison results:

- **L2a (link health):** TBS5530 locked, SNR 19.0 dB, BER 0 %, 0 invalid-sync
  packets, re-capture bitrate 65.33 Mbps (matches source). PASS.
- **L2b (structure):** identical TS id `0x7919`, identical 10 services with the
  same names (TRT COCUK HD, TRT1 HD, ...), identical PMT/PCR PIDs. PASS.
- **L2c (ES bit-exactness):** extracted video PID `0x640` (PES payload only,
  PCR/header ignored) from both files and compared as a hash multiset.
  **All 76,944 source video payload packets have a byte-identical match in the
  re-capture (100 %).** The re-capture has 1,648 extra packets, which are
  duplicates from the `--infinite` loop seam (the 20 s window spans the loop
  wrap), not content differences. `tscmp --search-reorder` confirmed the streams
  are the same content with a cyclic offset (source pkt 0 = capture pkt 63,919).
  PASS.

Conclusion: the play -> modulate -> demodulate -> capture path is lossless at the
elementary-stream level. The only differences are TS-layer artefacts (null-packet
stuffing to fill modulator capacity, and loop-wrap duplicates) inherent to an
infinite playout.

## dvbhub feature work (2026-06-15)

Following the successful chain test, the Rust binary gained:

- **`loopback` fix:** the Dektec playout no longer uses `--instant-detach` in
  loopback mode. With detach on, `tsp` exited right after preloading the on-board
  FIFO, so transmission stopped after ~1.6 s and the receiver never locked. The
  playout now stays attached for the whole capture and is killed afterwards.
  `PlayParams.instant_detach` is `true` only for finite fire-and-forget `play`.
- **`loopback` tuner auto-detect:** `--tuner-device` is now optional; the TBS5530
  S/S2 frontend is found via device discovery (`Discovery::satellite_tuner_address`).
- **`analyze` subcommand:** `dvbhub analyze --input file.ts [--json]` prints TS id,
  bitrate and the service list (name, id, provider, scrambled) via
  `tsp -P analyze --json`.
- **`capture-all` subcommand:** captures every reference transponder sequentially
  from one tuner, analyzing each. Verified on PCTV 461: **4 of 4 TPs captured**
  with full per-TP service listing (TP#1 TRT HD group, TP#3 Dogan Medya,
  TP#4 Turksat group incl. TGRT HABER HD).
- `capture` gained an optional `--analyze` flag to print the service list after a
  capture.

The binary is built in Docker (`docker/app/indockerbuild.sh`) as a static musl
`x86_64` binary at `build/dvbhub`.

## Web UI (`dvbhub serve`) — 2026-06-16

Local axum server on `http://127.0.0.1:8080` with embedded HTML + WebSocket push.

Verified on PCTV 461 (multiswitch, DiSEqC port A):

| Feature | Result |
| ------- | ------ |
| Device list + health | PASS — TSDuck v3.44, PCTV 461 + TBS5530 + DTU-315 enumerated |
| Tune / monitor TP#1 | PASS — LOCKED, SNR ~13 dB, BER 0 % |
| Live service list | PASS — 10 TRT services after `analyze --json` brace-depth fix (~10 s) |
| Capture via API | PASS — `POST /api/session/capture` → 53 MB `captures/web_api_test.ts` (12 s) |
| NIT scan API | PASS — TP#4 NIT scan returned **56 transponders** (~8 min runtime) |
| FTA playback API | WIRED — `POST /api/session/play` + HLS at `/hls/stream.m3u8` (ffmpeg 8.1 present; not yet browser-tested) |

API routes: `/api/session/tune`, `/capture`, `/scan`, `/play` (+ DELETE stops), `/ws`.

Known: only one tuner session at a time (tune / capture / scan / play are mutually exclusive).
