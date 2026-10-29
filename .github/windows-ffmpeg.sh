#!/usr/bin/env bash
# ffmpeg for Windows builds: the newest shared LGPL release build from
# BtbN/FFmpeg-Builds (headers, import libraries and DLLs). Sets FFMPEG_DIR for the
# build, FFMPEG_BIN (the DLLs, to ship beside haze.exe) and puts them on PATH.
set -euo pipefail
asset=$(gh release view latest -R BtbN/FFmpeg-Builds --json assets -q '.assets[].name' |
  grep -E '^ffmpeg-n[0-9.]+-latest-win64-lgpl-shared-[0-9.]+\.zip$' | sort -V | tail -1)
echo "using $asset"
gh release download latest -R BtbN/FFmpeg-Builds -p "$asset" -D "$RUNNER_TEMP"
unzip -q "$RUNNER_TEMP/$asset" -d "$RUNNER_TEMP"
dir="$RUNNER_TEMP/${asset%.zip}"
echo "FFMPEG_DIR=$(cygpath -w "$dir")" >> "$GITHUB_ENV"
echo "FFMPEG_BIN=$dir/bin" >> "$GITHUB_ENV"
echo "$(cygpath -w "$dir/bin")" >> "$GITHUB_PATH"
