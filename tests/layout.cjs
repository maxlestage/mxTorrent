// Aucun défilement horizontal : chaque page, à chaque largeur, tient dans la fenêtre.
// Pour chaque page principale et chaque largeur (320 à 1920 px), on vérifie :
//   - document.documentElement.scrollWidth <= clientWidth (et celui du corps) ;
//   - qu'un défilement latéral est impossible (scrollX reste à 0) ;
//   - que le débordement n'est pas masqué globalement (overflow-x « visible » sur html et body) ;
//   - que chaque lien, bouton ou champ visible tient dans la largeur (rien de coupé, même
//     dans les barres fixes).
// Données réalistes : noms de torrents très longs sans espaces, FAQ et menu ouverts, FR et EN.
//   node tests/layout.cjs [http://localhost:8000]
const { chromium } = require("playwright");

const BASE = (process.argv[2] || "http://localhost:8000").replace(/\/+$/, "");
const WIDTHS = [320, 375, 768, 1024, 1280, 1920];
const LONG = "Big.Buck.Bunny." + "UnNomDeTorrentTresLongSansAucunEspace".repeat(6) + ".2160p.HDR.x265-mxTorrent";
const failures = [];

async function newPage(browser, w) {
  const small = w < 760;
  const ctx = await browser.newContext({ viewport: { width: w, height: 800 }, isMobile: small, hasTouch: small });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log(`[${w}px] erreur : ${e.message}`));
  return { ctx, page };
}

async function settle(page, ms = 800) {
  await Promise.race([page.evaluate(() => document.fonts.ready.then(() => true)), page.waitForTimeout(5000)]);
  await page.waitForTimeout(ms);
}

async function check(page, label, { noMask = false } = {}) {
  const m = await page.evaluate(() => {
    const d = document.documentElement;
    const b = document.body;
    const vw = d.clientWidth;
    const y = scrollY;
    // On tente un défilement latéral : il doit rester sans effet.
    scrollTo({ left: vw, top: y, behavior: "instant" });
    const moved = scrollX;
    scrollTo({ left: 0, top: y, behavior: "instant" });
    const name = (el) => {
      const cls = typeof el.className === "string" && el.className.trim() ? "." + el.className.trim().split(/\s+/).join(".") : "";
      const r = el.getBoundingClientRect();
      return `${el.tagName.toLowerCase()}${el.id ? "#" + el.id : ""}${cls} (${Math.round(r.left)} → ${Math.round(r.right)})`;
    };
    const outside = (el) => {
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.height > 0 && (r.right > vw + 1 || r.left < -1);
    };
    let culprits = [];
    if (d.scrollWidth > vw || b.scrollWidth > vw) {
      culprits = [...document.querySelectorAll("body *")].filter((el) => outside(el) && getComputedStyle(el).position !== "fixed").slice(0, 10).map(name);
    }
    // Commandes visibles coupées par le bord de la fenêtre (barres fixes comprises).
    const cut = [...document.querySelectorAll("a, button, input, summary, select, textarea")]
      .filter((el) => (el.checkVisibility ? el.checkVisibility({ visibilityProperty: true }) : el.offsetParent !== null))
      .filter((el) => !el.closest(".sr-only"))
      .filter(outside)
      .slice(0, 10)
      .map(name);
    return { sw: d.scrollWidth, bw: b.scrollWidth, cw: vw, moved, ox: [getComputedStyle(d).overflowX, getComputedStyle(b).overflowX], culprits, cut };
  });
  const problems = [];
  // En mode mobile, le navigateur pourrait élargir la zone de mise en page pour loger un
  // contenu trop large : la largeur visible doit rester celle de la fenêtre demandée.
  const vp = page.viewportSize();
  if (vp && m.cw > vp.width) problems.push(`zone de mise en page élargie (${m.cw} > fenêtre ${vp.width})`);
  if (m.sw > m.cw) problems.push(`document.scrollWidth ${m.sw} > clientWidth ${m.cw}`);
  if (m.bw > m.cw) problems.push(`body.scrollWidth ${m.bw} > clientWidth ${m.cw}`);
  if (m.moved !== 0) problems.push(`défilement latéral possible (scrollX = ${m.moved})`);
  if (noMask && m.ox.some((v) => v !== "visible")) problems.push(`débordement masqué globalement (overflow-x : ${m.ox.join(", ")})`);
  if (m.cut.length) problems.push(`commandes coupées : ${m.cut.join(", ")}`);
  if (problems.length) {
    failures.push(`${label} : ${problems.join(" ; ")}${m.culprits.length ? "\n    en cause : " + m.culprits.join(", ") : ""}`);
    console.log(`✗ ${label}`);
  } else {
    console.log(`✓ ${label}`);
  }
}

