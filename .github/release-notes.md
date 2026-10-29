## Download

| Platform | File |
|---|---|
| macOS 11+ (Apple Silicon and Intel) | `haze-…-macos.dmg` |
| Windows 10/11 (x64) | `haze-…-windows-x64.zip` |
| Linux x86_64 / ARM64 | `haze-…-linux-x86_64.tar.gz` / `haze-…-linux-aarch64.tar.gz` |

Bring your own playlist (M3U) or provider login (Xtream Codes). `SHA256SUMS.txt`
lists checksums of every file.

**macOS:** open the DMG and drag haze to Applications. haze isn't notarized by Apple
yet, so the first launch is blocked: open it once, then go to **System Settings →
Privacy & Security** and click **Open Anyway**. ffmpeg is built in.

**Windows:** unzip and run `haze.exe`; keep the DLLs beside it (ffmpeg, LGPL builds
from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds)). If SmartScreen says
"Windows protected your PC", click **More info → Run anyway**.

**Linux:** unpack and run `./haze`. It uses the system's ffmpeg libraries, built
against Ubuntu 24.04's (ffmpeg 6.1), plus ALSA and D-Bus (on Debian/Ubuntu:
`sudo apt install libavcodec60 libavformat60 libswscale7 libswresample4 libasound2t64
libdbus-1-3`). On other distributions build it from source (see the README). For a
menu entry, copy `haze` to `~/.local/bin`, `haze.desktop` to
`~/.local/share/applications` and `haze.png` to `~/.local/share/icons`.
