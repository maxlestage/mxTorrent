// Mise à jour : un navigateur qui a déjà une version installée (service worker et cache)
// doit voir la nouvelle version dès qu'il recharge la page — jamais l'ancienne.
//   node tests/update.cjs <base> <dossier servi> <commande qui y installe la nouvelle version>
const { chromium } = require("playwright");
const { execSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");
const [BASE, , INSTALL_NEW] = process.argv.slice(2);

(async () => {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "mx-maj-"));
  const ctx = await chromium.launchPersistentContext(profile);
  const page = await ctx.newPage();
  // La page peut se recharger d'elle-même (nouvelle version) : on réessaie.
  const build = async () => {
    for (let i = 0; i < 20; i++) {
      try {
        await page.waitForFunction(() => window.mxtorrent && !document.getElementById("boot"), null, { timeout: 60000 });
        return await page.evaluate(() => window.mxtorrentBuild || "(ancienne version sans numéro)");
      } catch (e) {
        await page.waitForTimeout(500);
      }
    }
    throw new Error("l'application ne démarre pas");
  };
  await page.goto(BASE + "/app/");
  const first = await build();
  // Laisse le service worker s'installer et remplir son cache, puis recharge sous son contrôle.
  await page.waitForFunction(() => navigator.serviceWorker.controller || navigator.serviceWorker.ready, null, { timeout: 30000 });
  await page.waitForTimeout(3000);
  await page.reload();
  const cached = await build();
  console.log("version installée :", first, "| servie par le cache :", cached);

  execSync(INSTALL_NEW, { stdio: "inherit" });
  const expected = fs.readFileSync(path.join(process.argv[3], "app/index.html"), "utf8").match(/mxtorrentBuild = "([^"]+)"/)[1];

  await page.reload();
  let got = await build();
  // Une nouvelle version peut prendre la main et recharger la page d'elle-même.
  for (let i = 0; i < 10 && got !== expected; i++) {
    await page.waitForTimeout(1000);
    got = await build().catch(() => got);
  }
  const ok = got === expected;
  console.log(ok ? `OK : après un seul rechargement, nouvelle version ${got}` : `ÉCHEC : version ${got} au lieu de ${expected}`);
  await ctx.close();
  process.exit(ok ? 0 : 1);
})();
