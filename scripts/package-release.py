#!/usr/bin/env python3
"""Package rebuilt Linux release binaries, documentation and the VS Code extension."""
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def main():
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise SystemExit('This package targets Linux x86_64; build on that platform')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    name = f'picsim-{version}-linux-x86_64'
    output = ROOT / 'target' / 'dist' / version
    output.mkdir(parents=True, exist_ok=True)
    subprocess.run([sys.executable, str(ROOT / 'scripts/package-vscode.py')], check=True)
    extension = json.loads((ROOT / 'tools/vscode-picsim/package.json').read_text())
    vsix = f"{extension['name']}-{extension['version']}.vsix"
    with tempfile.TemporaryDirectory(prefix='picsim-package-') as temporary:
        bundle = Path(temporary) / name
        bundle.mkdir()
        for binary in ('pic-sim', 'basil-sim', 'basil-sim-dap'):
            shutil.copy2(ROOT / 'target/release' / binary, bundle / binary)
        shutil.copytree(ROOT / 'devices', bundle / 'devices')
        (bundle / 'docs').mkdir()
        for document in ('PicSim_Manual.pdf', 'MANUAL.md', 'DAP.md', 'FACTS-SIM.md',
                         'REGRESSION-2026-11-17.md', f'RELEASE-{version}.md'):
            shutil.copy2(ROOT / 'docs' / document, bundle / 'docs' / document)
        examples = bundle / 'examples'
        examples.mkdir()
        for example in ('blink_q10.bas', 'blink.stim.toml'):
            shutil.copy2(ROOT / 'tests/sim' / example, examples / example)
        (bundle / 'README.txt').write_text(
            f'PicSim {version}\n\n'
            'Run ./pic-sim for the GUI or ./basil-sim --help for the CLI.\n'
            'Keep devices/ beside the binaries. See docs/ for setup and limits.\n'
            'The examples require the separately installed Basil compiler.\n'
            'Linux x86_64 binaries require compatible system libraries; the GUI\n'
            'also requires a graphical desktop. See the release notes.\n')
        archive = output / f'{name}.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            tar.add(bundle, arcname=name)
    shutil.copy2(ROOT / 'docs/PicSim_Manual.pdf', output / 'PicSim_Manual.pdf')
    shutil.copy2(ROOT / 'target' / vsix, output / vsix)
    assets = [archive, output / 'PicSim_Manual.pdf', output / vsix]
    checksums = []
    for asset in assets:
        with asset.open('rb') as stream:
            checksums.append(f'{hashlib.file_digest(stream, "sha256").hexdigest()}  {asset.name}\n')
    (output / 'SHA256SUMS').write_text(''.join(checksums))
    print(output)


if __name__ == '__main__':
    main()
