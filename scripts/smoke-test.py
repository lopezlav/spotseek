#!/usr/bin/env python3
"""Exercise installed binaries without using developer-installed dependencies."""
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import wave

bin_dir = Path(sys.argv[1]).resolve()
app = Path(sys.argv[2]).resolve()          # the app window (Rune UI)
backend = Path(sys.argv[3]).resolve()      # the engine it runs (Rust)
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

for tool, flag in [('yt-dlp', '--version'), ('deno', '--version'), ('ffmpeg', '-version')]:
    run([bin_dir / (tool + ext), flag])
run([backend, '--cli', '--help'])
assert run([backend, '--version']).startswith('spotseek-backend ')
# The window app starts and knows its screens (no GPU or display needed for this).
assert 'downloading' in run([app, '--list-scenes']).split()
with tempfile.TemporaryDirectory() as tmp:
    # A short 24-bit stereo tone, made here (the bundled ffmpeg has no test sources).
    wav = Path(tmp) / 'smoke.wav'
    with wave.open(str(wav), 'wb') as w:
        w.setnchannels(2)
        w.setsampwidth(3)
        w.setframerate(48000)
        frames = bytearray()
        for i in range(48000 // 2):
            v = int(math.sin(2 * math.pi * 440 * i / 48000) * 0x3FFFFF)
            frames += v.to_bytes(3, 'little', signed=True) * 2
        w.writeframes(bytes(frames))
    mp3 = Path(tmp) / 'smoke.mp3'
    run([bin_dir / ('ffmpeg' + ext), '-v', 'error', '-i', wav, '-c:a', 'libmp3lame', '-b:a', '320k', '-f', 'mp3', mp3])
    # The engine reads the codec the same way (convert::probe): `ffmpeg -i` with no output.
    info = subprocess.run([str(bin_dir / ('ffmpeg' + ext)), '-hide_banner', '-i', str(mp3)], env=env,
                          capture_output=True, text=True, timeout=60).stderr
    assert 'Audio: mp3' in info, info
print('Packaged app and all bundled tools passed the smoke test.')
