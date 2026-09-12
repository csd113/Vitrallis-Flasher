# Upstream licenses and distribution boundary

sunxi-tools declares GPL version 2 or later. Its source, objects and binary are not
copied into, linked to, or shipped with this application. `SunxiTool` invokes only a
separately selected external executable through a narrow read-only argv boundary.
The implementation parses its documented diagnostic output and attributes the source;
it does not translate or embed its FEL implementation.

Before any future bundling, review the exact build and license obligations, include
its notices and GPL text, and provide the corresponding source and build materials
using a compliant distribution method. Merely linking to a moving repository or
adding a license name is not a sufficient binary redistribution process.
[Exact upstream license](https://github.com/linux-sunxi/sunxi-tools/blob/d7bbd172a5da601a08f94479de308c6fb714a19a/LICENSE.md).

No root license grant was found in the pinned x-chip-tools or x-chip-os repositories.
Their contents remain reference-only in ignored local review checkouts. No flashing
script, initramfs code or live-build configuration is vendored. Permissions and the
licenses of kernel, U-Boot, Dropbear, packages and image contents need review before
adapting or distributing an installer/image. Public visibility does not settle reuse
permissions. Vitrallis-Shell also identifies its owned package as proprietary; no
Shell implementation source files, assets, bundle or installer scripts are bundled
here. Documentation quotes its one-line command and short optional startup recipe,
with links to the reviewed upstream instructions.

The initial Vitrallis-Flasher repository contained only a README and no license grant.
Project package metadata uses `LicenseRef-Proprietary` as a conservative marker; it
is not a new open-source license grant. The owner can choose project licensing
separately. CI creates workflow artifact downloads, not GitHub releases;
artifact access follows the repository's visibility and Actions permissions.

The desktop package includes `dependency-licenses.json`, available registry license
texts, Cargo.lock and checksums. Review these against actual linked target dependencies
before public distribution. Rust and native platform dependencies retain their own
licenses. No sunxi-fel/libusb executable or driver is silently downloaded or bundled.
