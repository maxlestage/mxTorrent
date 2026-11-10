// mxTorrent — service worker : hors ligne, partage Android, liens magnet.
//
// Règle d'or : ne jamais servir une ancienne version quand le réseau répond.
// - Pages, scripts, styles : le réseau d'abord (une revalidation rapide), le cache
//   seulement hors ligne.
// - Fichiers versionnés (`?v=…`, le module WebAssembly et sa colle) et images :
//   le cache d'abord — leur adresse change à chaque version, ils ne périment pas.
// - À chaque nouvelle version, tous les anciens caches sont supprimés.
const VERSION = "__BUILD__";
const CORE = `mx-core-${VERSION}`;
const RUNTIME = `mx-runtime-${VERSION}`;
const SHARE = "mx-share";

const PRECACHE = [
  "./",
  "index.html",
  "legal.html",
  "offline.html",
  "css/site.css",
  "js/main.js",
  "js/scenes.js",
  "js/i18n.js",
  "vendor/three.module.min.js",
  "manifest.webmanifest",
  "assets/icon.svg",
  "assets/icon-192.png",
  "assets/icon-512.png",
  "app/",
  `app/pkg/mxtorrent_app.js?v=${VERSION}`,
  `app/pkg/mxtorrent_app_bg.wasm?v=${VERSION}`,
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CORE)
      .then((c) => c.addAll(PRECACHE.map((u) => new Request(u, { cache: "reload" }))))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CORE && k !== RUNTIME && k !== SHARE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

// Fichiers ou liens partagés depuis le menu « Partager » du système (Android).
async function handleShare(request) {
  const base = new URL("./app/", self.registration.scope);
  const form = await request.formData();
  const files = form.getAll("files").filter((f) => f && typeof f !== "string");
  if (files.length) {
    const cache = await caches.open(SHARE);
    for (const [i, f] of files.entries()) {
      await cache.put(new Request(`./shared/${Date.now()}-${i}`), new Response(f, { headers: { "x-name": encodeURIComponent(f.name || "fichier") } }));
    }
    return Response.redirect(new URL("#shared", base).href, 303);
  }
  const text = [form.get("url"), form.get("text"), form.get("title")].filter(Boolean).join(" ");
  const m = text.match(/magnet:\?[^\s"']+/i) || text.match(/https?:\/\/\S+\.torrent\b/i);
  if (m) base.searchParams.set("magnet", m[0]);
  return Response.redirect(base.href, 303);
}

async function fromCaches(req, opts) {
  for (const name of [CORE, RUNTIME]) {
    const hit = await (await caches.open(name)).match(req, opts);
    if (hit) return hit;
  }
  return undefined;
}

async function cacheFirst(req) {
  const hit = await fromCaches(req);
  if (hit) return hit;
  const res = await fetch(req);
  if (res.ok) (await caches.open(RUNTIME)).put(req, res.clone());
  return res;
}

async function networkFirst(req, fallback) {
  try {
    // « no-cache » : le navigateur revalide auprès du serveur (304 si rien n'a changé).
    const res = await fetch(req, { cache: "no-cache" });
    if (res.ok) (await caches.open(RUNTIME)).put(req, res.clone());
    return res;
  } catch (e) {
    const hit = (await fromCaches(req)) || (await fromCaches(req, { ignoreSearch: true }));
    if (hit) return hit;
    if (fallback) return (await fromCaches(fallback)) || Response.error();
    throw e;
  }
}

self.addEventListener("fetch", (event) => {
  const req = event.request;
  const url = new URL(req.url);

  if (req.method === "POST" && url.pathname.endsWith("/app/share")) {
    event.respondWith(handleShare(req));
    return;
  }
  if (req.method !== "GET") return;

  // Polices Google : elles ne changent pas.
  if (url.origin === "https://fonts.googleapis.com" || url.origin === "https://fonts.gstatic.com") {
    event.respondWith(cacheFirst(req));
    return;
  }
  if (url.origin !== self.location.origin) return;
  // Le tracker et la passerelle ne passent jamais par le cache.
  if (url.pathname.endsWith("/announce") || url.pathname.includes("/relay")) return;

  if (req.mode === "navigate") {
    event.respondWith(networkFirst(req, "offline.html"));
    return;
  }
  const immutable = url.searchParams.has("v") || /\/(assets|vendor)\//.test(url.pathname) || /\.(png|jpe?g|svg|webp|woff2?|ttf)$/.test(url.pathname);
  event.respondWith(immutable ? cacheFirst(req) : networkFirst(req));
});