async function home(browser, w) {
  const { ctx, page } = await newPage(browser, w);
  await page.goto(BASE + "/?lang=fr", { waitUntil: "domcontentloaded" });
  await settle(page, 1200);
  await check(page, `accueil ${w}px`, { noMask: true });

  await page.evaluate(() => document.querySelectorAll("details").forEach((d) => { d.open = true; }));
  await page.waitForTimeout(300);
  await check(page, `accueil ${w}px, FAQ ouverte`, { noMask: true });

  // Un nom de torrent très long, sans espaces, dans les deux champs, et le message d'erreur.
  await page.fill("#magnet", LONG);
  await page.fill("#magnet2", LONG);
  await page.evaluate(() => document.querySelector(".magnet-form button[type=submit]").click());
  await page.waitForTimeout(700);
  await check(page, `accueil ${w}px, nom très long et message`, { noMask: true });

  if (w < 1000) {
    await page.evaluate(() => document.querySelector(".burger").click());
    await page.waitForTimeout(900);
    await check(page, `accueil ${w}px, menu ouvert`);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(900);
  }

  await page.evaluate(() => document.querySelector(".nav .lang-toggle").click());
  await page.waitForTimeout(600);
  await check(page, `accueil ${w}px, anglais`, { noMask: true });

  // Toute la page parcourue : apparitions, 3D épinglée, pied de page.
  await page.evaluate(async () => {
    for (let y = 0; y < document.documentElement.scrollHeight; y += innerHeight / 2) {
      scrollTo({ left: 0, top: y, behavior: "instant" });
      await new Promise((r) => setTimeout(r, 60));
    }
  });
  await page.waitForTimeout(800);
  await check(page, `accueil ${w}px, bas de page`, { noMask: true });
  await ctx.close();
}

async function simple(browser, w, path, label) {
  const { ctx, page } = await newPage(browser, w);
  await page.goto(BASE + path, { waitUntil: "domcontentloaded" });
  await settle(page);
  await check(page, `${label} ${w}px`, { noMask: true });
  await page.evaluate(() => scrollTo({ left: 0, top: document.documentElement.scrollHeight, behavior: "instant" }));
  await page.waitForTimeout(300);
  await check(page, `${label} ${w}px, bas de page`, { noMask: true });
  await ctx.close();
}

async function app(browser, w) {
  const { ctx, page } = await newPage(browser, w);
  await page.goto(BASE + "/app/?lang=fr");
  await page.waitForFunction(() => window.mxtorrent && document.getElementById("boot") === null, null, { timeout: 60000 });
  await page.waitForTimeout(600);
  await check(page, `client ${w}px, accueil`);

  // Un partage et un lien magnet aux noms très longs, sans espaces (la fenêtre « Partager » s'ouvre).
  await page.evaluate((name) => {
    window.mxtorrent.share_file(name + ".mkv", new Uint8Array(200000).map((_, i) => i % 251));
    window.mxtorrent.add_link("magnet:?xt=urn:btih:" + "c9e15763f722f23e98a29decdfae341b98d53056" + "&dn=" + encodeURIComponent(name));
  }, LONG);
  await page.waitForFunction(() => (JSON.parse(window.mxtorrent.status()).torrents || []).length >= 2, null, { timeout: 30000 });
  await page.waitForTimeout(1500);
  await check(page, `client ${w}px, noms très longs`);
  const state = await page.evaluate(() => {
    const c = document.getElementById("mx").getBoundingClientRect();
    return { panicked: window.mxtorrent.has_panicked(), canvasRight: c.right, vw: document.documentElement.clientWidth };
  });
  if (state.panicked) failures.push(`client ${w}px : l'application a planté avec des noms très longs`);
  if (state.canvasRight > state.vw + 1) failures.push(`client ${w}px : le canevas (${state.canvasRight}) dépasse la fenêtre (${state.vw})`);
  await ctx.close();
}

(async () => {
  const browser = await chromium.launch({ args: ["--disable-features=WebRtcHideLocalIpsWithMdns"] });
  for (const w of WIDTHS) {
    await home(browser, w);
    await simple(browser, w, "/legal.html", "informations légales");
    await simple(browser, w, "/offline.html", "hors ligne");
    await simple(browser, w, "/page-qui-n-existe-pas", "page 404");
    await app(browser, w);
  }
  await browser.close();
  if (failures.length) {
    console.error(`\nÉCHEC : défilement horizontal ou contenu coupé (${failures.length})\n- ${failures.join("\n- ")}`);
    process.exit(1);
  }
  console.log("\nOK : aucune page ne défile horizontalement, de 320 à 1920 px.");
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
