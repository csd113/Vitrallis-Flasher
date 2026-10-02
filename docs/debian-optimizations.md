# Debian 13 on CHIP: memory and NAND-write review

Apply the same storage policy to both `stock` and `vitrallis-default`. Preserve the
PocketHome experience and the NAND health services. These are **candidate image
settings**, not changes to an attached CHIP or an already built OS. The image planner
now reports the policy and hashes the draft configuration files; physical release
approval still requires runtime evidence.

## What the inspected image actually contains

The selected rootfs is `os-2026.09.23-010738/pocketchip-rootfs.tar.gz`, SHA-256
`1e516cade3085633f61697d69a5d95cb84a501d8b606247987db5837a53e19ef`. It was
inspected directly as an archive without extraction or execution. Selected facts
are recorded in [storage-inspection.json](storage-inspection.json); the July
`os-2026.07.29-024145` inspection remains the documented fallback. Batch 1
re-runs a machine-checkable subset of this audit on every build
(`images/assemble.py`): it fails closed on an fstab swap entry, an enabled
swap/zram unit link, or an active upstream journald directive, and records the
journald drop-ins, zram generator state, `tmp.mount` options and kernel
swap/zram/UBIFS symbols in the artifact set.

- `/etc/fstab` contains only the unconfigured-base-system comment. No swapfile or
  configured `.swap` unit was found in the inspected paths. This is not proof of the
  final running system's swap state: installers, generators or later configuration
  can change it.
- The pinned live-build configuration has an empty `LB_SWAP_FILE_PATH` and
  `LB_SWAP_FILE_SIZE="512"`. Treat this as a build setting to resolve explicitly,
  not evidence that the published rootfs has a working 512 MB swapfile. The completed
  image builder must suppress swap-image creation and audit the resulting image.
- Kernel `6.12.107+deb13-chip` has `CONFIG_SWAP=y`, `CONFIG_ZRAM=m`, the zram module,
  LZ4 support and zram writeback capability. No installed `zram-tools` or
  `systemd-zram-generator` package was found. Zswap is compiled in but not default-on.
- `CONFIG_UBIFS_ATIME_SUPPORT` is disabled. Adding `noatime` to this kernel's UBIFS
  root mount would not remove access-time writes that were already absent.
- Journald's inspected configuration leaves storage defaults commented; no `/etc`
  override was observed. Persistent journal behavior must be checked after boot.
