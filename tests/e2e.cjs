// Test de bout en bout : deux navigateurs isolés, un tracker local.
// Le premier partage un fichier, le second le télécharge par lien magnet, en WebRTC.
//   node tests/e2e.cjs [http://localhost:8000]
const { chromium } = require("playwright");
const BASE = process.argv[2] || "http://localhost:8000";
const TRACKER = BASE.replace(/^http/, "ws") + "/announce";
const SIZE = Number(process.env.SIZE || 3 * 1024 * 1024 + 12345);

async function openClient(browser, name) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  page.on("console", (m) => { if (m.type() === "error") console.log(`[${name}] ${m.text()}`); });
  page.on("pageerror", (e) => console.log(`[${name}] erreur : ${e.message}`));
  await page.goto(BASE + "/app/");
  await page.waitForFunction(() => window.mxtorrent && document.getElementById("boot") === null, null, { timeout: 60000 });
  await page.evaluate((t) => window.mxtorrent.set_trackers([t]), TRACKER);
  return page;
}

const status = (page) => page.evaluate(() => JSON.parse(window.mxtorrent.status()));

(async () => {
  const browser = await chromium.launch({ args: ["--disable-features=WebRtcHideLocalIpsWithMdns"] });
  const t0 = Date.now();
  const a = await openClient(browser, "A");
  const b = await openClient(browser, "B");

  // A partage un fichier pseudo-aléatoire.
  const digest = await a.evaluate(async (size) => {
    const data = new Uint8Array(size);
    let x = 12345;
    for (let i = 0; i < size; i++) { x = (x * 1103515245 + 12345) >>> 0; data[i] = x >>> 24; }
    window.mxtorrent.share_file("essai.bin", data);
    const h = await crypto.subtle.digest("SHA-256", data);
    return Array.from(new Uint8Array(h)).map((v) => v.toString(16).padStart(2, "0")).join("");
  }, SIZE);
  let sa;
  for (let i = 0; i < 100; i++) {
    sa = await status(a);
    if (sa.torrents && sa.torrents.length) break;
    await a.waitForTimeout(100);
  }
  const t = sa.torrents[0];
  console.log(`A partage ${t.name} (${t.state}) : ${t.magnet.slice(0, 70)}…`);

  // B l'ajoute par son lien magnet.
  await b.evaluate((m) => window.mxtorrent.add_link(m), t.magnet);
  let sb, last = "";
  const deadline = Date.now() + 90000;
  while (Date.now() < deadline) {
    sb = await status(b);
    const x = sb.torrents[0];
    const line = x ? `${x.state} ${(x.progress * 100).toFixed(1)} % pairs=${x.peers} trackers=${sb.trackers}` : "…";
    if (line !== last) { console.log("B :", line); last = line; }
    if (x && x.state === "Seeding") break;
    await b.waitForTimeout(250);
  }
  const x = sb.torrents[0];
  if (!x || x.state !== "Seeding") { console.error("ÉCHEC : téléchargement inachevé"); process.exit(1); }
  const got = await b.evaluate(async (ih) => {
    const bytes = window.mxtorrent.file_bytes(ih, 0);
    const h = await crypto.subtle.digest("SHA-256", bytes);
    return Array.from(new Uint8Array(h)).map((v) => v.toString(16).padStart(2, "0")).join("");
  }, x.info_hash);
  const ok = got === digest;
  const secs = (Date.now() - t0) / 1000;
  console.log(ok ? `OK : ${SIZE} octets identiques (SHA-256 ${got.slice(0, 16)}…) en ${secs.toFixed(1)} s` : `ÉCHEC : empreintes différentes`);
  const sa2 = await status(a);
  console.log(`A a envoyé ${sa2.torrents[0].uploaded} octets`);
  await browser.close();
  process.exit(ok ? 0 : 1);
})();
