# CLI / TUI requirements

Status: **implemented** (spec agreed 2026-06-19, implemented through 2026-07).
Last updated: 2026-10-13.

This document captures the top-level CLI shape, satellite database workflow, and
TUI behaviour for `dvbhub`. Sections were originally written as a draft; each one
now notes what was actually implemented. The ratatui screens are covered by
TestBackend tests (`cargo test`).

---

## 1. Top-level subcommands

| Subcommand | Status | Purpose |
|------------|--------|---------|
| `capture` | **done** (plain + `--tui`, S/T/C) | Record TS from a tuner |
| `playout` | **done** (`play` kept as alias; plain + `--tui`, S/T/C) | Play a TS file through the modulator |
| `watch` | **done** (plain + `--tui`, mpv / vlc; browser via `serve`) | Live FTA service to a player |
| `update-satellites-json` | **done** (`--satellite`, `--dry-run`) | Refresh satellite / transponder database from KingOfSat |

Auxiliary commands are kept as-is next to the four pillars: `devices`,
`transponders`, `tsduck`, `capture-all`, `scan`, `loopback`, `compare`, `analyze`,
`serve`, `completions`.

Non-TUI capture (current plain progress + live analyze) **remains the default**.
TUI is opt-in: `dvbhub capture --tui`.

---

## 2. Satellite / transponder database (`sat-tp.json`)

### User intent

- A persisted file listing satellites the user cares about (e.g. Turksat 42E,
  Hot Bird 13E, …).
- Per satellite:
  - **Antenna / dish settings:** LNB type, DiSEqC port, (optional) motor / USALS.
  - **Transponder list:** frequency, polarity, symbol rate, modulation, FEC, system.
- Command `update-satellites-json` fetches fresh TP lists from KingOfSat and updates
  the file.

### Overlap with existing code

The web UI already persists an equivalent structure as
`~/.config/dvbhub/satellites.json` (`SatelliteDb` in `app/src/satellites.rs`):

```json
{
  "satellites": [
    {
      "name": "Turksat 42E",
      "longitude": "42.0E",
      "lnb": "universal",
      "diseqc": 0,
      "transponders": [
        {
          "frequency_mhz": 11054,
          "polarity": "vertical",
          "symbol_rate_ksps": 30000,
          "modulation": "8-PSK",
          "fec": "3/4",
          "delivery_system": "DVB-S2",
          "name": ""
        }
      ]
    }
  ]
}
```

KingOfSat import (`parse_kingofsat_ini`, live fetch) is **already implemented**
for the web UI and bundled offline seeds under `app/data/kingofsat/*.ini`.

**Proposal:** Do not introduce a second file format. Either:

- **A (recommended):** Keep `satellites.json` path/name; add CLI alias
  `update-satellites-json` that refreshes it; document as “satellite TP database”.
- **B:** Rename to `sat-tp.json` and migrate existing data once.

User-specific fields (`lnb`, `diseqc`) must **survive** an online refresh
(only transponder rows are replaced/merged, not dish settings).

### Online data sources (canonical alternatives)

| Source | Pros | Cons |
|--------|------|------|
| **KingOfSat** (`dl.php?pos=…`) | Already parsed in-tree; community standard; good TP coverage | Scraping / ToS grey area; no official API; site layout can change |
| **LyngSat** | Similar coverage | Same scraping issues; different format; duplicate work |
| **Enigma2 `satellites.xml`** | De-facto standard in STB images; offline; includes LNB/DiSEqC | Large files; import mapping needed; not auto-updated |
| **On-air NIT scan** (`tsscan`) | Authoritative for what the dish actually receives | No friendly names; needs lock per TP; slow |
| **TSDuck** | — | No built-in satellite catalogue |

**Recommended strategy:**

1. **Primary refresh:** KingOfSat (already implemented) for TP lists.
2. **Ship offline seeds** in the repo (already done) so first run works without network.
3. **Optional later:** import Enigma2 `satellites.xml` for users migrating from STB setups.
4. **Verification pass (optional):** after update, `tsscan` on one reference TP per satellite to mark TPs as “seen on air”.

There is no single industry “canonical API” for satellite TP data; KingOfSat +
Enigma2 XML are the two practical ecosystems. KingOfSat is the right default
for this project given existing code.

---

## 3. `update-satellites-json`

Behaviour (implemented):

1. Read configured satellite list from the database file.
2. For each satellite, map `longitude` → KingOfSat `pos` code (e.g. `42.0E` → `420`).
3. Fetch `https://en.kingofsat.net/dl.php?pos=<code>` (or bundled fallback on failure).
4. Parse ini → update transponder arrays.
5. Preserve per-satellite `lnb`, `diseqc`, and `name`.
6. Write atomically; print summary (`updated N satellites, M transponders`).

