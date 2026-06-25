# Design Considerations: Development & Test Architecture

Status: discussion document (living). Captures the user's proposed test
methodology, open concerns, and the agent's analysis/answers. Nothing here is
final until explicitly agreed.

Kernel/driver baseline (all L1-verified on `7.0.0-20260615-sertac-dcn31fix`):

| Device   | Capability                              | Connectors / notes                    |
| -------- | --------------------------------------- | ------------------------------------- |
| DTU-315  | Output / playback (modulate) DVB-S/T/C  | USB 3.0; `/dev/Dtu4` (DTAPI/Dektec)   |
| TBS5530  | Capture (record) DVB-S/S2 and DVB-T/T2/C| 2 RF connectors, but ONE active demux |
| PCTV 461 | Capture (record) DVB-S/S2 only          | `/dev/dvb/adapter1`, m88ds3103c        |

Important hardware constraint discovered during bring-up: the TBS5530 exposes
`adapter0` with **two frontends** (`frontend0` = DVB-T/T2/C/ATSC, `frontend1` =
DVB-S/S2/S2X) but a **single `demux0`/`dvr0`**. So only **one frontend can
stream at a time** — the two RF connectors are "pick one", not simultaneous.
This is fine for the sequential loopback tests below.

---

## 1. Proposed test methodology (user)

### Test A - DVB-S loopback (Turksat)

1. Live satellite dish → **PCTV 461**. Record ~4-5 known Turksat transponders
   (TPs), 30 s each, with TSDuck → reference files `refS_<tp>.ts`.
2. Re-cable: **DTU-315 RF out → TBS5530 DVB-S input**.
3. **DTU-315 plays** `refS_<tp>.ts`; **TBS5530 records** it → `loopS_<tp>.ts`.
4. Compare `loopS_<tp>.ts` with `refS_<tp>.ts`. Expectation (user): "should be
   identical."

### Test B - DVB-T loopback (Istanbul test mux)

1. Live terrestrial antenna → **TBS5530** (DVB-T frontend). Record once →
   `refT.ts`.
2. Re-cable: **DTU-315 RF out → TBS5530 DVB-T input**.
3. **DTU-315 plays** `refT.ts`; **TBS5530 records** → `loopT.ts`.
4. Compare `loopT.ts` with `refT.ts`.

This is a classic **playrec (play + record) loopback** fidelity test. The
overall idea is sound and is exactly how you validate a modulate→demodulate
chain end-to-end.

---

## 2. The "birebir aynı" question (honest analysis)

Short answer: the two files will **NOT be byte-for-byte identical** with a naive
`cmp`/`sha256`, even when the chain is perfect. But the **content can be proven
identical** at the transport/elementary-stream level. Here is why, and how to
compare correctly.

### Why raw bytes differ even on a perfect chain

1. **Capture start offset (the big one).** The TBS5530 starts recording at an
   arbitrary moment and only after demod lock. So `loop.ts` begins at a random
   packet boundary relative to `ref.ts` — the streams are *rotated/shifted*, not
   aligned. A byte diff fails immediately even if every packet is correct.
2. **Null/stuffing packets (PID 0x1FFF).** A DVB multiplex is constant-bitrate.
   If the playback bitrate (modulation symbol rate × FEC × bits/symbol) does not
   match the recorded payload rate *exactly*, TSDuck/the modulator inserts or
   drops null packets to fill. Different null counts → different bytes.
3. **Lead-in garbage.** The first packets of `loop.ts` (before/around lock) can
   contain TS continuity errors that are not in `ref.ts`.
4. **PCR jitter / optional restamping.** If we let any plugin restamp PCRs to fit
   the new timing, those bytes change. (We will avoid restamping where possible.)
5. **TS packet reordering across PIDs.** The modulator emits a single serialized
   multiplex; relative interleaving of PIDs can shift slightly with stuffing.

Conclusion: equality must be defined at the right layer, not on raw bytes.

### How to compare correctly (what "identical" should mean)

Layered verification, from easy to strict:

- **L2a - Link health:** TBS5530 locks; signal/CN OK; **zero TS continuity
  errors** across the capture (after trimming lead-in). Proves the RF chain.
  Tools: `tsp -P continuity`, `tsanalyze`.
- **L2b - Structure equivalence:** same PID set, same PSI/SI (PAT/PMT/SDT),
  same services. Tools: `tsanalyze`, `tstables`/`tspsi`, compare reports.
