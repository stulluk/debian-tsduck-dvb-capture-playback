# Reference Transponders (Turksat 42E) - Test Set

Fixed reference DVB-S/S2 transponders used for capture/loopback tests. These are
the user's known-good TPs in Istanbul; recorded with PCTV 461 (and TBS5530) as
reference streams. Do not re-ask; use this list.

| # | Freq (MHz) | Pol | Symbol rate (kS/s) | Modulation | FEC | Delivery | Content (group)                    | Notes              |
| - | ---------- | --- | ------------------ | ---------- | --- | -------- | ---------------------------------- | ------------------ |
| 1 | 11054      | V   | 30000              | 8PSK       | 3/4 | DVB-S2   | TRT group (HD)                     | Lower band, V      |
| 2 | 11096      | H   | 30000              | QPSK       | 5/6 | DVB-S    | TRT group (SD)                     | Lower band, H      |
| 3 | 12245      | H   | 27500              | 8PSK       | 5/6 | DVB-S2   | Dogan Medya (Kanal D HD, CNN Turk) | Upper band, H      |
| 4 | 12380      | V   | 27500              | QPSK       | 3/4 | DVB-S    | Turksat group (TGRT Haber HD)      | Upper band, V; NIT |

Delivery system **verified by lock** (2026-06-15, PCTV 461 direct LNB):
TP #1/#3 are DVB-S2 (8PSK); TP #2/#4 are **DVB-S** (QPSK). TP #2/#4 do *not*
lock as DVB-S2 - this earlier looked like a "QPSK demod bug" but was simply the
wrong delivery system in our reference list. See `docs/test_log.md`.

Derived tuning parameters (universal Ku LNB: LO 9750/10600 MHz, 22 kHz tone for
upper band; band split at 11700 MHz):

| # | Band  | LNB LO (MHz) | 22 kHz tone | L-band IF (MHz) | Delivery system |
| - | ----- | ------------ | ----------- | --------------- | --------------- |
| 1 | Lower | 9750         | OFF         | 1304            | DVB-S2          |
| 2 | Lower | 9750         | OFF         | 1346            | DVB-S           |
| 3 | Upper | 10600        | ON          | 1645            | DVB-S2          |
| 4 | Upper | 10600        | ON          | 1780            | DVB-S           |

Notes:
- 8PSK implies DVB-S2; DVB-S is always QPSK. The delivery systems above are
  confirmed by actual lock + recording, not assumed.
- TP #4 (12380 V) carries the NIT - useful as the seed for a full
  network/transponder scan (`tsscan` with NIT-based discovery).
- Multiswitch / DiSEqC: Turksat on port A uses `--satellite-number 0` (TSDuck
  default). All four TPs lock and record via PCTV 461 when delivery systems are
  correct (see Test 1d in `docs/test_log.md`).
- Band/tone/LO are normally handled automatically by TSDuck's universal LNB
  default; we only specify frequency, polarity, symbol rate, modulation, FEC,
  and delivery system.

TSDuck capture sketch (exact flags finalized once TSDuck is built):

```
# Example: TP #1 (11054 V, DVB-S2 8PSK 3/4), 30 s, full TP (no PID filtering)
tsp -I dvb \
    --delivery-system DVB-S2 \
    --frequency 11054000000 \
    --polarity vertical \
    --symbol-rate 30000000 \
    --modulation 8-PSK \
    --fec-inner 3/4 \
  -P until --seconds 30 \
  -O file refS_11054V.ts
```

LNB / dish context: single universal LNB, Turksat 42.0E. Lower-band TPs use LO
9750 (no tone), upper-band TPs use LO 10600 (22 kHz tone on).
