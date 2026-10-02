# Validation record — 2026-09-12

Repository baseline: `80565d868ed51e62900c805e471931c8c8804f2f` (README only).
No commits were created. Host: macOS arm64, Rust 1.98.1. Reviewed upstream revisions
and artifact hashes are in `upstream-lock.json`.

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed after formatting the new workspace |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::all -D clippy::pedantic -D clippy::nursery -D clippy::cargo` | Passed; explicit documented transitive duplicate-name exceptions in clippy.toml |
| `cargo test --workspace --all-features` | 41 passed; 3 child fixture entries ignored in the parent harness and exercised by process tests |
| `python3 -m unittest discover -s images -p test_*.py` | 7 passed |
| `python3 -m unittest discover -s scripts -p test_*.py` | 8 passed; packaging integrity plus profile dispatch, physical refusal and unknown-profile rejection |
| `cargo build --workspace --all-features --release --locked` | Passed, macOS arm64 |
| `cargo +stable build --workspace --all-features --release --locked --target x86_64-apple-darwin` | Passed, macOS x86-64 using the same Rust 1.98.1 compiler |
| `python3 images/build.py plan` | Passed; prints blocked plan without privileged execution |
| `python3 images/build.py check` | Expected exit 2; missing immutable build inputs and compatibility evidence |
| HTTPS fetch via CLI | Passed against the pinned repository README and the 31 MB recovery asset, including GitHub redirects, through the actual TLS/verification/cache pipeline |
| Upstream candidate downloads | Recovery 31,628,120 bytes and rootfs 516,321,424 bytes matched recorded SHA-256 |
| Rootfs inspection | Read 50,936 archive members without host extraction; package inventory recorded |
| Linux arm64 disposable container: `python3 scripts/ci.py validate` | Passed: fmt, strict Clippy, 41 Rust tests, Python tests, release workspace build and manifest smoke |
| `python3 scripts/ci.py package` | Passed: unsigned macOS arm64, macOS x86-64 and Linux arm64 ZIPs; architecture checked and every packaged checksum reverified |
| `git diff --check` | Passed |

Coverage includes malformed/oversized manifests, unknown/duplicate fields, bad hashes,
truncated/oversized streams, cancelled imports, unsafe symlink/shared caches, stale
cache hits, snapshot pathname replacement, missing offline data, wrong/multiple FEL
candidates, unknown NAND, expired/wrong confirmation, changed device/NAND, cancellation
before erase, failure at every simulated write/verify boundary and successful fresh
retry. Process tests launch only this project's test executable and cover failure,
oversized output, timeout and cancellation. Transport tests cover reserved network
rejection and preservation of shorter deadlines.

Not performed: physical discovery/boot/erase/write/readback/reboot, driver changes,
ARM OS execution, actual Debian live-build, image reproducibility comparison,
Vitrallis installation, GitHub Actions execution, Windows native tests, Linux GUI runtime tests,
code signing, notarization or publishing. Cross-platform CI configuration is not
remote CI evidence. The real backend remains unconditionally blocked for writes.

Native macOS GUI smoke: launched the packaged app, followed FEL instructions through
simulated detection/preflight, observed disabled confirmation until the exact phrase,
then completed simulated install/readback with an explicit no-hardware completion
message. The egui window currently exposes limited native accessibility-tree content;
VoiceOver parity is not validated. No real-device mode was activated during this test.

Linux validation used an unprivileged disposable Docker container on the macOS host,
with the source checkout mounted read-only and build products under `work/linux-target`.
The validation image was based on Rust 1.98.1/trixie; its local image manifest was
`sha256:7ef21dfeefa87868c9e08214dc4f11c61c9cab7bc83b61fdb63aa22e21d369ef`.
This is host-flasher build/test evidence, not an armhf PocketCHIP OS build or hardware
validation. CLI simulation was separately exercised with exact and incorrect typed
confirmation; success and rejection both behaved as expected.

Full changed-file inventory: [files-changed.txt](files-changed.txt).

Desktop-profile follow-up: both host-script simulations completed through exact typed
confirmation, and both physical script requests exited 2 before device access. Both
physical CLI requests failed as intended. Stock confirmation was explicitly tested
against the Vitrallis-default manifest and could not authorize erase. Missing,
unknown or inconsistent manifest profiles and false Shell readiness are rejected.
Stock image plans omit the Shell bundle requirement and Shell-only blockers.

The updated macOS native GUI opened with PocketHome selected by default. The optional
Vitrallis-default choice persisted through preflight, confirmation and simulated
completion. No startup setting was changed. A fresh preview app bundle was used after
macOS rejected an overwritten development preview executable. This is not Developer
ID signing or notarization evidence.

Current profile changes were rebuilt for macOS arm64/x86-64 and Linux arm64. The
Linux container reran the full validation sequence with the new tests. All platform
ZIPs include the upgrade script, profile-aware planner/input lock, both simulation
manifests and updated documentation. The separately documented upstream Shell
one-liner was inspected but never executed; its complete release bundle remains
unavailable in the reviewed release inventory.

FEL illustration follow-up: added the supplied bare-CHIP jumper photograph and an
original PocketCHIP top-pad schematic, visually checked after SVG rendering. Labels
were checked against the manufacturer GPIO reference and an actual close-up photo.
Documentation distinguishes the schematic from physical in-case recovery validation.
Formatting, strict Clippy and the full 41-test Rust suite passed again. Application
code and dependencies did not change. Refreshed ZIPs include both offline image assets.

Debian storage review: reverified the rootfs SHA-256 and inspected fstab, kernel zram/
UBIFS settings, relevant packages and enabled-unit links without extraction or device
execution. Added shared candidate NAND-write policy and three unapplied configuration
drafts. Formatting, strict Clippy, all 41 Rust tests and 15 Python tests passed. Tests
cover identical policies for both desktops, no zram backing/fallback, and agreement
between the candidate settings and reported hashes. The packaged host planner was
checked from an extracted temporary package including its new storage module/configs.
No active swap, mounts, journal configuration, package set or device image was changed.

## Batch 0 provenance closure — 2026-09-25

Host: macOS arm64, Rust 1.98.1, Docker 29.8.0 (`--platform linux/amd64`). No device
was connected, discovered, booted, read from or written to. No NAND operation was
performed. `python3 scripts/ci.py validate` exited 0 after adding the provenance
model: `cargo fmt --check`, strict Clippy, 43 Rust tests (including new per-role size
limit and lock/catalog consistency tests), 9 image tests, 25 script tests, the new
`scripts/provenance.py check` (9 repositories, 8 assets, 8 release roles), both
`build.py plan` profiles, `build.py check` expected exit 2, the workspace release
build, both simulation manifests and `git diff --check`.

Upstream verification was performed during evidence collection: all U-Boot release
assets for three tags were downloaded and hashed; the two rootfs candidates, the
recovery initramfs and the live kernel `.deb` were downloaded and hashed; the CHIP
apt `InRelease` was verified with GPG against the repository key; and the accepted
kernel configuration was extracted from the pinned rootfs and asserted offline by
`scripts/provenance.py check-kernel-config`. The U-Boot binaries were rebuilt in
Docker from the tagged repositories: `sunxi-spl.bin` and `u-boot-dtb.bin` are
bit-identical to all three releases with `SOURCE_DATE_EPOCH` pinned, and
`u-boot-sunxi-with-spl.bin` differs only in the 5-byte legacy uImage time/header-CRC
pair. `python3 scripts/provenance.py verify-upstream` is the explicit network
re-verification command and is deliberately not part of routine CI.

An independent adversarial verification pass re-cloned every repository, re-downloaded
both rootfs artifacts and the other assets, rebuilt the U-Boot release in a fresh
container, ran a control experiment that removes `CONFIG_MTD_RAW_NAND=y` (UBI
disappears, confirming the Kconfig `imply` explanation), recomputed all 72
`reviewed_files` hashes and the six follow-up review hashes with no mismatches, and
re-extracted the kernel config, DTB, boot script and overlay. It confirmed the tag
mappings, package identities, license findings and boot layout, and refuted three
prose claims that were then corrected: the accepted rootfs was assembled on
2026-07-29 (not 07-24), the last package-repo state before that build was CI run 30
(`40a8723`, not `f18ad24`), and the earlier "LCD likely stays dark" inference was
wrong because `sun4i_tcon_probe()` itself defers until `panel-simple` loads. The
kernel source commit `d2fa89a` is recorded as strong inference rather than
cryptographic proof. The full verification record is in the Batch 0 report; the
repository's own checker does not depend on any of these external fetches.

Not performed in this batch: physical discovery/boot/erase/write/readback/reboot,
FEL payload upload, NAND or MTD operations, device kernel execution, full kernel
rebuild, rootfs rebuild, package installation, or any change to a connected device.
Remaining hardware-only questions are recorded in `docs/boot-layout.md` and
`docs/image-provenance.md`.

## Batch 1 image-builder validation — 2026-10-01

Host: macOS arm64, Python 3.13.5, Rust 1.98.1, Docker 29.8.1
(`--platform linux/amd64`). No device was connected or opened; no NAND, MTD,
FEL, USB or destructive operation was performed, and no NAND-writing path was
added.

Rootfs selected: `os-2026.09.23-010738` /
`pocketchip-rootfs-2026-09-23.tar.gz` (`1e516cad…`, 516,563,033 bytes), kernel
`6.12.107+deb13-chip` `6.12.107-1.31`. The July `os-2026.07.29-024145` archive
and its kernel config remain pinned and documented as the fallback. The selected
archive was re-inspected without extraction: 50,950 members, 8 char devices, 6
hardlinks, no xattrs or sparse members. Its DTB (`0132f7fa…`) and PocketCHIP
overlay (`3230ea7f…`) are byte-identical to the July candidate; the kernel
(`d3c044d8…`), config (`436a91ee…`) and boot script (`61901b03…`) differ. All
709 installed packages were recorded with per-package version, architecture,
status and `.list`/`.md5sums` hashes in
`images/package-inventory-6.12.107+deb13-chip.json`.

The pinned builder image was built from `debian:bookworm-slim` digest
`sha256:3783cc…4251` and the signed snapshot `20260930T000000Z` (InRelease
SHA-256 `77737fa4…e1aa`), installing `gcc 12.2.0-14+deb12u1`,
`libc6-dev 2.36-9+deb12u14` and `ca-certificates 20230311+deb12u1`. The SPL tool
container ran with `--network none` and only the pinned source tree mounted
read-only.

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed |
| strict Clippy (`-D warnings -D clippy::all -D clippy::pedantic -D clippy::nursery -D clippy::cargo`) | Passed |
| `cargo test --workspace --all-features --locked` | 43 passed, 3 ignored (unchanged Rust core) |
| image tests (`images/test_*.py`) | 60 passed |
| script tests (`scripts/test_*.py`) | 32 passed |
| `scripts/provenance.py check` | Passed: 9 repositories, 8 assets, 8 release roles |
| `scripts/provenance.py check-kernel-config` | Passed for the 6.12.107 config evidence |
| `images/build.py check` (stock) | exit 0, all route-1 inputs pinned |
| `images/build.py check --profile vitrallis-default` | expected exit 2, bundle still missing |
| `images/assemble.py fetch-assets` | 5 locked assets verified (rootfs, SPL, U-Boot ×2, initrd) |
| `images/assemble.py build` | complete Hynix + Toshiba sets emitted and self-validated |
| `images/assemble.py verify` | both physical manifests and `SHA256SUMS` valid |
| `images/assemble.py reproduce` | bit-identical, see below |
| `scripts/ci.py validate` | exit 0 (includes the full sequence above) |
| `cargo build --workspace --all-features --release --locked` | Passed |
| `scripts/ci.py package` | Passed: unsigned macOS arm64 ZIP with the new image-builder files |
| `git diff --check` | Passed |

### SPL reproducibility

Each variant ran twice in the pinned container with its deterministic entropy
stream bound over `/dev/urandom`; both runs and a third run under the
independent `gcc:12-bookworm` toolchain produced identical bytes. A control run
with real kernel entropy differs only outside the ECC-protected regions.

| Variant | OOB | Size | SHA-256 |
| --- | --- | ---: | --- |
| `spl-hynix` | 1664 | 4,620,288 | `0099342e6331e9880d704bf11eb75f1b7618be8cef05b2ae456918fa1010fc7a` |
| `spl-toshiba` | 1280 | 4,521,984 | `487edb2eda190bd98b85deacaf858fcc4583351c3210be4ebc19ee983d19dd45` |

Control comparison over all 256 pages: protected data/ECC regions differing =
0; unused tail/BBM regions differing = 512 of 512 (expected; only the replaced
entropy input differs). `images/spl.py` descrambles and verifies every page's
protected region against the exact source chunk.

### Complete artifact set and full-image reproducibility

`images/assemble.py reproduce` built the complete set twice from clean state
and compared every produced file:

```text
comparison: {"identical": true, "files": 20, "different": [], "only_first": [], "only_second": []}
manifest-hynix.json   sha256 0a9cb8281a31ab8c193ede75eb2748193e54aff4a7bc93b7f88f1f1ff555b93d
manifest-toshiba.json sha256 0585d1d3d158cb0e08c779ac22bea7aceeb0bed6088cdb3831f8aee90cabb9f8
rootfs-repacked.tar.gz sha256 2b71673de39192088f6a03b238877b3bee3b3a19de775d90db97c7bcf21d34e0
```

Both builds produced the same 20 files with no differences and no files present
in only one build; `images/assemble.py verify` revalidated both physical
manifests and `SHA256SUMS` for both. The pinned SPL tool binary hashed
`3971c30cb442800f559ce97b373f0d207e8cf65d7ca6a75d81dcfbf7e23c3156` and again
produced both variant hashes byte-identically.

The deterministic rootfs repack preserved all 50,950 members and 1,264,012,666
content bytes; source and output canonical member manifests matched exactly.

Not performed in this batch: any device or NAND interaction, FEL upload, UBI
format/mount, LCD/backlight/touch/keyboard behaviour, DIP/product-version
behaviour, power-loss behaviour and USB transport runtime behaviour. Those
remain in [physical validation](physical-validation.md) and are explicitly not
claimed here.

## Batch 2 host seams validation — 2026-10-01

Host: macOS arm64, Rust 1.98.1, Python 3.13.5. **No device was connected or
opened; no USB access, FEL transfer, RAM upload, NAND/MTD erase or write, UBI/UBIFS
operation, or destructive action was performed.** No physical manifest was approved,
`approved_physical_manifest_sha256` remains empty, and `NandPlan::authorize_execution`
always fails because no executor exists. The selected rootfs and every Batch 1 lock,
hash, manifest and reproducibility artefact are unchanged.

Batch 2 added the host seams documented in [architecture](architecture.md):
`ToolRunner` (external tools), `FelTransport` (scripted only; production
`UnavailableFel`), `NandPlan` (review-only planning from `VerifiedAssets`),
`HttpClient`, monotonic `Clock` with explicit `SessionConfig` TTL, and the shared
ordered script harness with terminal failure injection. Session orchestration now
acquires and rechecks assets through `VerifiedAssets`, builds the review plan at
preflight, and reports one terminal `Outcome`.

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed |
| strict Clippy (`-D warnings -D clippy::all -D clippy::pedantic -D clippy::nursery -D clippy::cargo`) | Passed |
| `cargo test --workspace --all-features --locked` | 106 passed, 6 ignored child fixtures |
| image tests (`images/test_*.py`) | 60 passed |
| script tests (`scripts/test_*.py`) | 32 passed |
| `scripts/provenance.py check` | Passed: 9 repositories, 8 assets, 8 release roles |
| `scripts/provenance.py check-kernel-config` | Passed for the 6.12.107 config evidence |
| `images/build.py plan` (both profiles) | Passed; physical plan still blocked |
| `images/build.py check` (stock) / vitrallis-default | exit 0 / expected exit 2 |
| `cargo build --workspace --all-features --release --locked` | Passed |
| CLI manifest validation | Both simulation manifests valid; not approval to flash |
| `git diff --check` | Passed |
| `python3 scripts/ci.py validate` | exit 0 (includes the full sequence above) |

### New deterministic coverage

* `ToolRunner`: scripted argv/environment/stdin matching, stdout/stderr capture,
  exit status, non-zero exit, spawn failure, timeout, cancellation, oversized and
  malformed output. `SunxiTool` now only ever calls the injected runner, including
  the read-only `--list` path.
* `FelTransport`: every operation (discover, identify, device info, RAM upload,
  execute, read memory, read status) has a scripted success and an injected
  failure; a failure or cancellation makes later calls fail instead of returning
  success. The production transport refuses every call with `FelUnavailable`.
* `VerifiedAssets`: complete inventories pass; incomplete inventories, altered
  hash/size specs, swapped roles and cancellation are rejected before planning.
* `NandPlan`: exact ordered steps, SPL-variant selection per NAND fixture,
  readback verification identity, wrong-variant/missing-digest/overlap/boot-region
  rejection and the always-failing execution gate.
* Device decision fixtures: exact `pocketchip` + A13/R8 advance; wrong SoC, wrong
  board, ambiguous/empty identity and unknown NAND reject; identification alone
  never uploads or executes.
* HTTP seam: success, HTTP error, truncated body, wrong `Content-Length`, hash
  mismatch, timeout, cancellation, bounded redirects and redirect-limit failure;
  offline acquisition never calls the client and missing offline files fail closed.
* Clock/session: not-yet-expired, exact TTL boundary, expired confirmation,
  deterministic advancement, monotonic stages, no byte progress regression, no
  event after Complete and no success stage after failure or cancellation.
* Failure/cancellation boundaries: acquisition, snapshot recheck, identification,
  plan validation, every scripted transport call, external tool invocation,
  recovery boot, erase, bootloader write, rootfs streaming, verification and
  session expiry. Terminal `Outcome` is `Success`, `Failure` or `Cancelled`, the
  prepared plan is dropped on any failure, and retries require a fresh preflight.

Not performed in this batch: physical FEL discovery, USB/driver behaviour, RAM
upload, recovery boot, NAND erase/write/readback, UBI format/mount, bad-block or
ECC behaviour, authentication of a recovery endpoint, and any hardware validation.
The scripted PocketCHIP/R8 + NAND fixtures prove host decision logic only. Those
items remain in [physical validation](physical-validation.md) and are explicitly
not claimed here.

## Batch 3 — Rust 1.99.0 baseline (2026-10-01)

Host: macOS arm64, rustc 1.99.0 (b940084d7 2026-09-28). The
toolchain pin, workspace `rust-version`, CI setup, README and contributor
guide now require 1.99.0. Package manifests inherit the workspace requirement.
CI calls the pinned setup script; the image-builder Dockerfile contains no
Rust compiler. Cargo.lock was preserved; no dependency update was needed.
Historical validation entries above retain the compiler actually used.

The initial strict Clippy run found three new `assert_is_empty` violations
in HTTP isolation tests. They now compare against a typed empty array so
failures display the unexpected requests; no lint was suppressed.

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed |
| `cargo check --workspace --all-targets --all-features --locked` | Passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings -D clippy::all -D clippy::pedantic -D clippy::nursery -D clippy::cargo` | Passed after the assertion fixes |
| `cargo test --workspace --all-features --locked` | Passed: 101 core tests and 5 integration tests; six child-process fixtures intentionally ignored and exercised by parent tests |
| `cargo build --workspace --all-features --release --locked` | Passed |
| `git diff --check` | Passed |