Flags:

- `--satellite NAME` — refresh one entry only (**done**).
- `--dry-run` — report changes without writing (**done**).
- `--add 42.0E` — add a new satellite slot then fetch (**not implemented**; add
  the entry by hand in `satellites.json`, then refresh).

---

## 4. `playout`

### User intent

- **Mandatory:** input `.ts` file path.
- On start: detect whether a **modulator** is present (DTU-315 via Dektec).
- Try to **read modulation parameters from the TS** automatically.
- If detection fails: stop and ask user (TUI multi-select menus **or** CLI flags like `tsp`).

### Implemented

`dvbhub playout` (alias `play`) plays a file via DTU-315 with explicit
`--frequency-mhz`, `--modulation`, `--symbol-rate-ksps`, `--code-rate`.
`playout --tui` picks the file in a file picker; when a capture sidecar
(`*_services.json`, written by `capture`) exists next to the `.ts`, its recorded
tune parameters pre-fill the modulation form. Otherwise the user fills the
satellite / terrestrial / cable manual form.

### Original draft flow

```
playout FILE.ts
  → discover modulator (tsdektec / devices.rs)
  → if none: error "no modulator"
  → tsanalyze / tsp -P analyze on file → suggest DVB-S/S2 params
  → if confident: start with derived params
  → else: TUI wizard OR require CLI flags (--frequency-mhz …)
  → tsp -I file … -O dektec …
```

Open: what “read from TS” means in practice — TS bitrate ≠ RF symbol rate; we may
only infer service structure, not full Dektec modulation token, without user
input or a reference transponder match.

---

## 5. `watch`

**Implemented (rewritten 2026-10-20).** `dvbhub watch` runs
`tsp -I dvb … -P zap <sid> -O file | <player> -` — mpv, else vlc, or `--player "…"`.
dvbhub only supervises the two processes: closing the player stops `tsp`; `tsp`
ending (no lock, busy tuner) stops the player and reports the error. Both children
die with dvbhub (PR_SET_PDEATHSIG), so a tuner is never left busy.

- mpv opens video in ≈ 2 s: the default lavf probe (5 MB) waited ~8 s on channels
  whose AC-3 tracks never yield parameters; dvbhub passes a 2 MB / 1 s probe.
- `--test --duration N` replaces the player with `ffmpeg -f null` and fails unless
  video frames decode in real time (headless verification).
- mpv writes its own log to `/tmp/dvbhub_mpv_log.txt` (VO/AO, dropped frames).

`watch --tui`: delivery → channel list (`[SAT]` rows show satellite, TP and sid),
`/` filters by name / provider / satellite, Enter watches, closing the player returns
to the list, `s` opens the scan form (satellite catalogue, UHF band or frequencies,
cable frequencies; live progress; Esc asks Y/N).

The embedded GStreamer player of the Cursor sessions was removed: the freezes it was
built to debug were caused by the PC's audio stack (PipeWire / ALC897), not by the
TS pipeline.

---

## 6. `capture` (non-TUI)

Kept as the default (no TUI):

- Flags / `--transponder` / explicit tune params.
- Plain `[dvbhub]` progress on stderr.
- Live `-P analyze --json` during capture.
- Optional `--analyze` post-check if live analyze missed SDT.
- Catalogue capture: `--satellite NAME --tp-frequency-mhz F` (one TP) or
  `--satellite NAME --full-satellite [--resume] [--max-tps N]` (every TP).

---

## 7. `capture --tui`

### Wizard steps (implemented)

1. **Device check** — list capture-capable tuners; user picks one (error if none).
2. **Delivery** — Satellite | Terrestrial | Cable.
3. **If Satellite:**
   - Pick satellite from `satellites.json`, or **MANUAL SETUP** (single-page form:
     frequency, SR, polarity, modulation, FEC, DiSEqC 1.0/1.1, LNB presets).
   - Ask mode: **Full satellite capture** vs **Single transponder capture**.
4. **Terrestrial / Cable** — manual forms (frequency, bandwidth / SR, modulation).
5. **Run** — three-panel TUI (see below). Top-right panel shows Quality (SNR),
   Strength, BER and Bitrate.

### Three-panel layout (SAT)

```
┌─ Scan target ─────────────┬─ Services ─────────────────────┐
│ Turksat 42E               │ TRT1 HD          10601  clear  │
│ > 11054 V 30000 8PSK      │ TRT HABER HD     10602  clear  │
│   11096 H 30000 QPSK      │ …                              │
│   …                       │                                │
├───────────────────────────┴────────────────────────────────┤
│ Status: LOCKED SNR 13.0 dB │ TP 3/42 │ Recording 12/30 s  │
└────────────────────────────────────────────────────────────┘
```

