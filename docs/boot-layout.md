# PocketCHIP NAND boot layout — evidence and open questions

This document is the Batch 0 source of truth for the on-NAND boot layout. It is
derived from upstream source and configuration; **nothing here has been validated
on hardware**. Every claim cites its evidence. Conflicts are preserved instead of
resolved by assumption.

## Established offsets

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
  flash is **UNRESOLVED** without hardware: the fallback slot is never
  populated, so a failed primary read should fail.
* Whether U-Boot or Linux ever writes an environment to `0xC00000` is
  **UNRESOLVED**; the reviewed configuration provides no environment location.

This must be resolved before Batch 5 (write path) chooses whether to populate a
redundant U-Boot copy. Batch 0 deliberately does not modify flashing code.

## Other unresolved layout facts (require hardware)

* Actual NAND geometry (usable pages, bad blocks, ECC strength/layout) per part.
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