This baseline validates host code only. Hardware work and the remaining
Batch 3 completion criteria are still in progress. Work on `main` was
explicitly requested for this batch; no push is authorized.

### Batch 3 native FEL and pre-flash evidence milestone

Live evidence is indexed in [the evidence directory](evidence/batch3/README.md).
The application identified the actual R8 BROM and SID, matching pinned
sunxi-tools and Linux nvmem. It physically uploaded/read back 256 scratch SRAM
bytes, executed only an ARM return instruction, and verified restoration.
This proves bounded SRAM operations; no recovery image or NAND executor was
exercised. Actual NAND geometry, ioctl enumeration, boot data/OOB and private
configuration backups are preserved with exact hashes.

Two physical findings corrected the initial native transport: successful BROM
status starts with the FFFF marker, and SID MMIO requires aligned word reads.
Nine FEL tests include the measured reply fixture and fixed word-reader sequence.
Formatting and all-target/all-feature workspace check passed. The final milestone
`python3 scripts/ci.py validate` run exited 0, including strict Clippy, workspace
tests, image/script tests, provenance, stock/Vitrallis planning checks and release
build. The ignored local log is `work/batch3/fel-milestone-validation.log`.
No final Batch 3 completion or destructive-flash validation is claimed.


### Batch 3 read-only recovery work in progress

