#!/usr/bin/env bash
# Vérifie qu'une nouvelle version est servie dès le premier rechargement (jamais une copie
# périmée du cache hors ligne). Simule une nouvelle publication en changeant le numéro de
# version de dist/ ; lance son propre serveur.
#   tests/update.sh [port]
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${1:-8002}"
WORK="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$WORK" 2>/dev/null || true' EXIT
cp -r dist/. "$WORK/"
OLD="$(sed -n 's/.*mxtorrentBuild = "\([^"]*\)".*/\1/p' "$WORK/app/index.html")"
NEW="$OLD-suivante"
PORT="$PORT" bin/mxtorrent-tracker --static "$WORK" > /dev/null 2>&1 &
sleep 1
node tests/update.cjs "http://localhost:$PORT" "$WORK" \
  "sed -i 's/$OLD/$NEW/g' '$WORK/sw.js' '$WORK/index.html' '$WORK/legal.html' '$WORK/app/index.html'"
