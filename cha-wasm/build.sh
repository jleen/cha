#!/bin/sh
# Assemble the serverless static build into cha-wasm/dist/: the shared front end
# (cha-gui/ui, copied — never edited in place), the matcher as WebAssembly, the
# worker that hosts it, words.txt, and the PWA files (manifest, icons, service
# worker). dist/ is complete and self-contained: deploy it with any static host,
# e.g. `rsync -a --delete cha-wasm/dist/ host:/srv/cha/`. See docs/static.md.
#
# Needs only `rustup target add wasm32-unknown-unknown` — no wasm-bindgen, no
# Node, no bundler — and plain POSIX tools, so it runs on macOS as well as Linux.
#
# Usage: cha-wasm/build.sh            then e.g.  python3 -m http.server -d cha-wasm/dist
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
dist=$here/dist
icons=$root/cha-gui/src-tauri/icons
apple_icons=$root/cha-gui/src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset

# Stripped: the symbol names are ~10% of the module and nothing reads them.
CARGO_PROFILE_RELEASE_STRIP=true \
  cargo build --manifest-path "$root/Cargo.toml" -p cha-wasm --release --target wasm32-unknown-unknown

rm -rf "$dist"
mkdir -p "$dist/icons"
cp -R "$root/cha-gui/ui/." "$dist/"
rm -rf "$dist/tests"
cp "$root/target/wasm32-unknown-unknown/release/cha_wasm.wasm" "$dist/"
cp "$here/www/worker.js" "$here/www/pwa.js" "$here/www/manifest.webmanifest" "$dist/"
if [ -f "$root/words.txt" ]; then
  cp "$root/words.txt" "$dist/"
else
  echo "warning: no words.txt at the repo root; the page will say so" >&2
fi

# Every icon is one the native apps already ship, copied rather than resized
# so this script needs no image tools:
# - tab icons and the manifest's "any" icons: the desktop set (rounded, with a
#   transparent margin), plus Android's 192px launcher icon, the size Chrome
#   asks for;
# - the iOS home-screen icon: iOS's own 180px app icon, because iOS fills
#   transparency with black and that one is already full-bleed;
# - the manifest's "maskable" icon (Android crops it to its launcher's shape):
#   iOS's full-bleed 1024px icon. Its glyph reaches exactly the 40%-radius safe
#   zone a maskable icon must keep content inside, so it survives a circle crop.
android_icons=$root/cha-gui/src-tauri/gen/android/app/src/main/res
cp "$icons/32x32.png" "$dist/icons/icon-32.png"
cp "$icons/64x64.png" "$dist/icons/icon-64.png"
cp "$android_icons/mipmap-xxxhdpi/ic_launcher.png" "$dist/icons/icon-192.png"
cp "$icons/128x128@2x.png" "$dist/icons/icon-256.png"
cp "$icons/icon.png" "$dist/icons/icon-512.png"
cp "$apple_icons/AppIcon-60x60@3x.png" "$dist/icons/apple-touch-icon.png"
cp "$apple_icons/AppIcon-512@2x.png" "$dist/icons/maskable-1024.png"

# Splice www/head.html (the transport marker transport.js looks for, plus the PWA
# links) into the copy, so the shared index.html stays identical for the Tauri
# app and cha-web. `r` and a temp file rather than `sed -i`, whose syntax differs
# between GNU and BSD sed.
sed "/<meta charset=\"UTF-8\" \/>/r $here/www/head.html" "$dist/index.html" > "$dist/index.html.tmp"
mv "$dist/index.html.tmp" "$dist/index.html"
grep -q 'name="cha-transport"' "$dist/index.html" || { echo "head injection failed" >&2; exit 1; }

# The service worker's precache list is every file built so far, plus "./" for
# the bare start URL. Its build id is the version plus a hash of all of them —
# see www/sw.js for why the hash is what triggers an update.
if command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum; }; else sha() { shasum -a 256; }; fi
version=$(awk -F'"' '/^\[workspace.package\]/ { f = 1; next } /^\[/ { f = 0 } f && /^version/ { print $2 }' "$root/Cargo.toml")
files=$(cd "$dist" && find . -type f | LC_ALL=C sort)
hash=$(cd "$dist" && for f in $files; do sha < "$f"; echo "$f"; done | sha | cut -c1-12)
list=$(printf '"./"'; for f in $files; do printf ', "%s"' "$f"; done)
sed -e "s|__CHA_BUILD__|$version-$hash|" -e "s|__CHA_FILES__|[$list]|" "$here/www/sw.js" > "$dist/sw.js"

echo "built $dist: $version-$hash, $(du -sh "$dist" | cut -f1)"