The ARMv7 release daemon builds in the pinned Rust 1.99.0 container. Eight core
protocol tests cover authentication/framing and controlled restart acceptance;
bootstrap tests cover bounded archive staging and cancellation. Four daemon tests
cover the measured memory node, NAND mount rejection and fixed module invocations.
Four Python tests cover bounded CPIO parsing. Current strict Clippy passes.

Three physical RAM boots prove the reviewed addresses and macOS ECM/ACM path.
Authenticated Ping and Inventory succeed. Actual wrong-key/SID/session/implementation
and previous-boot rejection tests pass. The first boot's incorrect kmod invocation
and second boot's memory-node mismatch are regression-tested. The third accepted
RAM restart but failed FEL rediscovery. The fourth boot loaded the reset module and returned to FEL successfully; native
rediscovery confirmed the same SID. No NAND operation was performed.
See the evidence index for exact hashes and measured failures. The corrected recovery milestone passed `cargo fmt --all --check`,
`cargo check --workspace --all-targets --all-features --locked`, strict workspace
Clippy, `cargo test --workspace --all-features --locked`, workspace release build,
all image/script tests, provenance/kernel configuration and stock/Vitrallis checks,
manifest checks and `git diff --check`. The complete established suite
`python3 scripts/ci.py validate` exited 0; its ignored local log is
`work/batch3/recovery-milestone-6-validation.log`.

