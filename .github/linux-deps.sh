#!/usr/bin/env bash
# The libraries haze builds against on Linux: ffmpeg's, ALSA, D-Bus (keyring, media
# keys) and what the window needs. The runners' package mirror sometimes stalls, so
# each try times out and the next one goes to another mirror.
set -u
apt="sudo apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=30"
for mirror in "" http://archive.ubuntu.com/ubuntu/ http://mirrors.edge.kernel.org/ubuntu/; do
  [ -n "$mirror" ] && echo "$mirror" | sudo tee /etc/apt/apt-mirrors.txt
  timeout 120 $apt update && timeout 600 $apt install -y \
    libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev \
    libasound2-dev libdbus-1-dev libxkbcommon-dev pkg-config clang && exit 0
  echo "apt failed on ${mirror:-the default mirror}"; sleep 10
done
exit 1