- **Left:** current satellite + TP list (highlight active TP).
- **Right:** services on the **currently tuned** TP (live analyze JSON).
- **Bottom:** lock / SNR / progress / errors / log tail.

### Full satellite vs TP capture

| Mode | Behaviour |
|------|-------------------|
| **Full sat** | For each TP on the satellite: tune → lock → (optional) list services → record N seconds → next TP. One output file per TP (matches existing web `capture-sat` naming). |
| **TP capture** | User selects one TP → record N seconds → done. |

### Keys (implemented)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move selection (wizard lists, forms) |
| `←` / `→` | Change option value in forms (e.g. DiSEqC, LNB preset) |
| `Enter` | Confirm selection |
| `Esc` | Back one wizard step |
| `Esc` / `q` during capture | Asks Y/N, then stops `tsp` gracefully |

Implementation: ratatui + shared `AppState` / events from `progress.rs` patterns;
TestBackend snapshot tests for each screen.

---

## 8. Test strategy (agreed)

- **TestBackend** — render widgets to buffer; assert frame text in `cargo test` (**done**).
- **Fixture replay** — feed recorded tsp stdout/stderr without hardware (not yet).
- **Non-TUI path** — plain `capture` / `playout` stay the default regression path;
  `--tui-auto` runs a non-interactive TUI smoke test.
- User validates keybindings and “feel” on real terminal locally.

---

## 9. Decisions (user confirmed 2026-06-19)

| Topic | Decision |
|-------|----------|
| Database file | **Keep** `~/.config/dvbhub/satellites.json`; `update-satellites-json` is the CLI command |
| Full sat output | **One `.ts` file per transponder** (same idea as current web SAT capture) |
| SAT scan goal | **Both:** record TS **and** save service metadata (names, ids, provider, bitrate if available) as JSON per TP |
| Per-TP folder layout | Under a satellite directory, **one subfolder per TP** containing both artefacts, e.g. `turksat42e/11054v30000/capture.ts` + `services.json` (exact names TBD) |
| TUI v1 delivery | **DVB-S/S2 only** at first; Terrestrial and Cable were added later (done) |
| Online TP source | **KingOfSat** primary (already in-tree); Enigma2 XML optional later |

### Resolved since

- Auxiliary commands are kept as-is.
- Playout pre-fills parameters from the capture sidecar; no RF inference from TS
  bitrate alone (bitrate does not determine symbol rate / FEC uniquely).
- `watch` is fully implemented (no stub).

---

## 10. Per-TP output layout (implemented)

Full satellite and single-TP catalogue captures write under
`<capture_dir>/<satellite_slug>/<freq><pol><sr>/` with timestamped files
(`app/src/sat_capture.rs`):

```
turksat42e/
  11054v30000/
    turksat42e_11054_v_30000_20260619_120000.ts             # timed TS recording
    turksat42e_11054_v_30000_20260619_120000_services.json  # ts_id, bitrate, services[], tune
  11096h30000/
    …
```

`--resume` skips TP folders that already contain a valid `.ts`.

`services.json` schema (reuse `StreamSummary` from `analyze.rs`):

```json
{
  "ts_id": 31001,
  "bitrate": 65330000,
  "services": [
    { "id": 10601, "name": "TRT1 HD", "provider": "TRT", "scrambled": false }
  ],
  "captured_at": "2026-06-19T12:00:00+03:00",
  "tune": {
    "frequency_mhz": 11054,
    "polarity": "vertical",
    "symbol_rate_ksps": 30000,
    "delivery_system": "DVB-S2",
    "modulation": "8-PSK",
    "fec": "3/4"
  }
}
```

Single-TP mode (`--tui` wizard → “TP capture”) uses the same folder/file pair for
one selected transponder only.

---

## 11. Open questions (remaining)

- `update-satellites-json --add <longitude>` (add a satellite without editing JSON).
- Fixture-replay tests for `tsp` output parsing without hardware.

---

## 12. Implementation order (all done)

1. `update-satellites-json` CLI — **done**.
2. CLI rename `play` → `playout` (+ `play` alias) — **done**.
3. Per-TP folder output (`.ts` + `_services.json`) for full-sat plain capture — **done**.
4. `capture --tui` wizard shell + TestBackend tests — **done**.
5. Three-panel SAT scan/capture view wired to real `tsp` — **done**.
6. Playout sidecar pre-fill + TUI wizard — **done**.
7. `watch` subcommand — **done**.