### Batch 3 authenticated boot readbacks

Six device tests pass, including raw data/OOB separation, exact-length rejection
and fixed physical-offset-preserving arguments. The new protocol test rejects a
reply for a different boot partition. Strict Clippy and all-target/all-feature
workspace check pass. `python3 scripts/ci.py validate` exited 0 for the readback
implementation; the ignored log is `work/batch3/readback-milestone-7-validation.log`.
The RAM template reproduced with the same SHA256 in two independent builder runs.
The fifth physical boot read all four regions through the new CLI/daemon path;
its first connection timed out, then authenticated Ping and an explicit diagnostic
retry succeeded. No automatic retry or write occurred. Raw programmed hashes differ
from the original uncorrected snapshots. Final ECC-aware verification and normal
boot/fallback characterization remain open.

### Batch 3 kernel-corrected U-Boot readback

Seven daemon tests pass, including rejection of normal kernel ECC interpretation
for the special SPL layout. The protocol test also rejects a reply carrying the
wrong interpretation. Protocol v2 makes that wire change explicit. The sixth real
RAM boot matched the original corrected U-Boot digest, with 6,136 corrected bits
and zero uncorrectable failures. `python3 scripts/ci.py validate` exited 0; log:
`work/batch3/readback-milestone-8-validation.log`. All-target/all-feature workspace
check, formatting and diff checks also passed. This is readback of the original
installation, not destructive-flash or BROM fallback validation.

