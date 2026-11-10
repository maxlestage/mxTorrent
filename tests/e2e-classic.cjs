// Test de bout en bout avec un vrai client classique : un navigateur télécharge,
// par la passerelle chiffrée Brocolis, un fichier que seul un pair TCP possède
// (Transmission, annoncé sur un tracker UDP).
//   node tests/e2e-classic.cjs <base> <lien magnet> <sha256 attendu>
const { chromium } = require("playwright");
const [BASE, MAGNET, SHA] = process.argv.slice(2);
(async () => {
  const browser = await chromium.launch();
  const page = await (await browser.newContext()).newPage();
  page.on("pageerror", (e) => console.log("erreur :", e.message));
  const t0 = Date.now();
  await page.goto(BASE + "/app/");
  await page.waitForFunction(() => window.mxtorrent && !document.getElementById("boot"), null, { timeout: 60000 });
  // Aucun tracker WebSocket : seul le réseau classique, par la passerelle.
  await page.evaluate((b) => { window.mxtorrent.set_trackers([]); window.mxtorrent.set_gateway(b); }, BASE + "/");
  await page.evaluate((m) => window.mxtorrent.add_link(m), MAGNET);
  let last = "", s, x;
  const deadline = Date.now() + 120000;
  while (Date.now() < deadline) {
    const st = await page.evaluate(() => JSON.parse(window.mxtorrent.status()));
    // Moteur occupé à cet instant : « {} », on réessaie.
    if (!st.gateway) { await page.waitForTimeout(100); continue; }
    s = st;
    x = s.torrents[0];
    const line = `passerelle=${s.gateway.phase} pairs relayés=${s.gateway.peers} · ${x ? x.state + " " + (x.progress * 100).toFixed(0) + " %" : "…"}`;
    if (line !== last) { console.log(`${((Date.now() - t0) / 1000).toFixed(1)} s : ${line}`); last = line; }
    if (x && x.state === "Seeding") break;
    await page.waitForTimeout(300);
  }
  if (!x || x.state !== "Seeding") {
    console.log("ÉCHEC : téléchargement inachevé", JSON.stringify(s && s.gateway));
    if (x) console.log("pairs :", await page.evaluate((ih) => window.mxtorrent.peers(ih), x.info_hash));
    console.log("journal :\n" + (await page.evaluate(() => window.mxtorrent.log())));
    process.exit(1);
  }
  const got = await page.evaluate(async (ih) => {
    const h = await crypto.subtle.digest("SHA-256", window.mxtorrent.file_bytes(ih, 0));
    return Array.from(new Uint8Array(h)).map((v) => v.toString(16).padStart(2, "0")).join("");
  }, x.info_hash);
  const ok = got.startsWith(SHA);
  console.log(ok ? `OK : fichier identique (SHA-256 ${got.slice(0, 16)}…), empreinte de la passerelle ${s.gateway.fingerprint}` : `ÉCHEC : empreinte ${got}`);
  await browser.close();
  process.exit(ok ? 0 : 1);
})();