- The shipped `tmp.mount` has a 50% RAM ceiling and a one-million-inode ceiling.
  Debian 13 uses tmpfs for `/tmp` by default, but fstab or local overrides can change
  the effective mount. [Debian release notes](https://www.debian.org/releases/trixie/release-notes/issues.en.html#the-temporary-files-directory-tmp-is-now-stored-in-a-tmpfs).
- Enabled links include `ubihealthd`, package-update timers, `fake-hwclock`,
  `plocate-updatedb`, `logrotate`, `fstrim` and e2scrub. Enabled links in an archive
  are not proof that those services ran or wrote data.

## Proposed defaults and tradeoffs

| Area | Candidate decision | Reason / practical limit |
| --- | --- | --- |
| NAND-backed swap | Prohibit it in both profiles; no swapfile, NAND swap volume, mtdswap or zram writeback to NAND | Avoid paging writes to the internal flash. Do not silently substitute disk swap if zram fails. |
| RAM-only zram | Test LZ4 with logical size `min(usable RAM / 4, 128 MiB)` | Start small on a 512 MiB board. Compression costs CPU and consumes real RAM; this is a tuning candidate, not measured extra capacity. |
| System journal | Volatile journal with `RuntimeMaxUse=8M`, `RuntimeMaxFileSize=1M`, `RuntimeKeepFree=16M` | Reduces persistent journal writes. Previous-boot logs disappear; limits are journal accounting limits, not a hard total process-RAM cap. |
| `/tmp` | Keep tmpfs; test a 64 MiB / 16k-inode ceiling | Limits temporary-file pressure. Large downloads/builds may need a deliberate persistent or external work directory. |
| `/var/tmp`, user data, dpkg state | Keep persistent | Avoid breaking applications, updates, saves and recovery. Do not move all of `/var` or `/var/cache` to RAM. |
| UBI / UBIFS / `ubihealthd` | Preserve wear leveling, scrubbing, health state and crash-recovery behavior | Their maintenance writes serve a purpose; do not disable them to make a write counter look better. |
| Access-time policy | No change for the inspected kernel | UBIFS atime support is already disabled. Recheck if the kernel changes. |
| Indexing and caches | Measure first; make `plocate` indexing optional if it is a material writer | Disabling indexing changes `locate` freshness. Avoid indiscriminate cache purges or service removal. |
| Updates, clock persistence, networking | Preserve | Blanket timer removal can impair security updates, clock recovery, networking or the stock desktop. |

The draft files live in `images/storage-candidates/`; their intended destinations
appear in `images/build.py plan` output. They are **not automatically copied into a
rootfs**, installed on the host, or sent to a device.

## Swap: disable flash paging, not necessarily all paging

Zram stores pages in compressed RAM. It can also be configured with a backing device;
that optional feature would defeat a RAM-only policy if it points at NAND. Leave
`writeback-device` unset and verify the running `backing_dev` value. A 128 MiB logical
zram device is not 128 MiB of free physical memory: actual use depends on workload and
compression, and incompressible pages still consume RAM. The kernel documents these
mechanisms in its [zram guide](https://www.kernel.org/doc/html/v6.12/admin-guide/blockdev/zram.html).

The proposed manager is Debian's `systemd-zram-generator`, which is not yet in the
candidate package lock. It avoids a custom root shell service. Add exactly one
reviewed manager only when its package and dependencies are pinned; test that it
loads the matching kernel module. The draft uses only a fixed device section and
arithmetic size expression, with no external-program directives.
[Debian's generator configuration reference](https://manpages.debian.org/trixie/systemd-zram-generator/zram-generator.conf.5.en.html).

Do not use `vm.swappiness=0` as a claim that swap is disabled, mask `swap.target`
(which also obstructs intended zram activation), or replace zram with zswap backed
by NAND. Zswap can evict compressed pages to its backing swap device; it does not
provide the same no-flash-write guarantee.
[Kernel zswap documentation](https://www.kernel.org/doc/html/latest/admin-guide/mm/zswap.html).

A strict no-swap configuration can be tested as the baseline. On 512 MiB, compare
its OOM behavior with small RAM-only zram under the actual PocketHome and Vitrallis
workloads. Never run `swapoff -a` automatically against a busy device: removing active
swap can require memory it does not have. For a future build, remove disk-swap
activation from the image before first boot, inspect fstab/units/generators/hooks and
boot arguments, then verify runtime state. No such device mutation is implemented here.

## Logs and temporary files

The volatile journald draft keeps diagnostics during a boot while avoiding persistent
journal files. Export relevant logs before a planned reboot. For a debugging image,
allow a deliberately bounded persistent journal instead and disclose its additional
writes. Do not disable error reporting or silently forward volatile logs into a
second persistent syslog daemon. Also inspect application logs, journal namespaces,
`wtmp`/`btmp`, apt logs and health statistics: journald settings do not control all
writers. [Debian journald reference](https://manpages.debian.org/trixie/systemd/journald.conf.5.en.html).

The `/tmp` draft retains sticky permissions, `nosuid`, `nodev` and Debian's execution
semantics. It does not add `noexec`, which can disrupt installers. Tmpfs has a maximum,
not an upfront allocation; it can also be swapped, so its NAND benefit depends on
excluding NAND-backed swap. Do not use bounded `/tmp` for the rootfs archive or a
complete Vitrallis release bundle. Keep `/var/tmp` persistent and avoid hiding existing
files with a live mount. Test ENOSPC behavior as well as OOM behavior.

## Avoid generic SSD tuning on raw NAND

The rootfs uses UBI/UBIFS, not an SSD's block filesystem. Keep UBIFS compression and
power-cut recovery. Do not add ext4-specific options, disable synchronization, or
stretch writeback intervals merely to reduce apparent writes; those changes can
increase data loss or latency. UBI supplies wear leveling; disabling its health
watchdog or relocating `/var/cache/ubihealthd.v1.stats` without understanding its
persistence contract is counterproductive.
[Kernel UBIFS documentation](https://www.kernel.org/doc/html/v6.12/filesystems/ubifs.html).

`fstrim` and e2scrub are not optimizations for a UBIFS root. Assess their actual work
before changing timers; globally disabling them could affect supported external block
devices. Similarly, keep clock-save and package-maintenance services unless measured
behavior justifies a narrowly scoped change. Do not remove stock functionality for
an unmeasured boot-time improvement.

## Release validation

For each NAND part and each desktop profile, record baseline and candidate results
under identical workloads. Include cold boot, idle, Wi-Fi, audio, terminal use, package
updates, application switching, memory pressure, full temporary storage, low free NAND
space and an authorized power-loss/recovery test. The test must demonstrate usable
latency and clean failure, not just lower counters.

Read-only inspection commands on a separately authorized test device include:

```text
cat /proc/swaps
cat /proc/meminfo
cat /proc/pressure/memory
cat /proc/pressure/io
cat /sys/block/zram0/backing_dev
cat /sys/block/zram0/mm_stat
findmnt -T / -o TARGET,SOURCE,FSTYPE,OPTIONS
findmnt -T /tmp -o TARGET,SOURCE,FSTYPE,OPTIONS
systemd-analyze cat-config systemd/journald.conf
journalctl --disk-usage
systemctl list-timers --all
systemctl status ubihealthd
```

Missing zram files are expected on the no-swap baseline; unavailable PSI interfaces
must be recorded rather than fabricated. After boot, startup, pressure and reboot,
`/proc/swaps` must contain no disk/NAND-backed entry; a zram-enabled image must show
only the intended zram device with no backing device. Check all configured zram
instances, not just zram0. Verify effective configuration after package upgrades too.

Collect process-level write attribution where practical and UBI erase-counter
min/max/distribution and health information from the reviewed tooling. Block-device
counters alone can miss raw-MTD activity. No hardware measurements were made here,
and no percentage write reduction or NAND-lifetime increase is claimed.
