# LIVE recovery protocol gate

The host should reuse proven x-chip-tools NAND/SPL behavior through an external,
reviewed recovery boundary. It must preserve the measured NAND ECC/layout and
avoid the broken fastboot/SLC path. Native verification decodes the reviewed
boot0 format; image encoding still uses the unmodified pinned upstream builder.
The current upstream installer is **not** an implementation of this protocol.
Batch 3 has a bounded diagnostic client and daemon with physically measured
SID-bound authentication over macOS ECM.

Required sequence before implementing a real backend:

1. Enumerate every FEL candidate and reject unknown, missing-SID or multiple devices.
2. Establish PocketCHIP board identity and a known full NAND part/geometry without
   erasing to discover it. A13/R8 alone is insufficient.
3. Verify a reviewed immutable complete image set, compatible kernel/DTB/recovery,
   both NAND SPL variants and padded bootloader, transport support and power guidance.
4. Obtain a confirmation bound to fresh device identity, release and full manifest hash.
5. FEL-load a reviewed non-destructive recovery boot payload using fixed reviewed
   addresses and structured argv. Booting arbitrary U-Boot scripts from a manifest
   would violate the security boundary; payload review must inspect all embedded code.
6. Bind the USB recovery endpoint to the selected SID using a fresh session challenge
   and authenticated peer identity. Do not trust a fixed IP, MAC, shared throwaway key,
   network route or disabled SSH host checking. Validate ECM/NCM for macOS and an
   explicitly supported Windows gadget mode; RNDIS alone is not cross-platform.
7. Recheck board/NAND and the nonce before the device accepts an erase capability.
   Device-side code owns partition layout, SLC handling and UBIFS geometry. Unknown
   geometry is fatal, never a fallback to Hynix.
8. Stream a bounded verified rootfs using a fixed protocol operation. Device extraction
   must validate paths/links/expansion and refuse partial-success reports. No remote
   shell strings or manifest commands are allowed. Track explicit bootloader and rootfs
   write status; network byte counts alone are not NAND completion evidence.
9. Verify bootloader and persisted rootfs readback using a defined canonical inventory
   that handles UBIFS metadata, ECC and bad blocks. A transport hash is insufficient.
   Finish sync, non-lazy unmount and UBI detach successfully before reporting complete.
10. On failure, time out/cancel safely, keep a recovery diagnostic, invalidate erase
    permission and require fresh preflight. Never retry erase or reboot automatically.
    On success, instruct the user to remove the jumper and power-cycle.

The mock backend checks this host lifecycle and failure order, including both NAND
identities. It does not simulate ECC, real bad blocks, transport authentication,
physical readback or power-failure atomicity. Those require real engineering and
explicit hardware validation. The host download pipeline is already reusable for
future approved payloads without exposing physical erase now.

## Batch 2 host seam mapping

Batch 2 implements the parts of this protocol that need no hardware.
`FelTransport` names discovery, identification, device information, RAM upload,
execution and memory/status readback; the original Batch 2 `UnavailableFel`
refuses every operation. Batch 3 now uses `NativeFel` for physical diagnostics.
`IdentifiedTarget::identify` enforces the closed board/SoC/NAND decision and rejects
ambiguous or unknown fixtures. `NandPlan` records the ordered operations, exact
lengths/digests and readback checks that steps 5-9 require, with the unresolved
physical questions attached as hard `PlanGate`s; `authorize_execution` always fails.
`VerifiedAssets` supplies only manifest-verified bytes. Steps 4-10 still require the
reviewed protocol, device daemon, session authentication and hardware validation;
nothing in Batch 2 executes a plan or contacts a device.

## Batch 3 measured recovery boundary

`flasher-core::recovery` and the ARMv7 `flasher-recovery` daemon expose only
`Ping`, `Inventory`, fixed-region `BootReadback` and RAM-only `ReturnToFel`. A fresh 32-byte boot
secret, independently checked 16-byte SID and random session ID bind each boot.
HMAC-SHA256 from the existing ring dependency authenticates both roles, protocol
version, implementation hash and fresh client/server nonces. Frames bind direction
and monotonically increasing sequence numbers and reject lengths over 64 KiB
before allocation. Socket I/O supports cancellation and bounded deadlines.
Credentials are redacted from Debug and excluded from tracked evidence. Public
diagnostic inventory is authenticated but not encrypted.

