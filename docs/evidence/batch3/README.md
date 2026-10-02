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
This is not a recovery DRAM load address. Recovery DRAM initialization, full
payload boot and authentication remain unresolved.

No recovery payload has booted and no destructive flash has occurred yet.
