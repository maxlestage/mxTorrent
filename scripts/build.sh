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
# Sur Heroku, Cargo construit dans le cache du buildpack (CARGO_TARGET_DIR).
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
WASM_BINDGEN="${WASM_BINDGEN:-wasm-bindgen}"

command -v "$WASM_BINDGEN" >/dev/null || {
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
"$WASM_BINDGEN" --target web --no-typescript --out-dir "$OUT/app/pkg" \
  "$TARGET_DIR/$TARGET/$PROFILE/mxtorrent_app.wasm"

# Optimisation de la taille (binaryen), si disponible : `npm ci` l'installe.
WASM_OPT="${WASM_OPT:-$(command -v wasm-opt || echo node_modules/.bin/wasm-opt)}"
if [ "$PROFILE" = "release" ] && [ -x "$WASM_OPT" ]; then
  if "$WASM_OPT" -Os --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
      --enable-mutable-globals --enable-reference-types --enable-multivalue \
      -o "$OUT/app/pkg/opt.wasm" "$OUT/app/pkg/mxtorrent_app_bg.wasm"; then
    mv "$OUT/app/pkg/opt.wasm" "$OUT/app/pkg/mxtorrent_app_bg.wasm"
  else
    echo "wasm-opt a échoué : module non optimisé" >&2
    rm -f "$OUT/app/pkg/opt.wasm"
  fi
fi

# Version du cache hors ligne : change à chaque construction.
VERSION="$(git rev-parse --short HEAD 2>/dev/null || date +%s)-$(date +%s)"
sed -i "s/__BUILD__/$VERSION/" "$OUT/sw.js"

du -sh "$OUT/app/pkg/mxtorrent_app_bg.wasm" | awk '{print "module wasm : " $1}'
echo "prêt : $OUT/  (cargo run -p mxtorrent-tracker -- --static $OUT)"
