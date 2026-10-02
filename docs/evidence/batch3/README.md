# Batch 3 evidence in progress

Captured on the authorized development PocketCHIP on 2026-10-01 using
macOS USB networking and SSH with existing host-key verification. Credentials
are absent from these files. SSH establishes current OS access, not FEL proof.

- `starting-inventory.json`: exact kernel log, live DT source, SID bytes, MTD/UBI
  sysfs, mounted filesystems, network state and kernel configs. Each command
  records status/error/truncation. The boot-file hash command timed out during
  the user's concurrent apt upgrade; it is repeated in the post-upgrade inventory.
- `mtd-ioctl-inventory.json`: MEMGETINFO geometry and MEMGETBADBLOCK enumeration
  on read-only descriptors, plus the post-upgrade package and boot-file inventory.
  Ioctls and sysfs share the kernel driver; this cross-check does not constitute
  an independent raw NAND ID measurement. All 65 unavailable rootfs blocks have
  partition-relative offsets. Their physical translation still needs source
  review and recovery validation.
- `boot-readbacks.json`: full raw data and OOB digests for the four boot partitions.
  Binary readbacks are in ignored `work/batch3/pre-flash/`; preserve them with the
  checkout. No erase/write operation was performed. The failed initial mode
  selection and its correction are explicitly recorded.
- `uboot-corrected-readback.json`: kernel-corrected/randomizer-decoded U-Boot
  readback. No new uncorrectable errors; 5459 bits corrected across the 4 MiB
  region. Its bytes differ from the pinned U-Boot. The readback banner is
  `U-Boot 2022.01-dirty (Jun 12 2026 - 15:49:44 +0000)`. This does not decode
  BROM SPL ECC, which differs from the ordinary kernel NAND policy.
- `configuration-backup.json`: digest of private `/boot`, `/etc` and dpkg-status
  archive. It may contain credentials and is mode 0600 in an ignored mode 0700
  directory. Only its metadata is tracked; never commit the archive.
- `pre-fel-readiness.json`: empty dpkg audit, no apt/dpkg processes and SID recheck.
- `pre-fel-shutdown.json`: successful orderly shutdown command before requesting
  the physical FEL bridge. This records command acceptance, not observation of
  a physical power-off.
- `observations.json`: compact index of measured values and unresolved gates.

The four boot areas report no bad blocks. The rootfs exposes SLC emulation:
16 KiB pages, 2 MiB eraseblocks and 2,064,384-byte UBI LEBs. The physical NAND
reports 8 GiB and 4 MiB eraseblocks. Treating it as one flat byte-addressed
4 GiB device would lose the physical/logical distinction.

At `0x00C00000`, every raw byte and OOB byte is FF. Linux's `env` label does
not establish an environment there. Pinned U-Boot source expects a redundant
U-Boot at that offset and disables saved environments. Its fallback remains
unproven; the NAND plan and destructive authorization have not been changed.

Native FEL/BROM identification and bounded SRAM execution are now physically
validated. `fel-native-validation.json` records the application's exact BROM
packet and SID, the four-byte initial probe and successful rediscovery.
`fel-sram-256-validation.json` extends the probe to all 256 allowed scratch bytes.
`fel-external-discovery.json` independently agrees with the Linux and native SID.
`fel-status-measurement.json` preserves the exact successful eight-byte status
from a diagnostic-only instrumented copy of pinned sunxi-tools. Its marker is
FFFF, not zero. `fel-sid-byte-read.json` shows why BROM byte-copy reads cannot
identify the SID: they return only each register's low byte. The native transport
uses a fixed aligned-word reader, verifies its upload, and restores scratch.
Both findings are regression-tested using `fixtures/fel/r8-batch3.json`.

SRAM 0x1000 was selected from pinned sunxi-tools `soc_info.c` for SoC 0x1625.
The source identifies the FEL stacks at 0x1c00-0x1fff and 0x5c00-0x6fff, and
other BROM state at 0x7c00-0x7fff; the tested 0x1000-0x10ff range avoids them.
This is not a recovery DRAM load address. At the FEL milestone, recovery DRAM initialization, full payload boot and
authentication were still unresolved; later measurements below establish them.

