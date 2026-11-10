#!/usr/bin/env bash
# Test avec un vrai client BitTorrent classique (Transmission) : il partage un fichier
# annoncé sur un tracker UDP et un tracker HTTP ; un navigateur le télécharge par la
# passerelle (TCP + MSE vers Transmission, Brocolis 🥦 entre le navigateur et elle).
#   tests/classic.sh [http://localhost:8000]
# Prérequis : transmission-daemon, transmission-cli, node (npm ci), dist/ construit,
# et un serveur lancé avec MXT_RELAY_ALLOW_PRIVATE=1 (Transmission est en local).
set -euo pipefail
BASE="${1:-http://localhost:8000}"
cd "$(dirname "$0")/.."
WORK="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; sleep 1; rm -rf "$WORK" 2>/dev/null || true' EXIT

node tests/udp-tracker.mjs > "$WORK/udp.log" 2>&1 &
node tests/http-tracker.mjs > "$WORK/http.log" 2>&1 &
mkdir -p "$WORK/data" "$WORK/conf"
head -c 8388608 /dev/urandom > "$WORK/data/libre.bin"
SHA="$(sha256sum "$WORK/data/libre.bin" | cut -c1-16)"
transmission-create -t udp://127.0.0.1:8969/announce -t http://127.0.0.1:8970/announce -o "$WORK/libre.torrent" "$WORK/data/libre.bin" > /dev/null
cat > "$WORK/conf/settings.json" <<JSON
{ "download-dir": "$WORK/data", "dht-enabled": false, "lpd-enabled": false, "pex-enabled": false, "utp-enabled": false,
  "encryption": 1, "peer-port": 51413, "peer-port-random-on-start": false, "port-forwarding-enabled": false,
  "rpc-enabled": true, "rpc-port": 9091, "rpc-authentication-required": false, "rpc-whitelist-enabled": false,
  "rpc-host-whitelist-enabled": false }
JSON
transmission-daemon -f -g "$WORK/conf" > "$WORK/daemon.log" 2>&1 &
for _ in $(seq 1 20); do transmission-remote 9091 -l > /dev/null 2>&1 && break; sleep 0.5; done
transmission-remote 9091 -a "$WORK/libre.torrent" --download-dir "$WORK/data" > /dev/null
# Transmission vérifie le fichier puis s'annonce sur les deux trackers.
for _ in $(seq 1 60); do grep -q "51413" "$WORK/udp.log" && grep -q "51413" "$WORK/http.log" && break; sleep 1; done
HASH="$(transmission-show "$WORK/libre.torrent" | sed -n 's/.*Hash v1: //p')"
echo "Transmission partage $HASH ($SHA…)"
node tests/e2e-classic.cjs "$BASE" "magnet:?xt=urn:btih:$HASH&tr=udp%3A%2F%2F127.0.0.1%3A8969%2Fannounce" "$SHA"
node tests/e2e-classic.cjs "$BASE" "magnet:?xt=urn:btih:$HASH&tr=http%3A%2F%2F127.0.0.1%3A8970%2Fannounce" "$SHA"
echo "Côté Transmission (drapeau E : connexion chiffrée) :"
transmission-remote 9091 -t 1 -pi
