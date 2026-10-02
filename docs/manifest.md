# Image manifest v1

`manifests/simulation.json` is the complete executable example, with nonbootable fixture
bytes. Raw JSON is capped at 64 KiB, parsed strictly with unknown/duplicate fields
rejected at every level. Empty, trailing and malformed JSON fail. The supported values:

| Field | Contract |
| --- | --- |
| schema_version | integer 1 |
| release | 1–96 ASCII letters/digits, period, underscore, plus or hyphen |
| board / soc | pocketchip / allwinner-r8 |
| os / architecture | debian-13-trixie / armhf |
| installer_protocol | integer 1; planned protocol, no compatible real recovery shipped |
| minimum_flasher | exact 0.1.0 contract baseline |
| profile | required: stock or vitrallis-default; no implicit manifest default |
| vitrallis | stock requires not-installed; vitrallis-default requires blocked; ready is rejected |
| assets | exactly one of each eight roles below |

Every asset declares role, HTTPS URL, exact nonzero byte size, lowercase 64-digit
SHA-256 and provenance (HTTPS repository, full 40-digit lowercase commit, license
identifier). Provenance is recorded, not treated as an authorization signature.
There are no filename, command, environment, erase-offset or address fields.

| Role | Maximum size | Intended meaning |
| --- | --- | --- |
| uboot | 4 MiB | Complete FEL-loaded bootloader (BROM header + SPL + U-Boot + DTB) |
| uboot-nand | 4 MiB | U-Boot payload written to NAND at `0x800000`, zero-padded to one erase block; distinct from `uboot` |
| kernel | 16 MiB | Recovery kernel loaded into RAM; must match the `dtb` |
| dtb | 1 MiB | Recovery device tree; must match the `kernel` and carry `__symbols__` if an overlay is applied |
| recovery | 40 MiB | Recovery initramfs; it is *not* self-contained and requires `kernel` + `dtb` + a reviewed boot script |
| spl-hynix | 8 MiB | Raw NAND SPL image for SK Hynix H27UCG8T2ETR (OOB 1664) |
| spl-toshiba | 8 MiB | Raw NAND SPL image for Toshiba TC58TEG5DCLTA00 (OOB 1280) |
| rootfs | 2 GiB | Debian rootfs archive streamed into the on-device UBIFS installer |

The installed kernel, installed DTB and PocketCHIP device-tree overlay are not
separate manifest roles: they are part of the pinned rootfs and are loaded from
UBIFS by its `/boot/boot.scr`. The role meanings above are the intended physical
semantics established in Batch 0; the parser currently enforces only inventory
completeness, uniqueness, size bounds and provenance shape. A physical-release
validator must additionally assert these semantics, format/type and cross-asset
compatibility before any manifest can be approved. See
[image provenance](image-provenance.md).

Maximum aggregate disk need is bounded by this fixed inventory. Acquisition retains
cache entries plus private snapshots and an in-progress temporary file; allow room
for roughly three times the selected set. An out-of-space error aborts preflight.
Downloads never accept a byte beyond the declared size, transparent content encoding,
partial-response status, or mismatched Content-Length. Streaming EOF must match both
size and hash. Redirects are limited to five and remain HTTPS. Socket I/O has a
five-second idle bound, DNS/connect/header setup bounds, and each transfer has a
30-minute overall deadline. Cancellation is checked between chunks and socket I/O.
DNS cancellation can wait for the bounded resolver call to return.

`fetch` is explicit asset acquisition, not release approval. The physical release
catalog is empty. Future release selection must use reviewed immutable manifest
hashes/signatures and an explicit compatibility/rollback policy; never “latest”.
Files from `upstream-lock.json` are research candidates, not a flashable manifest.

A parsed manifest is not planning input by itself. Acquired bytes become usable only
through `VerifiedAssets::verify`, which requires a complete inventory matching the
manifest role/size/hash for every asset and rechecks each private snapshot.
Application-side NAND planning accepts only `VerifiedAssets` plus a validated
`IdentifiedTarget`; a manifest object, raw bytes or filesystem path cannot produce a
`NandPlan`, and no plan can execute in Batch 2.

The GUI and CLI default to `stock`. Explicit `vitrallis-default` selection uses
`manifests/simulation-vitrallis.json`, a distinct nonbootable release. The compiled
catalog verifies both the release ID and profile against its manifest. Profile is
part of the hashed JSON, so the two profiles have different erase confirmations.
Neither a `stock` profile nor `not-installed` grants physical approval. There are no
legacy published manifests to migrate; this is the initial unpublished v1 contract.

## Host-side physical manifests (Batch 1)

`images/assemble.py` additionally emits a **physical manifest** per NAND variant
(`manifest-hynix.json`, `manifest-toshiba.json`) for the artifact set it built.
This is a separate, stricter host-side document: it names the exact artifact and
input files, their sizes and SHA-256 values, the fixed NAND layout, the
kernel/DTB/overlay/boot-script/rootfs compatibility evidence, the storage audit
and package inventory hashes, and the pinned toolchain. `images/physical.py`
validates it offline, including magic/format checks on every role, byte-distinct
roles, the exact SPL geometry for the declared variant, no layout overlap,
rootfs at `0x1000000`, immutable provenance for every input, and an approval
record that binds the manifest hash and every artifact hash. The approved list
in `upstream-lock.json` stays empty, so no physical manifest can authorize a
write; this document set is not yet the application's release manifest.

The measured Hynix review layout uses the single verified `uboot_nand` artifact
for primary and redundant U-Boot slots; no new role or manifest-controlled
address is introduced. Session 17 physically proves the redundant slot at
`0xC00000`. This does not approve the manifest or enable execution. Toshiba
backup behavior remains unmeasured and its review plan stays unchanged.
