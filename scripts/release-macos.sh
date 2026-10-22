#!/usr/bin/env bash
# release-macos.sh <version> — the Slint window (M12) for macOS (Apple
# silicon): HIT_ENTER.app and a .dmg, in target/release-upload/, the
# checksums appended to SHA256SUMS. Run on the Mac (docs/BUILD_MACOS.md).
#
# Assembled by hand, as the Linux packages are: an .app is a folder with an
# Info.plist. Ad-hoc signed (no Apple developer account): users clear the
# quarantine once (BUILD_MACOS.md §6).
set -euo pipefail
version=${1:?usage: release-macos.sh <version, e.g. 0.1.0-beta.3>}
cd "$(dirname "$0")/.."
export RUSTFLAGS="--remap-path-prefix=$HOME/.cargo/registry/src=/cargo --remap-path-prefix=$HOME/.cargo/git/checkouts=/cargo-git --remap-path-prefix=$PWD=/hit-enter --remap-path-prefix=$HOME=/home"
export CFLAGS="-ffile-prefix-map=$PWD=/hit-enter -ffile-prefix-map=$HOME=/home"
export CXXFLAGS="$CFLAGS"
# 11.0: the first macOS on Apple silicon.
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo build --release --locked -p hit-desktop

bin=target/release/hit-enter
if grep -aqF "$HOME" "$bin"; then echo "the binary still names $HOME — not releasable" >&2; exit 1; fi
# Only /usr/lib and /System: a Homebrew dylib runs here and crashes on
# every other Mac (BUILD_MACOS.md §3). Read otool's output first — with
# pipefail, `otool | grep -q` can pass when grep stops early.
libs=$(otool -L "$bin")
if grep -qE "/opt/homebrew|/usr/local" <<<"$libs"; then
  echo "links a Homebrew library — users do not have it:" >&2; echo "$libs" >&2; exit 1
fi

work=target/macos-pkg
app=$work/HIT_ENTER.app
rm -rf "$work" && mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
strip -x -o "$app/Contents/MacOS/hit-enter" "$bin"
cp crates/hit-desktop/packaging/icons/icon.icns "$app/Contents/Resources/"
cp THIRD_PARTY_NOTICES.md "$app/Contents/Resources/"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>HIT_ENTER</string>
  <key>CFBundleDisplayName</key><string>HIT_ENTER</string>
  <!-- The Tauri app's identifier: the same data folder and keychain. -->
  <key>CFBundleIdentifier</key><string>app.hitenter.desktop</string>
  <key>CFBundleExecutable</key><string>hit-enter</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.social-networking</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- Without it macOS refuses the microphone, and voice is silent. -->
  <key>NSMicrophoneUsageDescription</key>
  <string>HIT_ENTER uses the microphone for voice channels and calls, and only while you are in one.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>HIT_ENTER link</string>
      <key>CFBundleURLSchemes</key><array><string>hit-enter</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
codesign --force --deep --sign - "$app"

out=target/release-upload
mkdir -p "$out"
dmg=$out/HIT_ENTER_${version}_aarch64.dmg
stage=$work/dmg
mkdir -p "$stage" && cp -R "$app" "$stage/" && ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname HIT_ENTER -srcfolder "$stage" -ov -format UDZO "$dmg"
# Always the current notices: a copy left by an older build is stale.
cp THIRD_PARTY_NOTICES.md "$out/"
(cd "$out" && shasum -a 256 -- "$(basename "$dmg")" >> SHA256SUMS)
ls -lh "$out"