At the FEL milestone, no recovery payload had booted and no destructive flash has occurred yet.


`recovery-boot-1.json` and `recovery-usb-1.json` record the first real RAM-only
recovery boot. The fixed reviewed script loaded the pinned kernel, DTB and
PocketCHIP overlay, then the read-only initramfs. macOS enumerated the recovery
gadget at 480 Mbit/s, assigned DHCP address 192.168.81.10 and received two ICMP
replies from 192.168.81.1. The authenticated TCP endpoint was not reachable;
no authenticated inventory, NAND execution or final OS boot is claimed.
Private session images/configuration remain outside tracked evidence.

`recovery-authentication-2.json` records a physical authenticated Ping and rejection
of wrong key, SID, session, daemon identity and previous-boot credentials, followed
by a successful fresh authenticated reconnect. Inventory initially failed because
the actual DT memory node is named `memory`; a deterministic test protects the fix.

`recovery-boot-3-tool.json` and `recovery-inventory-3.json` record the third RAM boot
and authenticated board, memory, kernel, MTD/ECC, boot-log and nanddump capability
inventory. All five partitions report writable capability, but no write operation
is exposed or authorized by this diagnostic protocol.

`recovery-return-to-fel-3.json` records an accepted restart followed by failed FEL
rediscovery and TCP timeout. It preserves the failure rather than claiming re-entry.
The kernel's reset driver is modular and absent in that template; a corrected image
includes it. The subsequent fourth boot physically validated the correction.

`recovery-boot-4-tool.json`, `recovery-inventory-4.json` and
`recovery-return-to-fel-4.json` establish the corrected RAM recovery boot, loaded
sunxi-wdt driver, authenticated inventory and successful authenticated return to
BROM FEL on the same SID. The physical bridge remained connected. This proves a
reload route without another manual power cycle; it does not characterize normal
NAND boot or approve destructive execution.

`recovery-boot-5-tool.json` and `recovery-boot-readbacks-5.json` record authenticated
raw data/OOB readback of all four boot blocks. The fourth remains completely FF
and matches the original raw/OOB hashes. Programmed regions differ between raw
reads, showing why ECC-aware interpretation is required. Zero error-counter
increments on a no-ECC read do not establish corrected-read health.

`spl-original-raw-analysis.json` derives four boot0 groups per original block,
64-page spacing, eGON header and 16 KiB declared size by applying the reviewed
scrambler to the private backups. Eight raw decoded copies have 13–57 bit
differences from a majority reconstruction whose stored checksum matches.
This reconstruction is host analysis, not BCH correction or a BROM fallback test.

`recovery-boot-6-tool.json` and `recovery-uboot-corrected-readback-6.json` record
protocol v2 and a kernel-corrected U-Boot read. The 4 MiB data hash matches the
original corrected backup exactly (`c76993ede3ceab2ba56e37b027c43896f4e4a79058cf4197aa7d1a7118b10224`),
with 6,136 corrected bits and zero uncorrectable errors. The per-page correction
totals do not establish the maximum errors in a single ECC codeword. No new
bootloader was written, and normal NAND boot/fallback remains to be tested.

`uboot-backup-source-policy.json` preserves exact source/configuration hashes and
separates proposed redundant-U-Boot behavior from actual NAND contents. The source
tries `0xc00000` after a primary-load error and disables saved environments, but
that does not prove the original installed SPL used those compiled options or
that fallback has physically succeeded. The plan gate descriptions now acknowledge
measured diagnostic recovery while retaining release/executor approval barriers.

`original-nand-normal-boot.json` records SSH inventory after the user removed the
FEL bridge and rebooted to the Vitrallis shell. The original UBIFS UUID matches
the baseline; no MMC storage boot alternative was present. This proves the
original installation still boots, not a new reflash or a particular SPL fallback.

