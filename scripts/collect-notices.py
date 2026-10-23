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
    'sockseek-LICENSE.txt': f"https://raw.githubusercontent.com/fiso64/sockseek/{v['sockseek']}/LICENSE",
}
for name, url in urls.items():
    subprocess.run(['curl', '-fsSL', '--retry', '3', '-o', str(out / name), url], check=True)
shutil.copyfile(root / 'THIRD_PARTY_NOTICES.md', out / 'THIRD_PARTY_NOTICES.md')
shutil.copyfile(root / 'packaging/tools.json', out / 'tools.json')
if (root / 'LICENSE').exists():
    shutil.copyfile(root / 'LICENSE', out / 'Spotseek-LICENSE.txt')
# Include the notices from all Go modules, including indirect dependencies.
subprocess.run(['go', 'mod', 'download'], cwd=root, check=True)
raw = subprocess.check_output(['go', 'list', '-m', '-json', 'all'], cwd=root, text=True)
decoder = json.JSONDecoder()
modules = []
while raw.strip():
    module, end = decoder.raw_decode(raw.lstrip())
    raw = raw.lstrip()[end:]
    if module.get('Main') or not module.get('Dir'):
        continue
    label = module['Path'] + '@' + module['Version']
    modules.append(label)
    dest = out / 'go' / label.replace('/', '_')
    for item in Path(module['Dir']).iterdir():
        if item.is_file() and item.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE', 'AUTHORS')):
            dest.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(item, dest / item.name)
(out / 'go-modules.txt').write_text('\n'.join(modules) + '\n')
(out / 'SOURCES.txt').write_text('\n'.join([
    'Spotseek: https://github.com/lopezlav/spotseek (see the matching release tag)',
    f"Sockseek: https://github.com/fiso64/sockseek/tree/{v['sockseek']}",
    f"yt-dlp: https://github.com/yt-dlp/yt-dlp/tree/{v['ytdlp']}",
    f"Deno: https://github.com/denoland/deno/tree/{v['deno']}",
    f"FFmpeg binaries and build notices: https://github.com/eugeneware/ffmpeg-static/releases/tag/{v['ffmpeg']}",
    'FFmpeg source/build provenance: see ffmpeg-README.txt and https://github.com/eugeneware/ffmpeg-static#sources-of-the-binaries',
]) + '\n')
