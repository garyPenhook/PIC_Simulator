#!/usr/bin/env python3
"""Install release binaries and device descriptions to a user prefix atomically."""
import argparse
import os
from pathlib import Path
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def copy_atomic(source, destination, mode):
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f'.{destination.name}.', dir=destination.parent)
    try:
        with os.fdopen(fd, 'wb') as output, source.open('rb') as input_file:
            shutil.copyfileobj(input_file, output)
        os.chmod(temporary, mode)
        os.replace(temporary, destination)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, default=Path.home() / '.local')
    args = parser.parse_args()
    prefix = args.prefix.expanduser().resolve()
    binaries = [ROOT / 'target/release' / name for name in ('basil-sim', 'pic-sim', 'basil-sim-dap')]
    devices = sorted((ROOT / 'devices').glob('*.toml'))
    documentation = [ROOT / 'docs' / name for name in ('PicSim_Manual.pdf', 'MANUAL.md', 'DAP.md', 'FACTS-SIM.md')]
    for source in binaries + devices + documentation:
        if not source.is_file():
            raise SystemExit(f'Missing {source}; build the release workspace and manual first')
    for source in binaries:
        destination = prefix / 'bin' / source.name
        copy_atomic(source, destination, 0o755)
        print(destination)
    for source in devices:
        copy_atomic(source, prefix / 'share/picsim/devices' / source.name, 0o644)
    for source in documentation:
        copy_atomic(source, prefix / 'share/doc/picsim' / source.name, 0o644)
    print(f'Installed {len(devices)} device descriptions and {len(documentation)} documents')


if __name__ == '__main__':
    main()
