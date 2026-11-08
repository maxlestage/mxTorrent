#!/usr/bin/env bash
# Construit mxTorrent sur Heroku, lancé par RustConfig une fois que le buildpack
# emk/rust a installé Rust. Fonctionne aussi à la main : ./heroku-build.sh
#   bin/mxtorrent-tracker   le serveur : tracker WebSocket + site + client
#   dist/                   le site, le client et son module WebAssembly
# Le Procfile lance : bin/mxtorrent-tracker --static dist
set -euo pipefail
cd "$(dirname "$0")"

# Sur Heroku, Cargo construit dans le cache du buildpack : les constructions
# suivantes ne recompilent que ce qui a changé.
target="${CARGO_TARGET_DIR:-target}"
tools="$target/outils"
mkdir -p "$tools" bin

echo "-----> Serveur (tracker WebSocket et fichiers statiques)"
cargo build --release -p mxtorrent-tracker
cp "$target/release/mxtorrent-tracker" bin/
echo "       bin/mxtorrent-tracker prêt"

echo "-----> Client WebAssembly"
rustup target add wasm32-unknown-unknown

# wasm-bindgen doit avoir exactement la version de Cargo.lock : binaire précompilé,
# ou compilé si le téléchargement échoue.
version=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p')
wb="$tools/wasm-bindgen-$version"
if [ ! -x "$wb/wasm-bindgen" ]; then
  mkdir -p "$wb"
  curl -sSfL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}/wasm-bindgen-${version}-x86_64-unknown-linux-musl.tar.gz" \
    | tar -xz --strip-components=1 -C "$wb" \
    || cargo install wasm-bindgen-cli --version "$version" --locked --root "$wb" --bin wasm-bindgen
  [ -x "$wb/wasm-bindgen" ] || mv "$wb/bin/wasm-bindgen" "$wb/"
fi

# wasm-opt (binaryen) réduit le module d'environ 20 % ; facultatif.
binaryen=132
wo="$tools/binaryen-$binaryen/bin/wasm-opt"
if [ ! -x "$wo" ]; then
  curl -sSfL "https://github.com/WebAssembly/binaryen/releases/download/version_${binaryen}/binaryen-version_${binaryen}-x86_64-linux.tar.gz" \
    | tar -xz -C "$tools" && mv "$tools/binaryen-version_${binaryen}" "$tools/binaryen-$binaryen" \
    || echo "       binaryen indisponible : module non optimisé"
fi

WASM_BINDGEN="$wb/wasm-bindgen" WASM_OPT="$wo" ./scripts/build.sh
echo "       dist/ prêt"
