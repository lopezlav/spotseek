#!/usr/bin/env python3
"""Download the pinned, checksum-verified release tools (build time only)."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    manifest = json.loads((ROOT / 'packaging/tools.json').read_text())
    if len(sys.argv) != 3 or sys.argv[2] not in manifest['platforms']:
        sys.exit('usage: fetch-tools.py <output-dir> <darwin-arm64|darwin-x64|win-x64>')
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for asset in manifest['platforms'][sys.argv[2]]:
        with tempfile.TemporaryDirectory() as tmp:
            download = Path(tmp) / 'download'
            subprocess.run(['curl', '-fL', '--retry', '3', '-o', str(download), asset['url']], check=True)
            data = download.read_bytes()
            if hashlib.sha256(data).hexdigest() != asset['sha256']:
                sys.exit(f"Checksum mismatch: {asset['url']}")
            if 'member' in asset:
                with zipfile.ZipFile(io.BytesIO(data)) as archive:
                    data = archive.read(asset['member'])
            target = out / asset['name']
            target.write_bytes(data)
            target.chmod(0o755)
            print(f'Verified {target}', flush=True)


if __name__ == '__main__':
    main()
