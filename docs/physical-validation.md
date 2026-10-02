# Physical validation still required

Batch 3 live inventory and boot readbacks are recorded in
[evidence](evidence/batch3/README.md). No production installation has completed.
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
independent full backup data/OOB still erased. Backup restoration awaits FEL
re-entry, and new-release SPL acceptance remains pending. The production
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
