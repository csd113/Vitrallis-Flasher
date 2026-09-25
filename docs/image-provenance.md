# Image provenance — Batch 0 artifact closure

This document is the evidence-backed map of every artifact the future physical
flasher needs, where each one comes from, which exact revision produced it, and
what remains unresolved. It is backed by `upstream-lock.json` (validated
offline by `python3 scripts/provenance.py check`) and by the checked-in kernel
configuration under `images/evidence/`.

Status vocabulary used here:

* **VERIFIED** — the identity, source revision and hash were established from
  primary evidence in this batch.
* **VERIFIED BUT PREBUILT** — the bytes are identified and hash-pinned, but the
  upstream build has not been reproduced byte-for-byte.
* **PARTIAL** — the source is identified, but the artifact cannot yet be
  represented as a fully pinned input.
* **BLOCKED / UNKNOWN** — not enough evidence.

No artifact is approved for flashing. `physical_roles` in the lock records all
eight manifest roles; a role with no artifact is explicit, not silently empty.

## Release roles

| Role | Meaning (from the reviewed recovery flow) |
| --- | --- |
| `uboot` | Complete FEL-loaded bootloader image (BROM header + SPL + U-Boot + DTB); loaded with `sunxi-fel -p uboot`. |
| `uboot-nand` | U-Boot payload written to NAND at `0x800000`, zero-padded to one erase block (4 MiB). Distinct from `uboot`. |
| `kernel` | Recovery kernel loaded at `0x42000000`; comes from the pinned rootfs, not a separate download. |
| `dtb` | Device tree loaded at `0x43000000`; must match the kernel build and carry `__symbols__` for overlay fixups. |
| `recovery` | Recovery initramfs loaded at `0x43300000`. It is not self-contained: it requires `kernel` + `dtb` + a reviewed boot script. |
| `spl-hynix` | Raw NAND SPL image for SK Hynix H27UCG8T2ETR (OOB 1664 bytes/page). |
| `spl-toshiba` | Raw NAND SPL image for Toshiba TC58TEG5DCLTA00 (OOB 1280 bytes/page). |
| `rootfs` | Debian 13/trixie rootfs archive streamed to the on-device UBIFS installer (`mtd4`). |

The *installed* kernel, DTB and PocketCHIP overlay are not separate flash roles:
they live inside the pinned rootfs and are loaded from UBIFS by the installed
`/boot/boot.scr`. The host-generated installer boot script is not a manifest
artifact either; `lib-nand.sh` builds it from reviewed fixed commands.

## Pinned sources

