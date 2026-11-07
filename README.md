<div align="center">

<img src="site/assets/icon.svg" width="120" alt="">

# mxTorrent

**Le torrent directement dans le navigateur.**
Un client BitTorrent écrit en Rust, compilé en WebAssembly, dessiné avec egui.
Rien à installer : il tourne dans un onglet, sur ordinateur comme sur téléphone, et s'installe comme une application (PWA).

[Site](https://maxlestage.github.io/mxTorrent/) · [Ouvrir le client](https://maxlestage.github.io/mxTorrent/app/) · [Tracker auto-hébergé](#tracker-auto-hébergé)

</div>

---

## Ce que fait mxTorrent

| | |
|---|---|
| 🧲 **Liens magnet et `.torrent`** | Collez un lien, une empreinte (hex ou base32), une URL, ou déposez un fichier. Les métadonnées arrivent des pairs (BEP 9 et BEP 10). |
| 📤 **Partage instantané** | Déposez des fichiers : le torrent est créé dans le navigateur, partagé aussitôt, avec un lien magnet et un QR code. |
| 🔗 **Pair à pair en WebRTC** | Les données passent directement d'un navigateur à l'autre, chiffrées (DTLS). Le tracker WebSocket ne fait que présenter les pairs. |
| 🧩 **Le vrai protocole** | Poignée de main, champs de bits, pièce la plus rare d'abord, demandes en file, fin de partie, vérification SHA-1, envoi aux autres pairs. |
| 🌐 **Sources web** | Les torrents avec des sources HTTP (BEP 19) se téléchargent même sans pair. |
| 🎬 **Fichiers** | Choix des fichiers à télécharger, aperçu (vidéo, audio, image, PDF, texte) et enregistrement dès qu'ils sont complets. |
| 🌍 **Compatible WebTorrent** | Mêmes trackers et même protocole que webtorrent.io, Instant.io, WebTorrent Desktop. |
| 📲 **PWA** | Installable, fonctionne hors ligne, ouvre les liens `magnet:` et les fichiers `.torrent`, reçoit les fichiers partagés depuis Android. |
| 🇫🇷 🇬🇧 | Interface en français et en anglais, thème sombre ou clair, mise en page ordinateur et téléphone. |

## Architecture

```
crates/
  core/      mxtorrent-core     le moteur BitTorrent, sans aucune entrée-sortie
  app/       mxtorrent-app      le client : egui + WebRTC + WebSocket, compilé en WebAssembly
  tracker/   mxtorrent-tracker  un tracker WebSocket compatible WebTorrent (et serveur du site)
site/                           le site vitrine (PWA, Three.js) et la page du client (site/app/)
scripts/                        construction, génération des images
tests/e2e.cjs                   deux navigateurs s'échangent un fichier de bout en bout
```

```
 navigateur A                         tracker (WebSocket)                       navigateur B
┌──────────────────────┐   offres SDP   ┌─────────────────┐   offres SDP   ┌──────────────────────┐
│ egui (interface)     │ ─────────────▶ │ relaie offres   │ ─────────────▶ │ egui (interface)     │
│ engine (WebRTC, WS)  │ ◀───────────── │ et réponses     │ ◀───────────── │ engine (WebRTC, WS)  │
│ mxtorrent-core       │                └─────────────────┘                │ mxtorrent-core       │
└──────────┬───────────┘                                                   └───────────┬──────────┘
           └──────────────── canal de données WebRTC chiffré (protocole BitTorrent) ───┘
```

**`mxtorrent-core`** ne connaît ni le réseau, ni l'horloge, ni les fichiers : on lui donne des octets
(`on_data`), il rend des octets (`poll_transmit`) et des événements. C'est ce qui permet de le tester
par des essaims entiers simulés en mémoire, et de le brancher sur WebRTC dans le navigateur.

**`mxtorrent-app`** relie le moteur au navigateur. Chaque rappel JavaScript (WebSocket, WebRTC,
`fetch`) dépose un événement dans une file que le moteur vide quand il a la main : pas de
réentrance, pas de verrou. Une horloge fait avancer les sessions même quand l'onglet est en arrière-plan.

## Démarrage rapide

Il faut Rust (stable), la cible WebAssembly et `wasm-bindgen` :

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
npm ci                      # facultatif : wasm-opt (taille) et Playwright (tests)

./scripts/build.sh          # construit dist/ : le site, le client, le module wasm
cargo run -p mxtorrent-tracker -- --static dist
```

Puis ouvrez <http://localhost:8000> (le site) ou <http://localhost:8000/app/> (le client).
Le même serveur est aussi un tracker : `ws://localhost:8000/announce`.

## Tests

```sh
cargo test -p mxtorrent-core -p mxtorrent-tracker -p mxtorrent-app   # unitaires + essaims simulés
node tests/e2e.cjs http://localhost:8000                             # deux Chromium, un vrai échange WebRTC
```

Le test de bout en bout ouvre deux navigateurs isolés : le premier partage un fichier, le second
l'ajoute par son lien magnet, récupère les métadonnées, télécharge en WebRTC et compare l'empreinte
SHA-256. La CI GitHub Actions lance tout cela à chaque envoi.

## Tracker auto-hébergé

```sh
cargo run -p mxtorrent-tracker --release -- --port 8000
```

Placez-le derrière un proxy HTTPS (Caddy, nginx…) pour obtenir une adresse `wss://`, puis ajoutez-la
dans **Réglages → Trackers WebSocket** du client. Il respecte la variable `PORT` (Heroku, Render, Fly.io…).

## Déploiement sur GitHub Pages

Le workflow `.github/workflows/pages.yml` construit et publie `dist/` à chaque envoi sur `main`.
Il suffit, une fois, d'aller dans **Settings → Pages** du dépôt et de choisir **GitHub Actions** comme source.

## Ce qu'un navigateur ne peut pas faire

Soyons honnêtes sur les limites :

- **Pairs WebRTC uniquement.** Un navigateur ne peut pas ouvrir de connexions TCP ou UDP brutes :
  mxTorrent échange avec les clients WebTorrent (et les sources web), pas avec les pairs qui ne parlent
  que TCP/UDP (qBittorrent, Transmission… sans WebRTC). Les trackers `udp://` et `http://` sont ignorés.
- **Stockage en mémoire.** Les fichiers vivent dans la mémoire de l'onglet : quelques centaines de Mio
  partout, jusqu'à ~2 Gio sur ordinateur. Enregistrez-les une fois complets ; gardez l'onglet ouvert pour partager.
- **Accessibilité.** egui dessine dans un canevas ; la prise en charge des lecteurs d'écran reste limitée.

## Licence

[MIT](LICENSE) © Maxime Lestage. Partagez uniquement des contenus dont vous avez les droits.
