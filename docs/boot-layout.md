# PocketCHIP NAND boot layout — evidence and open questions

The original layout below was derived in Batch 0 from upstream source and
configuration. Batch 3 has now measured the development PocketCHIP's geometry,
raw boot-region contents and corrected U-Boot readback. Source-derived fallback
behavior remains separate from physical boot validation; unresolved claims are
retained until measured. Evidence is indexed in [the Batch 3 record](evidence/batch3/README.md).

## Source-derived layout

| Offset | Size | U-Boot `mtdparts` name | Linux DTS partition label | Written by upstream x-chip-tools | Evidence |
| --- | --- | --- | --- | --- | --- |
| `0x000000` | `0x400000` (4 MiB, 1 erase block) | `SPL` | `SPL` | `nand write.raw.noverify ${spl_img} 0x0 $PAGES_PER_EB` | `lib-nand.sh:76`, `sun5i-r8-chip.dts.nand.patch:41-44`, `nand.cfg:4` |
| `0x400000` | `0x400000` | `SPL.backup` | `SPL.backup` | `nand write.raw.noverify ${spl_img} 0x400000 $PAGES_PER_EB` | `lib-nand.sh:77`, `sun5i-r8-chip.dts.nand.patch:45-48`, `nand.cfg:4` |
| `0x800000` | `0x400000` | `U-Boot` | `U-Boot` | `nand write ${UBOOT_ADDR} 0x800000 0x400000` | `lib-nand.sh:78`, `sun5i-r8-chip.dts.nand.patch:49-52`, `nand.cfg:4` |
| `0xC00000` | `0x400000` | `U-Boot.backup` | `env` | **not written** | `nand.cfg:4`, `nand.cfg:11`, `sun5i-r8-chip.dts.nand.patch:53-56` |
| `0x1000000` | remaining SLC region | `rootfs` (`slc`) | `rootfs` (`slc-mode`) | formatted in place: `ubiformat /dev/mtd4`, then UBIFS | `nand.cfg:4`, `sun5i-r8-chip.dts.nand.patch:57-61`, `flash-live.sh:68-93` |

Supporting facts:

* U-Boot's partition string is `nand0:0x400000(SPL),0x400000(SPL.backup),0x400000(U-Boot),0x400000(U-Boot.backup),-(rootfs)slc`
  (`work/upstream/x-chip-uboot/nand.cfg:4`).
* The Linux DTS declares the same five offsets; the partition at `0xC00000` is
  labeled `env`, and `rootfs` at `0x1000000` carries `slc-mode`
  (`work/upstream/x-chip-linux-deb/sun5i-r8-chip.dts.nand.patch:36-62`).
* The recovery flow treats the UBI volume as `mtd4` and formats it directly
  (`work/upstream/x-chip-tools/flash-live.sh:68-84`). With either naming, the
  rootfs is the fifth partition (`mtd4`).
* U-Boot reads its primary payload from `0x800000` and its redundant payload
  from `0xC00000`: `CONFIG_SYS_NAND_U_BOOT_OFFS` defaults to `0x800000` for
  `NAND_SUNXI` and `CONFIG_SYS_NAND_U_BOOT_OFFS_REDUND=0xc00000` is set in
  `nand.cfg:11` (`work/lead/uboot-cfg/u-boot/drivers/mtd/nand/raw/Kconfig:582-594`,
  `common/spl/spl_nand.c:173-183`). `NAND_SUNXI` selects
  `SYS_NAND_U_BOOT_LOCATIONS`, which is what activates those offsets
  (`drivers/mtd/nand/raw/Kconfig:326-333`).
* The SPL image itself fills one erase block with SPL copies at the
  BROM-probed pages `0/64/128/192` (`lib-nand.sh:42-60`), so a single
  `nand write.raw.noverify` of one erase block carries all four BROM copies.
* The installer boot script erases the whole boot region before rewriting it:
  `nand erase 0x0 0x1000000` (`flash-live.sh:137-140`).

## Batch 3 measured state

The actual NAND is Hynix H27UCG8T2ETR-BC, manufacturer/device ID `ad:de`,
with 8 GiB physical capacity, 16 KiB pages, 4 MiB physical eraseblocks and
1,664-byte OOB. The four boot partitions expose one physical eraseblock each
at the source-derived offsets above. Their bad-block counts are zero. Both
original Linux observations and authenticated RAM recovery agree on the layout.
The normal kernel ECC policy is 56 bits per 1,024 bytes; SPL's boot0 image uses
its separate source-derived 64-bit ECC layout.

The rootfs partition starts at physical `0x1000000` and enables SLC-on-MLC
emulation. It exposes 4,286,578,688 logical bytes in 2 MiB eraseblocks, rather
than an 8 GiB flat write range. Original ioctl enumeration found 65 unavailable
logical blocks, consistent with 61 bad blocks plus four reserved BBT blocks.
Original UBI uses 2,064,384-byte LEBs, a 16,384-byte VID-header offset and
32,768-byte data offset. These values must constrain the eventual per-part
executor; they do not authorize execution by themselves.

Raw data/OOB reads of the programmed boot regions differ across captures.
Kernel-corrected U-Boot data matches the original 4 MiB backup SHA256 exactly:
`c76993ede3ceab2ba56e37b027c43896f4e4a79058cf4197aa7d1a7118b10224`.
The authenticated corrected read recorded 6,136 corrected bits and zero
uncorrectable errors. Raw digest equality cannot be the verification rule for
these programmed regions.