### Batch 3 saved SPL BCH-64 verification and original normal boot

Three native decoder tests pass: pinned encoder known answer, corrections through
64 bits across data/parity, metadata correction, invalid lengths/copy indices,
erased data, a rejected 65-bit pattern and cancellation. The same three tests
pass for ARMv7 under qemu (0.69 seconds); this is emulation, not live recovery.
The fixture was independently checked against the exact padding page of the
locked Batch 1 Hynix artifact. No executable firmware is tracked in the fixture.

`python3 scripts/ci.py validate` exited 0, including formatting, strict Clippy,
workspace tests, Python image/script tests, provenance/kernel configuration,
stock/Vitrallis plan checks, release build, manifest checks and diff validation.
Log: `work/batch3/boot0-milestone-validation.log`. The separate
`cargo check --workspace --all-targets --all-features --locked` also passed.

All eight original SPL copies from hash-verified raw captures decode to the same
16 KiB digest and valid eGON checksum. This is software correction of measured
captures, not BROM fallback proof. After removing the FEL bridge, the user observed
the Vitrallis shell and SSH inventory confirmed the unchanged root UUID and
healthy original boot. No NAND erase/write or new installation is claimed.

### Batch 3 live SPL decoder preparation and restoration candidate

Protocol v3 and the ARMv7 recovery daemon are built. Four decoder/export tests
pass natively and under ARMv7 qemu; export rejects unverified/cancelled data and
preserves existing files. Eight daemon tests pass, including raw input for the
boot0 decoder and rejection on non-SPL regions. The host rejects a corrected SPL
reply with missing copies. No live v3 recovery measurement is claimed yet.