- **L2c - Payload/ES bit-exactness (the real proof):** pick one service or one
  elementary-stream PID (e.g. a video PID). From both files: filter to that PID,
  drop null packets, align on a common anchor (first PUSI with matching
  payload), then compare payloads. If the **elementary stream bytes are
  identical**, the modulate→demodulate path is lossless.
  Tools: `tscmp` (TSDuck's TS comparison tool, supports `--pid`,
  `--payload-only`, resync), or extract ES via `tsp -P pes`/`-P filter` and
  `cmp` the aligned ES.

`tscmp` is purpose-built for this ("compare the binary content of two TS
files", with PID selection, payload-only mode, and resynchronization). It is the
natural primary comparison tool, with `tsanalyze` for the structural report.

### Recommendation to maximize fidelity

- When playing back via DTU-315, set the modulation parameters (symbol rate,
  FEC, constellation, roll-off for DVB-S; bandwidth, constellation, code rate,
  guard interval for DVB-T) to **match the original TP exactly**. Then the
  modulation CBR equals the recorded CBR → native-rate playback, minimal/
  identical stuffing → best chance of even packet-level equivalence.
- Record the **full TP** on the reference capture (no PID filtering), so the
  played multiplex matches what the modulator expects.
- Trim lead-in and stop cleanly (fixed packet count or duration) to make
  alignment deterministic.

### Verdict

The mechanism is logical and worth doing. We just replace "files must be byte-
identical" with a **3-level pass criterion (L2a link, L2b structure, L2c ES
bit-exact)**. L2c gives you the rigorous "lossless" proof you actually want; raw
`sha256` equality is the wrong metric for a live-demodulated capture.

---

## 3. Open concerns / risks to validate during testing

- **DTU-315 DVB-T support & rate matching:** confirm DTAPI/Dektec plugin
  modulates COFDM at the needed bitrate; verify the file→modulator rate path
  (native vs regulated).
- **TBS5530 single-demux constraint:** cannot capture DVB-S and DVB-T at the
  same time; tests are strictly sequential.
- **RF levels / attenuation:** DTU-315 output into TBS5530 input may need an
  attenuator to avoid overdriving the tuner front-end (watch CN/AGC).
- **Permissions:** user is in `video` group; `/dev/dvb/*` and `/dev/Dtu4`
  accessible without root (to confirm for the app).
- **Clock domains:** modulator and demodulator have independent clocks; expect
  PCR jitter, not data loss. Avoid PCR restamping unless required.

---

## 4. TSDuck build & deploy (to detail later)

- Build the latest TSDuck **inside Docker** (per host-protection rule: no build
  toolchain on host), produce a portable artifact, deploy/run on host.
- Must include **Dektec support** so `-I dektec` / `-O dektec` (DTU-315) work,
  linking the DTAPI shipped in `LinuxSDK/DTAPI/`.
- Need both: DVB tuner input (`-I dvb`) for PCTV/TBS capture, Dektec output
  (`-O dektec`) for DTU-315 playback, `tscmp`/`tsanalyze` for verification.
- Open: static vs dynamic build, exact version pin, how DTAPI is bundled.

---

## 5. Frontend: Rust + ratatui (TUI) vs TSDuck Python API (local WebIF)

The app must: detect devices, check/load drivers, run capture/playback wizards,
show live status. Question: native TUI or a local web interface?

### Option 1 - Rust + ratatui TUI (orchestrates `tsp` as subprocess)

- Portability: **excellent** — single static binary, cross-compiles to
  Linux/macOS/Windows, no runtime. Matches the existing CI rule (static builds
  for 3 platforms).
- UI richness: good for dashboards, limited for forms/graphs vs a browser.
- Windows caveat: terminal behavior (PowerShell/Windows Terminal/legacy console)
  varies; works but needs care.
- **Testability by the agent: hard.** Driving an interactive TUI autonomously
  needs a PTY + snapshot/expect harness; I cannot "see" it the way a user does.
  Verification is mostly indirect (unit tests + parsing the `tsp` JSON it emits).

### Option 2 - TSDuck Python API + local WebIF (browser)

- UI richness: **excellent** — forms, live charts (signal/bitrate), tables.
- TSDuck integration: tight via the Python bindings, OR still subprocess `tsp`.
- Portability: the **UI is 100% portable** (any browser). The backend (Python)
  needs a runtime/venv per OS; Windows packaging is heavier (PyInstaller) and
  TSDuck must be installed for the Python bindings to import.
