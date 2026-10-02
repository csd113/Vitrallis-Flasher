# Release blockers

Physical flashing remains disabled. This checklist tracks the work needed for a
usable hardware release; passing the mock suite alone does not complete these items.

## Host architecture status (Batch 2, 2026-10-01)

Batch 2 refactored the host backend behind narrow, deterministic seams and is
complete: `ToolRunner` for external tools, `FelTransport` with a scripted test
double and an explicitly unavailable production transport, review-only `NandPlan`
planning, `VerifiedAssets` as the only planning input, an `HttpClient` seam, a
monotonic `Clock` plus `SessionConfig` TTL, and a shared ordered script harness for
failure injection. `NandPlan::authorize_execution` always fails and no executor
exists. Every Batch 1 guarantee, hash, lock, manifest rule and test remains in
place.

**No physical PocketCHIP access, FEL transfer, NAND erase/write, USB operation or
destructive action occurred during Batch 2.** The seams are host-side architecture
work; they are not hardware validation and they close none of the release blockers
below. Batch 3 must add a reviewed, non-destructive real transport behind
`FelTransport` and prove identification on authorized hardware before any write
path is considered.

## Scope: fresh installation, user-managed backups

Flashing erases all existing data on the PocketCHIP's internal NAND. Both `stock` and
`vitrallis-default` install a fresh OS. The app will not preserve, back up, migrate
or restore the previous installation or its documents, saves, applications and
settings. Users must back up anything they want to keep to another device before
flashing. Backup/restore features and in-place OS upgrades are not planned and are
not release blockers. “Stock” means the new image provides the PocketHome experience.

## 1. Implement real recovery and NAND installation

- [ ] Finalize the pre-erase sequence, including a verified, non-destructive RAM
  recovery boot for board/NAND identification where required. Update the
  [protocol specification](docs/recovery-protocol.md) to match the implementation.
- [ ] Implement the real FEL boot path and device-side recovery service. Use fixed,
  reviewed operations and addresses; never execute manifest-supplied commands.
- [ ] Identify the actual board and full NAND part/geometry before erase. Support
  Hynix H27UCG8T2ETR and Toshiba TC58TEG5DCLTA00 without defaulting unknown parts to
  either variant; preserve the proven SPL/ECC/bad-block and SLC handling.
- [ ] Implement an authenticated recovery connection bound to the selected FEL SID
  and a fresh session. Reject ambiguous devices, spoofed endpoints and changed identity.
- [ ] Require complete verified assets and an explicit device/image/profile-bound
  erase confirmation before any NAND mutation. Present the data-loss warning in the
  physical GUI and CLI flow; do not offer a preserve-data mode.
- [ ] Implement bootloader writes and bounded rootfs streaming/extraction on the
  device, validating paths, links and expanded size. Keep UBIFS creation on-device.
- [ ] Verify persisted bootloader/rootfs contents, then complete synchronization,
  clean unmount and UBI detach. Download hashes and bytes transferred are insufficient.
- [ ] Handle cancellation, disconnects, timeouts and failed writes without reporting
  success or automatically retrying erase/reboot. Require fresh preflight on retry.
- [ ] Connect the physical backend to GUI, CLI and the upgrade script only after the
  corresponding image, protocol and hardware gates pass.

## 2. Build and approve the Debian 13 images

Batch 0 (2026-09-25) established artifact provenance: every physical role has an
identified source in `upstream-lock.json`; `docs/image-provenance.md` maps the
relationships. Batch 1 (2026-10-01) selected the September rootfs, made the
per-part SPLs deterministic and implemented the real host-side builder
(`images/assemble.py`) with strict physical-manifest validation. The remaining
image work is:

- [x] Finish the isolated Linux image builder. `images/assemble.py` verifies the
  pinned assets, stream-scans the rootfs without extraction, rebuilds the
  pinned SPL tool in a digest-pinned container with no network, generates both
  SPL variants deterministically, pads U-Boot, audits storage/compatibility,
  repacks the rootfs deterministically and emits validated physical manifests.
- [x] Choose and record one rootfs route: the accepted pinned prebuilt
  `os-2026.09.23-010738` archive was adopted. The older
  `os-2026.07.29-024145` archive remains pinned as the documented fallback.
- [x] Lock the build container and snapshot: `images/inputs.lock.json` records
  the base-image manifest digest, the signed `snapshot.debian.org` timestamp,
  the pinned SPL tool files and the checked-in package inventory. No live CHIP
  apt repository is consumed; the selected rootfs's package state is bound by
  `images/package-inventory-6.12.107+deb13-chip.json`.
- [x] Generate `spl-hynix`/`spl-toshiba` deterministically from the locked
  `sunxi-spl.bin` and pinned tool, run each variant twice and hash-pin the
  results; see `docs/image-build.md`. No NAND write was added.
- [ ] Build twice in clean environments and compare normalized image hashes
  (Batch 1 recorded a bit-identical complete-set comparison; repeat in remote
  CI on a second host before release). Record provenance, package inventory and
  matching kernel, DTB, recovery and bootloader assets.
- [x] Validate immutable physical manifests semantically (roles, formats,
  exact hashes/sizes, distinctness, DTB/overlay, boot script, kernel/rootfs,
  SPL variant, layout, immutability, approval binding) with positive and
  negative tests. `approved_physical_manifest_sha256` stays empty: no manifest
  is approved and no production write path exists. The restricted Batch 3
  original-SPL diagnostic grants no release approval.
- [ ] Validate the fresh `stock` image's PocketHome menu/startup, compatible stock
  apps and hardware configuration. It must contain no Vitrallis startup hooks/binaries.
