#!/usr/bin/env python3
"""Select a Debian 13 PocketCHIP recovery profile. Physical upgrades remain blocked.

Run on the host beside the packaged flasher. Stock PocketHome is the default.
--plan is read-only; --simulate uses the CLI's verified mock recovery state machine.
This is not an in-place apt upgrade and cannot preserve an existing NAND filesystem.
"""
import argparse
import importlib.util
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def image_plan(profile):
    source = ROOT / 'images/build.py'
    spec = importlib.util.spec_from_file_location('flasher_image_build', source)
    if spec is None or spec.loader is None:
        raise ValueError('packaged image planner is missing')
    builder = importlib.util.module_from_spec(spec)
    # The bundled planner imports its sibling storage policy; no PATH command lookup.
    sys.path.insert(0, str(source.parent))
    try:
        spec.loader.exec_module(builder)
    finally:
        sys.path.pop(0)
    return builder.plan(builder.load_inputs(ROOT / 'images/inputs.lock.json'), profile)


def simulation_command(profile):
    if profile not in ('stock', 'vitrallis-default'):
        raise ValueError('unknown installation profile')
    name = 'flasher-cli.exe' if os.name == 'nt' else 'flasher-cli'
    # No PATH search or arbitrary executable parameter. Development builds are local.
    for path in (ROOT / name, ROOT / 'target/release' / name, ROOT / 'target/debug' / name):
        if path.is_file() and not path.is_symlink():
            return [str(path.resolve(strict=True)), 'simulate', '--profile', profile]
    raise ValueError('flasher-cli is missing; extract the whole package or run cargo build -p flasher-cli')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=('stock', 'vitrallis-default'), default='stock')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--plan', action='store_true', help='show the selected profile and remaining release gates')
    mode.add_argument('--simulate', action='store_true', help='exercise mock recovery; requires typed ERASE confirmation')
    args = parser.parse_args(argv)
    if args.simulate:
        print('SIMULATION ONLY. No device OS, files or startup settings will change.', flush=True)
        return subprocess.run(simulation_command(args.profile), check=False, timeout=600).returncode
    print(json.dumps(image_plan(args.profile), indent=2))
    if args.plan:
        return 0
    print('Upgrade blocked: no approved physical image or authenticated recovery protocol. '
          'No USB access, downloads or device changes were attempted. '
          'Use --plan to review or --simulate to test the flow.', file=sys.stderr)
    return 2


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
    except KeyboardInterrupt:
        sys.exit(130)