`spl-original-bch64-readback.json` records native software BCH-64 correction of
the hash-verified original raw captures. All eight copies produce the same
16 KiB SHA256 and valid eGON checksum. Per-codeword corrections peak at nine
bits. The padding-only known-answer fixture in `fixtures/nand/` comes from the
already-pinned image builder and contains no executable SPL program. Live
authenticated decoding, regenerated-image BROM acceptance and physical fallback
tests remain outstanding. A decoder result must still match a trusted artifact
digest; BCH bounded-distance correction alone cannot authenticate intended data.

`original-spl-restoration-candidate.json` records a clean encoding of the verified
original 16 KiB SPL. Two unmodified pinned-encoder runs match byte-for-byte;
page/BBM validation passes, and native decoding of all four generated copies
matches the original digest with zero corrections. The program and generated
NAND bytes stay private in ignored `work/batch3/`; only evidence metadata is
tracked. This is a restoration candidate, not physical BROM acceptance, an
approved release artifact or evidence of a NAND write.

`pre-fallback-tool-capabilities.json` rechecks the exact SID over SSH and captures
the installed OS's `nandwrite`/`flash_erase` help. It confirms the raw OOB and
no-bad-block-skipping options needed for a fixed boot-region restoration path.
Recovery execution of these tools and destructive fallback testing remain unproven.

`pre-recovery-v3-shutdown.json` records a clear apt/dpkg process check and accepted
orderly shutdown before requesting the FEL bridge for the v3 live decoder test.
This records command acceptance, not observation of physical power-off or FEL.

`recovery-boot-7-tool.json`, `recovery-inventory-7.json`,
`recovery-spl-bch64-readbacks-7.json` and
`recovery-uboot-corrected-readback-7.json` record protocol v3 on the same SID.
Authenticated Ping, inventory and live native BCH-64 decoding all pass. Each
SPL block takes about 2.1 seconds; all eight copies match the original digest
and eGON checksum, with a maximum seven-bit correction in one codeword.
Corrected U-Boot matches the original digest, with 6,496 corrected bits and
zero uncorrectable errors. These actual v3 reports now back the trial guard
fixtures. No NAND erase/write, generated-image acceptance or BROM fallback
selection is claimed.

- `recovery-template-v4.json`: reproducible protocol-v4 payload metadata. Includes
  the exact private original-SPL restoration encoding for the restricted trial;
  no original executable bytes or session secrets are tracked. Physical v4
  preflight/mutation measurements are not claimed by this build record.

- `recovery-boot-8-tool.json`: exact pinned v4 RAM boot after authenticated return
  to FEL and independent same-SID discovery.
- `recovery-trial-preflight-8.json`: successful device-local preparation followed
  by disconnect, with the discarded connection token omitted.
- `recovery-trial-cancellation-8.json`: actual host SIGINT during preparation,
  terminal CancelledBeforeDispatch host journal and no Execute record.
- `recovery-auth-rejections-8.json`: actual wrong-key, wrong-SID and stale-session
  rejection, followed by valid-session Pong. No mutation was requested.
- `recovery-primary-after-cancellation-8.json`: all four primary copies still
  match the original after cancellation and authentication rejection.
- `recovery-primary-erase-trial-8.json`: application-only fixed primary erase,
  exact full raw data/OOB erasure and durable Verified journal (18.041 seconds).
- `recovery-boot-chain-after-primary-erase-8.json`: independent post-erase backup
  SPL, corrected U-Boot and inventory, with unchanged digests and zero ECC errors.
  Primary erasure and BROM fallback are physically proven; the later ninth session records clean restoration.

- `normal-backup-spl-boot-8.json`: new cold NAND boot after bridge removal,
  independent matching SID/kernel/root UUID, unchanged bad blocks, zero ECC failures
  and complete post-boot primary raw readback still FF. This proves backup SPL
  boot on the measured Hynix unit; it does not identify the selected copy within
  that backup block or validate a new installation.
- `post-fallback-orderly-shutdown.json`: no active apt/dpkg process and successful
  orderly original-OS shutdown before reentering FEL for clean primary restoration.