The daemon loads fixed matching SID and reset modules through `ToolRunner` and
checks Linux nvmem against the host's freshly identified FEL SID. The host pins
the exact diagnostic utility, bootloader, kernel, DTB, overlay, daemon and RAM
template. These diagnostic pins do not approve a physical NAND manifest. The
reviewed U-Boot script has no NAND operation. The host appends a mode-0600 boot
credential archive; Linux documents concatenated compressed/uncompressed newc
archives in its [initramfs buffer format](https://docs.kernel.org/driver-api/early-userspace/buffer-format.html).

Three physical RAM boots established the reviewed load addresses, macOS ECM
networking and output-only ACM diagnostics. The second boot authenticated a Ping
and rejected wrong keys, SID, session, implementation hash and prior-boot credentials.
The third returned authenticated board, memory, kernel and NAND inventory. Its
DT memory node is `memory`, rather than `memory@40000000`; this measured difference
now has a regression test. The first boot exposed an incorrect kmod applet
invocation, also covered by a regression test.

The third boot accepted ReturnToFel but did not re-enumerate. The pinned kernel
builds its reset driver as a module, omitted by that template; this is the likely
cause, derived from configuration and upstream restart-handler source. A corrected
template includes and verifies the driver before accepting connections. The fourth boot verified the loaded driver in its boot log and successfully
returned to FEL, where the native transport rediscovered the same SID. RestartAccepted means request acceptance;
it is not proof of FEL re-entry. The restart refuses NAND-backed mounts or attached
UBI and requires the physical FEL bridge to remain connected.

The blank display is not used as proof of startup. ACM connects no getty, SSH or
interactive input. ECM DHCP, TCP authentication and inventory are measured on
macOS; NCM and Windows workflows remain unresolved. The daemon exposes no NAND
write/erase request. All destructive plan gates remain closed. Evidence is indexed
in [the Batch 3 evidence directory](evidence/batch3/README.md).

The fifth physical boot exercised `BootReadback` on all four 4 MiB boot blocks.
Requests contain a closed region enum, with no address, length, path or command.
The daemon rechecks each partition's measured geometry, offset and zero bad-block
count, then runs fixed `nanddump --noecc --oob --bb=dumpbad` arguments through
`ToolRunner`. Raw output stays in a private temporary RAM directory. Only bounded
hashes, first bytes, page marker observations, erased-page lists and counters return.
The host rejects a response for a different region. Readbacks are diagnostic;
uncorrected digest equality is not a production verification policy. `0xC00000`
is still erased and its boot fallback role remains unproven.

Protocol version 2 makes the readback interpretation explicit: `Raw` or
`KernelCorrected`. The host rejects a response for another interpretation. Kernel
correction is permitted only on U-Boot; SPL uses a different boot0 ECC layout and
must not be silently interpreted using the normal kernel layout. The sixth RAM
boot returned a kernel-corrected U-Boot digest identical to the preserved original,
with 6,136 corrected bits and zero uncorrectable errors. The raw/OOB digests are
still diagnostic, not a flat-NAND verification policy. Historical evidence retains
the protocol version actually measured.

The separate `boot0-readback` CLI diagnostic decodes bounded saved 4 MiB SPL
data captures with native BCH-64. The reviewed boot0 format uses GF(2^14),
primitive polynomial 0x5803, 1,024 data bytes, four FF metadata bytes and 112
parity bytes, with the builder's byte bit order and LFSR seed 0x4a80. It requires
zero post-correction syndromes and validates the eGON/SPL header and checksum.
It reads regular capture files and emits bounded reports; it exposes no NAND
mutation. The padding-only pinned known-answer test covers up to 64 bit errors
across data and parity, including metadata errors. All eight original captured
copies decode to the same program digest, but this host diagnostic does not
establish live protocol verification or physical fallback behavior.

Protocol version 3 adds `Boot0Corrected` for SPL primary/backup only. The daemon
reads raw data/OOB with kernel ECC disabled and runs the native decoder on all
four copies. Reports retain the raw data/OOB hashes and add `spl_copies`, containing
each corrected program digest, correction counts and validated header/checksum.
The host rejects missing copies, mismatched interpretations and invalid counts;
no corrected SPL report can be substituted for ordinary kernel-corrected U-Boot.
The seventh RAM boot physically measured this revision: all eight live copies
match the original digest/checksum, maximum correction is seven bits per
codeword, and each whole block read/decode takes about 2.1 seconds. Corrected
U-Boot also matches the original digest with zero uncorrectable failures. It
still exposes no erase/write operation.

`boot0-recover-source` prepares a private restoration source from two preserved
captures. All eight copies must decode, agree byte-for-byte and match the
explicit recorded SHA256 before any output file is created. Publication is
atomic without overwrite, into an existing private directory. Cancellation or
validation failure publishes nothing. This diagnostic grants no write approval.

The `boot_trial` core module prepares the restricted original-SPL fallback test
for the exact sacrificial SID. Its preflight requires local RAM-only mounts,
no attached UBI, the pinned recovery kernel/board/NAND identity and measured
geometry, zero boot bad blocks, unchanged rootfs bad-block inventory, all four
backup SPL digests and the original kernel-corrected U-Boot digest. The recovery
caller must collect these observations locally and immediately before mutation;
host-provided claims cannot stand in for that collection.

The only diagnostic tool operations defined there are one-block primary SPL
erasure and raw data/OOB restoration to `/dev/mtd0`, without bad-block skipping
or automatic retry. Restoration takes a private snapshot only after exact
clean-image hash, all BBM bytes and native BCH decoding pass, then rechecks the
open snapshot before writing. Separate verification helpers require full raw
erasure or all four corrected original SPL copies. Tool acceptance alone is
not verification or success. The snapshot check is available as the read-only
`boot0-check-restoration` CLI command.

Protocol v4 connects these helpers through two closed requests. PrepareSplTrial
collects local preconditions, validates the exact restoration file and retains a
private snapshot, and issues a fresh 30-second ticket. For erasure it additionally
requires an intact primary. ExecuteSplTrial consumes that ticket on the same
connection before checking it; wrong tokens/operations, expiry or disconnect
cannot reuse it. The daemon freshly measures SID, geometry, mounts and the backup
boot chain and revalidates the open snapshot before recording dispatch. No host
report substitutes for device-local measurements.

Execute reception is the commit boundary. The device finishes its bounded operation
and readback even if the host disconnects. It erases exactly mtd0, verifies every raw
data/OOB byte is erased, optionally restores the fixed original encoding, and
rechecks backup SPL and U-Boot before issuing SplTrialVerified. A RAM boot permits
at most two dispatched attempts, including failed/indeterminate attempts; a
reconnect does not resume or retry them. Device journals are private and fsynced
but RAM-only, retained across connections and lost at reboot/power loss. Host
journals provide persistent diagnosis. Protocol v4 allows 60 seconds per transport
component with 500 ms cancellation polling, replacing the former 15-second bound.
Neither the ticket nor this diagnostic constitutes release-manifest approval.
`NandPlan` production authorization remains denied. Physical trial measurements
are still required; no v4 erase/write has been dispatched yet.

The host trial journal publishes a new private intent file atomically without
overwrite and fsyncs it before preparation or dispatch. It records public SID,
boot session, implementation digest, fixed image/program digests and a closed
operation enum; it never serializes the session HMAC key. Ordered records move
through Intent, Prepared and Dispatched. Verified requires the operation's
checked physical readback. A lost response after dispatch is Indeterminate;
cancellation/failure before dispatch is terminal and cannot become Verified.
An I/O failure poisons further journal transitions. The bounded reader rejects
partial/reordered/modified-schema records and grants no automatic resumption.
Unix parent-directory fsync is implemented; Windows directory durability remains
unmeasured. `trial-journal-read` provides read-only inspection. The journal is
connected to `recovery-spl-trial`: intent precedes preparation, fsynced dispatch
precedes the Execute frame, and exact verified readback precedes Verified.
`recovery-spl-preflight` prepares then disconnects without dispatch, dropping the
device ticket. No destructive request has been dispatched yet.

The v4 builder requires `--original-spl` pointing to the private, pinned clean
restoration candidate. It rejects changed bytes before output creation and puts
the image at one fixed private RAM path. Original executable firmware stays out
of Git. The candidate is independently decoded again inside recovery before any
trial. The template/daemon pins in `recovery_boot.rs` bind this complete payload.
