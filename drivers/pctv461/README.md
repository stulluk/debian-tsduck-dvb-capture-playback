# PCTV 461e v3 DKMS overlay

Out-of-tree DKMS package that adds Hauppauge / PCTV **461e v3** support
(USB ID `2013:0462`, Montage **m88ds3103c**) on stock Ubuntu/Debian kernels
that do not yet ship those linux-media patches.

- Installer: [`../../scripts/install-pctv461-dkms.sh`](../../scripts/install-pctv461-dkms.sh)
- Host notes: [`../../docs/ubuntu_stock_kernel_install.md`](../../docs/ubuntu_stock_kernel_install.md)
- Patches (from linux-media / adapted for torvalds tags):

  - `patches/0002-m88ds3103-Implement-3103c-chip-support.patch`
  - `patches/0003-em28xx-Add-Hauppauge-461e-v3.patch`

Do **not** commit the generated `/usr/src/pctv461-*` tree; the script rebuilds
it from torvalds sources + these patches.
