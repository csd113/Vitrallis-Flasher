# Debian 13 PocketCHIP image builder (Batch 1)

The target is armhf Debian 13/trixie on the PocketCHIP. Image construction
belongs on an isolated Linux/CI host; the Windows/macOS/Linux desktop core never
runs apt, losetup, mkfs.ubifs or privileged image hooks. **No NAND write path,
device access or hardware behaviour is implemented here.**

```text
python3 images/assemble.py fetch-assets --assets work/assets
python3 images/assemble.py build --assets work/assets --output work/batch1/set --work work/batch1/work
python3 images/assemble.py verify --set work/batch1/set
python3 images/assemble.py reproduce --assets work/assets --output work/batch1/repro --work work/batch1/repro-work
python3 images/build.py plan
python3 images/build.py check
python3 -m unittest discover -s images -p test_*.py
```

`fetch-assets` is the only network command and downloads exactly the locked
release assets by HTTPS, verifying size and SHA-256 before publishing each file
atomically. `build` and `reproduce` work from those files and the pinned host
container; nothing else reaches the network. `verify` revalidates an existing
set, including both physical manifests, without building anything.

## Route decision: pinned prebuilt rootfs

Batch 1 selected **route 1** from the original two honest routes: consume the
pinned prebuilt September rootfs. The decision and evidence:

| | selected (`os-2026.09.23-010738`) | fallback (`os-2026.07.29-024145`) |
| --- | --- | --- |
| asset | `pocketchip-rootfs-2026-09-23.tar.gz` `1e516cad…` 516,563,033 B | `pocketchip-rootfs.tar.gz` `010eb2a0…` 516,321,424 B |
| source commit | `f9191c2` | `7584eab` |
| kernel | `6.12.107+deb13-chip` `6.12.107-1.31` | `6.12.94+deb13-chip` `6.12.94-1.29` |
| kernel available upstream today | yes; the rootfs config (`436a91ee…`) and vmlinuz (`d3c044d8…`) match the independently hash-pinned `.deb` | no (404) |
| DTB / PocketCHIP overlay | `0132f7fa…` / `3230ea7f…` | byte-identical |
| boot script | `61901b03…`, overscan bootargs | `1fa91fee…` |
| package state | 709 packages, per-package `.list`/`.md5sums` hashes recorded | older set |

The September candidate matches the newer pinned kernel and repository state,
so it was adopted. `docs/rootfs-inspection.json` and
`docs/storage-inspection.json` were regenerated from the selected archive. The
July archive, its kernel config evidence and its derived kernel remain checked
in and locked as the documented fallback; they are not referenced by the
selected physical roles.

The selected rootfs is still a **pinned prebuilt binary**. It cannot be rebuilt
from current upstreams (the live CHIP apt repository deletes replaced packages
and upstream recorded no Debian snapshot), so its installed package set is
bound by `images/package-inventory-6.12.107+deb13-chip.json`: for every one of
the 709 installed packages the recorded version, architecture, status and
hashes of the dpkg `.list`/`.md5sums` files are asserted against the archive.
The rootfs is consumed byte-for-byte; no live package repository is used.

## What the builder does

1. **Locks.** `images/inputs.lock.json` (schema 3) pins the selected rootfs,
   the fallback, the SPL tool source files, the container digest, the signed
   Debian snapshot, all dependency revisions, the package-inventory digest and
   the derived SPL/padded-U-Boot hashes. `upstream-lock.json` records the full
   provenance closure. `images/assemble.py` refuses to build unless the offline
   provenance checks pass.
2. **Asset verification.** Every consumed asset (rootfs, `sunxi-spl.bin`,
   `u-boot-dtb.bin`, `u-boot-sunxi-with-spl.bin`, `initrd.uimage`) is checked
   by exact size and SHA-256 before use.
3. **Rootfs scan, never extraction.** The archive is streamed once: the kernel,
   DTB, overlay, boot script, kernel config and dpkg status are extracted; the
   dpkg `.list`/`.md5sums` files are hashed; members, types, device nodes,
   hardlinks, systemd links, journald/zram configuration and storage state are
   inspected. Unsafe paths, sparse members, disallowed types and unexpected PAX
   metadata fail closed.
4. **Package inventory.** The dpkg database is turned into a deterministic,
   sorted inventory and compared with the checked-in file and the input lock.
5. **Compatibility.** The extracted kernel, DTB, overlay and boot script are
   validated against `images/compatibility.json` and the provenance lock: DTB
   `__symbols__` labels, overlay `__fixups__` references, DIP selection
   constants, boot-script release paths and `bootz` addresses, dpkg kernel
   version and the kernel module tree.
6. **Storage audit.** The actual fstab, enabled unit links, journald
   configuration, zram generator state and kernel swap/zram/UBIFS symbols are
   audited against the reviewed storage policy. NAND-backed swap or an enabled
   swap/zram manager fails the build. The policy drafts remain unapplied
   (`applied: false`); applying them to a device is a later, hardware-gated
   batch.
7. **Deterministic SPL generation.** See below.
8. **Deterministic U-Boot padding.** `u-boot-dtb.bin` is zero-padded to exactly
   4 MiB and checked against the locked derived hash.
