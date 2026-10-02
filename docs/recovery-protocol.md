# LIVE recovery protocol gate

The host should reuse proven x-chip-tools NAND/SPL behavior through an external,
reviewed recovery boundary. It must not invent host-side ECC or use the broken
fastboot/SLC path. The current upstream installer is **not** an implementation of
this proposed protocol. No physical protocol client or device daemon is shipped.

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
execution and memory/status readback; its production default `UnavailableFel`
refuses every operation and only `ScriptedFel` answers in tests.
`IdentifiedTarget::identify` enforces the closed board/SoC/NAND decision and rejects
ambiguous or unknown fixtures. `NandPlan` records the ordered operations, exact
lengths/digests and readback checks that steps 5-9 require, with the unresolved
physical questions attached as hard `PlanGate`s; `authorize_execution` always fails.
`VerifiedAssets` supplies only manifest-verified bytes. Steps 4-10 still require the
reviewed protocol, device daemon, session authentication and hardware validation;
nothing in Batch 2 executes a plan or contacts a device.
