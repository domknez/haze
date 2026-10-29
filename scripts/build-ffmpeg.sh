#!/bin/sh
# Build the small static ffmpeg that the macOS release links: demuxers, decoders,
# network protocols, scaling and resampling, with the system's TLS and GPU decoding
# and no outside libraries (so LGPL).
#
#   scripts/build-ffmpeg.sh <prefix> [arm64|x86_64]
#
# Build haze against it with
#   PKG_CONFIG_PATH=<prefix>/lib/pkgconfig cargo build --release --features static-ffmpeg
set -e
prefix=$1
arch=${2:-$(uname -m)}
version=${FFMPEG_VERSION:-n9.0.2}
[ -n "$prefix" ] || { echo "usage: $0 <prefix> [arm64|x86_64]" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git clone --quiet --depth 1 --branch "$version" https://github.com/FFmpeg/FFmpeg "$work/src"
cd "$work/src"

set -- --enable-securetransport --enable-videotoolbox
flags="-arch $arch -mmacosx-version-min=11.0"
if [ "$arch" != "$(uname -m)" ]; then
  set -- "$@" --enable-cross-compile --target-os=darwin --arch="$arch"
fi

./configure --prefix="$prefix" --cc=clang \
  --extra-cflags="$flags" --extra-ldflags="$flags" \
  --enable-static --disable-shared --enable-pic \
  --disable-autodetect --disable-programs --disable-doc --disable-debug \
  --disable-avdevice --disable-avfilter --disable-encoders --disable-muxers \
  --disable-filters --disable-indevs --disable-outdevs \
  --enable-zlib "$@"
make -j"$(sysctl -n hw.ncpu)" >/dev/null
make install >/dev/null
echo "ffmpeg $version ($arch) in $prefix"
