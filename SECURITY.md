# Security

Production writes remain disabled in the real backend. The separate Batch 3
original-SPL diagnostic accepts only a measured sacrificial SID, fixed mtd0
operations and an exact original-program restoration image. Device-local
preflight, single-use connection tickets and fsynced journals precede mutation;
checked readback precedes success. It does not authorize release installation. No manifest field, environment variable or GUI control can enable production
installation. Native FEL uses fixed reviewed diagnostic ranges; RAM bootstrap
invokes the exact pinned utility with structured, code-controlled arguments.
The application never invokes upstream flashing shell scripts.

## Trust boundaries

Manifests, downloads, cache contents, device output, filesystem paths and restored
images are untrusted. Parsing limits precede allocation or use. Unknown JSON fields,
duplicate fields, wrong board/SoC/OS, unknown protocol versions and incomplete
inventories are rejected. URLs cannot contain credentials, insecure schemes, local
hostnames, literal addresses or unusual ports. Every redirect is revalidated; the
network connector rejects private/reserved resolved addresses before connection.
TLS verification is mandatory. Proxy environment variables are intentionally ignored.

A checksum establishes integrity relative to the selected manifest, not publisher
identity. There is **no approved physical manifest**. `upstream-lock.json` records
review evidence; editing it does not enable the backend. A future physical catalog
must pin whole-manifest digests in the reviewed application or verify a signed
catalog using an embedded trust root. Release rollback/version policy also needs
review before any such catalog can be enabled.

Asset writes use private temporary files, exact-size checks, SHA-256 and atomic
no-clobber publication. Existing cache entries are fully rechecked. Verified handles
hold private copies and are rehashed immediately before the simulated recovery boot.
A cache filename cannot redirect the operation to unverified bytes. No archives are
extracted on the host. Cancellation/failure drops temporary snapshots and invalidates
the session. A hard process kill can leave temporary cache files, but these are never
recognized as verified hits. Corrupt hash-named cache entries fail closed.

Batch 2 planning consumes only `VerifiedAssets` plus a validated
`IdentifiedTarget`; filesystem paths and unverified bytes cannot produce a
`NandPlan`. The plan has no executor, `authorize_execution` always fails, and the
Batch 2 unavailable FEL implementation remains a fail-closed seam; Batch 3
native FEL diagnostics and pinned recovery bootstrap are separate reviewed paths. Scripted
tool/FEL/HTTP/clock doubles exist for tests and record exact calls; they cannot be
enabled by a manifest, flag or environment variable. External tools run only
through the bounded `ToolRunner`, and session TTLs read only the injected monotonic
clock.

Cache/offline paths reject symlink components and nonregular files. Use a cache owned
by the current user. Unix group/other-writable cache directories are refused. Windows
ACL ownership enforcement is not implemented; administrators/users must provide a
private directory. The design does not defend against another process with the same
user's privileges concurrently modifying private handles, replacing executables, or
changing filesystem ancestors. Network-mounted filesystems may have weaker atomicity
and blocking behavior; use a local filesystem.

The GUI has a bounded event queue and log. Executable output is bounded and not echoed
as terminal control sequences. No network calls occur at startup or during simulation.
An explicit fetch contacts the manifest's servers and reveals normal HTTP/TLS metadata.
Copying logs is a local user action. Logs can contain device SID and local I/O paths.

## Reporting

Use the repository's private security-reporting channel if enabled, or contact the
repository owner privately before filing exploit details publicly. Include the
commit, platform, a minimal non-destructive reproduction and sanitized logs. Never
attach credentials, personal rootfs contents or a destructive proof on hardware.
