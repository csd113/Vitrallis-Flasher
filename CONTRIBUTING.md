# Contributing

Keep changes focused, preserve user edits and do not create commits without an
explicit request. Core code must stay independent of host `/sys`, udev, apt, sudo,
losetup and mkfs.ubifs. Use structured argv, bounded input and fail-closed types.
Do not add dependencies without checking the standard library and existing crates.
No production `unwrap`, `expect`, panic, arbitrary manifest commands or unsafe Rust.

Use the pinned Rust toolchain, Python 3.11 or newer for build scripts, and committed Cargo.lock. Linux GUI builds require
X11/Wayland, xkbcommon, GL/EGL headers and pkg-config; the CI setup script lists the
packages. Windows builds use MSVC. macOS builds use Xcode command-line tools.
`python scripts/ci.py setup` is intended for disposable CI runners, not end-user setup.

Run these checks before handoff:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::all -D clippy::pedantic -D clippy::nursery -D clippy::cargo
cargo test --workspace --all-features
cargo build --workspace --all-features --release --locked
python3 -m unittest discover -s images -p test_*.py
python3 -m unittest discover -s scripts -p test_*.py
python3 scripts/provenance.py check
python3 scripts/provenance.py check-kernel-config
python3 images/build.py plan
```

The full host-side image build additionally needs Docker, the pinned builder
image and the locked assets; it is explicit and never part of routine checks:

```text
python3 images/assemble.py fetch-assets --assets work/assets
python3 images/assemble.py build --assets work/assets --output work/batch1/set --work work/batch1/work
python3 images/assemble.py verify --set work/batch1/set
python3 images/assemble.py reproduce --assets work/assets --output work/batch1/repro --work work/batch1/repro-work
```

If formatting fails, format only touched packages, then rerun the check. Tests live
beside their behavior. The six ignored child fixture tests are deliberately invoked
by the `ToolRunner`/process tests; they are not untested feature coverage. Do not run
all ignored tests directly: one deliberately fails, one exits non-zero, and another
emits excessive output. New host seams must keep the `VerifiedAssets` planning gate,
the scripted failure-injection harness and the no-physical-executor guarantee.

`python scripts/ci.py validate` runs equivalent checks with a locked dependency graph.
`python scripts/ci.py package` creates host-specific unsigned local artifacts with
checksums and dependency notices. It neither signs nor publishes a release.

Before enabling any physical operation, complete every gate in
[physical validation](docs/physical-validation.md), close the reviewed license/image
blockers, and review the resulting changes. Do not substitute physical flashing for
mock tests, silently bypass identity checks, or label simulation as hardware evidence.
