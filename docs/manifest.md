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

| Role | Maximum size |
| --- | --- |
| uboot | 4 MiB |
| kernel | 16 MiB |
| dtb | 1 MiB |
| recovery | 40 MiB |
| spl-hynix / spl-toshiba | 8 MiB each |
| uboot-nand | 4 MiB |
| rootfs | 2 GiB |

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

The GUI and CLI default to `stock`. Explicit `vitrallis-default` selection uses
`manifests/simulation-vitrallis.json`, a distinct nonbootable release. The compiled
catalog verifies both the release ID and profile against its manifest. Profile is
part of the hashed JSON, so the two profiles have different erase confirmations.
Neither a `stock` profile nor `not-installed` grants physical approval. There are no
legacy published manifests to migrate; this is the initial unpublished v1 contract.
