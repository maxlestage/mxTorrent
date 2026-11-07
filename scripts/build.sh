#!/usr/bin/env bash
# Construit le site et le client dans dist/ :
#   dist/            le site vitrine (PWA)
#   dist/app/        le client egui
#   dist/app/pkg/    le module WebAssembly et sa colle JavaScript
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="${PROFILE:-release}"
TARGET=wasm32-unknown-unknown
OUT=dist

command -v wasm-bindgen >/dev/null || {
  echo "wasm-bindgen manque : cargo install wasm-bindgen-cli --version 0.2.130" >&2
  exit 1
}

if [ "$PROFILE" = "release" ]; then
  cargo build -p mxtorrent-app --release --target "$TARGET"
else
  cargo build -p mxtorrent-app --target "$TARGET"
fi

rm -rf "$OUT"
mkdir -p "$OUT"
cp -r site/. "$OUT/"
wasm-bindgen --target web --no-typescript --out-dir "$OUT/app/pkg" \
  "target/$TARGET/$PROFILE/mxtorrent_app.wasm"

if command -v wasm-opt >/dev/null && [ "$PROFILE" = "release" ]; then
  wasm-opt -Os --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    -o "$OUT/app/pkg/mxtorrent_app_bg.wasm" "$OUT/app/pkg/mxtorrent_app_bg.wasm"
fi

# Version du cache hors ligne : change à chaque construction.
VERSION="$(git rev-parse --short HEAD 2>/dev/null || date +%s)-$(date +%s)"
sed -i "s/__BUILD__/$VERSION/" "$OUT/sw.js"

du -sh "$OUT/app/pkg/mxtorrent_app_bg.wasm" | awk '{print "module wasm : " $1}'
echo "prêt : $OUT/  (cargo run -p mxtorrent-tracker -- --static $OUT)"
