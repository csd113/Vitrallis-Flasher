# Physical validation still required

Batch 3 live inventory and boot readbacks are recorded in
[evidence](evidence/batch3/README.md). No production installation has completed.
The exact locked Hynix SPL now passes isolated normal boot with the backup
fully erased. RAM session 16 restores the original backup while preserving the
release primary and original U-Boot. Locked U-Boot acceptance and the full
production installation remain pending.

Native FEL identity, SRAM upload/readback/execute/restore and RAM-only recovery
boot are now physically measured. Recovery authentication and board/NAND inventory
succeed over macOS ECM. The initial RAM restart failed; adding the pinned reset module corrected it.
Authenticated recovery now returns to FEL and the native transport rechecks SID. Authenticated raw boot readbacks independently match the erased fourth block;
programmed raw hashes differ, requiring ECC-aware verification. The seventh RAM
boot verifies all eight original SPL copies with native BCH-64 and matches the
kernel-corrected U-Boot digest, with zero uncorrectable failures. These are live
original-installation readbacks, not generated-image BROM acceptance or fallback
selection. No production NAND executor or physical manifest is approved, and
no destructive installation has run.

Batch 2 added host abstractions and scripted failure injection without hardware
access. Those historical tests remain distinct from the measured Batch 3 evidence.
The remaining release gates below must be closed using exact application/upstream
revisions, hashes, board/NAND identity and sanitized logs. Failed or incomplete
hardware tests never count as successful validation.

## Release gates

- Establish permissions for upstream recovery/image reuse and distribution; review
  pinned bootloader/SPL assets, build provenance and corresponding sources.
- Close the reproducible image input lock; build twice and compare hashes. The
  host-side closure is complete: Batch 1 builds the complete artifact set twice
  from clean state with bit-identical output and validates both physical
  manifests. Hardware validation still must verify the stock PocketHome desktop,
  menus, startup, compatible applications and calibration. For
  `vitrallis-default` only, additionally verify the complete ARM bundle, ABI,
  automatic startup and Marshmallow fallback/removal contract.
- Implement and review the authenticated fixed-operation recovery protocol described
  in `recovery-protocol.md`. Keep real writes blocked until it passes review.
- Prove board/NAND identification before erase. Preserve Hynix/Toshiba geometry and
  reject unknown parts; validate SPL primary/backup, padded U-Boot and SLC rootfs.
  `spl-hynix`/`spl-toshiba` are pinned as deterministic derived artifacts from the
  locked `sunxi-spl.bin` and pinned tool; hardware must still confirm that the BROM
  and controller accept them on both parts.
- Validate the LCD module path on hardware: the published `-chip` kernel builds the
  DRM/tcon stack in but the panel/backlight/touch drivers as modules, and the boot
  script passes no initramfs. `sun4i_tcon_probe()` defers until `panel-simple` loads
  from the rootfs; confirm the tcon re-probe and that `/dev/fb0`/a DRM connector
  appears. See [image provenance](image-provenance.md).
- Bind fresh recovery identity and session nonce to the selected FEL SID. Test wrong
  endpoint, spoofed IP/MAC, untrusted SSH peer, unplug/replug and multiple devices.

## Device/host matrix

Test separate Hynix H27UCG8T2ETR and Toshiba TC58TEG5DCLTA00 boards on Windows x86-64,
macOS arm64, macOS x86-64 and Linux x86-64; add Linux arm64. Record FEL enumeration,
USB permissions, driver identity, recovery gadget re-enumeration, networking and
endpoint authentication. Windows needs deliberate WinUSB and gadget-driver validation;
macOS needs a working ECM/NCM solution. No driver changes should be automatic.

For each board, test a full erase/write/readback, bad-block handling, correct bootloader
copies and normal NAND boot with the jumper removed. Verify USB/network disconnect,
slow transfer, cancellation at every boundary, tool crash, verification mismatch,
low-power interruption and a fresh recovery retry. A verification failure must not
produce “Complete”, reboot, or preserve prior erase authorization. Use sacrificial
or backed-up hardware under explicit authorization for destructive fault tests.

Validate the actual LCD/backlight, keyboard, touchscreen calibration, Home/Power key,
Wi-Fi, sound, battery reporting and PocketHome stock behavior for both profiles.
For `vitrallis-default`, additionally test Vitrallis launch and all three native utilities,
app focus/resume, systemd session cleanup, startup failure and return to Marshmallow.
Verify the stock image contains no Vitrallis files or startup hooks. Include cold boot,
readability at 480×272, CPU/RAM and storage constraints. Package presence alone cannot
prove these behaviors.

