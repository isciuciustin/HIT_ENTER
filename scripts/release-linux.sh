#!/usr/bin/env bash
# release-linux.sh <version> — the Slint window (M12) as a .deb and
# an AppImage, with SHA256SUMS, in target/release-upload-native/. Tauri's
# bundler made these for the web window; here they are assembled by hand.
#
# The package is named `hit-enter` and installs /usr/bin/hit-enter: it
# replaces the Tauri beta on upgrade. Paths are remapped (no home directory
# in the binary), and the build fails if this machine's home survives.
set -euo pipefail
version=${1:?usage: release-native-linux.sh <version, e.g. 0.1.0-beta.3>}
cd "$(dirname "$0")/.."
root=$PWD

# Built in Ubuntu 22.04 (packaging/linux-build.Dockerfile): glibc 2.35, so
# the binary runs on Debian 12 / Ubuntu 22.04 and newer. Built here on Arch
# it needed glibc 2.43 and ran almost nowhere (found 2026-10-18).
image=hit-enter-build:ubuntu22.04
docker build -q -t "$image" -f crates/hit-desktop/packaging/linux-build.Dockerfile crates/hit-desktop/packaging >/dev/null
docker run --rm \
  -v "$PWD:/hit-enter" -w /hit-enter \
  -v hit-enter-cargo-registry:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/hit-enter/target/u2204 -e RUSTUP_TOOLCHAIN=1.98.1 \
  -e RUSTFLAGS="--remap-path-prefix=/usr/local/cargo/registry/src=/cargo --remap-path-prefix=/hit-enter=/hit-enter" \
  -e CFLAGS="-ffile-prefix-map=/hit-enter=/hit-enter" -e CXXFLAGS="-ffile-prefix-map=/hit-enter=/hit-enter" \
  "$image" \
  sh -c "cargo build --release --locked -p hit-desktop \
    && cp -L /usr/lib/x86_64-linux-gnu/libopus.so.0 target/u2204/ \
    && chown -R $(id -u):$(id -g) target/u2204"

bin=target/u2204/release/hit-enter
if grep -aqF "$HOME" "$bin"; then
  echo "the binary still names $HOME — not releasable" >&2
  exit 1
fi
glibc=$(objdump -T "$bin" | grep -oE "GLIBC_[0-9.]+" | sort -Vu | tail -1)
echo "needs $glibc at most"

work=target/native-pkg
out=target/release-upload
rm -rf "$work" "$out" && mkdir -p "$work" "$out"
icons=crates/hit-desktop/packaging/icons
summary="Group chat where every app is a relay"

desktop() { # $1: Exec, $2: Icon
  cat <<EOF
[Desktop Entry]
Type=Application
Name=HIT_ENTER
Comment=$summary
Exec=$1 %u
Icon=$2
Terminal=false
Categories=Network;Chat;InstantMessaging;
MimeType=x-scheme-handler/hit-enter;
StartupWMClass=hit-enter
EOF
}

# ---- .deb: debian-binary, control.tar.xz, data.tar.xz, in that order ----
deb=$work/deb
mkdir -p "$deb/usr/bin" "$deb/usr/share/applications" "$deb/usr/share/doc/hit-enter" "$deb/DEBIAN"
strip -o "$deb/usr/bin/hit-enter" "$bin"
desktop hit-enter hit-enter > "$deb/usr/share/applications/hit-enter.desktop"
for s in 32x32:32x32.png 128x128:128x128.png 256x256:128x128@2x.png 512x512:icon.png; do
  d=$deb/usr/share/icons/hicolor/${s%%:*}/apps
  mkdir -p "$d" && cp "$icons/${s#*:}" "$d/hit-enter.png"
done
cp THIRD_PARTY_NOTICES.md "$deb/usr/share/doc/hit-enter/"
# The betas so far were versioned "0.1.0-beta.N" (Tauri's bundler): keep
# that, or apt orders the next one *below* them. ("0.1.0~beta.3" sorts
# before "0.1.0-beta.2" and the upgrade was refused — found 2026-10-18.)
# The final 0.1.0 will need "0.1.0-1" or later to sort above the betas.
debver=$version
size=$(du -sk --exclude=DEBIAN "$deb" | cut -f1)
cat > "$deb/DEBIAN/control" <<EOF
Package: hit-enter
Version: $debver
Architecture: amd64
Maintainer: isciuciustin
Installed-Size: $size
Depends: libc6 (>= 2.35), libgcc-s1, libstdc++6, libfontconfig1, libfreetype6, libasound2t64 | libasound2, libopus0, libxkbcommon0, libwayland-client0 | libx11-6
Recommends: libxkbcommon-x11-0, libxcursor1, libxi6, libdbus-1-3
Section: net
Priority: optional
Homepage: https://hitenter.app
Description: $summary
 Groups with channels over a peer-to-peer network. Your identity is a Nostr
 key in your OS keychain; every running app keeps and relays its groups.
 Group messages are stored in plain text on every member's disk, by design.
 No telemetry.
EOF
(
  cd "$deb"
  echo "2.0" > ../debian-binary
  tar --owner=0 --group=0 -cJf ../control.tar.xz -C DEBIAN .
  tar --owner=0 --group=0 --exclude=./DEBIAN -cJf ../data.tar.xz .
)
debfile=$out/hit-enter_${debver}_amd64.deb
(cd "$work" && ar rc "$root/$debfile" debian-binary control.tar.xz data.tar.xz)

# ---- AppImage: the same binary, libopus beside it ----
tool=$HOME/.cache/hit-enter-tools/appimagetool-1.9.0-x86_64.AppImage
want=46fdd785094c7f6e545b61afcfb0f3d98d8eab243f644b4b17698c01d06083d1
if [ ! -x "$tool" ]; then
  mkdir -p "$(dirname "$tool")"
  curl -sSLf -o "$tool" https://github.com/AppImage/appimagetool/releases/download/1.9.0/appimagetool-x86_64.AppImage
  chmod +x "$tool"
fi
echo "$want  $tool" | sha256sum -c --quiet
app=$work/HIT_ENTER.AppDir
mkdir -p "$app/usr/bin" "$app/usr/lib"
cp "$deb/usr/bin/hit-enter" "$app/usr/bin/"
# Not every desktop has Opus; the rest (fontconfig, ALSA, Wayland/X11) it
# does, and bundling those breaks more than it fixes.
# Ubuntu 22.04's, from the build container: the host's needs its newer glibc.
cp target/u2204/libopus.so.0 "$app/usr/lib/"
cp "$icons/128x128@2x.png" "$app/hit-enter.png"
desktop hit-enter hit-enter > "$app/hit-enter.desktop"
cat > "$app/AppRun" <<'EOF'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
export LD_LIBRARY_PATH="$here/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$here/usr/bin/hit-enter" "$@"
EOF
chmod +x "$app/AppRun"
ARCH=x86_64 "$tool" --no-appstream "$app" "$out/HIT_ENTER_${version}_x86_64.AppImage" >/dev/null 2>&1

cp THIRD_PARTY_NOTICES.md "$out/"
(cd "$out" && sha256sum -- * > SHA256SUMS)
ls -lh "$out"
