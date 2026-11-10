// mxTorrent — interactions du site : langue, animations, 3D, installation.
import { EN } from "./i18n.js";

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

// ------------------------------------------------------------------ langue
const FR = {};
$$("[data-i18n]").forEach((el) => { FR[el.dataset.i18n] = el.textContent; });
$$("[data-i18n-placeholder]").forEach((el) => { FR["ph:" + el.dataset.i18nPlaceholder] = el.placeholder; });

function pickLang() {
  const q = new URLSearchParams(location.search).get("lang");
  if (q === "en" || q === "fr") return q;
  try {
    const saved = localStorage.getItem("mx-lang");
    if (saved) return saved;
  } catch {}
  return (navigator.language || "fr").toLowerCase().startsWith("fr") ? "fr" : "en";
}
let lang = pickLang();

function applyLang() {
  document.documentElement.lang = lang;
  const dict = lang === "en" ? EN : FR;
  $$("[data-i18n]").forEach((el) => {
    const v = dict[el.dataset.i18n] ?? FR[el.dataset.i18n];
    if (v != null) el.textContent = v;
  });
  $$("[data-i18n-placeholder]").forEach((el) => {
    const k = el.dataset.i18nPlaceholder;
    el.placeholder = (lang === "en" ? EN["ph:" + k] : null) ?? FR["ph:" + k];
  });
  $$("[data-lang-label]").forEach((el) => { el.textContent = lang === "en" ? "FR" : "EN"; });
  // Les liens vers le client gardent la langue choisie.
  $$("a[href^='app/'], form.magnet-form, form.footer-magnet").forEach((el) => {
    if (el.tagName === "A") el.setAttribute("href", `app/?lang=${lang}`);
    else {
      let h = el.querySelector("input[name=lang]");
      if (!h) { h = document.createElement("input"); h.type = "hidden"; h.name = "lang"; el.appendChild(h); }
      h.value = lang;
    }
  });
  document.title = lang === "en" ? "mxTorrent — torrents right in your browser" : "mxTorrent — le torrent directement dans le navigateur";
  splitTitles();
}

$$(".lang-toggle").forEach((b) => b.addEventListener("click", () => {
  lang = lang === "en" ? "fr" : "en";
  try { localStorage.setItem("mx-lang", lang); } catch {}
  applyLang();
}));

// ------------------------------------------------------------------ titre lettre par lettre
function splitTitles() {
  let delay = 0.15;
  $$(".split").forEach((el) => {
    const text = el.textContent;
    el.setAttribute("aria-label", text);
    el.textContent = "";
    text.split(" ").forEach((word, wi, words) => {
      const w = document.createElement("span");
      w.className = "word";
      w.setAttribute("aria-hidden", "true");
      for (const c of word) {
        const s = document.createElement("span");
        s.className = "ch";
        s.textContent = c;
        s.style.animationDelay = `${delay}s`;
        delay += 0.028;
        w.appendChild(s);
      }
      el.appendChild(w);
      if (wi < words.length - 1) el.appendChild(document.createTextNode(" "));
    });
  });
}
applyLang();

// ------------------------------------------------------------------ apparitions au défilement
const revealObs = new IntersectionObserver((entries) => {
  entries.forEach((e) => {
    if (e.isIntersecting) {
      e.target.classList.add("in");
      revealObs.unobserve(e.target);
    }
  });
}, { threshold: 0.12, rootMargin: "0px 0px -40px 0px" });
$$(".reveal").forEach((el) => {
  // Décalage en cascade entre éléments frères.
  const sibs = [...el.parentElement.children].filter((c) => c.classList.contains("reveal"));
  el.style.setProperty("--d", `${Math.min(sibs.indexOf(el), 8) * 0.08}s`);
  revealObs.observe(el);
});

