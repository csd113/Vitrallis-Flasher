# Debian 13 PocketCHIP image-builder scaffold

The target is armhf Debian 13/trixie, based on x-chip-os commit
`7584eab1aafb1667bd89ae210dcd641efc7cc5b5`. Image construction belongs on an isolated
Linux/CI host; the Windows/macOS/Linux desktop core never runs apt, losetup,
mkfs.ubifs or privileged image hooks.

```text
python3 images/build.py plan
python3 images/build.py plan --profile vitrallis-default
python3 images/build.py check
python3 -m unittest discover -s images -p test_*.py
```

`plan` prints reviewable argv and stages without executing them. `check` intentionally
returns exit 2 and names unresolved inputs. It cannot be turned into a “ready” build
by changing a compatibility flag. `images/Dockerfile` describes the future Linux
build environment; it requires an explicit base image and generated signed snapshot
sources. There is no automatic privileged builder or downloadable approved OS yet.

The source's moving Debian/CHIP apt feeds and `debian:trixie` base do not form a
reproducible closure. `inputs.lock.json` deliberately leaves container digest, signed
Debian snapshot and CHIP snapshot/package hashes unfilled. The optional
`vitrallis-default` profile also requires the complete Vitrallis bundle. Supplying fabricated hashes or a moving tag would hide these blockers.
The timestamp is fixed for normalized outputs; it does not freeze packages by itself.

A completed builder must materialize the pinned source in a fresh working tree,
configure signed immutable mirrors, lock all package versions and key fingerprints,
run the existing PocketCHIP live-build configuration, and produce a package inventory.
Do not disable package signature checking. Only disposable Linux workers may run
required image construction privileges; never mount a developer's host devices.

`images/repack.py` is functional scaffolding: it takes an already prepared rootfs tree,
rejects special files, sorts entries, retains numeric ownership/modes/symlink metadata,
normalizes mtime and gzip headers, and atomically publishes a new archive without
overwriting anything. It never extracts an archive or follows symlink directories.
The source tree must be private and quiescent. Build twice in clean environments and
compare the complete rootfs hashes before claiming reproducibility.

```text
python3 images/repack.py images/work/prepared-rootfs images/output/candidate.tar.gz --epoch 1785292905
```

Prepare those directories explicitly. This repacker does not create Debian or certify
its contents. Image validation must separately inspect archive paths, hardlinks and
symlinks, maximum expansion and entry counts, expected ownership, kernel/DTB pairing,
on-device extractor policy and all installed hooks before an image can be approved.

## Current Vitrallis requirements and blocker

The pinned downloaded image contains PocketHome despite the stale source README.
It also contains SDL2 2.32.4 and Awesome 4.3, which are promising matches for the
inspected Shell contract. Presence alone does not prove compatibility. The image
needs a complete checksum-verified matching ARM
Shell/Terminal/Notepad/Files bundle, and tests of its existing PocketHome menu,
Marshmallow fallback, Awesome Home key, systemd user session, and SDL/libc ABI.
Python Tk is optional for separately installed Python/Tk applications; the current
native Shell window-focus implementation uses Awesome directly. No Shell integration was performed and no Vitrallis files were modified.

Kernel LCD/backlight/input wiring, authenticated USB recovery transport and NAND
verification remain additional release gates. The manifest parser requires `vitrallis: "not-installed"` for `profile: "stock"`
and `vitrallis: "blocked"` for `profile: "vitrallis-default"`; neither grants physical
approval. The real backend refuses every write independently.

## Stock and optional Vitrallis profiles

`plan` and `check` default to `--profile stock`. Shared build, licensing, hardware
and PocketHome preservation gates apply to both profiles. The stock plan excludes
Vitrallis installation and startup changes, and does not require its bundle hash or
Shell compatibility evidence. The optional `--profile vitrallis-default` adds those
requirements and explicit startup/fallback validation. Both plans always report
`blocked`; merely filling input hashes does not approve a physical release.

The plan records the intended desktop, whether Vitrallis is installed, preservation
of PocketHome, and the fact that a recovery reimage does not preserve current NAND
data. Package inventory and hardware/session checks must substantiate “stock” before
release. No in-place apt migration is provided. See [upgrade profiles](upgrade-profiles.md)
for the user-facing script and exact optional startup contract.

## Memory and NAND-write policy

Both profiles include `storage_policy` in their plan. `images/storage.py` records
NAND-backed swap prohibition, candidate RAM-only zram/log/tmp settings, configuration
hashes and release checks. The three files under `images/storage-candidates/` are
configuration drafts with fixed intended destinations; no installer executes them.
The shared input lock now includes storage/memory validation as a release blocker.
See [the image inspection and optimization review](debian-optimizations.md) for observed
upstream defaults, tradeoffs and the required before/after hardware measurements.