- **Testability by the agent: easy.** I can drive the running web UI end-to-end
  via Browser MCP (navigate, click, fill forms, read state, screenshot). This is
  a major practical advantage for autonomous iteration.

### Option 3 (hybrid) - Rust backend serving a local web UI

- A single static **Rust** binary that (a) orchestrates `tsp` and (b) serves a
  small local web UI (e.g. embedded assets + HTTP/WebSocket on `localhost`).
- Portability: **best of both** — one static binary per OS, no Python runtime,
  yet a rich browser UI.
- Testability: **easy** — agent drives it via Browser MCP like Option 2.
- Cost: more engineering than a pure TUI; need a small web frontend.

### Trade-off summary

| Criterion              | TUI (Rust)      | WebIF (Python)   | Hybrid (Rust+web) |
| ---------------------- | --------------- | ---------------- | ----------------- |
| Single-binary deploy   | ++              | -                | ++                |
| Win/Mac/Linux portable | ++ (binary)     | + (UI) / - (run) | ++                |
| UI richness            | +               | ++               | ++                |
| Agent can self-test    | - (PTY/snapshot)| ++ (Browser MCP) | ++ (Browser MCP)  |
| Engineering effort     | low             | medium           | medium-high       |
| TSDuck coupling        | loose (subproc) | tight (bindings) | loose (subproc)   |

### Agent's leaning (for discussion, not decided)

For *testability + portability together*, **Option 3 (Rust backend + local web
UI)** is the strongest: keeps the single static cross-platform binary you want,
keeps loose `tsp` subprocess coupling (version-independent), and lets me verify
the whole UI autonomously with Browser MCP. Pure ratatui is the lowest effort but
the hardest for me to test; pure Python WebIF is the easiest to test but the
weakest to package for Windows.

---

## 6. Decisions taken (2026-06-15)

1. **Test pass criterion: L2a/L2b/L2c accepted** (link health, structure
   equivalence, ES bit-exactness) instead of raw byte equality. We additionally
   match DTU-315 modulation to the original TP params to maximize packet-level
   equivalence.
2. **Reference TPs fixed** - see `reference_transponders.md` (4 Turksat TPs).
3. **TSDuck: latest from git, built in Docker.** Static vs dynamic resolved -
   see section 7.
4. **Frontend: Hybrid (Rust backend + local web UI), TUI dropped.** Rationale and
   the Python-API-vs-subprocess analysis - see section 8.

---

## 7. TSDuck build: static vs dynamic, and "bundling DTAPI"

### Static vs dynamic - the honest answer for TSDuck

TSDuck is **plugin-based**: `tsp` loads each plugin (`tsplugin_*.so`, e.g. `dvb`,
`dektec`, `file`, `analyze`) as a **shared object at runtime via dlopen**. This
means a "fully static `tsp`" is not how TSDuck is designed - the plugin model
inherently needs dynamic loading. So the static-vs-dynamic choice does **not**
give the usual single-file benefit here.

What actually gives us the "clean removal" benefit you want is **prefix
isolation**, not static linking:

- Build TSDuck in Docker, install into a **self-contained prefix directory** on
  the host (e.g. `/opt/tsduck-<version>/` with `bin/`, `lib/`, plugins).
- Run via that prefix (PATH + rpath/`LD_LIBRARY_PATH`), so nothing leaks into
  system dirs.
- Uninstall = `rm -rf /opt/tsduck-<version>/`. Clean, and disk is plentiful.

Recommendation: **prefix-isolated install** (dynamic internally, as TSDuck
intends). We do not need static linking for our dev/test workflow; it would fight
the plugin architecture for no practical gain. (We can still statically link any
trivial external deps if the build makes it easy, but that is secondary.)

Since our Rust binary calls `tsp` as a subprocess, TSDuck is deployed as this
isolated prefix and the Rust app just needs its path.

### What "bundling DTAPI" means

- **DTAPI** is Dektec's proprietary user-space C++ API for their devices. It
  ships **only as a precompiled object** (`DTAPI64.o`) + header in
  `LinuxSDK/DTAPI/` - no source. (Different layer from the kernel `Dtu` driver:
  DTAPI is user-space, the driver is the kernel module that owns `/dev/Dtu4`.)