| Repository | Revision | Tag → commit | License status | Purpose |
| --- | --- | --- | --- | --- |
| [u-boot/u-boot](https://github.com/u-boot/u-boot) | `d637294e264adfeb29f390dfc393106fd4d41b17` | `v2022.01` (annotated, tag object `18f2590b…`) | GPL-2.0-or-later | Upstream U-Boot source |
| [nextthingco/x-chip-uboot](https://github.com/nextthingco/x-chip-uboot) | `0e17d167ce72977420e4e54656d97de1f3237885` | `uboot-2026.09.13-122745` | UNRESOLVED — no root license grant | Bootloader patches, NAND config, release binaries |
| [nextthingco/x-chip-linux-deb](https://github.com/nextthingco/x-chip-linux-deb) | `d2fa89a991f3a1aa5ca812268d75acc82e6b933e` | none | CC0-1.0 for tooling; patches modify GPL-2.0-only Linux | Kernel config fragment and device-tree patches for the pinned rootfs |
| [nextthingco/x-chip-deb-repo](https://github.com/nextthingco/x-chip-deb-repo) | `40a8723194368dd29c663b0af01f9737c10edf8f` | none | UNRESOLVED — no root license grant | Last package-repo state published before the accepted rootfs build (kernel deb built at the prior run, `f18ad24`) |
| [nextthingco/CHIP-dt-overlays](https://github.com/nextthingco/CHIP-dt-overlays) | `e79aee33ef7664c16de02acbc3fe00ce5f3d6ada` | none | MIT (`debian/copyright`) | PocketCHIP/DIP device-tree overlays |
| [nextthingco/x-chip-os](https://github.com/nextthingco/x-chip-os) | `7584eab1aafb1667bd89ae210dcd641efc7cc5b5` | `os-2026.07.29-024145` | UNRESOLVED — no root license grant | live-build rootfs recipe |
| [nextthingco/x-chip-tools](https://github.com/nextthingco/x-chip-tools) | `215f98eed44babb699c129bb124537d32c414102` | `installer-2026.06.19-181233` | UNRESOLVED — no root license grant; committed throwaway key | Recovery flow, boot layout, SPL generation, recovery initramfs |
| [linux-sunxi/sunxi-tools](https://github.com/linux-sunxi/sunxi-tools) | `d7bbd172a5da601a08f94479de308c6fb714a19a` | none recorded | GPL-2.0-or-later | `sunxi-fel`, `sunxi-nand-image-builder` |
| [csd113/Vitrallis-Shell](https://github.com/csd113/Vitrallis-Shell) | `40b232780729853cf49691d160517ab866b4addb` | none | UNRESOLVED at the reviewed commit; MIT added later at `3146f8f` | Optional desktop profile, never bundled |

Current default-branch heads observed on 2026-09-25 are recorded separately in
the lock (`head`). The accepted pin is the `commit` field; `head` is evidence
that the upstream branch has moved. Notably, `x-chip-os` HEAD is now
`f9191c2` (2026-09-22), three commits past the accepted rootfs recipe, and the
CHIP apt repository now publishes kernel `6.12.107-1.31` instead of the
`6.12.94-1.29` that is inside the accepted rootfs.

### Rootfs candidate decision

Batch 0 keeps the already-reviewed rootfs
`pocketchip-rootfs.tar.gz` (`os-2026.07.29-024145`, `010eb2a0…`) as the
artifact referenced by the physical roles, and records the newer
`pocketchip-rootfs-2026-09-23.tar.gz` (`os-2026.09.23-010738`, `1e516cad…`)
as a fully identified alternative in the lock. Both are hash-pinned; neither is
approved for flashing.

| | accepted (`os-2026.07.29`) | alternative (`os-2026.09.23`) |
| --- | --- | --- |
| source commit | `7584eab` | `f9191c2` |
| kernel | `6.12.94+deb13-chip` `6.12.94-1.29` | `6.12.107+deb13-chip` `6.12.107-1.31` |
| kernel package available upstream today | no (404) | yes, hash-pinned in the live repo |
| boot script bootargs | no overscan parameters | `fw_devlink=permissive sun4i_tv.overscan_x=40 overscan_y=20` |
| existing repo inspection docs | yes (`rootfs-inspection.json`, `storage-inspection.json`) | package/licence audit in Batch 0 only |

The alternative kernel's configuration was verified from its published `.deb`
(`436a91ee…`): all 35 `nand.cfg` lines resolve and the NAND/UBI/UBIFS stack is
built-in, identical in the boot-critical symbols to the accepted kernel. A
future batch may adopt the newer rootfs; doing so requires re-running the
storage-policy inspection against that archive, because its installed package
set differs (the accepted rootfs's kernel is no longer downloadable, and
`pocket-home 0.0.8` / `tic80 1.1.2837-1` have changed bytes at the same version
in the live repository — version strings are not content pins).

## Artifact matrix

| Role | Artifact | SHA-256 | Size | Status | Reproducibility |
| --- | --- | --- | --- | --- | --- |
| `uboot` | `u-boot-sunxi-with-spl.bin` (uboot-2026.09.13-122745) | `23d2730799a109946753147413153687877e41265981833c4466d034d2279d05` | 721,776 | VERIFIED BUT PREBUILT | SOURCE KNOWN / BUILD NOT YET REPRODUCED |
| `uboot-nand` | `u-boot-dtb.bin` zero-padded to 4 MiB | `2c5de011e950263c1e940c0a926863404d3dc4a936ae20823226d5b2b5dbafb8` | 4,194,304 | VERIFIED | deterministic transform of bit-reproduced `u-boot-dtb.bin` (`f64e582f…`, 688,944 B) |
| `kernel` | `boot/vmlinuz-6.12.94+deb13-chip` inside rootfs | `c159646938235663a7997798fa5edde02889f835e33a19f6323a1d8c9ac7513c` | 6,676,992 | VERIFIED BUT PREBUILT | PINNED PREBUILT BINARY (inside rootfs) |
| `dtb` | `boot/dtbs/6.12.94+deb13-chip/sun5i-r8-chip.dtb` inside rootfs | `0132f7fa312542c374c7e5dfec2a1a12893b9cbe490d22e165fd7a89de51762e` | 25,537 | VERIFIED BUT PREBUILT | REPRODUCIBLE FROM PINNED SOURCE (rebuild proven; same bytes ship in both kernel releases) |
| `recovery` | `initrd.uimage` (installer-2026.06.19-181233) | `5b8b392c095fd37472f9a08f1ed8f6cdbb6d6a04bb36a0696dbcd3033c0f3b25` | 31,628,120 | VERIFIED BUT PREBUILT | PINNED PREBUILT BINARY |
| `spl-hynix` | not yet a published artifact | — | — | PARTIAL | generated from `sunxi-spl.bin` + OOB 1664; padding nondeterminism unresolved |
| `spl-toshiba` | not yet a published artifact | — | — | PARTIAL | generated from `sunxi-spl.bin` + OOB 1280; padding nondeterminism unresolved |
| `rootfs` | `pocketchip-rootfs.tar.gz` (os-2026.07.29-024145) | `010eb2a0cb59334f068d3a5e6989bdc486715362e7a5d04fbb329e7b556e0de2` | 516,321,424 | VERIFIED BUT PREBUILT | PINNED PREBUILT BINARY |

The released SPL source is `sunxi-spl.bin` (`879cff4d6345a12091fa8084bab5a002556989b2d10ce1898905dd8666729ba0`,
16,384 bytes) from the same U-Boot release. The two per-part NAND images are
built by `sunxi-nand-image-builder` (sunxi-tools `d7bbd172`) with
`-c 64/1024 -p 16384 -o 1664|1280 -u 1024 -e 4194304 -b -s`.

## Compatibility relations the flasher must enforce

| Relation | Constraint | Evidence / status |
| --- | --- | --- |
| kernel ↔ DTB | Both come from the same `6.12.94+deb13-chip` package inside the pinned rootfs; DTB compiled with `__symbols__`. | Verified hashes; filenames encode the kernel version. |
| DTB ↔ PocketCHIP overlay | Overlay fixups require `pwm`, `pio`, `reg_vcc3v3`, `i2c1`, `rtp`, `tcon0`, `tcon0_out`, `tve0`, `uart3`, and pinctrl labels; all are present in the locked DTB's `__symbols__`. | Overlay `x-chip-pocketchip.dtbo` (`3230ea7f…`) was rebuilt byte-identically from `CHIP-dt-overlays` `e79aee3` and statically applied with `fdtoverlay`; the merged tree contains the panel, backlight, TCA8418 keyboard and touch properties. |
| overlay ↔ rootfs | The overlay is installed by `chip-dt-overlays` 0.8 into `/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo` and applied by U-Boot from UBIFS when the DIP EEPROM reports PID `0x0001`. | `boot.scr` extracted from the locked rootfs; `debian/chip-dt-overlays.install`. |
| base DTB ↔ kernel source | The locked DTB (`0132f7fa…`) was reproduced byte-identically from Debian `linux 6.12.107-1` plus `sun5i-r8-chip.dts.nand.patch` and `sun5i-r8-chip.dtb-symbols.patch` (`dtc -@`); the same DTB file appears in both kernel releases. | Rebuild evidence in the Batch 0 report; DTB hash identical in rootfs and debs. |
| SPL ↔ NAND part | Per-part OOB geometry: Hynix 1664, Toshiba 1280; the SPL source is identical. Runtime detection uses byte `*0x1c03035` (`40` Toshiba, `60` Hynix). | `lib-nand.sh`; hardware validation pending. |
| U-Boot ↔ MTD layout | Both U-Boot `mtdparts` and the kernel DTS place SPL at `0x0`, backup at `0x400000`, U-Boot at `0x800000`, the fourth slot at `0xC00000` and rootfs at `0x1000000`. The fourth slot is named `U-Boot.backup` by U-Boot and `env` by Linux. | See [boot layout](boot-layout.md); unresolved. |
| boot script ↔ addresses | Installer boot script loads zImage/DTB/boot.scr/initrd at `0x42000000`/`0x43000000`/`0x43100000`/`0x43300000`; installed `boot.scr` loads kernel/DTB from UBIFS at the same `0x42…`/`0x43…`. | `flash-live.sh`, extracted `boot.scr`. |
| kernel ↔ modules | The rootfs contains `/usr/lib/modules/6.12.94+deb13-chip`; panel/backlight/keyboard/touch are modules (`DRM_PANEL_SIMPLE=m`, `BACKLIGHT_PWM=m`, `PWM_SUN4I=m`, `KEYBOARD_TCA8418=m`, `TOUCHSCREEN_SUN4I=m`) while DRM/DRM_SUN4I/DRM_SUN4I_BACKEND are built-in. | `sun4i_tcon_probe()` returns `-EPROBE_DEFER` when the panel driver is not yet bound (`sun4i_tcon.c`), so the driver core re-probes the tcon when `panel-simple` loads from the rootfs after UBIFS mounts; the `sun4i_rgb_init()` swallow path is not reached for a present panel endpoint. Runtime display behavior is still **unvalidated — requires hardware**; the residual risk is that `panel-simple` never loads at all. |
| rootfs ↔ kernel package | Rootfs dpkg database records `linux-image-6.12.94+deb13-chip 6.12.94-1.29`; the installed `/boot/config-…` is the checked-in evidence. | `docs/rootfs-inspection.json`, `docs/storage-inspection.json`. |

## Reproducibility

* **U-Boot/SPL** — reproduced in this batch in a Docker `debian:bookworm`
  container (`arm-linux-gnueabihf-gcc` 12.2.0, binutils 2.40 — the toolchain
  strings embedded in the releases). With `SOURCE_DATE_EPOCH` pinned to the
  release banner time:
  - `sunxi-spl.bin` and `u-boot-dtb.bin` are **bit-identical** to every release
    (2026-06-09, 2026-06-12, 2026-09-13);
  - `u-boot-sunxi-with-spl.bin` differs from every release in exactly **5
    bytes**: the legacy uImage `time` field and its header CRC, which the
    original workflow set from a second wall-clock reading ~23 s after the
    version banner. Payload and data CRC match.
  The binaries are therefore explained and payload-reproducible; the combined
  image is not bit-identical with a single pinned timestamp. The
  `debian:bookworm` base image and apt toolchain are not pinned by the upstream
  recipe.
* **CHIP DT overlays** — all eight shipped `.dtbo` files in `chip-dt-overlays`
  0.8 were rebuilt from `CHIP-dt-overlays` `e79aee3` and are byte-identical to
  the deb payload.
* **Recovery initramfs** — built with live `debootstrap` against
  `deb.debian.org`; not reproducible from the pinned repository alone. Its
  build script deletes `/usr/share/doc`, so the distributed artifact carries 107
  packages without notices.
* **Kernel** — built by `apt-get source linux` from live Debian mirrors with a
  CI run-number version serial (`6.12.94-1.29` = run 29); not reproducible from
  the pinned repository alone. The realized configuration of the accepted
  package is checked in and asserted. The source revision `d2fa89a` is
  established by config discrimination among reachable commits:
  `CONFIG_DRM_SUN4I_BACKEND=y` appears in `nand.cfg` only at `d2fa89a` (and its
  byte-identical successor `6ed9015`), and the accepted kernel's built-in config
  matches it; `.29` can only be CI run 29, whose `x-chip-linux-deb` gitlink was
  `d2fa89a`; `6ed9015` also postdates the rootfs build. This is
  strong inference, not cryptographic proof: the `.deb` embeds no source
  revision, run-29 logs are unavailable, and a force-pushed revision cannot be
  fully excluded. The Debian base source (`linux 6.12.94-1`) is not pinned by
  the repository.
* **Rootfs** — built from live mirrors with no snapshot; not reproducible from
  current upstreams. It is consumed as a pinned prebuilt artifact.

## Licensing and redistribution

The per-component audit is in [upstream licenses](upstream-licenses.md) and in
the `license` fields of the lock. Headline findings:

* `CHIP-dt-overlays` is MIT; `u-boot/u-boot` is GPL-2.0-or-later;
  `sunxi-tools` is GPL-2.0-or-later; `x-chip-linux-deb`'s tooling is CC0-1.0.
* `x-chip-uboot`, `x-chip-os`, `x-chip-tools` and `x-chip-deb-repo` have **no
  root license grant**: `UNRESOLVED — redistribution status not established`.
  The compiled U-Boot/kernel images remain GPL-family binaries, but the
  repository scripts/configs themselves have no observed grant.
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
```

`verify-upstream` is deliberately not part of routine CI so that validation
does not depend on upstream availability. It fails closed on any size/hash or
tag/commit mismatch.

## Unresolved items carried into later batches

1. `spl-hynix` / `spl-toshiba` are generated images with random padding
   upstream. Batch 1 must either generate them deterministically (fixed
   padding) and record hashes, or represent them as derived outputs rather than
   downloaded assets.
2. The fourth NAND slot at `0xC00000` is named `U-Boot.backup` by U-Boot and
   `env` by Linux, and upstream never writes it. See [boot layout](boot-layout.md).
3. The live CHIP apt repository is mutable and has already replaced the kernel
   package inside the pinned rootfs. No old package or Debian snapshot state is
   archived by upstream.
4. **LCD path (highest-value hardware check).** The published `-chip` kernel
   builds DRM/tcon in but the panel, PWM backlight and touch drivers as modules,
   and `boot.scr` passes no initramfs. Static re-analysis shows
   `sun4i_tcon_probe()` itself returns `-EPROBE_DEFER` while the panel driver is
   unbound, so the driver core re-probes the tcon when `panel-simple` loads from
   the rootfs; an earlier "tcon swallows the defer and the LCD stays dark"
   inference was refuted. The overlay and DTB wiring are verified; actual
   display behavior and the case where `panel-simple` never loads still require
   a device.
5. The boot script ignores the DIP `product_version` byte, so v72/v73 keymap
   revisions both receive the v73 keymap; no `gpio-keys` node exists for a
   home/power button in the base DTB or overlay.
6. Actual NAND geometry, bad-block/ECC behaviour and power-loss behaviour remain
   hardware-only questions.
