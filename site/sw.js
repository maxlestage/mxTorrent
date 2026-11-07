// mxTorrent — service worker : hors ligne, partage Android, liens magnet.
const VERSION = "__BUILD__";
const CORE = `mx-core-${VERSION}`;
const RUNTIME = "mx-runtime";
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
  "assets/screens/desktop.png",
  "assets/screens/phone.png",
  "app/",
  "app/pkg/mxtorrent_app.js",
  "app/pkg/mxtorrent_app_bg.wasm",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(CORE).then((c) => c.addAll(PRECACHE)).then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith("mx-core-") && k !== CORE).map((k) => caches.delete(k))))
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

self.addEventListener("fetch", (event) => {
  const req = event.request;
  const url = new URL(req.url);

  if (req.method === "POST" && url.pathname.endsWith("/app/share")) {
    event.respondWith(handleShare(req));
    return;
  }
  if (req.method !== "GET") return;

  // Polices Google : cache d'abord.
  if (url.origin === "https://fonts.googleapis.com" || url.origin === "https://fonts.gstatic.com") {
    event.respondWith(
      caches.open(RUNTIME).then(async (c) => {
        const hit = await c.match(req);
        if (hit) return hit;
        const res = await fetch(req);
        if (res.ok || res.type === "opaque") c.put(req, res.clone());
        return res;
      }),
    );
    return;
  }
  if (url.origin !== self.location.origin) return;

  // Pages : le réseau d'abord, le cache sinon, puis la page hors ligne.
  if (req.mode === "navigate") {
    event.respondWith(
      fetch(req)
        .then((res) => {
          const copy = res.clone();
          caches.open(RUNTIME).then((c) => c.put(req, copy));
          return res;
        })
        .catch(async () => (await caches.match(req, { ignoreSearch: true })) || (await caches.match("offline.html"))),
    );
    return;
  }

  // Le reste : réponse immédiate depuis le cache, mise à jour en arrière-plan.
  event.respondWith(
    caches.match(req).then((hit) => {
      const net = fetch(req)
        .then((res) => {
          if (res.ok) {
            const copy = res.clone();
            caches.open(RUNTIME).then((c) => c.put(req, copy));
          }
          return res;
        })
        .catch(() => hit);
      return hit || net;
    }),
  );
});
