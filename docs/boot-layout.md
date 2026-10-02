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

### Clean original-program primary restoration

The ninth RAM session restores the primary through the authenticated trial in
19.525 seconds. All four corrected program digests match the original; correction
counts are 16, 17, 13 and 27 bits, maximum five in one codeword, and zero
uncorrectable failures. Raw interleaved SHA256 is
`df701b1ab09d5b84ebe3e8e9277737f0f4ddb3cd4bef9f40bd00e6123d1929b0`,
which differs from the validated clean input. Backup and corrected U-Boot remain
intact. This proves write plus ECC-aware readback, not BROM selection of the
restored primary. A controlled backup-isolation trial is prepared under protocol
v5 to establish that additional fact. The selected release's SPL program is also
not assumed identical to the installed original.

### Rootfs bad-block offset correlation

The upstream [Linux v6.12 MTD core](https://raw.githubusercontent.com/torvalds/linux/v6.12/drivers/mtd/mtdcore.c)
translates SLC eraseblock starts before adding the partition offset:
`physical = 0x1000000 + (logical_partition_offset / 0x200000) * 0x400000`.
Applying this source-derived rule to the original read-only ioctl enumeration
matches all 61 BBT-reported bad physical addresses in the tenth RAM recovery log.
The four additional unavailable addresses are the final four physical blocks,
consistent with the separately measured four BBT blocks. The ioctl alone does
not by itself classify reservation or factory/runtime origin. See
[the exact correlation fixture](evidence/batch3/rootfs-bad-block-offset-correlation.json).

This is an eraseblock-start rule, not a page/byte translation: SLC reads and
writes additionally use the NAND pairing scheme. Exact installed-kernel source,
marker/reservation classification, utility write skip behavior and physical
rootfs installation remain to be validated. The eleventh RAM session subsequently
captures all 2,044 entries through authenticated mtdinfo enumeration, with the
same 65 unavailable indices as the preserved direct ioctl capture; see
[the fresh recovery map](evidence/batch3/recovery-rootfs-map-11.json). No
production gate is enabled by this correlation. Deterministic fixture tests also
reject treating the logical partition offsets as flat physical byte offsets.

### Exact original and locked release comparison

The decoded original SPL and locked 16 KiB release SPL differ at 12 bytes:
three in the stored checksum and nine in the ASCII build timestamp banner.
Every byte outside those two fields is identical. The banners date the original
build to June 12 and the locked build to September 13, 2026. See
[the SPL comparison](evidence/batch3/original-vs-locked-release-spl.json).
This narrows the program difference but does not prove BROM acceptance of the
exact release encoding or replace its physical test.

The full corrected original U-Boot and locked padded release U-Boot differ at
376,738 bytes, all below offset 688,938. Their specific source/config differences
remain unreviewed. Original-program boot and restoration evidence therefore do
not validate the locked release U-Boot. See
[the U-Boot comparison](evidence/batch3/original-vs-locked-release-uboot.json).

### Hynix physical marker access preparation

Linux stable v6.12.107 selects `NAND_BBM_LASTPAGE` for Hynix MLC and the
`dist3` pairing scheme for H27UCG8T2ETR-BC. A physical 4 MiB block has 256 pages;
its marker page is 255. SLC group 0 exposes 128 pages: page 0 maps to physical
0 and subsequent logical pages map to `2 * page - 1`. The final SLC page maps
to physical 253. Thus a raw read through the SLC rootfs partition still does not
directly expose the physical last-page marker. These are source-derived rules,
not a newly measured raw marker. Exact Debian/platform patch equivalence remains
under review. Sources and hashes are in
[the marker preparation record](evidence/batch3/hynix-marker-probe-preparation.json).

`images/marker_probe.py` prepares a pinned RAM diagnostic DTB with one read-only,
non-SLC alias for the first BBT-reported bad block observed at physical `0x38800000`.
It appends the alias after all original partitions and checks every original
node/property and child order. Standard overlay merging was rejected: its new
child appeared first in offline testing and would renumber the existing MTD
paths. The diagnostic DTB is reproducible and independently parses with dtc;
it is integrated into the closed protocol v7 diagnostic bootstrap/read operation
and has now been loaded on hardware in RAM session 13. Repeated physical
last-page OOB observations are identical; see the record below. This preparation
changes no NAND plan or production authorization.

Recovery inventory now enumerates bounded canonical MTD class entries, including
an optional sixth alias. Missing, noncanonical or larger layouts fail closed.
The original-SPL trial continues to require exactly five partitions, so a
physical marker alias blocks both preparation and fresh execution preflight.
Host and ARMv7 tests cover this boundary. RAM session 13 physically preserves
all original indices, reports the alias and rejects SPL preflight.

### Isolated clean restored-primary acceptance

After the requested bridge-removed power cycle, normal SSH returns on the same
SID with a new boot ID. Independent `nanddump --noecc --oob --bb=dumpbad` of
the full backup block confirms every data and OOB byte is still `0xff`. The
normal boot therefore establishes BROM acceptance of the clean restored primary
encoding, independently of the erased backup. It does not select one of the
four primary intra-block copies or validate the locked release program. UBI
attaches with the original root UUID, zero corrupted PEBs and zero MTD ECC
failures. See [the isolated boot record](evidence/batch3/normal-restored-primary-isolated-boot-10.json).
The original backup is subsequently restored and independently verified in
[RAM session 12](evidence/batch3/recovery-backup-restoration-trial-12.json).

The first probed bad block at physical `0x38800000` has a 4 MiB eraseblock,
16 KiB page, 1,664 OOB bytes and zero access flags (read-only, non-SLC). The
alias reports one bad block and no BBT reservation. Repeated fixed raw reads of
physical page 255 return identical evidence: all OOB bytes are zero and the data
digest matches a 16 KiB zero page. This directly observes non-`0xff` marker
bytes on one BBT-reported bad block; it does not establish factory/runtime
origin or the contents of all other bad blocks. The rootfs map and original
primary/backup/U-Boot readbacks remain healthy with zero ECC failures.
See [the physical read](evidence/batch3/recovery-physical-marker-13.json) and
[policy/boot-chain checks](evidence/batch3/recovery-marker-policy-and-protected-chain-13.json).
