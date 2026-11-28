#!/usr/bin/env bash
# Builds the universal "Screen Translate.app", the updater tarball and the dmg into target/bundle.
# Usage: packaging/macos/bundle.sh <version>
# Opt-in for local tests (the updater env hooks exist only in debug builds): PROFILE=debug ARCHS=aarch64.
set -euo pipefail
cd "$(dirname "$0")/../.."
V=${1:?usage: bundle.sh <version>}
APP="Screen Translate.app"
OUT=target/bundle
PROFILE=${PROFILE:-release}
ARCHS=${ARCHS:-"aarch64 x86_64"}
BIN=screen-translate

BUILT=()
for arch in $ARCHS; do
  cargo build $([ "$PROFILE" = release ] && echo --release) -p screen-translate --target "$arch-apple-darwin"
  BUILT+=("target/$arch-apple-darwin/$PROFILE/$BIN")
done

rm -rf "$OUT"
mkdir -p "$OUT/$APP/Contents/MacOS" "$OUT/$APP/Contents/Resources"
lipo -create -output "$OUT/$APP/Contents/MacOS/screen-translate" "${BUILT[@]}"
cp packaging/icons/icon.icns "$OUT/$APP/Contents/Resources/icon.icns"
sed "s/@VERSION@/$V/g" packaging/macos/Info.plist >"$OUT/$APP/Contents/Info.plist"

# Ad-hoc signature, same as the Tauri build's signingIdentity "-".
codesign --force --deep --sign - "$OUT/$APP"
codesign --verify --deep --strict "$OUT/$APP"

# Updater tarball: a single top-level .app dir and no AppleDouble files.
TAR="$OUT/Screen.Translate_universal.app.tar.gz"
COPYFILE_DISABLE=1 tar --no-mac-metadata -czf "$TAR" -C "$OUT" "$APP"
if tar -tzf "$TAR" | grep -qv "^$APP/"; then
  echo "bad tar layout" >&2
  exit 1
fi

# dmg with an /Applications link.
mkdir -p "$OUT/dmg"
cp -R "$OUT/$APP" "$OUT/dmg/"
ln -s /Applications "$OUT/dmg/Applications"
hdiutil create -volname "Screen Translate" -srcfolder "$OUT/dmg" -ov -format UDZO \
  "$OUT/Screen.Translate_${V}_universal.dmg"
rm -rf "$OUT/dmg"
