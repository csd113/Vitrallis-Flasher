# Upstream review — 2026-09-12

Full revisions, reviewed file hashes and verified asset bytes are recorded in
[`upstream-lock.json`](../upstream-lock.json). Review checkouts and large downloaded
images are ignored under `work/upstream`; none are redistributed in packages.

| Project | Reviewed revision |
| --- | --- |
| [x-chip-tools](https://github.com/nextthingco/x-chip-tools/tree/215f98eed44babb699c129bb124537d32c414102) | 215f98eed44babb699c129bb124537d32c414102 |
| [x-chip-os](https://github.com/nextthingco/x-chip-os/tree/7584eab1aafb1667bd89ae210dcd641efc7cc5b5) | 7584eab1aafb1667bd89ae210dcd641efc7cc5b5 |
| [sunxi-tools](https://github.com/linux-sunxi/sunxi-tools/tree/d7bbd172a5da601a08f94479de308c6fb714a19a) | d7bbd172a5da601a08f94479de308c6fb714a19a |
| [Vitrallis-Shell](https://github.com/csd113/Vitrallis-Shell/tree/40b232780729853cf49691d160517ab866b4addb) | 40b232780729853cf49691d160517ab866b4addb |

## LIVE installer

x-chip-tools recommends FEL boot into Linux, streaming the rootfs over USB networking,
and formatting through the device kernel's UBI stack. Its fastboot path bypasses SLC
handling and is documented broken; that path is excluded here. The host-side script
is not portable and does not meet this application's erase/identity requirements.
[Reviewed source](https://github.com/nextthingco/x-chip-tools/blob/215f98eed44babb699c129bb124537d32c414102/flash-live.sh).

Review found four blockers: it erases to initialize NAND detection, uses a fallback
part on unknown identity, lacks authenticated SSH peer binding, and offers no durable
readback verification contract. The recovery gadget supplies only RNDIS; macOS needs
a separately validated networking mode. The flasher therefore implements the flow
and failure policy using mocks while keeping its real boundary read-only.

Hynix H27UCG8T2ETR and Toshiba TC58TEG5DCLTA00 require distinct SPL/OOB handling.
The part table is retained explicitly; no new ECC, NAND writer or host UBIFS generator
was invented. Future work must adapt proven upstream code with permission and
physical evidence. [NAND source](https://github.com/nextthingco/x-chip-tools/blob/215f98eed44babb699c129bb124537d32c414102/lib-nand.sh).

## Candidate assets inspected

| Asset | Bytes | SHA-256 |
| --- | ---: | --- |
| installer-2026.06.19-181233 / initrd.uimage | 31,628,120 | 5b8b392c095fd37472f9a08f1ed8f6cdbb6d6a04bb36a0696dbcd3033c0f3b25 |
| os-2026.07.29-024145 / pocketchip-rootfs.tar.gz | 516,321,424 | 010eb2a0cb59334f068d3a5e6989bdc486715362e7a5d04fbb329e7b556e0de2 |

Both assets were downloaded via HTTPS and rehashed locally; both tags resolve to the
reviewed source revisions. This proves byte integrity against the recorded release
metadata, not hardware safety. Their immutable download URLs are in the lock file.

A streaming, nonextracting inspection read 50,936 rootfs archive members, the package
database and selected configuration. The observed package inventory is recorded in
[rootfs-inspection.json](rootfs-inspection.json). PocketHome 0.0.8, Awesome 4.3,
SDL2 2.32.4, Python 3.13, systemd 257 and kernel 6.12.94-chip are present. Python Tk
and xinput were not observed in that installed-package set. The source README's
PocketHome portability warning is stale relative to this artifact. No ARM binary was
executed, and package presence does not establish session or hardware compatibility.
[OS source](https://github.com/nextthingco/x-chip-os/tree/7584eab1aafb1667bd89ae210dcd641efc7cc5b5).

## Vitrallis compatibility

The inspected Shell installation contract expects four matching ARM native binaries,
a valid existing PocketHome menu/assets, Awesome integration and a systemd user
session preserving Marshmallow recovery. It documents an image-matched SDL2/libc ABI;
its PocketCHIP tooling uses Python. Current window focus uses Awesome directly;
Tk is conditional on separately installed Python applications, not a baseline native
Shell dependency. The candidate image needs a verified full bundle and complete
session/dependency validation before
integration can be claimed. This repository does not modify Vitrallis-Shell.
[Installation contract](https://github.com/csd113/Vitrallis-Shell/blob/40b232780729853cf49691d160517ab866b4addb/docs/devices/pocketchip.md).

## FEL identity

The pinned sunxi-fel list implementation prints USB bus/address, SoC name and nonzero
SID. Its A13 identifier covers several SoCs including R8; it cannot establish the
PocketCHIP board or NAND. Malformed output and missing/zero SID fail closed. Device
re-enumeration changes invalidate prepared authorization.
[Enumeration source](https://github.com/linux-sunxi/sunxi-tools/blob/d7bbd172a5da601a08f94479de308c6fb714a19a/fel.c).

## Installer follow-up review

The desktop-profile follow-up inspected Vitrallis-Shell commit
[`02df65ba08e694777031fe3bd7a0b8274ddf4e45`](https://github.com/csd113/Vitrallis-Shell/tree/02df65ba08e694777031fe3bd7a0b8274ddf4e45).
README, PocketCHIP installation documentation and bootstrap hashes are recorded in
`upstream-lock.json` separately from the earlier source review. The bootstrap was
read, never executed or bundled. Its one-line command is reproduced in this project's
README as an optional user action on an already compatible device.

The inspected public release inventory contains beta2.5 and earlier standalone
binaries, without the new complete `.vtrbundle` and matching helpers. That blocks
completion of the one-line installer today. The bootstrap itself is present on
`main`; the current upstream README's bootstrap-publication warning lags that state.
Current prerequisites are Debian 12+ armhf, glibc 2.36+, SDL2 2.26.5+, Python 3.8+,
HTTPS curl/CA certificates, PocketHome, Awesome 4.x and a systemd user session.
The bootstrap preserves Marshmallow startup. Default Vitrallis startup is a separate,
opt-in Awesome block; the stock Flasher profile excludes both installation and that
block. No upstream checkout or physical device was modified.