- TSDuck's **Dektec plugin** (`-I dektec` / `-O dektec`, used to drive the
  DTU-315) must be **linked against DTAPI** at build time. TSDuck's build can
  either auto-download a known-good DTAPI or use a local one.
- "Bundling DTAPI" = building TSDuck **with Dektec support enabled** so the
  DTAPI object gets compiled into `tsplugin_dektec.so`. Because DTAPI is a static
  `.o`, it ends up **inside** that plugin - there is no separate DTAPI runtime
  library to ship. We just must NOT disable Dektec (`NODEKTEC`) and should align
  the DTAPI version with our installed driver (LinuxSDK 2026.05.0) if any version
  mismatch shows up.

Net: build TSDuck latest in Docker, Dektec enabled, install to an isolated
prefix on host; the dektec plugin carries DTAPI internally.

---

## 8. Hybrid architecture: Python API vs subprocess (clarified)

User's worry: subprocess control means "kill the process" with no graceful exit,
whereas the Python API feels more granular (precise start/stop, JSON service
lists, device-present checks). Clarification below.

### Graceful stop with subprocess - the worry is unfounded

`tsp` handles **SIGINT/SIGTERM gracefully**: on signal it stops the input,
**flushes the processing chain, and closes outputs cleanly** - so a `-O file`
capture is finalized into a valid `.ts`. We send `SIGTERM` from Rust; we do
**not** `kill -9`. This is functionally equivalent to the Python binding's
`abort()` (both ask the same engine to terminate cleanly). No data loss, no
truncated file.

### The Python API is the same engine, not "more granular"

The TSDuck Python (and Java/C++) bindings wrap **libtsduck** and run the same
plugin chain in-process. They expose `start()` / `waitForTermination()` /
`abort()`. They do **not** provide richer data than the CLI plugins - service
lists, signal status, bitrate, PSI/SI all come from the **same plugins**
(`analyze`, `signal-monitor`, table extractors). So "granular start/stop + JSON
service list + device-present check" is achievable **both** ways.

### What subprocess `tsp` gives us (enough for the UI)

- **Graceful start/stop:** spawn `tsp`; stop via SIGTERM ⇒ clean file.
- **JSON output:** `tsanalyze --json`, `tsp -P analyze --json`, table extraction
  as JSON ⇒ PID list, service list (PAT/PMT/SDT), bitrate per PID.
- **Live status while capturing:** a `signal-monitor`/analyze plugin can stream
  JSON lines (lock, C/N, bitrate) that the Rust backend parses and pushes to the
  web UI over WebSocket.
- **Live control of a running session:** `tsp` has a control port; a running
  chain can be queried/adjusted without restart.
- **Device discovery:** `tslsdvb` / `tsscan` list tuners and scan a band/NIT.

Windows note: signals differ on Windows (no POSIX SIGTERM); graceful stop there
uses a console control event / the `tsp` control port. The Rust layer abstracts
this per-OS, so the cross-platform story holds.

### Decision: Hybrid Rust binary, subprocess orchestration

- One **static, cross-platform Rust binary** with a shared internal **engine**
  module that spawns and controls `tsp`/`tsanalyze`/`tscmp` and parses their JSON.
- Two front-ends over the same engine:
  - **CLI mode:** `tsduck-app capture --device pctv461 --tp 11054V30000 ...`
    (scriptable, like the old bash scripts).
  - **Web mode:** `tsduck-app serve` ⇒ rich local browser UI on `localhost`
    (forms, live signal/bitrate charts, service lists). Agent-testable via
    Browser MCP.
- Loose coupling to TSDuck (subprocess) ⇒ independent of TSDuck version and
  language; clean prefix deploy.

### Batch / scan features (parity with old bash scripts and beyond)

The hybrid binary can reproduce and extend `multi-dvbs-capture.sh` /
`satcapture.sh`:

- **Capture-all:** iterate a TP table (reference set, or a full Turksat TP list,
  seeded via NIT on TP #4 / `tsscan`), tune each, capture N seconds, while
  recording per-TP **lock + C/N + service list** to a report.
- **Scan & catalog:** `tsscan` (NIT-based) to enumerate all transponders and
  their services into a JSON/HTML catalog shown in the web UI.
- **Live signal + channel list during capture:** parse `tsp` JSON to show, for
  the tuned TP, signal status and the list of services/PIDs in real time.

So yes - "record every Turksat TP in one go, showing signal status and the
channel list per TP" is fully in scope for the hybrid Rust binary.
