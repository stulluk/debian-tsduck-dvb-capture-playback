---
title: "Getting the Hauppauge WinTV-NOVA-S2 / PCTV 461e v3 working on Debian (mainline kernel)"
date: 2026-06-15
tags: [linux, dvb, debian, kernel, hauppauge, tsduck]
slug: pctv-461e-v3-debian
---

# Getting the Hauppauge WinTV-NOVA-S2 / PCTV 461e v3 working on Debian

I have a small USB DVB-S2 satellite stick that ships under two names:
**Hauppauge WinTV-NOVA-S2** and **PCTV 461e**. The latest revision (USB id
`2013:0462`, the "461e v3") moved to a new demodulator chip, and that one tiny
hardware change is enough to make the stick *not work* on most current Linux
kernels. This post walks through what is going on and exactly how I got it
capturing live Turksat transponders on Debian with a mainline kernel.

## The hardware inside

Under the hood the 461e v3 is:

- **Empia EM28178** USB bridge (`em28xx` driver)
- **Montage M88DS3103C** demodulator - note the **C**; older units used the
  `B` revision
- **Montage M88TS2022** tuner (`ts2020` driver)
- **Allegro A8293** LNB power controller / SEC (`a8293` driver)

The "C" demod is the whole story. It has different clocking, initialization and
lock registers than the `B`, so the in-tree `m88ds3103` driver could not drive
it, and the USB id `2013:0462` was not in the `em28xx` device table.

## Why it does not "just work" yet

Support for this exact hardware was submitted to linux-media on **2026-03-17**
by Bradford Love (two patches: *m88ds3103: Implement 3103c chip support* and
*em28xx: Add Hauppauge 461e v3*). It has since landed in the mainline tree, but:

- it is **not** in the v6.17 / v7.0 kernel tags, and
- the firmware is **never** bundled in the kernel - you always install it
  yourself.

So on a typical Debian box today you either run a kernel newer than the merge,
or you patch and build one.

## Step 1 - kernel with 461e v3 support

If your kernel predates the merge (check with `grep PCTV_461E_V3
/usr/src/linux/drivers/media/usb/em28xx/em28xx.h`), apply the two linux-media
patches and rebuild. I keep them in my kernel build tree as:

```
0002-m88ds3103-Implement-3103c-chip-support.patch
0003-em28xx-Add-Hauppauge-461e-v3.patch
```

The em28xx patch just adds a board entry and the `2013:0462` id; the m88ds3103
patch adds the whole `3103c` code path (reverse-engineered from the Windows
driver, so there are some "magic" register writes).

## Step 2 - install the firmware

This is the step everyone misses. The demod needs `dvb-demod-m88ds3103c.fw`
(version 3.8) in `/lib/firmware`:

```bash
sudo curl -fL -o /lib/firmware/dvb-demod-m88ds3103c.fw \
  https://hauppauge.s3.us-east-1.amazonaws.com/linux/dvb-demod-m88ds3103c.fw
```

Without it the demod stays in "cold state" and never downloads, so nothing
locks.

## Step 3 - plug it in and verify

```bash
$ dmesg | grep -E "461e v3|m88ds3103|firmware version"
em28xx 1-3:1.0: Identified as PCTV DVB-S2 Stick (461e v3) (card=108)
m88ds3103 12-006a: downloading firmware from file 'dvb-demod-m88ds3103c.fw'
m88ds3103 12-006a: firmware version: 3.8

$ ls /dev/dvb/adapter0/
demux0  dvr0  frontend0  net0
```

Full chain up: em28178 + m88ds3103c + ts2022 + a8293. The stick is now a normal
Linux DVB adapter.

## Step 4 - tune a transponder

Install the v4l-utils DVB tools (`dvb-tools` on Debian) and try a known
transponder. Two things bit me here, so learn from my pain:

### Gotcha 1 - LNB power

If you connect a dish's universal LNB **directly** to the stick, the 461 must
power the LNB itself. The A8293 does this fine - I measured **12.69 V** for a
13 V (vertical) command and **17.65 V** for 18 V (horizontal) right at the
F-connector. If you instead feed it from an **externally powered multiswitch**,
the stick does not need to power anything and you select the satellite with a
DiSEqC port (port A = satellite number 0). Both work.

### Gotcha 2 - DVB-S vs DVB-S2

This one cost me an hour. On Turksat 42E some transponders are **DVB-S2** (8PSK)
and some older ones are plain **DVB-S** (QPSK). DVB-S is *always* QPSK. If you
tell the demod to look for DVB-S2 on a DVB-S carrier, it tunes, sees a signal,
but **never locks** - which looks exactly like a driver bug but is not. Set the
right delivery system and it locks instantly.

A working channel file (`chan.conf`) for a DVB-S2 transponder:

```
[TRT_HD]
	DELIVERY_SYSTEM = DVBS2
	FREQUENCY = 11054000
	POLARIZATION = VERTICAL
	SYMBOL_RATE = 30000000
	INNER_FEC = 3/4
	MODULATION = PSK/8
	SAT_NUMBER = 0
```

and a DVB-S one (note `DVBS` + `QPSK`):

```
[TRT_SD]
	DELIVERY_SYSTEM = DVBS
	FREQUENCY = 11096000
	POLARIZATION = HORIZONTAL
	SYMBOL_RATE = 30000000
	INNER_FEC = AUTO
	MODULATION = QPSK
	SAT_NUMBER = 0
```

Then:

```bash
# lock + show signal/C-N
dvbv5-zap -a 0 -c chan.conf -l UNIVERSAL -t 10 TRT_HD

# lock + record raw TS to a file
dvbv5-zap -a 0 -c chan.conf -l UNIVERSAL -t 30 -o out.ts -P TRT_HD
```

A healthy lock looks like:

```
Lock   (0x1f) Signal= -79.03dBm C/N= 13.42dB postBER= 0
```

## Results

On my setup all four reference Turksat 42E transponders lock and record cleanly
on the 461 - both with a direct LNB and through a multiswitch on DiSEqC port A:

| Transponder        | Delivery | Modulation | Result            |
| ------------------ | -------- | ---------- | ----------------- |
| 11054 V 30000      | DVB-S2   | 8PSK       | lock, C/N ~13 dB  |
| 11096 H 30000      | DVB-S    | QPSK       | lock, 27 services |
| 12245 H 27500      | DVB-S2   | 8PSK       | lock, C/N ~18 dB  |
| 12380 V 27500      | DVB-S    | QPSK       | lock, 20 services |

## Takeaways

- The 461e v3 needs **both** a recent-enough kernel (461e v3 / m88ds3103c
  support) **and** the `dvb-demod-m88ds3103c.fw` firmware.
- The driver itself is fine; my "no lock" problems were (1) the multiswitch
  DiSEqC path and (2) using DVB-S2 settings on a DVB-S carrier.
- Measure the F-connector voltage with a multimeter when in doubt - it instantly
  tells you whether the SEC/LNB power path is the problem.

Once it locks, it is just a standard DVB adapter, so tools like TSDuck can
capture and analyze the transport stream like any other tuner.
