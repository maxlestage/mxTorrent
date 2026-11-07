// Génère les icônes PNG, l'image de partage (Open Graph) et les captures du client.
//   node scripts/assets.cjs [http://localhost:8000]   (captures : tracker local + dist/ construits)
const { chromium } = require("playwright");
const fs = require("fs");
const path = require("path");
const SITE = path.join(__dirname, "..", "site");
const BASE = process.argv[2];
const svg = fs.readFileSync(path.join(SITE, "assets/icon.svg"), "utf8");

(async () => {
  const browser = await chromium.launch({ args: ["--disable-features=WebRtcHideLocalIpsWithMdns"] });
  const page = await browser.newPage();
  const icon = async (file, size, pad = 0, bg = "transparent") => {
    await page.setViewportSize({ width: size, height: size });
    await page.setContent(`<body style="margin:0;background:${bg};display:grid;place-items:center;width:${size}px;height:${size}px">
      <div style="width:${size - pad * 2}px;height:${size - pad * 2}px">${svg.replace("<svg ", '<svg width="100%" height="100%" ')}</div></body>`);
    await page.screenshot({ path: path.join(SITE, "assets", file), omitBackground: bg === "transparent" });
  };
  await icon("icon-192.png", 192);
  await icon("icon-512.png", 512);
  await icon("apple-touch-icon.png", 180, 0, "#07070d");
  await icon("favicon-32.png", 32);
  // Icône « maskable » : fond plein et marge de sécurité.
  await page.setViewportSize({ width: 512, height: 512 });
  await page.setContent(`<body style="margin:0;width:512px;height:512px;background:radial-gradient(70% 70% at 50% 38%,#24145a,#07070d);display:grid;place-items:center">
    <div style="width:330px;height:330px">${svg.replace('<rect width="512" height="512" rx="112" fill="url(#bg)"/>', "").replace("<svg ", '<svg width="100%" height="100%" ')}</div></body>`);
  await page.screenshot({ path: path.join(SITE, "assets/icon-maskable-512.png") });

  // Image de partage 1200 × 630.
  await page.setViewportSize({ width: 1200, height: 630 });
  await page.setContent(`<body style="margin:0;width:1200px;height:630px;overflow:hidden;background:radial-gradient(60% 70% at 75% 40%,#2a1660,#07070d 70%);font-family:'Space Grotesk',system-ui,sans-serif;color:#ececfa">
    <link href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@500;700&display=swap" rel="stylesheet">
    <div style="position:absolute;right:60px;top:95px;width:440px;height:440px">${svg.replace("<svg ", '<svg width="100%" height="100%" ')}</div>
    <div style="position:absolute;left:80px;top:150px;width:640px">
      <div style="font-size:30px;color:#22d3ee;letter-spacing:.12em;text-transform:uppercase;font-weight:500">Rust · WebAssembly · egui</div>
      <div style="font-size:96px;font-weight:700;line-height:1;margin:18px 0"><span style="color:#8b5cf6">mx</span>Torrent</div>
      <div style="font-size:38px;line-height:1.25;color:#c8c8e0">Le torrent directement dans le navigateur. Rien à installer.</div>
    </div></body>`);
  await page.waitForTimeout(1200);
  await page.screenshot({ path: path.join(SITE, "assets/og.png") });

  if (BASE) {
    // Captures du vrai client : un partage et un téléchargement en cours.
    for (const [file, opts] of [["desktop.png", { viewport: { width: 1280, height: 800 } }], ["phone.png", { viewport: { width: 412, height: 860 }, isMobile: true, hasTouch: true }]]) {
      const ctx = await browser.newContext({ ...opts, locale: "fr-FR" });
      const p = await ctx.newPage();
      await p.goto(BASE + "/app/");
      await p.waitForFunction(() => window.mxtorrent && !document.getElementById("boot"), null, { timeout: 60000 });
      await p.evaluate((t) => window.mxtorrent.set_trackers([t]), BASE.replace(/^http/, "ws") + "/announce");
      await p.evaluate(() => {
        const mk = (n, s) => { const d = new Uint8Array(n); for (let i = 0; i < n; i++) d[i] = (i * s) & 255; return d; };
        window.mxtorrent.share_file("Big-Buck-Bunny-1080p.mp4", mk(2_400_000, 7));
        window.mxtorrent.share_file("photos-vacances.zip", mk(900_000, 13));
        window.mxtorrent.add_link("magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny");
      });
      await p.waitForTimeout(1500);
      // Ferme les fenêtres (bienvenue, partage) et laisse partir les notifications.
      for (let i = 0; i < 3; i++) { await p.keyboard.press("Escape"); await p.waitForTimeout(300); }
      await p.waitForTimeout(5500);
      await p.screenshot({ path: path.join(SITE, "assets/screens", file) });
      await ctx.close();
    }
  }
  await browser.close();
})();
