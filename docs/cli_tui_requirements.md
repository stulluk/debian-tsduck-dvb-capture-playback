# CLI / TUI requirements (draft)

Status: **user proposal + agent review** — not implemented yet.  
Last updated: 2026-06-19.

This document captures the proposed top-level CLI shape, satellite database
workflow, and TUI behaviour for `dvbhub`. It will drive ratatui work with
TestBackend snapshot tests.

---

## 1. Top-level subcommands

| Subcommand | Status | Purpose |
|------------|--------|---------|
| `capture` | existing (plain); TUI via `--tui` | Record TS from a tuner |
| `playout` | rename from current `play` | Play a TS file through the modulator |
| `watch` | **coming soon** | Live FTA service to player/HLS |
| `update-satellites-json` | **new** | Refresh satellite / transponder database from KingOfSat |

**Note:** Today the binary also has `devices`, `transponders`, `analyze`,
`capture-all`, `loopback`, `serve`. This spec does not say whether those stay
as auxiliary commands or move under the four pillars above. See open questions.

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

Behaviour (draft):

1. Read configured satellite list from the database file.
2. For each satellite, map `longitude` → KingOfSat `pos` code (e.g. `42.0E` → `420`).
3. Fetch `https://en.kingofsat.net/dl.php?pos=<code>` (or bundled fallback on failure).
4. Parse ini → update transponder arrays.
5. Preserve per-satellite `lnb`, `diseqc`, and `name`.
6. Write atomically; print summary (`updated N satellites, M transponders`).

Flags (TBD):

- `--satellite NAME` — refresh one entry only.
- `--add 42.0E` — add a new satellite slot then fetch.
- `--dry-run` — show diff without writing.

---

## 4. `playout`

### User intent

- **Mandatory:** input `.ts` file path.
- On start: detect whether a **modulator** is present (DTU-315 via Dektec).
- Try to **read modulation parameters from the TS** automatically.
- If detection fails: stop and ask user (TUI multi-select menus **or** CLI flags like `tsp`).

### Existing code

`dvbhub play` already plays a file via DTU-315 with explicit
`--frequency-mhz`, `--modulation`, `--symbol-rate-ksps`, `--code-rate`.

### Draft flow

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

**Coming soon** — placeholder subcommand or hidden until implemented.

Existing `dvbhub watch --service …` CLI could become the non-TUI backend later.

---

## 6. `capture` (non-TUI)

**Unchanged** from current behaviour:

- Flags / `--transponder` / explicit tune params.
- Plain `[dvbhub]` progress on stderr.
- Live `-P analyze --json` during capture.
- Optional `--analyze` post-check if live analyze missed SDT.

---

## 7. `capture --tui`

### Wizard steps (draft)

1. **Device check** — list capture-capable tuners; user picks one (multi-select menu if several; error if none).
2. **Delivery** — Satellite | Terrestrial | Cable (T/C may show “coming soon” initially).
3. **If Satellite:**
   - Pick satellite from `satellites.json` / `sat-tp.json`.
   - Ask mode: **Full satellite capture** vs **Single transponder capture**.
4. **Run** — enter three-panel TUI (see below).

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

| Mode | Behaviour (draft) |
|------|-------------------|
| **Full sat** | For each TP on the satellite: tune → lock → (optional) list services → record N seconds → next TP. One output file per TP (matches existing web `capture-sat` naming). |
| **TP capture** | User selects one TP → record N seconds → done. |

### Keys (draft)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move selection (left panel in browse; wizard lists) |
| `Enter` | Confirm selection |
| `Tab` | Switch focus panel (when in scan view) |
| `q` / `Esc` | Quit / abort capture |
| `?` | Help |

Implementation: ratatui + shared `AppState` / events from `progress.rs` patterns;
TestBackend snapshot tests for each screen.

---

## 8. Test strategy (agreed)

- **TestBackend** — render widgets to buffer; assert frame text in `cargo test`.
- **Fixture replay** — feed recorded tsp stdout/stderr without hardware (later).
- **`--plain`** — non-TUI regression path stays available.
- User validates keybindings and “feel” on real terminal locally.

---

## 9. Decisions (user confirmed 2026-06-19)

| Topic | Decision |
|-------|----------|
| Database file | **Keep** `~/.config/dvbhub/satellites.json`; `update-satellites-json` is the CLI command |
| Full sat output | **One `.ts` file per transponder** (same idea as current web SAT capture) |
| SAT scan goal | **Both:** record TS **and** save service metadata (names, ids, provider, bitrate if available) as JSON per TP |
| Per-TP folder layout | Under a satellite directory, **one subfolder per TP** containing both artefacts, e.g. `turksat42e/11054v30000/capture.ts` + `services.json` (exact names TBD) |
| TUI v1 delivery | **DVB-S/S2 only**; Terrestrial and Cable show “coming soon” |
| Online TP source | **KingOfSat** primary (already in-tree); Enigma2 XML optional later |

### Still open

- Auxiliary commands (`serve`, `devices`, `analyze`, …): keep as-is?
- Playout auto-detect: how much to infer from TS before prompting user?
- `watch`: stub subcommand only until implemented?

---

## 10. Per-TP output layout (draft)

Full satellite capture writes under `<capture_dir>/<satellite_slug>/`:

```
turksat42e/
  11054v30000/
    capture.ts          # timed TS recording
    services.json       # live analyze summary: ts_id, bitrate, services[]
  11096h30000/
    capture.ts
    services.json
  …
```

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

- Auxiliary commands (`serve`, `devices`, `analyze`, `loopback`, …): keep alongside the four pillars?
- Playout: how much modulation can we infer from TS alone before prompting?
- `watch`: visible stub with “coming soon” or hidden until ready?

---

## 12. Suggested implementation order

1. `update-satellites-json` CLI (reuse `satellites.rs`; no TUI yet).
2. CLI rename `play` → `playout` (+ `play` alias) — **done**.
3. Per-TP folder output (`capture.ts` + `services.json`) for full-sat plain capture.
4. `capture --tui` wizard shell + TestBackend tests (device + delivery + satellite pick).
5. Three-panel SAT scan/capture view wired to real `tsp`.
6. Playout auto-detect + TUI wizard (v1).
7. `watch` subcommand stub → real implementation.
