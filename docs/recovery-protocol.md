# LIVE recovery protocol gate

The host should reuse proven x-chip-tools NAND/SPL behavior through an external,
reviewed recovery boundary. It must not invent host-side ECC or use the broken
fastboot/SLC path. The current upstream installer is **not** an implementation of
this proposed protocol. Batch 3 now has a bounded read-only client and daemon; physical endpoint authentication remains unproven.

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