- `recovery-boot-9-tool.json`: a fresh SID-bound v4 RAM boot after confirmed
  backup boot and orderly shutdown.
- `recovery-primary-restoration-trial-9.json`: application-only clean original-SPL
  restoration, all four corrected program digests equal the original, 73 corrected
  bits total (maximum five per codeword), zero uncorrectable failures and durable
  Verified host journal (19.525 seconds). Raw encoding hash differs after write;
  corrected program verification is used. BROM primary selection is not claimed.
- `recovery-boot-chain-after-primary-restoration-9.json`: independent unchanged
  backup SPL, corrected U-Boot and inventory after restoration.
- `recovery-template-v5.json`: build metadata for closed primary/backup isolation
  operations. The tenth session records physical v5 dispatch; isolated restored-primary boot remains pending.

- `recovery-boot-10-tool.json`: fresh pinned v5 RAM boot after authenticated return
  to FEL and independent same-SID identification.
- `recovery-backup-trial-preflight-10.json`: successful device-local backup-erase
  preparation, followed by disconnect; the discarded ticket is omitted.
- `recovery-backup-erase-interrupted-host-10.json`: actual SIGINT one second after
  durable Dispatched, CLI exit 1, empty success output and terminal Indeterminate
  journal. No automatic retry was performed.
- `recovery-backup-after-host-interruption-10.json`: a new authenticated read-only
  connection proves every backup data/OOB byte is FF after the device finishes
  its committed erase. This does not change the interrupted host journal.
- `recovery-protected-chain-after-backup-erase-10.json`: independent original
  corrected primary SPL and U-Boot digests and unchanged inventory. The backup
  remains erased for a pending isolated cold boot of the clean restored primary.

- `rootfs-bad-block-offset-correlation.json`: source-derived eraseblock-start
  translation cross-checked against the original 65 ioctl-unavailable blocks and
  all 61 BBT-reported bad physical addresses in the tenth recovery boot log. The four
  remaining addresses are the final four physical blocks, consistent with the
  BBT count. Source hashes and exact input hashes are recorded. This does not
  establish page mapping, live recovery ioctl enumeration or write skip behavior.

- `rootfs-map-parser-preparation.json`: reviewed mtdinfo 2.3.0 source metadata and
  matching utility version/help strings extracted from the pinned v5 template.
  Records the existing read-only map operation and its suppressed unsupported
  ioctl behavior. The strict parser/capture foundation is scripted-tested; live
  v5 has no map request, and physical utility execution remains pending.

- `recovery-template-v6.json`: reproducible, byte-identical repeated payload for
  the authenticated read-only rootfs map diagnostic. Exact daemon/template hashes
  are pinned. Protocol v6 integration and response validation are tested; this
  build record claims no physical v6 boot or map execution.

- `original-vs-locked-release-spl.json`: exact private decoded-original versus
  locked-release comparison. All 12 differing bytes are in the eGON checksum
  or ASCII build banner; every other byte is identical. Metadata only; this
  does not establish physical acceptance of the exact locked release encoding.
- `original-vs-locked-release-uboot.json`: full corrected original and locked
  padded U-Boot digests differ at 376,738 bytes. Specific source/config causes
  remain unreviewed; exact new-release write/readback/boot acceptance is pending.

- `recovery-boot-11-preflight-rejection.json`: the first v6 boot attempt rejects
  the stale v5 byte-length expectation before upload. The correction binds named
  template bytes/hash and daemon hash to tracked metadata in a regression test.
- `recovery-boot-11-tool.json`: corrected pinned v6 RAM bootstrap after authenticated
  v5 return-to-FEL and independent same-SID native discovery.
- `recovery-rootfs-map-11.json`: actual authenticated v6 rootfs map, all 2,044
  logical blocks, 65 unavailable indices identical to the original ioctl
  enumeration and final BBT block unavailable. Geometry/counters remained stable;
  zero ECC failures. Read-only enumeration, not write/skip validation.