Finally test unsigned/signed distribution separately: Windows SmartScreen and driver
signing, macOS Developer ID/notarization, Linux library availability and artifact
checksums/notices. The existing CI matrix checks compilation/tests only and cannot
substitute for this hardware and distribution evidence.

## Memory pressure and NAND writes

Run the [storage-policy validation matrix](debian-optimizations.md#release-validation)
for both desktop profiles and NAND parts. Reject disk/NAND swap, zram backing-device
writeback, unbounded temporary growth, broken UBI health persistence, and any implicit
fallback to disk swap. Compare no-swap and small RAM-only zram workloads, verify log
retention behavior and update compatibility, and preserve crash recovery. Package
presence and configuration files alone are not runtime proof.

### Original-SPL trial preparation

Protocol v4 integrates the reviewed fixed mtd0 trial. Host/ARMv7 tests cover
tickets, pre-dispatch cancellation, lost responses and exact verified completion.
The recovery template contains the pinned private original-SPL restoration image;
local tool capabilities and backup boot-chain health are checked before dispatch.
Physical v4 preparation, host cancellation, wrong-key/SID/stale-session rejection
and fixed primary erase are measured. The raw primary readback is completely
erased, and backup SPL/U-Boot retain their original digests without ECC failures.
Normal backup boot is independently confirmed over SSH while full raw primary
readback remains erased. SID/kernel/root UUID are unchanged, all ECC failure
counters are zero and bad-block counts remain stable. Clean restoration is now physically verified with all four original program
digests and zero uncorrectable failures. Protocol v5 also physically erases the
backup while protecting the primary. Host SIGINT after dispatch produces an
Indeterminate journal and no success; independent authenticated readback proves
the device completed erasure with primary/U-Boot intact. Isolated restored-primary acceptance is now measured by normal SSH boot with
independent full backup data/OOB still erased. Backup restoration is subsequently verified in RAM session 12 with independent
primary/backup/U-Boot checks. New-release SPL acceptance remains pending. The production
manifest/plan and full reflash remain blocked until their required evidence exists.

### Fresh recovery rootfs bad-block map

Protocol v6 now physically enumerates the rootfs through the fixed read-only
mtdinfo map operation. All 2,044 logical eraseblocks are reported, including the
same 65 unavailable indices as the original ioctl enumeration. Device-local
geometry/counters are stable before and after capture, with zero ECC failures;
the host validates every entry after authentication. The final block is reported
BAD, preventing acceptance of an early stopped utility query. Original primary
program and U-Boot digests remain healthy and the backup remains fully erased.
This establishes fresh recovery enumeration, not bad-marker classification,
write skip behavior or a production installation. Evidence:
[the actual map](evidence/batch3/recovery-rootfs-map-11.json).

### Exact locked Hynix primary: physical programming and isolation

RAM session 15 physically authenticates the pinned v8 implementation on the same
SID and original five-partition layout. The closed `ProgramReleasePrimary`
diagnostic reaches Verified in 20.428 seconds. All four physical SPL copies decode
to `879cff4d6345a12091fa8084bab5a002556989b2d10ce1898905dd8666729ba0`
with checksum `0x5305ee97`; native BCH decoding corrects 65 bits total, maximum
three per 1 KiB chunk, with zero ECC failures. Raw encoded readback is not byte
identical to the clean artifact; decoded program verification is the completion
criterion. An independent subsequent read also checks all four release copies,
the original backup program and original U-Boot digest.

The original `EraseBackup` preflight now correctly rejects this release primary
without Execute; a fresh authenticated Pong still succeeds. The separately
guarded `EraseBackupForReleasePrimary` then reaches Verified in 18.800 seconds.
Independent readback confirms all 256 data pages and OOB pages erased, interleaved
SHA256 `71c220404abcfabfbeb7480a98fcbc9e44e64290ee9c0c9b8a65d6aabefefdab`,
while the release primary and original U-Boot remain healthy with zero ECC
failures. Both host journals are version 2, bind the exact three release hashes
and end in Verified. The original restoration snapshot remains available in RAM.

The requested next action is a normal boot with the FEL bridge removed. **Locked
release BROM acceptance is still pending**; programming/readback does not prove
it. No release U-Boot, physical manifest approval or full production reflash is
established here. Evidence: `evidence/batch3/recovery-program-release-primary-trial-15.json`,
`recovery-boot-chain-after-release-primary-15.json`,
`recovery-original-backup-guard-rejects-release-primary-15.json`,
`recovery-erase-backup-for-release-primary-trial-15.json`, and
`recovery-release-primary-isolated-preboot-15.json` in the same directory. Private
version-2 journals are under `work/batch3/locked-release-trial-session-15/`.

### Original and release U-Boot provenance cross-check

Current byte comparisons bind the saved original kernel-corrected 4 MiB U-Boot
exactly to the preserved `tag-0612-epoch` rebuild after zero padding. The exact
locked 4 MiB artifact similarly matches `head-epoch`. Both corresponding rebuilt
SPL programs match their original/locked pins. The generated configurations are
byte-identical; embedded U-Boot DTBs are byte-identical at 25,864 bytes and SHA256
`6dbd6067868a79d08e6f4e9d5bd2dcffafa1b4cab936410609f0073725736736`,
although the release DTB is shifted 176 bytes later. Common NAND/SLC and w1
patches match exactly. The September build inputs additionally contain two
composite-video patches and use its release timestamp. This narrows the earlier
unattributed binary discrepancy using exact preserved rebuild bytes, rather than
assuming a changed banner is the only difference. No new rebuild was run here.

The identical configuration selects `CONFIG_ENV_IS_NOWHERE`, primary U-Boot
at `0x800000` and redundant U-Boot at `0xc00000`. SPL source tries the redundant
location after a primary load error. Its NAND reader advances consecutive
physical pages; it does not establish a boot-region bad-block skip policy.
Strict good-block prerequisites remain necessary for raw boot slots. The Linux
`env` label does not establish saved-environment use. Physical redundant U-Boot
fallback and exact release U-Boot acceptance remain pending, and this evidence
does not authorize a production write at `0xc00000`.

Hash-pinned comparisons and build inputs are recorded in
`evidence/batch3/uboot-rebuild-original-release-crosscheck.json`; source locations
and hashes remain in `uboot-backup-source-policy.json` in the same directory.

### Exact locked Hynix primary: isolated normal boot measured

After the user removed the FEL bridge and powered on, strict known-host SSH
returns with new boot ID `055f9c77-8e1c-406e-a11d-e25d6dec63ee` and kernel
`6.12.107+deb13-chip`. Independent normal-OS raw `nanddump` confirms all
4,620,288 backup data/OOB bytes remain FF. The primary is also dumped raw; host
native BCH decoding checks all four copies against the exact locked program
digest. Their corrected-bit totals are 23, 14, 18 and 22 (maximum four per chunk);
all MTD ECC failure counts are zero and bad blocks remain `[0,0,0,0,61]`.

UBI attaches with 1,979 good PEBs, 65 unavailable PEBs and zero corrupted PEBs.
The existing dynamic rootfs volume remains 1,960 LEBs with 2,064,384 usable bytes
per LEB. This is physical acceptance of the exact locked Hynix SPL while the
backup block is erased. The particular in-block copy selected by BROM remains
unobserved. This boot uses the original U-Boot and existing rootfs; it is not
proof of locked U-Boot acceptance, a newly installed OS or full production flash.

Evidence: `evidence/batch3/normal-locked-release-primary-isolated-boot-15.json`.
Private raw/data dumps are under `work/batch3/locked-release-trial-session-15/`.
No package-management process was active before clean SSH poweroff (exit 0).
Backup restoration is pending the requested return to FEL.

### Original backup restored after isolated release-primary boot

RAM session 16 authenticates the same pinned v8 implementation on the same SID.
`RestoreBackupForReleasePrimary` preflight requires the exact release primary and
original U-Boot. Its closed restoration reaches Verified in 20.487 seconds and
the durable host journal ends in Verified with version-2 release pins. All four
backup copies match the original program; native correction totals are 19, 15,
18 and 15 (67 total, maximum four per chunk), with zero ECC failures. The raw
interleaved digest differs from the clean encoding, as expected for corrected
NAND data. Independent subsequent readbacks confirm the release primary,
original backup, original U-Boot and full raw FF fourth block. The five-partition
RAM inventory remains healthy. No U-Boot or rootfs diagnostic write was dispatched.

Evidence: `evidence/batch3/recovery-backup-restoration-preflight-16.json`,
`recovery-backup-restoration-trial-16.json` and
`recovery-boot-chain-after-backup-restoration-16.json` in the same directory.
The private version-2 journal is under `work/batch3/locked-release-trial-session-16/`.

### Bounded U-Boot candidate preparation

The read-only `uboot-check-pair` CLI validates a fixed 8 MiB bundle containing
512 alternating original/release 8 KiB chunk pairs. It checks the bundle and both
4 MiB program digests before creating private snapshots, then revalidates their
open descriptors. The pair contains bytes only; no caller address, command or
replacement digest is accepted. It exposes no NAND operation.

A bounded host archive probe adds that bundle to the pinned v8 archive with the
unchanged v8 daemon: the compressed image is 41,673,253 bytes, leaving 269,787
bytes under the existing 40 MiB limit. This is a size probe, not a loaded or
reviewed U-Boot-execution payload. It uses the existing gzip archive builder and
adds no decompression dependency. The next daemon/build still needs validation
against this limit, authenticated physical boot and a separately guarded U-Boot
diagnostic. Exact release U-Boot boot and redundant-slot behavior remain pending.
Evidence: `evidence/batch3/uboot-pair-host-check.json` and
`uboot-pair-ram-size-preparation.json` in the same directory.

### Closed U-Boot diagnostic preparation (v9)

The new recovery payload contains the pinned original/release pair and six
closed mtd2/mtd3 diagnostic operations. Its prerequisites protect the opposite
U-Boot and the exact mixed SPL chain restored in session 16. Both templates
are byte-identical, 41,687,740 bytes, below the unchanged 40 MiB load bound.
Version-3 journals and prepared replies bind all candidate/protected SPL hashes.
Normal U-Boot ECC remains separate from boot0 SPL ECC. This is software/build
validation; physical v9 boot, locked release U-Boot acceptance and measured SPL
fallback to the fourth block remain pending. Production flashing stays blocked.

### RAM session 17: release U-Boot backup and isolated preboot

The pinned v9 image is physically loaded after authenticated RAM return from
session 16 to FEL and fresh native confirmation of the same SID. Authenticated
inventory and independent boot reads preserve the release primary SPL, original
backup SPL, original primary U-Boot and erased fourth block. The no-write backup
preflight validates the exact pair and protected-chain bindings.

`ProgramReleaseUbootBackup` completes Verified in 30.904 seconds. Its complete
kernel-corrected 4 MiB data hash is the exact locked release `2c5de011…afb8`;
OOB markers remain FF and ECC failures remain zero. Independent readbacks verify
the unchanged original primary U-Boot and both SPL programs. This measures
program/readback acceptance, not normal boot acceptance.

The second and final permitted session dispatch, `EraseOriginalUbootPrimary`,
completes Verified in 28.095 seconds. Independent raw primary readback contains
FF in every data/OOB byte, hash `71c22040…fdab`, while both SPL programs and the
corrected release backup remain intact. All MTD ECC failures are zero; the four
boot blocks remain good and rootfs bad/BBT counts remain 61/4. Both journals are
version 3 with Intent → Prepared → Dispatched → Verified. Rootfs is untouched.
Cold normal boot with the bridge removed is requested; actual SPL fallback and
release U-Boot boot are still pending. No production reflash is claimed.

Evidence under `evidence/batch3/`: `recovery-boot-17-v9-tool.json`,
`recovery-inventory-17-v9.json`, `recovery-boot-chain-before-uboot-17.json`,
`recovery-uboot-backup-preflight-17.json`,
`recovery-program-release-uboot-backup-trial-17.json`,
`recovery-boot-chain-after-uboot-backup-17.json`,
`recovery-erase-original-uboot-primary-trial-17.json` and
`recovery-release-uboot-backup-isolated-preboot-17.json`. Private version-3
journals are under `work/batch3/uboot-trial-session-17/`.

### Host-only rootfs installation stream audit

While the isolated U-Boot cold boot remains pending, a complete bounded scan of
the exact stock rootfs archive establishes installation requirements without
extraction or NAND access. The compressed archive is 516,563,033 bytes, larger
than the recovery RAM capacity. It contains 50,950 members and 1,264,012,666
regular-file bytes; the largest regular file is 115,964,520 bytes. Content hashing
uses 8 KiB reads. Counts and compressed digest agree with the preserved Batch 1
build report; kernel, DTB, overlay and boot-script contents independently match
the preserved actual files.

The archive contains 36,116 regular files, 5,067 directories, 9,753 symlinks,
six hardlinks and eight character devices. All hardlinks target earlier regular
files; no member has a non-directory archive ancestor. Numeric owners, device
major/minor values and setuid/setgid modes are recorded. There are 633 absolute
symlinks and 1,192 symlinks with parent components. These are legitimate rootfs
metadata; future extraction must contain resolution within the new root and
preserve links rather than following them into the RAM recovery filesystem.
The device entries are only the eight recorded standard character devices,
not a grant to install arbitrary device nodes.

The ordered canonical semantic digest, using the existing repack metadata/content
encoding, is `50deb4906c1cfc5bf30b766cdb0e711713b8564022b65a686878ef6bab500c35`.
The report also records critical content hashes, ownership/modes and boot/OS
identity links. This is host-only evidence, not an implemented installer,
readback of installed contents, physical manifest approval or a flashed OS.
Production rootfs streaming, UBI installation and semantic completion checks
remain required. Evidence:
`evidence/batch3/rootfs-installation-stream-audit.json`; private audit script/log:
`work/batch3/audit-rootfs-installation.py` and
`work/batch3/rootfs-installation-stream-audit.log`.