9. **Deterministic rootfs repack.** The pinned archive is rewritten
   member-by-member without extraction. Numeric ownership, modes, symlink
   targets, hardlinks and device major/minor are preserved; mtimes are
   normalized to `source_date_epoch`, owner/group names and gzip headers are
   cleared. Source and output canonical member manifests must be equal. The
   repack is a reproducibility artifact; the physical `rootfs` role remains the
   exact upstream archive bytes.
10. **Physical manifests.** One manifest per NAND variant is written and
    immediately validated by `images/physical.py`; a build cannot complete with
    an invalid manifest. `verify` revalidates a set offline.

## Deterministic Hynix/Toshiba SPL generation

Upstream `lib-nand.sh` builds one erase block of four 64-page BROM groups and
fills the unused pages from `/dev/urandom`; the pinned tool additionally fills
each page's unused space from `/dev/urandom`. Batch 1 keeps the unmodified
pinned `sunxi-nand-image-builder` and replaces only those entropy inputs with a
documented, content-addressed stream:

```text
seed    = SHA-256("Vitrallis Flasher deterministic NAND SPL padding v1")
block_i = SHA-256(seed || 0x00 || label || big-endian-uint64(i))
```

The tool source is built in the digest-pinned container from
`linux-sunxi/sunxi-tools` `d7bbd172` with
`-O2 -Wall -Wextra -Dfls=sunxi_fls` and a checked-in version header. The
consolidated 256 KiB source is `[sunxi-spl.bin (16 KiB) + pad-group (48 KiB)] ×
4`; each variant runs twice with its own deterministic stream bound over
`/dev/urandom`, and the two outputs must be byte-identical. `images/spl.py`
then validates the geometry (exact size from the OOB width), BBM bytes and the
descrambled ECC-protected region of all 256 pages against the exact source
chunk each page must carry.

Recorded results (also in `docs/image-provenance.md`):

| Variant | OOB | Size | SHA-256 |
| --- | --- | --- | --- |
| `spl-hynix` | 1664 | 4,620,288 | `0099342e6331e9880d704bf11eb75f1b7618be8cef05b2ae456918fa1010fc7a` |
| `spl-toshiba` | 1280 | 4,521,984 | `487edb2eda190bd98b85deacaf858fcc4583351c3210be4ebc19ee983d19dd45` |

The deterministic and upstream-random outputs were compared page by page in
Batch 1: the ECC-protected regions are identical and the differences are
confined to the unused tail/BBM padding regions, so the layout semantics are
preserved. Outputs are identical across two independent toolchains (the pinned
snapshot image and a `gcc:12-bookworm` image).

## Pinned build environment

`images/Dockerfile` builds the only host tool used by the pipeline from
`debian:bookworm-slim` pinned by manifest-list digest
`sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251`
(resolved `linux/amd64` `sha256:f3034a6e…`) using the signed snapshot
`snapshot.debian.org/archive/debian/20260930T000000Z/` (InRelease SHA-256
`77737fa4b34f2693e982cc9ee35736816c35a7778fc2d326cc1bbf5b301fe1aa`). The
container runs with `--network none` and only the pinned source tree mounted
read-only. The image is not pinned by a rebuilt digest because Docker image
metadata carries build time; the base digest, snapshot timestamp and exact
package versions are pinned instead (recorded as evidence in
`docs/validation.md`).

## Physical manifest validation

`images/physical.py` implements the approval gate. A manifest names
`artifacts/<file>` and `inputs/<file>` by path but identity is always the file
bytes. The validator checks: required roles present and unique; magic/format of
every role (eGON SPL header, FEL bootloader with an embedded CRC-checked U-Boot
uImage matching `u-boot-dtb.bin`, exact 4 MiB zero-padded U-Boot, ARM zImage
magic, FDT/overlay FDT, uImage script and initrd, gzip tar rootfs); exact
hashes against both file bytes and the provenance lock; per-role size limits
and the exact geometry-derived SPL sizes; byte-distinct roles; DTB
`__symbols__` vs overlay `__fixups__`; boot-script release paths and addresses;
kernel bytes, DTB, overlay and dpkg version inside the rootfs archive; the
declared SPL variant and page structure; fixed NAND offsets with no overlap and
rootfs at `0x1000000`; immutable provenance and pinned toolchain for every
input; and an approval record that binds the exact manifest hash and every
artifact hash. `upstream-lock.json`'s `approved_physical_manifest_sha256`
stays empty: no physical manifest is approved, and no write path exists.

## Current limits and blockers

- The selected rootfs is pinned prebuilt and cannot be rebuilt from current
  upstreams; redistribution permissions are still unresolved.
- Panel/backlight/touch/keyboard behaviour, both NAND geometries, ECC and
  bad-block runtime behaviour, power-loss behaviour and USB transport remain
  hardware-only questions, recorded in `docs/boot-layout.md` and
  `docs/image-provenance.md`.
- The fourth NAND slot at `0xC00000` remains ambiguous; Batch 1 does not
  populate it.
- The optional `vitrallis-default` profile still requires a complete pinned
  bundle and stays blocked; stock is the only profile the builder emits.
- Package/security updates on a flashed device, and the real NAND write path,
  are later batches. This document does not approve flashing.