- `recovery-boot-chain-after-rootfs-map-11.json`: subsequent primary original-program,
  completely erased backup, corrected original U-Boot and recovery inventory
  checks. The isolated normal boot still requires bridge removal and power cycle.
- `locked-release-hynix-spl-native-review.json`: software decoding of the hash-verified
  locked generated Hynix image: all four programs match the locked input digest
  and checksum with zero corrections. No physical write or BROM boot is claimed.
  A guard test rejects treating this program as the original restoration image.

- `hynix-marker-probe-preparation.json`: Linux stable v6.12.107 source hashes,
  physical-last-page versus SLC pairing distinction, offline overlay ordering
  discrepancy and metadata for the reproducible append-only read-only marker
  alias DTB. Six deterministic tests protect transformation and file boundaries.
  No diagnostic DTB load, raw marker read or NAND write has occurred.

- `recovery-template-v6-inventory-guard.json`: reproducible v6 recovery payload
  that reports the optional sixth MTD alias instead of silently excluding it.
  Original-SPL write preflight still requires exactly five partitions. This
  updated payload has not yet been loaded; session 11 uses the preceding v6
  implementation. ARMv7 qemu and host tests cover enumeration and alias rejection.

- `normal-restored-primary-isolated-boot-10.json`: normal OS SSH inventory and
  boot log on the same SID, new boot ID `5f110c7b-5b90-40ce-b067-667d17295156`,
  after the requested bridge-removed power cycle. An independent full raw
  backup read remains entirely erased in data and OOB, proving acceptance of
  the clean restored primary. UBI has 1,979 good, 65 unavailable and zero
  corrupted PEBs; original root UUID and zero NAND ECC failures are preserved.
  No locked release boot is claimed. Package activity was absent before clean
  poweroff; backup restoration remains pending FEL re-entry.

- `recovery-template-v7.json`: reproducible pinned protocol v7 payload and
  closed physical last-page diagnostic through the exact read-only alias DTB.
  Host and ARMv7 qemu tests cover alias/status/page bounds, authentication and
  response mismatch. Subsequent session 13 records below measure the exact
  payload/alias load and read. This preserves historical v6 evidence and grants
  no production authorization.

- `recovery-boot-12-tool.json`,
  `recovery-backup-restoration-preflight-12.json`,
  `recovery-backup-restoration-trial-12.json` and
  `recovery-boot-chain-after-backup-restoration-12.json`: pinned previously
  measured v6 RAM reload after isolated primary boot, authenticated local
  preflight, actual fixed original backup restoration in 19.344 seconds and
  durable Verified journal. All four backup programs match the original digest
  after 61 total correctable bit errors (maximum three in one BCH step), with
  zero uncorrectable errors. Independent reads confirm both original SPL blocks
  and unchanged corrected U-Boot; all MTD ECC failure counts remain zero.
  Raw programmed bytes differ from the clean encoding, so verification uses
  corrected program identity. A host/ARM regression fixture preserves that
  distinction and rejects wrong target or altered program digest.

- `recovery-boot-13-marker-tool.json`, `recovery-physical-marker-13.json` and
  `recovery-marker-policy-and-protected-chain-13.json`: actual pinned v7 RAM
  boot with the reviewed alias DTB, authenticated six-partition inventory and
  repeated identical raw page 255/OOB observations. The alias is read-only,
  non-SLC and at the fixed physical offset; original indices are unchanged.
  All 1,664 OOB bytes are zero and data digest matches a zero page. A v6 hello
  and SPL preflight with the alias are rejected; fresh v7 requests remain healthy.
  Independent original primary/backup/U-Boot readback and rootfs map preserve
  digests, geometry and zero ECC failure counts. No application NAND mutation
  was dispatched. Factory/runtime bad-block origin remains unproved.

- `recovery-boot-14-standard-tool.json` and
  `recovery-standard-v7-policy-and-boot-chain-14.json`: authenticated v7 reload
  using the original standard DTB. Inventory has exactly five original
  partitions; marker requests are rejected without the alias and original-SPL
  preflight succeeds without Execute. Independent both-SPL/U-Boot checks retain
  original digests and zero ECC failure counts. No application NAND mutation
  was dispatched. Device is left in this standard RAM recovery session.

