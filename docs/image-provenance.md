# Image provenance — Batch 0 closure, Batch 1 selection

This document is the evidence-backed map of every artifact the future physical
flasher needs, where each one comes from, which exact revision produced it, and
what remains unresolved. It is backed by `upstream-lock.json` (validated
offline by `python3 scripts/provenance.py check`) and by the checked-in kernel
configuration under `images/evidence/`.

Batch 0 established the provenance closure. Batch 1 selected the September
rootfs, replaced the SPL padding nondeterminism with a documented deterministic
method, and implemented the real host-side builder
(`docs/image-build.md`). Status vocabulary is unchanged:

* **VERIFIED** — the identity, source revision and hash were established from
  primary evidence.
* **VERIFIED BUT PREBUILT** — the bytes are identified and hash-pinned, but the
  upstream build has not been reproduced byte-for-byte.
* **PARTIAL** — the source is identified, but the artifact cannot yet be
  represented as a fully pinned input.
* **BLOCKED / UNKNOWN** — not enough evidence.

No artifact is approved for flashing. `physical_roles` in the lock records all
eight manifest roles; every role now resolves to an identified artifact.

## Release roles

| Role | Meaning (from the reviewed recovery flow) |
| --- | --- |
| `uboot` | Complete FEL-loaded bootloader image (BROM header + SPL + U-Boot + DTB); loaded with `sunxi-fel -p uboot`. |
| `uboot-nand` | U-Boot payload written to NAND at `0x800000`, zero-padded to one erase block (4 MiB). Distinct from `uboot`. |
| `kernel` | Recovery kernel loaded at `0x42000000`; comes from the selected rootfs, not a separate download. |
| `dtb` | Device tree loaded at `0x43000000`; must match the kernel build and carry `__symbols__` for overlay fixups. |
| `recovery` | Recovery initramfs loaded at `0x43300000`. It is not self-contained: it requires `kernel` + `dtb` + a reviewed boot script. |
| `spl-hynix` | Raw NAND SPL image for SK Hynix H27UCG8T2ETR (OOB 1664 bytes/page), generated deterministically. |
| `spl-toshiba` | Raw NAND SPL image for Toshiba TC58TEG5DCLTA00 (OOB 1280 bytes/page), generated deterministically. |
| `rootfs` | Debian 13/trixie rootfs archive streamed to the on-device UBIFS installer (`mtd4`). |

The *installed* kernel, DTB and PocketCHIP overlay are not separate flash
roles: they live inside the pinned rootfs and are loaded from UBIFS by the
installed `/boot/boot.scr`. The host-generated installer boot script is not a
manifest artifact either; `lib-nand.sh` builds it from reviewed fixed commands.

## Pinned sources

