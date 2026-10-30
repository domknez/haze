#!/bin/sh
# Build haze.app (universal: Apple Silicon + Intel) and dist/haze-<version>-macos.dmg,
# with ffmpeg built in (scripts/build-ffmpeg.sh, kept under target/ffmpeg-<arch>).
#
# Signing, all optional:
#   MACOS_SIGN_IDENTITY   e.g. "Developer ID Application: Name (TEAMID)": hardened
#                         runtime + timestamp. Without it the app is ad-hoc signed and
#                         Gatekeeper asks the user to allow it once.
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#                         notarize and staple the DMG (needs MACOS_SIGN_IDENTITY).
set -e
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
dist=dist
app=$dist/haze.app
dmg=$dist/haze-$version-macos.dmg
export MACOSX_DEPLOYMENT_TARGET=11.0

rm -rf "$app" "$dmg"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# ffmpeg's VideoToolbox code asks for the macOS version (`@available`), answered by
# clang's runtime library, which rustc's link line leaves out.
clang_rt=$(clang --print-resource-dir)/lib/darwin/libclang_rt.osx.a

for arch in arm64 x86_64; do
  target=$([ $arch = arm64 ] && echo aarch64-apple-darwin || echo x86_64-apple-darwin)
  ffmpeg=$PWD/target/ffmpeg-$arch
  [ -f "$ffmpeg/lib/libavcodec.a" ] || ./scripts/build-ffmpeg.sh "$ffmpeg" "$arch"
  rustup target add "$target" >/dev/null 2>&1 || true
  triple=$(echo "$target" | tr a-z- A-Z_)
  env "CARGO_TARGET_${triple}_RUSTFLAGS=-C link-arg=$clang_rt" \
  PKG_CONFIG_PATH=$ffmpeg/lib/pkgconfig PKG_CONFIG_ALLOW_CROSS=1 \
    cargo build --release --locked --target "$target" --features static-ffmpeg
done
lipo -create -output "$app/Contents/MacOS/haze" \
  target/aarch64-apple-darwin/release/haze target/x86_64-apple-darwin/release/haze
# Nothing from Homebrew or elsewhere outside the system may be linked.
if otool -L "$app/Contents/MacOS/haze" | tail -n +2 | grep -v -e /System/ -e /usr/lib/; then
  echo "haze links libraries outside the system (above)" >&2
  exit 1
fi

cp assets/haze-icon-pack/haze.icns "$app/Contents/Resources/haze.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>haze</string>
  <key>CFBundleDisplayName</key><string>haze</string>
  <key>CFBundleIdentifier</key><string>io.github.domknez.haze</string>
  <key>CFBundleExecutable</key><string>haze</string>
  <key>CFBundleIconFile</key><string>haze</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSLocalNetworkUsageDescription</key>
  <string>haze finds AirPlay speakers on your network and plays the sound of your channels on them.</string>
  <key>NSBonjourServices</key>
  <array>
    <string>_airplay._tcp</string>
    <string>_raop._tcp</string>
    <string>_dacp._tcp</string>
  </array>
  <key>NSHumanReadableCopyright</key><string>MIT License. Free and open source.</string>
</dict>
</plist>
PLIST

if [ -n "$MACOS_SIGN_IDENTITY" ]; then
  codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$app"
else
  codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
staging=$work/dmg
mkdir "$staging"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -volname haze -srcfolder "$staging" -fs HFS+ -format UDZO -ov "$dmg" >/dev/null

if [ -n "$MACOS_SIGN_IDENTITY" ]; then
  codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$dmg"
  if [ -n "$APPLE_ID" ] && [ -n "$APPLE_TEAM_ID" ] && [ -n "$APPLE_APP_PASSWORD" ]; then
    xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
      --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$dmg"
  fi
fi

echo "$dmg"
