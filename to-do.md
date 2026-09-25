# Release blockers

Physical flashing remains disabled. This checklist tracks the work needed for a
usable hardware release; passing the mock suite alone does not complete these items.

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
identified source or an explicit unresolved status in `upstream-lock.json`;
`docs/image-provenance.md` maps the relationships. The remaining image work is:

- [ ] Finish the isolated Linux image builder based on the reviewed x-chip-os source;
  the current builder only prints plans/checks.
- [ ] Choose and record one rootfs route: consume the accepted pinned prebuilt
  rootfs (`os-2026.07.29-024145`), adopt the newer pinned alternative
  (`os-2026.09.23-010738`), or build from a Debian snapshot plus rebuilt CHIP
  packages. Only then can `container_digest`, `debian_snapshot`,
  `chip_snapshot_sha256` and `package_lock_sha256` be filled honestly.
- [ ] When building, lock the build-container digest, signed Debian snapshot and
  all package versions/checksums. The CHIP signing key fingerprint
  (`6584A42C802AE168A2985797C2B5998BA4BEE115`) and the accepted package hashes
  are recorded; the live apt repo is rebuilt from scratch and deletes old packages.
- [ ] Build twice in clean environments and compare normalized image hashes. Record
  provenance, package inventory and matching kernel, DTB, recovery and bootloader assets.
- [ ] Decide how `spl-hynix`/`spl-toshiba` become manifest assets (deterministic
  generation vs published images); see `docs/image-provenance.md`.
- [ ] Validate the fresh `stock` image's PocketHome menu/startup, compatible stock
  apps and hardware configuration. It must contain no Vitrallis startup hooks/binaries.
- [ ] Define and validate immutable approved physical manifests and release selection,
  including profile compatibility, asset hashes, minimum flasher version and offline
  installation. Keep the physical catalog empty until approval is substantiated.

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

- [ ] Apply and audit the [candidate storage policy](docs/debian-optimizations.md)
  during the controlled image build; drafts currently have no effect on a device.
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
