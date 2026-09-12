# Debian 13 upgrade profiles

The project offers two explicit desktop choices. **Stock PocketHome is the default**;
users do not need Vitrallis to use the planned Debian 13 image. Both physical paths
remain unavailable until the image and recovery release gates pass. The script and
wizard currently provide a read-only plan and a complete mock recovery run.

## Host upgrade script

Run `scripts/upgrade_debian13.py` from the source checkout or extracted platform ZIP
on the host computer. It needs Python 3; it never invokes apt, sudo, shell strings,
remote installer scripts or administrative USB changes.

```text
python3 scripts/upgrade_debian13.py --plan
python3 scripts/upgrade_debian13.py --profile vitrallis-default --plan
python3 scripts/upgrade_debian13.py --simulate
python3 scripts/upgrade_debian13.py --profile vitrallis-default --simulate
```

Without either mode flag it returns exit 2 with the release blockers, before USB
access, process execution or downloads. `--plan` exits 0 while explicitly reporting
`status: blocked`. `--simulate` invokes the adjacent/local built CLI with structured
arguments, preserves interactive confirmation and propagates the CLI result. No
`--force`, arbitrary command or physical bypass exists. In source builds, run
`cargo build -p flasher-cli` first; extracted packages already include that executable.

CLI equivalents are `flasher-cli simulate --profile stock` and
`flasher-cli simulate --profile vitrallis-default`. A physical request such as
`flasher-cli upgrade --profile stock` returns a failure before device access.
Changing desktop choice requires a fresh preflight and its corresponding manifest
confirmation. The GUI exposes the choice at Welcome and repeats it at confirmation.

## What stock preservation means

The stock image must retain PocketHome/Marshmallow as the normal desktop, its menu
and launcher behavior, the existing Awesome home-screen startup, hardware controls,
calibration and compatible stock applications. It must not contain Vitrallis binaries,
shortcuts or startup hooks. The image build plan carries these requirements separately
from Vitrallis release gates; the absence of a Shell bundle cannot force users toward
or block the stock profile on its own.

Debian 13 requires a modern OS and package set. This is not a promise that every
Jessie-era package, proprietary app or original kernel can remain unchanged. The
reviewed candidate contains PocketHome, but preservation of the entire stock
experience still needs an application inventory and device validation.

A FEL recovery reimage erases NAND. It cannot retain the current filesystem in place.
No supported in-place Jessie-to-trixie migration or automatic backup/restore is
implemented here. Before any future physical release, provide a separately verified
backup and restoration procedure for documents, app data, menu customizations and
calibration; do not blindly restore old system files over Debian 13. Current releases
make no data-preservation claim and cannot erase a device.

## Optional Vitrallis default

The `vitrallis-default` profile requires all stock-image gates plus a complete,
compatible Vitrallis Shell/Terminal/Notepad/Files bundle and matching installer,
session helper and uninstaller. Installation must use the reviewed normal-user
integration contract. PocketHome, its menu/assets and `launch_home_screen()` remain
available; login, calibration and recovery services are not replaced.

The upstream bootstrap linked in [README](../README.md#install-vitrallis-separately)
installs Vitrallis alongside Marshmallow; it does not enable automatic startup. The
optional profile must additionally back up the current `~/.config/awesome/rc.lua`,
validate the existing configuration, and append exactly the upstream block after the
existing startup code:

```lua
-- BEGIN optional Vitrallis startup
require('gears').timer.start_new(5, function()
    require('awful').spawn({os.getenv('HOME') .. '/.local/share/vitrallis/launch'}, false)
    return false
end)
-- END optional Vitrallis startup
```

This opens Vitrallis after the existing home screen starts. A failed launch leaves
Marshmallow available. The reviewed upstream uninstaller recognizes this exact block
and removes it while preserving surrounding edits. Modified or duplicate blocks must
require reconciliation rather than overwriting user configuration. Never replace the
entire Awesome configuration or suppress its home-screen startup.

This block is a documented build/integration contract, **not an executed hook in this
prototype**. The optional physical profile remains blocked until fresh device tests
cover installation, boot into Vitrallis, launch failure, Marshmallow selection, Home
key restoration and offline removal. The published standalone beta2.5 binaries
cannot substitute for the pending complete release bundle.

See the [reviewed upstream startup instructions](https://github.com/csd113/Vitrallis-Shell/blob/02df65ba08e694777031fe3bd7a0b8274ddf4e45/docs/devices/pocketchip.md#optional-startup),
[image build gates](image-build.md) and [physical validation checklist](physical-validation.md).
