# Dependency decisions

The starting repository had no crates. These dependencies supply capabilities the
standard library cannot safely implement alone:

| Crate | Reason |
| --- | --- |
| serde / serde_json | Strict versioned contract parsing, duplicate/unknown-field rejection |
| sha2 | Reviewed SHA-256 implementation; no custom cryptography |
| thiserror | Typed, source-preserving error reporting |
| url | Standards-based HTTPS/redirect parsing |
| ureq + rustls | Bounded verified TLS downloads, with decompression and proxy use disabled |
| tempfile | Secure temporary file creation and atomic no-clobber publication |
| eframe / egui | Portable native window, input, font and rendering support without a browser |
| ctrlc | Safe portable CLI cancellation without custom signal-handler unsafe code |
| rusb 0.9.4 / libusb | Safe USB access for explicit FEL framing, bounded transfers and BROM status checking |

Batch 3 adds rusb because neither the standard library nor an existing crate
provides USB access. The pinned sunxi-fel utility discards BROM status, so it
cannot implement the required fail-closed status boundary without modification.
The native transport uses only safe rusb APIs; workspace unsafe code remains
forbidden. The vendored feature builds libusb for portable builds; a C compiler
is required. libusb's LGPL license and corresponding-source/relinking obligations
must be included in a distributed native package. The previously pinned external
sunxi-fel remains available as an explicitly selected diagnostic alternative.

ureq is pinned exactly to 3.4.1 because its transport-extension API is explicitly
unversioned. A small wrapper places a five-second bound on each socket operation
while preserving the overall transfer deadline. It also checks cancellation. A
connector rejects private/reserved DNS answers before opening a connection. TLS
remains provided by upstream rustls; no verification override is used. Test these
boundaries and inspect the upstream API before changing ureq.

`clippy.toml` permits specific duplicate **crate names**, not a lint group. The GUI's
winit/glutin/clipboard/Wayland adapters currently require incompatible ABI generations
of objc2/block2, smithay/calloop, rustix, Windows bindings and several support crates.
Their ranges cannot be unified by selecting a single version. The proc-macro ecosystem
is transitioning from syn 2 to 3; rustls's ring and tempfile use distinct getrandom
families. png and flate2 use different miniz_oxide versions. These are transitive
exceptions, listed explicitly in the configuration; all other cargo, pedantic,
nursery and correctness checks remain enabled. Reevaluate the exceptions when updating
the lock file. Do not pin old insecure libraries merely to eliminate duplicate names.

The GUI uses the GL renderer and embedded fonts. Optional WebGPU dependencies may
appear in Cargo.lock/metadata but are not enabled in the native build. Core and CLI do
not depend on the GUI crates. No SDL2, system libusb, web runtime, database, SSH library
or host filesystem-generation tool is required for simulation and asset verification.


The recovery protocol directly uses the already locked ring 0.17.14 dependency
for operating-system randomness and constant-time HMAC verification. No custom
cryptography was added. The ARMv7 recovery compiler image pins Rust 1.99.0 and
a signed Debian snapshot in `recovery/Dockerfile`; cross libc headers are required
alongside the GCC cross compiler. Native USB is an optional core feature so the
device daemon does not carry desktop libusb.