// ------------------------------------------------------------------ compteurs
const countObs = new IntersectionObserver((entries) => {
  entries.forEach((e) => {
    if (!e.isIntersecting) return;
    countObs.unobserve(e.target);
    const el = e.target;
    const to = Number(el.dataset.count);
    const suffix = el.dataset.suffix || "";
    const t0 = performance.now();
    const dur = reduced ? 1 : 1600;
    const step = (now) => {
      const k = Math.min(1, (now - t0) / dur);
      const v = Math.round(to * (1 - Math.pow(1 - k, 4)));
      el.textContent = v + suffix;
      if (k < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });
}, { threshold: 0.6 });
$$("[data-count]").forEach((el) => countObs.observe(el));

// ------------------------------------------------------------------ navigation
const nav = $("#nav");
const progress = $(".read-progress span");
let lastY = scrollY;
function onScroll() {
  const y = scrollY;
  nav.classList.toggle("scrolled", y > 20);
  nav.classList.toggle("hidden", y > lastY && y > 400 && !menu.classList.contains("open"));
  lastY = y;
  const max = document.documentElement.scrollHeight - innerHeight;
  progress.style.transform = `scaleX(${max > 0 ? y / max : 0})`;
}
addEventListener("scroll", onScroll, { passive: true });

const links = $$(".nav-links a");
const sections = links.map((a) => $(a.getAttribute("href"))).filter(Boolean);
const navObs = new IntersectionObserver((entries) => {
  entries.forEach((e) => {
    if (e.isIntersecting) {
      links.forEach((a) => a.classList.toggle("active", a.getAttribute("href") === "#" + e.target.id));
    }
  });
}, { rootMargin: "-45% 0px -50% 0px" });
sections.forEach((s) => navObs.observe(s));

// Menu plein écran du téléphone.
const burger = $(".burger");
const menu = $("#menu");
function setMenu(open) {
  burger.setAttribute("aria-expanded", String(open));
  if (open) {
    menu.hidden = false;
    requestAnimationFrame(() => menu.classList.add("open"));
  } else {
    menu.classList.remove("open");
    setTimeout(() => { if (!menu.classList.contains("open")) menu.hidden = true; }, 700);
  }
  document.body.style.overflow = open ? "hidden" : "";
}
burger.addEventListener("click", () => setMenu(burger.getAttribute("aria-expanded") !== "true"));
$$("#menu a").forEach((a) => a.addEventListener("click", () => setMenu(false)));
addEventListener("keydown", (e) => { if (e.key === "Escape") setMenu(false); });

onScroll();

// ------------------------------------------------------------------ effets de pointeur
const fine = matchMedia("(hover: hover) and (pointer: fine)").matches;

// Cartes inclinées en 3D, avec un projecteur qui suit le pointeur.
$$(".tilt").forEach((card) => {
  card.addEventListener("pointermove", (e) => {
    const r = card.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    card.style.setProperty("--mx", `${x * 100}%`);
    card.style.setProperty("--my", `${y * 100}%`);
    if (fine && !reduced) card.style.transform = `rotateX(${(0.5 - y) * 14}deg) rotateY(${(x - 0.5) * 16}deg) translateZ(10px)`;
  });
  card.addEventListener("pointerleave", () => { card.style.transform = ""; });
});

// Boutons « aimantés ».
if (fine && !reduced) {
  $$(".magnetic").forEach((b) => {
    b.addEventListener("pointermove", (e) => {
      const r = b.getBoundingClientRect();
      const x = e.clientX - r.left - r.width / 2;
      const y = e.clientY - r.top - r.height / 2;
      b.style.transform = `translate(${x * 0.25}px, ${y * 0.35}px)`;
    });
    b.addEventListener("pointerleave", () => { b.style.transform = ""; });
  });
  const glow = $(".cursor-glow");
  addEventListener("pointermove", (e) => { glow.style.setProperty("--gx", `${e.clientX}px`); glow.style.setProperty("--gy", `${e.clientY}px`); }, { passive: true });
}

// Appareils de l'aperçu : parallaxe au défilement et au pointeur.
const devices = $(".devices");
const laptop = $(".laptop");
const phone = $(".phone");
let mxp = 0, myp = 0;
addEventListener("pointermove", (e) => { mxp = e.clientX / innerWidth - 0.5; myp = e.clientY / innerHeight - 0.5; }, { passive: true });
function animateDevices() {
  const r = devices.getBoundingClientRect();
  if (r.bottom > 0 && r.top < innerHeight) {
    const k = (r.top + r.height / 2 - innerHeight / 2) / innerHeight; // -1 à 1
    laptop.style.transform = `rotateX(${10 + k * 12 - myp * 6}deg) rotateY(${-8 + k * 10 + mxp * 10}deg)`;
    phone.style.transform = `rotateY(${-18 + k * 24 + mxp * 16}deg) rotateX(${8 - k * 8}deg) translateZ(80px) translateY(${k * 60}px)`;
  }
  requestAnimationFrame(animateDevices);
}
if (!reduced) requestAnimationFrame(animateDevices);

// ------------------------------------------------------------------ étapes du fonctionnement
const how = $("#fonctionnement");
const steps = $$(".step");
const stepObs = new IntersectionObserver((entries) => {
  entries.forEach((e) => e.target.classList.toggle("active", e.isIntersecting));
}, { rootMargin: matchMedia("(max-width: 640px)").matches ? "-50% 0px -20% 0px" : "-40% 0px -40% 0px" });
steps.forEach((s) => stepObs.observe(s));
function howProgress() {
  const r = how.getBoundingClientRect();
  const total = r.height - innerHeight;
  return Math.max(0, Math.min(1, -r.top / (total || 1)));
}

// ------------------------------------------------------------------ formulaires magnet
function looksLikeTorrent(v) {
  v = v.trim();
  return /^magnet:\?/i.test(v) || /^[0-9a-f]{40}$/i.test(v) || /^[a-z2-7]{32}$/i.test(v) || /^https?:\/\/.+/i.test(v);
}
const toast = $("#toast");
function say(text) {
  toast.textContent = text;
  toast.classList.add("show");
  clearTimeout(say.t);
  say.t = setTimeout(() => toast.classList.remove("show"), 3200);
}
$$("form.magnet-form, form.footer-magnet").forEach((f) => f.addEventListener("submit", (e) => {
  const input = f.querySelector("input");
  if (!input.value.trim()) {
    e.preventDefault();
    location.href = `app/?lang=${lang}`;
    return;
  }
  if (!looksLikeTorrent(input.value)) {
    e.preventDefault();
    input.animate([{ transform: "translateX(0)" }, { transform: "translateX(-8px)" }, { transform: "translateX(8px)" }, { transform: "translateX(0)" }], { duration: 300, iterations: 2 });
    say(lang === "en" ? "That doesn't look like a magnet link or an info hash." : "Ce n'est pas un lien magnet ni une empreinte.");
  }
}));

// ------------------------------------------------------------------ installation (PWA)
let deferred = null;
const installBtn = $("#install-btn");
addEventListener("beforeinstallprompt", (e) => {
  e.preventDefault();
  deferred = e;
  installBtn.hidden = false;
});
installBtn.addEventListener("click", async () => {
  if (!deferred) return;
  deferred.prompt();
  await deferred.userChoice;
  deferred = null;
  installBtn.hidden = true;
});
addEventListener("appinstalled", () => {
  installBtn.hidden = true;
  $("#install-done").hidden = false;
  say(lang === "en" ? "mxTorrent is installed 🎉" : "mxTorrent est installé 🎉");
});
if (matchMedia("(display-mode: standalone)").matches) $("#install-done").hidden = false;

// ------------------------------------------------------------------ pied de page
$("#year").textContent = new Date().getFullYear();
$(".to-top").addEventListener("click", () => scrollTo({ top: 0, behavior: reduced ? "auto" : "smooth" }));
function netStatus() {
  const on = navigator.onLine;
  const el = $("#net-status");
  el.dataset.i18n = on ? "ft.online" : "ft.offline";
  el.textContent = (lang === "en" ? EN : FR)[el.dataset.i18n] || (on ? "En ligne" : "Hors ligne");
  $(".pulse").classList.toggle("off", !on);
}
FR["ft.offline"] = "Hors ligne";
addEventListener("online", netStatus);
addEventListener("offline", netStatus);
netStatus();

// Logo animé du pied de page (canevas 2D : pas besoin d'un contexte WebGL de plus).
(function footerLogo() {
  const c = $("#footer-canvas");
  const g = c.getContext("2d");
  const dpr = Math.min(devicePixelRatio, 2);
  c.width = c.height = 120 * dpr;
  g.scale(dpr, dpr);
  let visible = false;
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; if (visible) requestAnimationFrame(draw); }).observe(c);
  function draw(now) {
    const t = reduced ? 0 : now / 1000;
    g.clearRect(0, 0, 120, 120);
    const orbits = [["#8b5cf6", 50, 16, -24, 3.5], ["#22d3ee", 40, 12, 42, 3], ["#ec4899", 30, 9, 100, 2.5]];
    orbits.forEach(([col, rx, ry, deg, w], k) => {
      g.save();
      g.translate(60, 60);
      g.rotate((deg * Math.PI) / 180 + t * 0.15);
      g.strokeStyle = col;
      g.lineWidth = w;
      g.beginPath();
      g.ellipse(0, 0, rx, ry, 0, 0, Math.PI * 2);
      g.stroke();
      const a = t * (1 + k * 0.45) + k * 2;
      g.fillStyle = col;
      g.shadowColor = col;
      g.shadowBlur = 12;
      g.beginPath();
      g.arc(Math.cos(a) * rx, Math.sin(a) * ry, 5 - k * 0.8, 0, Math.PI * 2);
      g.fill();
      g.restore();
    });
    const grd = g.createLinearGradient(48, 48, 72, 72);
    grd.addColorStop(0, "#8b5cf6");
    grd.addColorStop(1, "#22d3ee");
    g.fillStyle = grd;
    g.shadowColor = "#8b5cf6";
    g.shadowBlur = 20;
    g.beginPath();
    g.arc(60, 60, 12 + Math.sin(t * 2) * 1.2, 0, Math.PI * 2);
    g.fill();
    g.shadowBlur = 0;
    if (visible && !reduced) requestAnimationFrame(draw);
  }
})();

// ------------------------------------------------------------------ 3D
(async () => {
  const S = await import("./scenes.js");
  if (!S.supportsWebGL()) return;
  try {
    const hero = S.heroSwarm($("#hero-canvas"));
    const meterFill = $("#how-meter-fill");
    const meterText = $("#how-meter-text");
    const cube = S.piecesCube($("#how-canvas"), howProgress, (pct) => {
      meterFill.style.transform = `scaleX(${pct / 100})`;
      meterText.textContent = `${pct} %`;
    });
    const box = S.sandbox($("#shield-canvas"));
    const k = S.knot($("#cta-canvas"));
    $(".hero-fallback").style.opacity = "0.5";
    if (reduced) [hero, cube, box, k].forEach((s) => s.renderOnce());
  } catch (e) {
    console.warn("3D indisponible :", e);
  }
})();

// ------------------------------------------------------------------ hors ligne
if ("serviceWorker" in navigator) {
  addEventListener("load", () => navigator.serviceWorker.register("sw.js", { updateViaCache: "none" }).catch(() => {}));
}
