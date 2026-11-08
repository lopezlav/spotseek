#!/usr/bin/env python3
"""Collect upstream notices and exact source references into each installer."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
out = Path(sys.argv[1])
platform = sys.argv[2]
out.mkdir(parents=True, exist_ok=True)
manifest = json.loads((root / 'packaging/tools.json').read_text())
v = manifest['versions']
ffplatform = 'win32-x64' if platform == 'win-x64' else platform
urls = {
    'ffmpeg-LICENSE.txt': f"https://github.com/eugeneware/ffmpeg-static/releases/download/{v['ffmpeg']}/{ffplatform}.LICENSE",
    'ffmpeg-README.txt': f"https://github.com/eugeneware/ffmpeg-static/releases/download/{v['ffmpeg']}/{ffplatform}.README",
    'yt-dlp-LICENSE.txt': f"https://raw.githubusercontent.com/yt-dlp/yt-dlp/{v['ytdlp']}/LICENSE",
    'yt-dlp-THIRD-PARTY.txt': f"https://raw.githubusercontent.com/yt-dlp/yt-dlp/{v['ytdlp']}/THIRD_PARTY_LICENSES.txt",
    'deno-LICENSE.txt': f"https://raw.githubusercontent.com/denoland/deno/{v['deno']}/LICENSE.md",
}
for name, url in urls.items():
    subprocess.run(['curl', '-fsSL', '--retry', '3', '-o', str(out / name), url], check=True)
shutil.copyfile(root / 'THIRD_PARTY_NOTICES.md', out / 'THIRD_PARTY_NOTICES.md')
shutil.copyfile(root / 'packaging/tools.json', out / 'tools.json')
if (root / 'LICENSE').exists():
    shutil.copyfile(root / 'LICENSE', out / 'Spotseek-LICENSE.txt')
# The Rune UI fonts.
fonts = out / 'fonts'
fonts.mkdir(parents=True, exist_ok=True)
for item in (root / 'ui/assets').glob('*-LICENSE.txt'):
    shutil.copyfile(item, fonts / item.name)
# The app window (ui) and the engine (engine) are both Rust: every crate of both.
packages = {}
members = set()
for manifest_path in ['ui/Cargo.toml', 'engine/Cargo.toml']:
    meta = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--format-version', '1', '--locked', '--manifest-path', str(root / manifest_path)],
        text=True))
    members |= set(meta['workspace_members'])
    for pkg in meta['packages']:
        packages[pkg['id']] = pkg
crates = []
for pkg in packages.values():
    if pkg['id'] in members:
        continue
    label = f"{pkg['name']}@{pkg['version']}"
    crates.append(f"{label} ({pkg.get('license') or 'see files'})")
    # A crate in a git workspace (Rune) keeps its license at the workspace root.
    folder = Path(pkg['manifest_path']).parent
    for d in [folder, *list(folder.parents)[:3]]:
        found = [i for i in d.iterdir() if i.is_file() and i.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE'))]
        if found:
            dest = out / 'rust' / label
            dest.mkdir(parents=True, exist_ok=True)
            for item in found:
                shutil.copyfile(item, dest / item.name)
            break
(out / 'rust-crates.txt').write_text('\n'.join(sorted(crates)) + '\n')
(out / 'SOURCES.txt').write_text('\n'.join([
    'Spotseek: https://github.com/lopezlav/spotseek (see the matching release tag)',
    f"yt-dlp: https://github.com/yt-dlp/yt-dlp/tree/{v['ytdlp']}",
    f"Deno: https://github.com/denoland/deno/tree/{v['deno']}",
    f"FFmpeg binaries and build notices: https://github.com/eugeneware/ffmpeg-static/releases/tag/{v['ffmpeg']}",
    'FFmpeg source/build provenance: see ffmpeg-README.txt and https://github.com/eugeneware/ffmpeg-static#sources-of-the-binaries',
]) + '\n')
