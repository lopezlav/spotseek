#!/usr/bin/env python3
"""Exercise installed binaries without using developer-installed dependencies."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

bin_dir = Path(sys.argv[1]).resolve()
app = Path(sys.argv[2]).resolve()
ext = '.exe' if sys.platform == 'win32' else ''
env = os.environ.copy()
system_path = str(Path(os.environ['SystemRoot']) / 'System32') if ext else '/usr/bin:/bin'
env['PATH'] = str(bin_dir) + os.pathsep + system_path

def run(args):
    result = subprocess.run([str(a) for a in args], env=env, check=False,
                            capture_output=True, text=True, timeout=60)
    print((result.stdout + result.stderr)[:2000], flush=True)
    result.check_returncode()
    return result.stdout

for tool, flag in [('sockseek', '--help'), ('yt-dlp', '--version'), ('deno', '--version'),
                   ('ffmpeg', '-version'), ('ffprobe', '-version')]:
    run([bin_dir / (tool + ext), flag])
run([app, '--cli', '--help'])
with tempfile.TemporaryDirectory() as tmp:
    mp3 = Path(tmp) / 'smoke.mp3'
    run([bin_dir / ('ffmpeg' + ext), '-v', 'error', '-f', 'lavfi', '-i',
         'sine=frequency=440:duration=0.2', '-c:a', 'libmp3lame', '-b:a', '320k', mp3])
    probe = json.loads(run([bin_dir / ('ffprobe' + ext), '-v', 'error', '-show_streams', '-of', 'json', mp3]))
    assert probe['streams'][0]['codec_name'] == 'mp3', probe
print('Packaged app and all bundled tools passed the smoke test.')
