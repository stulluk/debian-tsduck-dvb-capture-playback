# dvbhub — feature-complete work & verification plan

Started 2026-10-20. Source of requirements: the original Cursor sessions (June–July 2026)
plus `docs/cli_tui_requirements.md`. This plan is the tracking document; each item is
ticked only after it was verified as described in the "Verify" column.

## Decisions (2026-10-20)

| Topic | Decision |
| ----- | -------- |
| Rewrite vs refactor | Targeted refactor. The tsp-subprocess engine, satellite catalogue and TUIs are hardware-verified and stay. |
| Watch (terminal) | Remove the embedded GStreamer player. `tsp -P zap … -O file \| mpv -` (vlc fallback) is the only path. GStreamer is no longer linked into the binary. |
| Watch (web) | HLS (video copied, audio transcoded to AAC). Target: first frame ≤ ~5 s after lock. |
| Web access | localhost only (default bind 127.0.0.1), no authentication. |
| Test hardware | Live Hot Bird 13E on the TBS5530 DVB-S input; DTU-315 RF out → TBS5530 DVB-T/C input for playback and T/C loopback tests. |

## Requirements

### Web UI

| Id | Requirement |
| -- | ----------- |
| W1 | Landing page with four tiles: DVB devices, TS capture, TS playback, Watch TV. Low-CPU hot-plug monitor; top-right device badges (one per physical device, green/red); "no DVB devices" message and greyed tiles; tiles enabled by device role (capture / playback). Hot-plug events also on stdout and in the log file. |
| W2 | Devices page: one coloured card per device (frontends, delivery systems, driver, USB speed). |
| W3 | Capture page: device dropdown → one tab per supported delivery (S/T/C). Satellite: TP capture (reference TP or manual, "save as reference transponder" → `reference_transponders.json`) and SAT capture (satellite + TP add/remove/edit, KingOfSat import). File names `<sat>_<mhz>_<pol>_<sr>_<date>.ts` + `_services.json`. Live lock/signal/services during capture; stop button. |
| W4 | Playback page: pick file and modulation (pre-filled from the `_services.json` sidecar), play/stop, loop, "save as reference capture" → `reference_captures.json`. |
| W5 | Watch TV: "please scan" when no channels; otherwise Scan / Watch tabs. Scan for S/T/C. Channel list on the left (scan order) with S/T/C tabs; click plays in the browser; switching channels plays the right service. |
| W6 | Settings (capture directory, persistent) and a live, line-by-line log view; the same log goes to a file. |

### CLI / TUI

| Id | Requirement |
| -- | ----------- |
| C1 | `capture`, `playout`, `watch`, `update-satellites-json`, each with `--tui` where applicable; helpers `devices`, `analyze`, `loopback`, `serve`. |
| C2 | `Esc` goes back one screen everywhere; while capture/playout runs it asks Y/N before stopping `tsp`. |
| C3 | `watch --tui`: channel list per delivery (↑↓ PgUp/PgDn, Enter plays), mpv preferred, vlc fallback, closing the player returns to the list and stops `tsp`. |

### Open items carried over

| Id | Item |
| -- | ---- |
| X1 | Loopback test harness (L2a/L2b/L2c via `tscmp`, see `design_consideration.md`). |
| X2 | CI: Linux x86_64 release build. |

## Known defects found on 2026-10-20

| Id | Defect |
| -- | ------ |
| D1 | Channel records store a fixed `/dev/dvb/adapterN` path (changes with plug order) and no satellite / LNB / DiSEqC context, so a channel cannot be re-tuned reliably. |
| D2 | Web top-right badges show one badge per frontend (TBS5530 twice). |
| D3 | Web DVB-T / DVB-C scanning is "coming soon". |
| D4 | ~2,000 lines of experimental watch code (GStreamer embed / pipe variants), 37 compiler warnings. |
| D5 | Web UI is a single 1,300-line HTML file. |
| D6 | DTU-315 DVB-C support not detected (`qam:qam-a:dvb-c` flag ignored). |
| D7 | Web jobs stopped each other (no loopback from the browser); web satellite capture used another file layout without sidecars. |
| D8 | Browser playback of open-GOP / interlaced H.264 and MPEG-2 channels failed (decoder rejects the first samples). |
| D9 | WebSocket dropped clients that lagged behind the event stream. |

## Work phases

| Phase | Content | Verify |
| ----- | ------- | ------ |
| P1 | Watch cleanup: drop GStreamer, single mpv/vlc pipe, remove dead code, zero warnings. | `cargo test`, `watch --test` headless run on a Hot Bird FTA channel, TUI pty scenario. |
| P2 | Channel DB v2: stable device identity (USB VID:PID + serial/path, frontend by delivery), satellite/LNB/DiSEqC per channel, migration of the old file. | Unit tests; re-tune saved channels after re-plugging. |
| P3 | Scanning: S (TP list / NIT), T and C in CLI, TUI and web; save FTA channels. | Hot Bird scan finds FTA TV services; T/C via DTU-315 loopback. |
| P4 | Web watch: per-session HLS, fast start, correct channel switching, stop/cleanup. | Browser: `<video>` time advances, frames differ, switch channel → different picture; time-to-first-frame measured. |
| P5 | Web capture/playback/devices/settings/log pass (W1–W4, W6), split the UI into modules, fix D2. | Browser click-through of every page; API tests with curl; hot-plug test. |
| P6 | TUI regression pass (C1–C3). | pty + pyte scripted scenarios. |
| P7 | Loopback harness (X1) and Linux CI build (X2). | DTU-315 → TBS5530 run produces an L2a/b/c PASS report. |
| P8 | Docs (README, requirements, test log) and a final end-to-end PASS/FAIL report. | Report reviewed against this plan. |

## Progress (2026-10-20)

| Item | Status | Evidence |
| ---- | ------ | -------- |
| P1 watch cleanup | done | 0 warnings; `watch --test` PASS; SIGINT/SIGKILL cleanup PASS |
| P2 channel DB v2 | done | unit tests; channels re-tuned by delivery + model |
| P3 scanning S/T/C | done (S on hardware; T/C code paths) | Hot Bird: 76/123 TPs, 476 channels |
| P4 web watch | done | first frame 3.3 s, zap 3.8 s, no decode errors |
| P5 web pages | done (hot-plug test pending) | 21/21 API checks, capture / playout pages in the browser |
| P6 TUI pass | done | pty scenarios: watch filter + mpv, scan stop, capture |
| P7 loopback + CI | done (real DTU-315 run pending) | compare: synthetic PASS, negative controls FAIL; CI script PASS |
| P8 docs | done | README, requirements, test log |
| D1–D9 | fixed | see `docs/test_log.md` 2026-10-20 |

Pending (hardware): DTU-315 on a direct USB port → T2 / cable loopback with
`dvbhub loopback --delivery …` (L2a/b/c), web T/C scan and capture through the
loopback, and a plug / unplug check of the web badges.