Native BCH-64 decoding of the hash-verified original SPL captures validates
four eGON boot0 groups per block, with 64-page spacing and a 16 KiB SPL size.
All eight copies have corrected SHA256
`a2640b992973e0ff042ea37de543d89bf9f855d1b64e5f485657267980d5f3cb`
and pass the stored eGON checksum. The maximum observed correction is nine bits
in one protected codeword. This is software decoding of physical captures.
See [the BCH evidence](evidence/batch3/spl-original-bch64-readback.json).

The seventh RAM recovery boot also performed native BCH decoding on live raw
readbacks of both SPL blocks through authenticated protocol v3. All eight copies
match the same original program digest and eGON checksum. Maximum correction
is seven bits in one codeword, and each complete block read/decode takes about
2.1 seconds. Corrected U-Boot again matches the original digest, with 6,496 bits
corrected and zero uncorrectable errors. See
[the live SPL evidence](evidence/batch3/recovery-spl-bch64-readbacks-7.json).
The subsequent controlled trial proves backup boot on this Hynix unit; generated-image
acceptance remains unproven, as described below.

The fourth boot block at `0xC00000` remains completely FF in data and OOB,
and its raw hashes match the original backup. The Linux `env` label does not
establish a stored environment. The exact reviewed source/configuration hashes
and redundant-load behavior are preserved in
[the source-policy evidence](evidence/batch3/uboot-backup-source-policy.json).
After removing the FEL bridge, the original installation boots to the Vitrallis
shell and SSH returns. Its root UBIFS identity matches the baseline, with no
uncorrectable NAND/UBI failure in the captured boot log. See
[the normal-boot evidence](evidence/batch3/original-nand-normal-boot.json).
Generated SPL acceptance still requires physical testing. The original normal-boot
record predates the controlled primary erase and backup-boot result below.

## The `0xC00000` conflict

Two upstream components claim the same `0x400000`-byte slot:

1. U-Boot calls it `U-Boot.backup` and points its redundant U-Boot read at it
   (`nand.cfg:4,11`; `common/spl/spl_nand.c:176-183`). If the primary read at
   `0x800000` fails, SPL tries `0xC00000`.
2. The Linux device tree calls it `env` (`sun5i-r8-chip.dts.nand.patch:53-56`),
   the legacy NextThing name for the U-Boot environment partition.

The upstream flasher writes neither a second U-Boot copy nor an environment to
that slot (`lib-nand.sh:70-80`). The resolved U-Boot configuration has
`CONFIG_ENV_IS_NOWHERE=y` (no environment is stored anywhere: every
`CONFIG_ENV_IS_IN_*` is unset), so a stored environment at `0xC00000` is not
established by the reviewed configuration.

Consequences and status:

* The offset and size are agreed by both sources; only the name/purpose differ.
* Whether the redundant U-Boot fallback can succeed after an upstream-style
  flash is **UNRESOLVED** physically: the measured slot is erased, and source-derived
  fallback requires a valid programmed redundant payload.
* Whether U-Boot or Linux ever writes an environment to `0xC00000` is
  **UNRESOLVED**; the reviewed configuration provides no environment location.

This must be resolved before Batch 3 destructive execution chooses whether to populate a
redundant U-Boot copy. The current review plan still schedules no operation there.

## Other unresolved layout facts (require hardware)

* Hynix geometry and authenticated live SPL/U-Boot ECC readback are measured
  above; executable bad-block handling, release-artifact verification and
  Toshiba geometry remain unresolved.
* Whether the BROM/SPL accepts the `sunxi-nand-image-builder` output for both
  the Hynix H27UCG8T2ETR (OOB 1664) and Toshiba TC58TEG5DCLTA00 (OOB 1280)
  parts (`lib-nand.sh:22-34`).
* Bad-block handling and power-loss behaviour during SPL/U-Boot rewrite.
* The NAND-ID detection byte (`*0x1c03035`: `40` = Toshiba, `60` = Hynix) and
  the upstream fallback that defaults to Hynix when neither value matches
  (`lib-nand.sh:62-80`). Batch 0 does not endorse using that fallback.

## Related documents

* [image provenance](image-provenance.md) — artifact identities/hashes
* [recovery protocol](recovery-protocol.md) — host/device behavioural contract
* [physical validation](physical-validation.md) — hardware gates

### Controlled primary-SPL erase

The eighth authenticated RAM session erased only mtd0 through the fixed Flasher
trial. Full raw readback verifies 4,194,304 data bytes and 425,984 OOB bytes are FF,
including all 256 page markers; zero ECC failures remain. The durable host journal
records Verified. Independent backup SPL and corrected U-Boot readbacks still
match their original digests. The unit has an erased primary and intact backup. After removing the FEL bridge
and power-cycling, it boots the original Vitrallis installation through the backup. This
is a controlled diagnostic, not a newly flashed OS or approved release.

The cold backup boot has a new Linux boot ID
`ed39f489-88cd-4b32-bbc3-f40227e18d26`, matching nvmem SID, kernel
`6.12.107+deb13-chip` and original root UUID
`D4C5E196-7073-43CD-B90D-3CCF2A1AF8A1`. An independent SSH raw NAND
read after boot still finds every primary data/OOB byte FF, with interleaved digest
`71c220404abcfabfbeb7480a98fcbc9e44e64290ee9c0c9b8a65d6aabefefdab`.
This establishes BROM fallback to the preserved SPL backup block on this unit,
not which of its four identical copies was selected. All MTD ECC failure counters
are zero and bad-block inventory is unchanged. The result does not prove Toshiba
behavior, newly generated SPL acceptance or interruption during a write. See
[the backup-boot evidence](evidence/batch3/normal-backup-spl-boot-8.json).