- `recovery-template-v9.json`: reproducible bounded v9 payload; physically
  authenticated in session 17. The boot-tool, inventory, before/after chain,
  preflight, backup-program and primary-erase records ending in `17.json`
  preserve actual pinned U-Boot programming and complete primary erasure with
  both SPLs and the protected release backup healthy. The separate normal boot
  evidence below establishes acceptance and fallback.

- `rootfs-installation-stream-audit.json`: host-only full 8 KiB streaming audit
  of the exact stock archive, including canonical semantic digest, ownership,
  modes, character devices, hardlink topology and critical file/link metadata.
  Member/data counts and actual kernel/DTB/overlay/boot-script bytes match the
  preserved Batch 1 report/artifacts. This performs no extraction or NAND write
  and is not runtime rootfs verification or physical manifest approval.

- `rootfs-rust-parser-stream-check.json`: complete locked-archive inspection by
  the shared Rust parser/CLI, independently matching Python member/data counts
  and the canonical semantic digest. The decoder and parser must both exit
  successfully; no extraction or NAND access occurs. This does not approve
  assets, implement streaming installation or prove runtime filesystem contents.

- `normal-release-uboot-backup-isolated-boot-17.json`: actual bridge-free cold
  boot with a new SSH boot ID, same SID, fully erased primary U-Boot and exact
  kernel-corrected release U-Boot in the fourth block. Healthy ECC/bad-block
  counters and boot logs corroborate physical fallback to `0x00C00000`. Both
  SPLs remain populated, so their BROM selection is not inferred. The existing
  rootfs is unchanged; this is not production reflash completion.

- `rootfs-native-gzip-stream-check.json`: complete stock compressed archive
  inspection using the release CLI's native bounded decoder and shared tar
  parser. Compressed hash, member/data counts and semantic digest independently
  match prior evidence. No extraction, external decoder or NAND write occurs;
  production installation and runtime readback remain unimplemented.

- `rootfs-verified-snapshot-stream-check.json`: complete real stock rootfs
  inspection through cache lookup and the retained `VerifiedAsset` guard, with
  compressed hash checks before/after decoding and matching semantic inventory.
  The mixed diagnostic manifest's other roles are explicitly synthetic; this
  approves no physical manifest or complete asset set and performs no extraction
  or device mutation.

- `rootfs-verified-replay-stream-check.json`: complete locked stock archive
  delivery from the retained verified snapshot into the CLI discard consumer.
  Entry boundaries, chunk bounds, ordered file offsets and total counts match
  the inspected inventory; final semantic digest and compressed asset recheck
  pass. This is host-only replay without extraction or device access, not
  runtime installation/readback or physical release approval.

- `rootfs-contained-linux-fixture-check.json`: deterministic Linux filesystem
  fixture results under ARMv7 qemu and strict ARM Clippy. The retained empty
  private root capability rejects unsafe/symlink paths and existing leaves.
  This does not access the device, implement final filesystem metadata or
  satisfy physical UBI/UBIFS installation and verification gates.

- `rootfs-contained-links-metadata-fixture-check.json`: Linux/ARM fixture results
  for symlink/hardlink containment, inode identity, numeric UID/GID 1000, final
  setuid/setgid mode and invalid owner rejection. This does not access NAND or
  prove whole-tree installation, device nodes or semantic filesystem readback.


- `rootfs-contained-character-preflight-fixture-check.json`: ARMv7 Linux
  fixtures for closed character-node creation and captured-inode metadata,
  symlink substitution rejection, and complete inventory metadata preflight.
  No device access or physical UBI installation is claimed.

- `rootfs-installation-preflight-stream-check.json`: complete locked stock
  archive verified replay after whole-inventory owner/mode/hardlink preflight.
  Counts and semantic digest match the independent audit. Host-only delivery
  creates a private verified snapshot but performs no extraction or NAND work.
