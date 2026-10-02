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
