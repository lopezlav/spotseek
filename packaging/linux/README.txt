Spotseek for Linux (x64)

Install for your user (app menu entry and a "spotseek" command):
    ./install.sh
Or run it from this folder without installing:
    ./spotseek
Remove it:
    ~/.local/share/spotseek/install.sh --uninstall

All tools are included (sockseek, yt-dlp, ffmpeg, deno). The system needs what a
desktop normally has: a graphics driver with Vulkan or OpenGL, ALSA (libasound2)
for the player, D-Bus with a secret service (GNOME Keyring or KWallet) to keep
the Soulseek password, and ICU (libicu) for sockseek. Built on Ubuntu 22.04:
works on Ubuntu 22.04+, Debian 12+, Fedora 36+ and similar.

Licenses and sources are in the licenses folder.