- [ ] Complete release selection and offline installation for approved physical
  manifests once the hardware and redistribution gates pass.

## 3. Integrate the optional Vitrallis-default profile

[Vitrallis Shell beta2.6](https://github.com/csd113/Vitrallis-Shell/releases/tag/v0.1.0-beta2.6)
publishes the complete bundles and matching helpers. Publication is no longer the
blocker; Flasher-specific verification, integration and hardware validation remain.
These items must not hold back an independently validated stock-only release.

- [ ] Review and pin a complete ARM bundle and its matching bootstrap, installer,
  session helper and uninstaller from one release; verify hashes and provenance.
- [ ] Verify armhf/glibc/SDL compatibility, Python/runtime requirements, Awesome,
  PocketHome metadata/assets and the systemd user-session contract in the built image.
- [ ] Implement opt-in startup on the newly installed OS, retain PocketHome fallback,
  and test Shell/Terminal/Notepad/Files, Home bindings, startup failure and removal.
  Configuration rollback within this new installation is not pre-flash data recovery.
- [ ] Update the image lock, manifests and application status messages to reflect
  validated integration; never change a readiness flag without supporting evidence.

## 4. Validate memory and NAND-write settings

- [ ] Apply the [candidate storage policy](docs/debian-optimizations.md) to a
  device and validate it. Batch 1 audits the actual rootfs state during every
  build (no fstab swap, no enabled swap/zram units, journald/tmp/kernel
  symbols recorded) and fails closed on violations, but the configuration
  drafts still have no effect on a device.
- [ ] Verify no NAND-backed swap through fstab, units, generators, hooks and runtime
  state. If zram is adopted, pin exactly one manager and verify RAM-only operation
  with no backing device or disk-swap fallback.
- [ ] Test no-swap versus small zram workloads, compression/CPU cost, memory pressure,
  OOM behavior, `/tmp` exhaustion, package updates and both desktop profiles.
- [ ] Validate bounded logging and temporary storage, their retention/space tradeoffs,
  and actual application writes while preserving UBI health, clocks and update support.
- [ ] Measure baseline/candidate NAND activity and erase-counter health on both NAND
  variants before claiming endurance or performance improvements.

## 5. Prove operation on hardware and supported hosts

- [ ] Complete the first full install/readback/cold-boot cycle on one explicitly
  authorized test device and host, booting the fresh stock PocketHome image.
- [ ] Repeat on both NAND variants and all advertised hosts: Windows x86-64,
  macOS arm64/x86-64, Linux x86-64, and Linux arm64 if included in the release.
- [ ] Validate FEL access and recovery USB re-enumeration separately. Verify Windows
  WinUSB/tool dependencies and a supported recovery gadget mode; establish tested
  macOS ECM/NCM or another reviewed transport. RNDIS alone is not sufficient.
- [ ] Test missing drivers/permissions, multiple devices, unplug/replug, slow transfers,
  tool failures, corrupt images, readback failure, bad blocks and authorized power-loss
  recovery. Record exact hashes, hardware revisions and host/driver versions.
- [ ] Validate LCD/backlight, keyboard, touch calibration, Home/Power, Wi-Fi, sound,
  battery reporting, storage limits and cold boot. Test the top-pad/in-case FEL method
  before advertising it as a supported physical procedure.

## 6. Finish distribution and user-facing release checks

- [ ] Resolve upstream recovery/image redistribution permissions and provide required
  licenses, notices and corresponding sources for any bundled GPL components.
- [ ] Supply or clearly install the reviewed sunxi-fel/libusb dependencies on each
  host. Never silently replace USB drivers or alter unrelated host networking.
- [ ] Confirm clean CI builds/artifacts on every advertised target and test extracted
  packages on clean machines; compilation alone is not hardware or installation proof.
- [ ] Complete Windows signing/driver-distribution checks and macOS Developer ID/
  notarization for the intended public distribution. Document remaining limitations.
- [ ] Verify GUI readability/accessibility, progress, cancellation, actionable errors,
  and consistent fresh-install/data-loss messaging throughout documentation and UI.
- [ ] Publish approved OS assets/manifests and tested application packages through a
  deliberate release process, with checksums and platform instructions. The current
  CI produces workflow artifacts only, not an approved physical release.

Detailed supporting requirements: [image build](docs/image-build.md),
[physical validation](docs/physical-validation.md), [USB/driver guidance](docs/fel-drivers.md),
[recovery behavior](docs/recovery.md) and [upstream licensing](docs/upstream-licenses.md).

## Batch 3 progress (2026-10-01)

- [x] Pin Rust 1.99.0 and pass the required Rust baseline without lint suppression.
- [x] Preserve initial live inventory, full boot data/OOB readbacks, corrected
  U-Boot, private boot/configuration backup and post-upgrade package inventory.
- [x] Cross-check MTD geometry and enumerate unavailable rootfs blocks with ioctls.
- [x] Implement native, bounded FEL framing and restricted SRAM diagnostics,
  with host regression tests and native GUI/CLI discovery.
- [x] Physically identify BROM/FEL and validate 256-byte SRAM diagnostics.
- [x] Establish recovery load addresses, authenticated endpoint and boot on macOS.
- [x] Prove authenticated RAM-only return to FEL and recheck the same SID.
- [ ] Validate NCM and Windows recovery gadget workflows.
- [ ] Resolve SPL/ECC/fallback and logical/physical bad-block semantics.
- [ ] Approve exact physical artifacts, enable the gated executor and demonstrate
  production flashing, readback, rootfs verification and successful boot.

[evidence](docs/evidence/batch3/README.md) separates observations, source-derived
claims and open gates. Batch 3 is not complete.
