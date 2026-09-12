# Recovery and troubleshooting

Physical writes are blocked in this version. “Simulation complete” never means a
board has been flashed. GUI logs are bounded and can be copied explicitly; CLI errors
use nonzero exit status. Keep device SID and personal paths private when sharing logs.

| Symptom | Action |
| --- | --- |
| No accessible FEL candidate / external tool failed | Check data cable, FEL jumper, tool/libusb installation and platform USB permissions. A failed `--list` may mean absent or inaccessible USB. |
| Multiple candidates | Disconnect all other FEL devices, then start fresh. No automatic selection is offered. |
| Unknown SoC, SID or NAND | Stop. Do not force another board profile or use a Hynix fallback. |
| Physical recovery blocked | Expected: approved image and authenticated recovery protocol are not available. |
| Manifest rejected | Check exact schema/version, field names, eight roles, board/SoC/OS, HTTPS URLs and provenance. Commands and `vitrallis: ready` are forbidden. |
| Hash or length mismatch | Stop using that asset. Compare against a trusted manifest and obtain a complete new copy. |
| Stale/corrupt cache | Use a new private cache directory, or independently inspect the exact hash-named file before removing it. The app will not overwrite an existing corrupt entry. |
| Unsafe path | Use a local private directory with canonical path, no symlink components and regular files. Unix cache directories cannot be group/other-writable. |
| Download fails or stalls | Check server availability and trusted TLS configuration. Idle socket bound is five seconds; no certificate-verification bypass exists. Use a reviewed offline set when appropriate. |
| Cancelled or expired confirmation | Begin a new session and full preflight. Old confirmation is invalid. |
| Mock process/write/verify failure | Recovery state is expected. There is no automatic destructive retry or success report. |

For a future validated physical installation, a failure after erase may leave NAND
unbootable. Do not reboot automatically or disconnect mid-write; preserve logs and
follow the reviewed recovery-image procedure. Re-enter FEL with the jumper and run a
fresh preflight using a complete known-good image. Keep unrelated FEL devices absent.
A normal retry must independently rediscover NAND and verify every payload. No routine
can promise power-failure atomicity across NAND bootloader/rootfs erasure.

Only after real readback verification and clean unmount should the future application
instruct removal of the jumper and a power-cycle. No physical validation, jumper
manipulation, USB driver change or reboot was performed in this implementation.