The CLI recovered the original source from all eight preserved copies against
the recorded digest. Two pinned-source encoder runs generated identical clean
NAND bytes. Structural checks and independent native decoding of all four copies
pass with zero corrections. The source and encoded images remain private;
`original-spl-restoration-candidate.json` records metadata only. The protocol v3
RAM template also reproduced byte-for-byte in two builder runs.

`python3 scripts/ci.py validate` exited 0; log:
`work/batch3/boot0-live-milestone-validation.log`. The separate all-target,
all-feature workspace check also passed. Physical live decoding, restoration,
fallback and final application reflash remain outstanding.

### Batch 3 original-SPL fallback guard preparation

Five guard tests pass on the host and on ARMv7 under qemu (2.76 seconds).
They combine historical raw/BCH evidence explicitly as host fixtures, not live
protocol-v3 claims. Coverage includes wrong SID, changed geometry/NAND identity,
new bad blocks, corrupted backup chain, mounted NAND/attached UBI, process failure,
pre-execution cancellation, exact erased-data/OOB verification and corrupted
restoration input. Destructive tool calls are scripted; no test invokes a real
erase or write utility.

`boot0-check-restoration` passes on the actual private candidate, validating the
fixed encoded digest and zero-correction native decode before snapshot creation.
`python3 scripts/ci.py validate` exited 0; log:
`work/batch3/boot-trial-milestone-validation.log`. The separate all-target,
all-feature workspace check also passed. Device-local integration, journaling and
physical fallback testing remain open; no diagnostic erase/write RPC is exposed
and production plan authorization remains denied.
