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
permissions. Vitrallis-Shell had no license grant at the reviewed commits; an MIT
LICENSE appears at later HEAD, but its released binaries embed artwork whose rights
upstream itself documents as unresolved. No Shell implementation source files,
assets, bundle or installer scripts are bundled here. Documentation quotes its
one-line command and short optional startup recipe, with links to the reviewed
upstream instructions.

The initial Vitrallis-Flasher repository contained only a README and no license grant.
Project package metadata uses `LicenseRef-Proprietary` as a conservative marker; it
is not a new open-source license grant. The owner can choose project licensing
separately. CI creates workflow artifact downloads, not GitHub releases;
artifact access follows the repository's visibility and Actions permissions.

The desktop package includes `dependency-licenses.json`, available registry license
texts, Cargo.lock and checksums. Review these against actual linked target dependencies
before public distribution. Rust and native platform dependencies retain their own
licenses. No sunxi-fel/libusb executable or driver is silently downloaded or bundled.

## Batch 0 component findings (2026-09-25)

The detailed per-artifact status is in [`upstream-lock.json`](../upstream-lock.json);
this section records the observed license facts only. Where no grant was found, the
status is `UNRESOLVED — redistribution status not established`; that is not a claim
that redistribution is forbidden.

| Component | Observed license facts | Redistribution status |
| --- | --- | --- |
| U-Boot (`u-boot/u-boot` `v2022.01`) | GPL-2.0-or-later | source is redistributable under GPL |
| `x-chip-uboot` | No root license file; it patches and configures GPL U-Boot | UNRESOLVED for the repository's own files; built binaries are GPL-2.0-or-later |
| `x-chip-linux-deb` | `LICENSE` is CC0-1.0 (repository tooling); patches apply to GPL-2.0-only Linux source | UNRESOLVED for patch licensing; kernel binaries remain GPL-2.0-only |
| `x-chip-deb-repo` | No root license; `packaging/tic80/debian/copyright` covers only that package | UNRESOLVED; per-package copyrights govern built debs |
| `CHIP-dt-overlays` | `debian/copyright` grants MIT terms (`Files: *`) | MIT |
| `x-chip-os` | No root license file | UNRESOLVED for image contents; per-package licenses apply |
| `x-chip-tools` | No root license file; committed throwaway installer SSH key | UNRESOLVED |
| `sunxi-tools` | `LICENSE.md` GPL-2.0-or-later | GPL-2.0-or-later with source-offer obligations |
| `Vitrallis-Shell` | No license grant at the reviewed commits (`40b2327`, `02df65b`); an MIT `LICENSE` appears at later HEAD `3146f8f` | UNRESOLVED for the reviewed revision; never bundled |
| Debian packages inside the rootfs | Per-package `copyright` files; aggregate contains GPL/LGPL/BSD/MIT components | Must be redistributed with notices and source offers where required |

An aggregate Debian rootfs image carries the union of its packages' obligations.
Before Vitrallis-Flasher distributes the rootfs, U-Boot binaries or any built image,
it must resolve the `UNRESOLVED` entries above, include the required license texts
and notices, and provide corresponding source for GPL-family components using a
compliant source-offer method.