| Repository | Revision | Tag → commit | License status | Purpose |
| --- | --- | --- | --- | --- |
| [u-boot/u-boot](https://github.com/u-boot/u-boot) | `d637294e264adfeb29f390dfc393106fd4d41b17` | `v2022.01` (annotated, tag object `18f2590b…`) | GPL-2.0-or-later | Upstream U-Boot source |
| [nextthingco/x-chip-uboot](https://github.com/nextthingco/x-chip-uboot) | `0e17d167ce72977420e4e54656d97de1f3237885` | `uboot-2026.09.13-122745` | UNRESOLVED — no root license grant | Bootloader patches, NAND config, release binaries |
| [nextthingco/x-chip-linux-deb](https://github.com/nextthingco/x-chip-linux-deb) | `d2fa89a991f3a1aa5ca812268d75acc82e6b933e` (July kernel); `6ed9015281df266c73ce56673724cac0f18bc625` (selected kernel) | none | CC0-1.0 for tooling; patches modify GPL-2.0-only Linux | Kernel config fragment and device-tree patches |
| [nextthingco/x-chip-deb-repo](https://github.com/nextthingco/x-chip-deb-repo) | `40a8723194368dd29c663b0af01f9737c10edf8f` (July state); `20bb1e8795305729e4587225439cca04fc475182` (selected kernel state) | none | UNRESOLVED — no root license grant | Package repository behind the selected rootfs |
| [nextthingco/CHIP-dt-overlays](https://github.com/nextthingco/CHIP-dt-overlays) | `e79aee33ef7664c16de02acbc3fe00ce5f3d6ada` | none | MIT (`debian/copyright`) | PocketCHIP/DIP device-tree overlays |
| [nextthingco/x-chip-os](https://github.com/nextthingco/x-chip-os) | `f9191c2914c94a3bfe2030556cadf7aaa3d9b4ff` | `os-2026.09.23-010738` | UNRESOLVED — no root license grant | live-build rootfs recipe that produced the selected rootfs |
| [nextthingco/x-chip-tools](https://github.com/nextthingco/x-chip-tools) | `215f98eed44babb699c129bb124537d32c414102` | `installer-2026.06.19-181233` | UNRESOLVED — no root license grant; committed throwaway key | Recovery flow, boot layout, SPL generation, recovery initramfs |
| [linux-sunxi/sunxi-tools](https://github.com/linux-sunxi/sunxi-tools) | `d7bbd172a5da601a08f94479de308c6fb714a19a` | none recorded | GPL-2.0-or-later | `sunxi-fel`, deterministic `sunxi-nand-image-builder` |
| [csd113/Vitrallis-Shell](https://github.com/csd113/Vitrallis-Shell) | `40b232780729853cf49691d160517ab866b4addb` | none | UNRESOLVED at the reviewed commit; MIT added later at `3146f8f` | Optional desktop profile, never bundled |

Current default-branch heads observed on 2026-09-25 remain recorded separately
in the lock (`head`). The `x-chip-os` accepted revision is now `f9191c2`; the
July recipe commit `7584eab` is retained in `tag_history` as the fallback.

## Rootfs decision

Batch 1 adopted `pocketchip-rootfs-2026-09-23.tar.gz` (`os-2026.09.23-010738`,
`1e516cad…`, 516,563,033 B) and kept the July archive
(`pocketchip-rootfs.tar.gz`, `010eb2a0…`, 516,321,424 B) pinned and documented
as the fallback.

| | selected (`os-2026.09.23`) | fallback (`os-2026.07.29`) |
| --- | --- | --- |
| source commit | `f9191c2` | `7584eab` |
| kernel | `6.12.107+deb13-chip` `6.12.107-1.31` | `6.12.94+deb13-chip` `6.12.94-1.29` |
| kernel package available upstream today | yes, hash-pinned; config `436a91ee…` and vmlinuz `d3c044d8…` match the released `.deb` | no (404) |
| boot script | `61901b03…`, `fw_devlink=permissive` plus overscan args | `1fa91fee…` |
| DTB / PocketCHIP overlay | `0132f7fa…` / `3230ea7f…` | byte-identical |
| inspection evidence | `docs/rootfs-inspection.json`, `docs/storage-inspection.json` regenerated for this archive | Batch 0 record retained in the lock |

The selected archive's package state is additionally bound by
`images/package-inventory-6.12.107+deb13-chip.json` (709 packages with
per-package version, architecture, status and `.list`/`.md5sums` hashes). The
checked-in kernel configuration
`images/evidence/kernel-config-6.12.107+deb13-chip` (`436a91ee…`) is asserted
offline by `python3 scripts/provenance.py check-kernel-config`; all 35
`nand.cfg` requirements resolve. The selected archive cannot be rebuilt from
current upstreams because the live CHIP apt repository deletes replaced
packages and no Debian snapshot was recorded upstream; it is consumed as a
pinned prebuilt binary.

## Artifact matrix

| Role | Artifact | SHA-256 | Size | Status | Reproducibility |
| --- | --- | --- | --- | --- | --- |
| `uboot` | `u-boot-sunxi-with-spl.bin` (uboot-2026.09.13-122745) | `23d2730799a109946753147413153687877e41265981833c4466d034d2279d05` | 721,776 | VERIFIED BUT PREBUILT | SOURCE KNOWN / BUILD NOT YET REPRODUCED (5 uImage time/CRC bytes explained) |
| `uboot-nand` | `u-boot-dtb.bin` zero-padded to 4 MiB | `2c5de011e950263c1e940c0a926863404d3dc4a936ae20823226d5b2b5dbafb8` | 4,194,304 | VERIFIED | deterministic transform of bit-reproduced `u-boot-dtb.bin` (`f64e582f…`, 688,944 B) |
| `kernel` | `boot/vmlinuz-6.12.107+deb13-chip` inside rootfs | `d3c044d80034bb10513c5e987afe208b489c8f36c76a222f223ff63d1f8e1540` | 6,697,472 | VERIFIED BUT PREBUILT | PINNED PREBUILT BINARY (inside rootfs; matches the pinned live `.deb`) |
| `dtb` | `boot/dtbs/6.12.107+deb13-chip/sun5i-r8-chip.dtb` inside rootfs | `0132f7fa312542c374c7e5dfec2a1a12893b9cbe490d22e165fd7a89de51762e` | 25,537 | VERIFIED BUT PREBUILT | REPRODUCIBLE FROM PINNED SOURCE (rebuild proven in Batch 0; same bytes in both kernel releases) |
| `recovery` | `initrd.uimage` (installer-2026.06.19-181233) | `5b8b392c095fd37472f9a08f1ed8f6cdbb6d6a04bb36a0696dbcd3033c0f3b25` | 31,628,120 | VERIFIED BUT PREBUILT | PINNED PREBUILT BINARY |
| `spl-hynix` | `spl-hynix` (derived, OOB 1664) | `0099342e6331e9880d704bf11eb75f1b7618be8cef05b2ae456918fa1010fc7a` | 4,620,288 | VERIFIED | DETERMINISTIC FROM PINNED SOURCE + PINNED TOOL |
| `spl-toshiba` | `spl-toshiba` (derived, OOB 1280) | `487edb2eda190bd98b85deacaf858fcc4583351c3210be4ebc19ee983d19dd45` | 4,521,984 | VERIFIED | DETERMINISTIC FROM PINNED SOURCE + PINNED TOOL |
| `rootfs` | `pocketchip-rootfs-2026-09-23.tar.gz` (os-2026.09.23-010738) | `1e516cade3085633f61697d69a5d95cb84a501d8b606247987db5837a53e19ef` | 516,563,033 | VERIFIED BUT PREBUILT | PINNED PREBUILT BINARY |

The released SPL source is `sunxi-spl.bin`
(`879cff4d6345a12091fa8084bab5a002556989b2d10ce1898905dd8666729ba0`,
16,384 bytes) from the same U-Boot release.

## Deterministic SPL generation

Upstream generates the per-part images with `lib-nand.sh`, which appends
`/dev/urandom` padding between four 64-page BROM groups; the pinned tool also
fills unused page space from `/dev/urandom`. Batch 1 keeps the tool binary
semantics and replaces those two entropy inputs with a checked-in
content-addressed stream (`images/spl.py`):

```text
seed    = SHA-256("Vitrallis Flasher deterministic NAND SPL padding v1")
block_i = SHA-256(seed || 0x00 || label || big-endian-uint64(i))
```

The tool is rebuilt from `sunxi-tools` `d7bbd172`
(`nand-image-builder.c` `3876bac0…`) in the digest-pinned container with
`-Dfls=sunxi_fls`, and each variant runs twice with the deterministic stream
bound over `/dev/urandom`. The two runs, and two independent toolchains, agree
byte-for-byte:

| Variant | OOB | Size | SHA-256 |
| --- | --- | --- | --- |
| `spl-hynix` | 1664 | 4,620,288 | `0099342e6331e9880d704bf11eb75f1b7618be8cef05b2ae456918fa1010fc7a` |
| `spl-toshiba` | 1280 | 4,521,984 | `487edb2eda190bd98b85deacaf858fcc4583351c3210be4ebc19ee983d19dd45` |

A control comparison against an unmodified upstream-random run shows the
ECC-protected data/ECC regions are identical on all 256 pages and only the
unused tail/BBM padding differs. `images/spl.py` descrambles every page's
protected region and checks it against the exact SPL/pad source chunk; a wrong
OOB geometry, BBM byte, truncation or content mutation is rejected.

## Compatibility relations the flasher must enforce

| Relation | Constraint | Evidence / status |
| --- | --- | --- |
| kernel ↔ DTB | Both come from the same `6.12.107+deb13-chip` package inside the selected rootfs; DTB compiled with `__symbols__`. | Verified hashes; filenames encode the kernel version; validated again per build and by the physical manifest. |
| DTB ↔ PocketCHIP overlay | Overlay fixups require `pwm`, `pio`, `reg_vcc3v3`, `i2c1`, `i2c1_pins`, `pwm0_pin`, `rtp`, `tcon0`, `lcd_rgb565_pins`, `tcon0_out`, `tve0` and `uart3`; all are present in the locked DTB's `__symbols__`. | Overlay `x-chip-pocketchip.dtbo` (`3230ea7f…`) was rebuilt byte-identically from `CHIP-dt-overlays` `e79aee3` and statically applied with `fdtoverlay` in Batch 0; the validator re-checks the fixup/label relation on every manifest. |
| overlay ↔ rootfs | The overlay is installed by `chip-dt-overlays` 0.8 into `/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo` and applied by U-Boot from UBIFS when the DIP EEPROM reports PID `0x0001`. | `boot.scr` extracted from the selected rootfs; `debian/chip-dt-overlays.install`; validator compares the archive member hash to the overlay input. |
| base DTB ↔ kernel source | The locked DTB (`0132f7fa…`) was reproduced byte-identically from Debian `linux 6.12.107-1` plus `sun5i-r8-chip.dts.nand.patch` and `sun5i-r8-chip.dtb-symbols.patch` (`dtc -@`). | Batch 0 rebuild evidence; the same DTB file appears in both kernel releases. |
| SPL ↔ NAND part | Per-part OOB geometry: Hynix 1664, Toshiba 1280; the SPL source is identical. Runtime detection uses byte `*0x1c03035` (`40` Toshiba, `60` Hynix). | `lib-nand.sh`; deterministic per-part images recorded above; hardware validation pending. |
| U-Boot ↔ MTD layout | Both U-Boot `mtdparts` and the kernel DTS place SPL at `0x0`, backup at `0x400000`, U-Boot at `0x800000`, the fourth slot at `0xC00000` and rootfs at `0x1000000`. The fourth slot is named `U-Boot.backup` by U-Boot and `env` by Linux. | See [boot layout](boot-layout.md); unresolved; the physical validator enforces the fixed offsets, no overlap and the `0x1000000` rootfs start. |
| boot script ↔ addresses | Installer boot script loads zImage/DTB/boot.scr/initrd at `0x42000000`/`0x43000000`/`0x43100000`/`0x43300000`; installed `boot.scr` loads kernel/DTB from UBIFS at the same `0x42…`/`0x43…`. | `flash-live.sh`; extracted `boot.scr`; validator checks the release paths and `bootz` addresses. |
| kernel ↔ modules | The rootfs contains `/usr/lib/modules/6.12.107+deb13-chip`; panel/backlight/keyboard/touch are modules while DRM/DRM_SUN4I/DRM_SUN4I_BACKEND are built-in. | `sun4i_tcon_probe()` returns `-EPROBE_DEFER` when the panel driver is not yet bound, so the driver core re-probes the tcon when `panel-simple` loads from the rootfs after UBIFS mounts. Runtime display behaviour is still **unvalidated — requires hardware**; the residual risk is that `panel-simple` never loads at all. |
| rootfs ↔ kernel package | Rootfs dpkg records `linux-image-6.12.107+deb13-chip 6.12.107-1.31`; the installed `/boot/config-…` is checked-in evidence. | `docs/rootfs-inspection.json`, `docs/storage-inspection.json`, package inventory. |

## Reproducibility

* **U-Boot/SPL source** — reproduced in Batch 0 in a Docker `debian:bookworm`
  container (`arm-linux-gnueabihf-gcc` 12.2.0, binutils 2.40). With
  `SOURCE_DATE_EPOCH` pinned to the release banner time, `sunxi-spl.bin` and
  `u-boot-dtb.bin` are bit-identical to all three releases;
  `u-boot-sunxi-with-spl.bin` differs in exactly the 5-byte legacy uImage
  time/header-CRC pair.
* **Deterministic per-part SPLs** — generated twice per variant and in two
  independent toolchains with identical hashes; structure and content
  validated page-by-page as described above.
* **CHIP DT overlays** — all eight shipped `.dtbo` files in
  `chip-dt-overlays` 0.8 were rebuilt byte-identically from `CHIP-dt-overlays`
  `e79aee3`.
* **Recovery initramfs** — built upstream with live `debootstrap`; not
  reproducible from the pinned repository alone. Its build script deletes
  `/usr/share/doc`, so the distributed artifact carries 107 packages without
  notices.
* **Kernel** — the July kernel `6.12.94-1.29` was built from live Debian
  mirrors with a CI run-number serial and is not reproducible; the selected
  kernel `6.12.107-1.31` matches the hash-pinned live `.deb` byte-for-byte
  (config `436a91ee…`, vmlinuz `d3c044d8…`) but upstream logs do not prove its
  full build closure.
* **Rootfs** — the selected archive is consumed as a pinned prebuilt binary.
  Batch 1 additionally rewrites it deterministically without extraction and
  proves source/output member equivalence (same paths, types, modes, numeric
  ownership, devices, hardlinks, symlink targets and content hashes; mtimes and
  gzip headers normalized). The repacked archive is a reproducibility artifact,
  not the flash payload; the physical role stays the exact upstream bytes.
* **Complete artifact set** — `images/assemble.py reproduce` builds the entire
  set twice from clean state and compares every file. Batch 1 recorded a
  bit-identical result; see [validation](validation.md).

## Licensing and redistribution

The per-component audit is in [upstream licenses](upstream-licenses.md) and in
the `license` fields of the lock. Headline findings:

* `CHIP-dt-overlays` is MIT; `u-boot/u-boot` is GPL-2.0-or-later;
  `sunxi-tools` is GPL-2.0-or-later; `x-chip-linux-deb`'s tooling is CC0-1.0.
* `x-chip-uboot`, `x-chip-os`, `x-chip-tools` and `x-chip-deb-repo` have **no
  root license grant**: `UNRESOLVED — redistribution status not established`.
* The pinned rootfs retains Debian notices, but two CHIP packages are
  unresolved: `chip-power` `bin/*` relies on asserted third-party permission
  with no license text, and `pocketchip-onboard` ships PICO-8/Celeste/SunVox
  imagery under an unfilled Debian copyright template. `firmware-realtek` is
  binary-redistributable with notice and a no-reverse-engineering clause.
* Redistributing the rootfs, U-Boot or an image triggers GPL/LGPL source-offer
  obligations; the initramfs currently ships with its package notices deleted.

## Verification commands

```text
python3 scripts/provenance.py check                 # offline, also run by CI
python3 scripts/provenance.py report                # artifact matrix
python3 scripts/provenance.py check-kernel-config   # asserts the checked-in config
python3 scripts/provenance.py verify-upstream       # explicit network re-download/tag check
python3 images/assemble.py verify --set <set>       # physical manifest + checksum set
```

`verify-upstream` is deliberately not part of routine CI so that validation
does not depend on upstream availability. It fails closed on any size/hash or
tag/commit mismatch.

## Unresolved items carried into later batches

1. Actual NAND geometry, ECC/bad-block runtime behaviour and power-loss
   behaviour per part remain hardware-only questions.
2. The fourth NAND slot at `0xC00000` is named `U-Boot.backup` by U-Boot and
   `env` by Linux, and upstream never writes it. See [boot layout](boot-layout.md).
3. The live CHIP apt repository is mutable and has already replaced packages
   inside the pinned rootfs. No old package or Debian snapshot state is archived
   by upstream; the selected rootfs stays a pinned prebuilt input.
4. **LCD path (highest-value hardware check).** The `-chip` kernel builds
   DRM/tcon in but the panel, PWM backlight and touch drivers as modules, and
   `boot.scr` passes no initramfs. Static re-analysis shows
   `sun4i_tcon_probe()` itself returns `-EPROBE_DEFER` while the panel driver is
   unbound, so the driver core re-probes the tcon when `panel-simple` loads from
   the rootfs. The overlay and DTB wiring are verified; actual display behaviour
   and the case where `panel-simple` never loads still require a device.
5. The boot script ignores the DIP `product_version` byte, so v72/v73 keymap
   revisions both receive the v73 keymap; no `gpio-keys` node exists for a
   home/power button in the base DTB or overlay.
6. No physical manifest is approved and no NAND write path exists. Approval
   must bind the exact manifest and artifact hashes, and the hardware,
   redistribution and protocol gates in [to-do.md](../to-do.md) must pass
   first.
