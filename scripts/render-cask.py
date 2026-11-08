#!/usr/bin/env python3
"""Fill in the Homebrew cask: render-cask.py <version> <SHA256SUMS.txt> > spotseek.rb"""
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    version, sums_path = sys.argv[1], Path(sys.argv[2])
    sums = {}
    for line in sums_path.read_text().splitlines():
        parts = line.split()
        if len(parts) == 2:
            sums[parts[1].lstrip('*')] = parts[0].lower()
    cask = (ROOT / 'packaging/homebrew/spotseek.rb.in').read_text()
    for key, asset in [('SHA_ARM64', 'Spotseek-macOS-arm64.zip'), ('SHA_X64', 'Spotseek-macOS-x64.zip')]:
        if asset not in sums:
            sys.exit(f'{sums_path} has no line for {asset}')
        cask = cask.replace(f'@{key}@', sums[asset])
    sys.stdout.write(cask.replace('@VERSION@', version))


if __name__ == '__main__':
    main()
