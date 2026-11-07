// Les scènes 3D du site, avec Three.js.
// Chaque scène a son canevas, ne tourne que lorsqu'elle est visible,
// et réduit ses effets sur téléphone ou si l'utilisateur préfère moins de mouvement.
import * as THREE from "../vendor/three.module.min.js";

const VIOLET = new THREE.Color("#8b5cf6");
const CYAN = new THREE.Color("#22d3ee");
const PINK = new THREE.Color("#ec4899");
const AMBER = new THREE.Color("#fbbf24");

const small = matchMedia("(max-width: 640px)").matches;
const coarse = matchMedia("(pointer: coarse)").matches;
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

// Pointeur partagé : souris, doigt ou inclinaison du téléphone (-1 à 1).
export const pointer = { x: 0, y: 0, tx: 0, ty: 0 };
addEventListener("pointermove", (e) => {
  pointer.tx = (e.clientX / innerWidth) * 2 - 1;
  pointer.ty = (e.clientY / innerHeight) * 2 - 1;
}, { passive: true });
addEventListener("deviceorientation", (e) => {
  if (e.gamma == null) return;
  pointer.tx = Math.max(-1, Math.min(1, e.gamma / 30));
  pointer.ty = Math.max(-1, Math.min(1, (e.beta - 45) / 30));
}, { passive: true });

/** Texture ronde et douce pour les particules et les halos. */
function glowTexture() {
  const c = document.createElement("canvas");
  c.width = c.height = 64;
  const g = c.getContext("2d");
  const r = g.createRadialGradient(32, 32, 0, 32, 32, 32);
  r.addColorStop(0, "rgba(255,255,255,1)");
  r.addColorStop(0.25, "rgba(255,255,255,.7)");
  r.addColorStop(1, "rgba(255,255,255,0)");
  g.fillStyle = r;
  g.fillRect(0, 0, 64, 64);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}
const GLOW = glowTexture();

/** Monte un rendu sur un canevas ; la boucle s'arrête hors de l'écran. */
function stage(canvas, { fov = 50, z = 12, alpha = true } = {}) {
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: !small, alpha, powerPreference: "high-performance" });
  renderer.setPixelRatio(Math.min(devicePixelRatio, small ? 1.5 : 2));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(fov, 1, 0.1, 200);
  camera.position.z = z;
  let visible = false;
  let frame = null;
  const resize = () => {
    const w = canvas.clientWidth || 1;
    const h = canvas.clientHeight || 1;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  };
  new ResizeObserver(resize).observe(canvas);
  resize();
  const s = { renderer, scene, camera, canvas, update: null, t: 0 };
  let last = performance.now();
  const loop = (now) => {
    frame = null;
    if (!visible || document.hidden) return;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    s.t += reduced ? 0 : dt;
    pointer.x += (pointer.tx - pointer.x) * 0.05;
    pointer.y += (pointer.ty - pointer.y) * 0.05;
    s.update?.(s.t, dt);
    renderer.render(scene, camera);
    frame = requestAnimationFrame(loop);
  };
  const start = () => {
    if (frame == null && visible) {
      last = performance.now();
      frame = requestAnimationFrame(loop);
    }
  };
  new IntersectionObserver(([e]) => {
    visible = e.isIntersecting;
    start();
  }, { rootMargin: "100px" }).observe(canvas);
  document.addEventListener("visibilitychange", start);
  s.renderOnce = () => { s.update?.(s.t, 0); renderer.render(scene, camera); };
  return s;
}

// ===================================================================== héros : l'essaim

export function heroSwarm(canvas) {
  const s = stage(canvas, { fov: 55, z: 15 });
  const { scene, camera } = s;
  scene.fog = new THREE.FogExp2(0x07070d, 0.035);

  scene.add(new THREE.AmbientLight(0xffffff, 0.35));
  const l1 = new THREE.PointLight(VIOLET, 60, 40);
  l1.position.set(5, 4, 6);
  const l2 = new THREE.PointLight(CYAN, 50, 40);
  l2.position.set(-6, -3, 4);
  scene.add(l1, l2);

  const world = new THREE.Group();
  scene.add(world);

  // Le noyau : le torrent lui-même.
  const coreGeo = new THREE.IcosahedronGeometry(1.7, 1);
  const core = new THREE.Mesh(coreGeo, new THREE.MeshStandardMaterial({ color: 0x1a1040, emissive: VIOLET, emissiveIntensity: 0.55, metalness: 0.4, roughness: 0.25, flatShading: true }));
  const wire = new THREE.LineSegments(new THREE.WireframeGeometry(new THREE.IcosahedronGeometry(2.05, 1)), new THREE.LineBasicMaterial({ color: CYAN, transparent: true, opacity: 0.45 }));
  const halo = new THREE.Sprite(new THREE.SpriteMaterial({ map: GLOW, color: VIOLET, transparent: true, opacity: 0.8, depthWrite: false, blending: THREE.AdditiveBlending }));
  halo.scale.setScalar(9);
  world.add(core, wire, halo);

  // Trois orbites, comme le logo.
  const rings = [VIOLET, CYAN, PINK].map((c, i) => {
    const r = new THREE.Mesh(new THREE.TorusGeometry(3.1 + i * 0.25, 0.025, 8, 160), new THREE.MeshBasicMaterial({ color: c, transparent: true, opacity: 0.75 }));
    r.rotation.set(Math.PI / 2.4, (i * Math.PI) / 3, 0);
    world.add(r);
    return r;
  });

  // Les pairs, sur une coquille autour du noyau.
  const N = small ? 18 : 30;
  const peers = [];
  const peerGeo = new THREE.SphereGeometry(0.16, 16, 16);
  for (let i = 0; i < N; i++) {
    const y = 1 - (i / (N - 1)) * 2;
    const r = Math.sqrt(1 - y * y);
    const phi = i * 2.399963;
    const radius = 5 + Math.random() * 3.5;
    const base = new THREE.Vector3(Math.cos(phi) * r, y * 0.75, Math.sin(phi) * r).multiplyScalar(radius);
    const color = [VIOLET, CYAN, PINK][i % 3];
    const m = new THREE.Mesh(peerGeo, new THREE.MeshBasicMaterial({ color }));
    const g = new THREE.Sprite(new THREE.SpriteMaterial({ map: GLOW, color, transparent: true, opacity: 0.9, depthWrite: false, blending: THREE.AdditiveBlending }));
    g.scale.setScalar(1.1);
    m.add(g);
    world.add(m);
    peers.push({ mesh: m, base, speed: 0.05 + Math.random() * 0.12, phase: Math.random() * 6.28, bob: 0.2 + Math.random() * 0.5 });
  }

  // Les liens : noyau ↔ pair, et pair ↔ pair voisin.
  const links = [];
  peers.forEach((p, i) => {
    links.push([-1, i]);
    let best = -1, bd = 1e9;
    peers.forEach((q, j) => {
      if (j === i) return;
      const d = p.base.distanceTo(q.base);
      if (d < bd) { bd = d; best = j; }
    });
    if (best > i) links.push([i, best]);
  });
  const linkPos = new Float32Array(links.length * 6);
  const linkGeo = new THREE.BufferGeometry();
  linkGeo.setAttribute("position", new THREE.BufferAttribute(linkPos, 3));
  const linkLines = new THREE.LineSegments(linkGeo, new THREE.LineBasicMaterial({ color: 0x6d5bd0, transparent: true, opacity: 0.28, blending: THREE.AdditiveBlending }));
  world.add(linkLines);

  // Les paquets qui circulent sur les liens.
  const P = small ? 90 : 220;
  const pk = Array.from({ length: P }, () => ({ link: (Math.random() * links.length) | 0, t: Math.random(), v: 0.15 + Math.random() * 0.45, dir: Math.random() < 0.5 ? 1 : -1 }));
  const pkPos = new Float32Array(P * 3);
  const pkCol = new Float32Array(P * 3);
  pk.forEach((p, i) => { const c = [CYAN, PINK, VIOLET, AMBER][i % 4]; pkCol.set([c.r, c.g, c.b], i * 3); });
  const pkGeo = new THREE.BufferGeometry();
  pkGeo.setAttribute("position", new THREE.BufferAttribute(pkPos, 3));
  pkGeo.setAttribute("color", new THREE.BufferAttribute(pkCol, 3));
  const packets = new THREE.Points(pkGeo, new THREE.PointsMaterial({ size: 0.22, map: GLOW, vertexColors: true, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending }));
  world.add(packets);

  // Les étoiles.
  const S = small ? 900 : 2200;
  const st = new Float32Array(S * 3);
  for (let i = 0; i < S; i++) {
    const v = new THREE.Vector3().randomDirection().multiplyScalar(30 + Math.random() * 60);
    st.set([v.x, v.y, v.z], i * 3);
  }
  const starGeo = new THREE.BufferGeometry();
  starGeo.setAttribute("position", new THREE.BufferAttribute(st, 3));
  const stars = new THREE.Points(starGeo, new THREE.PointsMaterial({ size: 0.18, map: GLOW, color: 0xbfb8ff, transparent: true, opacity: 0.8, depthWrite: false }));
  scene.add(stars);

  const tmp = new THREE.Vector3();
  const center = new THREE.Vector3();
  const pos = (k) => (k < 0 ? center : peers[k].mesh.position);
  let scroll = 0;
  addEventListener("scroll", () => { scroll = Math.min(1, scrollY / innerHeight); }, { passive: true });

  s.update = (t) => {
    core.rotation.set(t * 0.2, t * 0.3, 0);
    wire.rotation.set(-t * 0.15, -t * 0.22, 0);
    const pulse = 1 + Math.sin(t * 2) * 0.04;
    core.scale.setScalar(pulse);
    halo.material.opacity = 0.6 + Math.sin(t * 2) * 0.15;
    rings.forEach((r, i) => { r.rotation.z = t * (0.2 + i * 0.07); });
    peers.forEach((p) => {
      const a = t * p.speed + p.phase;
      tmp.copy(p.base).applyAxisAngle(THREE.Object3D.DEFAULT_UP, a);
      tmp.y += Math.sin(t * 0.8 + p.phase) * p.bob;
      p.mesh.position.copy(tmp);
    });
    links.forEach(([a, b], i) => {
      const A = pos(a), B = pos(b);
      linkPos.set([A.x, A.y, A.z, B.x, B.y, B.z], i * 6);
    });
    linkGeo.attributes.position.needsUpdate = true;
    pk.forEach((p, i) => {
      p.t += p.v * 0.016 * p.dir;
      if (p.t > 1 || p.t < 0) { p.link = (Math.random() * links.length) | 0; p.t = p.dir > 0 ? 0 : 1; }
      const [a, b] = links[p.link];
      tmp.lerpVectors(pos(a), pos(b), p.t);
      pkPos.set([tmp.x, tmp.y, tmp.z], i * 3);
    });
    pkGeo.attributes.position.needsUpdate = true;
    stars.rotation.y = t * 0.01;
    world.rotation.y = t * 0.05 + pointer.x * 0.35;
    world.rotation.x = pointer.y * 0.2 + scroll * 0.6;
    camera.position.x = pointer.x * 1.2;
    camera.position.y = -pointer.y * 0.8 + scroll * 2;
    camera.position.z = 15 + scroll * 6 - (small ? -4 : 0);
    camera.lookAt(0, 0, 0);
  };
  return s;
}

// ===================================================================== fonctionnement : le cube des pièces

export function piecesCube(canvas, getProgress, onPercent) {
  const s = stage(canvas, { fov: 45, z: 16 });
  const { scene, camera } = s;
  scene.add(new THREE.AmbientLight(0xffffff, 0.5));
  const d = new THREE.DirectionalLight(0xffffff, 1.4);
  d.position.set(5, 8, 6);
  scene.add(d);
  const l = new THREE.PointLight(CYAN, 40, 50);
  l.position.set(-6, -4, 8);
  scene.add(l);

  const n = small ? 6 : 8;
  const count = n * n * n;
  const gap = 0.62;
  const box = new THREE.BoxGeometry(0.5, 0.5, 0.5);
  const mat = new THREE.MeshStandardMaterial({ metalness: 0.3, roughness: 0.35 });
  const mesh = new THREE.InstancedMesh(box, mat, count);
  mesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
  const group = new THREE.Group();
  group.add(mesh);
  scene.add(group);

  // Arêtes du cube : la forme finale, visible dès le départ.
  const edge = new THREE.LineSegments(new THREE.EdgesGeometry(new THREE.BoxGeometry(n * gap + 0.3, n * gap + 0.3, n * gap + 0.3)), new THREE.LineBasicMaterial({ color: VIOLET, transparent: true, opacity: 0.5 }));
  group.add(edge);

  // Chaque pièce : sa place finale, un point de départ lointain, et son ordre d'arrivée.
  const pieces = [];
  const half = ((n - 1) * gap) / 2;
  for (let x = 0; x < n; x++) for (let y = 0; y < n; y++) for (let z = 0; z < n; z++) {
    const target = new THREE.Vector3(x * gap - half, y * gap - half, z * gap - half);
    const from = new THREE.Vector3().randomDirection().multiplyScalar(9 + Math.random() * 8);
    pieces.push({ target, from, at: Math.random() * 0.9, spin: new THREE.Vector3(Math.random(), Math.random(), Math.random()).multiplyScalar(6) });
  }
  // La pièce la plus rare d'abord : un ordre qui part d'un coin, avec du hasard.
  pieces.forEach((p) => { p.at = 0.1 + 0.74 * (0.55 * Math.random() + 0.45 * ((p.target.x + p.target.y + half * 2) / (half * 4))); });

  // Quatre pairs en orbite qui envoient des paquets vers le cube.
  const peers = [VIOLET, CYAN, PINK, AMBER].map((c) => {
    const m = new THREE.Mesh(new THREE.SphereGeometry(0.28, 16, 16), new THREE.MeshBasicMaterial({ color: c }));
    const g = new THREE.Sprite(new THREE.SpriteMaterial({ map: GLOW, color: c, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending }));
    g.scale.setScalar(2);
    m.add(g);
    scene.add(m);
    return m;
  });
  const B = small ? 60 : 140;
  const beams = Array.from({ length: B }, (_, i) => ({ peer: i % 4, t: Math.random(), target: (Math.random() * count) | 0 }));
  const bPos = new Float32Array(B * 3);
  const bCol = new Float32Array(B * 3);
  beams.forEach((b, i) => { const c = [VIOLET, CYAN, PINK, AMBER][b.peer]; bCol.set([c.r, c.g, c.b], i * 3); });
  const bGeo = new THREE.BufferGeometry();
  bGeo.setAttribute("position", new THREE.BufferAttribute(bPos, 3));
  bGeo.setAttribute("color", new THREE.BufferAttribute(bCol, 3));
  scene.add(new THREE.Points(bGeo, new THREE.PointsMaterial({ size: 0.3, map: GLOW, vertexColors: true, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending })));

  const m4 = new THREE.Matrix4();
  const q = new THREE.Quaternion();
  const e = new THREE.Euler();
  const p3 = new THREE.Vector3();
  const sc = new THREE.Vector3();
  const col = new THREE.Color();
  const dim = new THREE.Color("#1d1d33");
  let shown = 0;
  let lastPct = -1;

  s.update = (t) => {
    const target = getProgress();
    shown += (target - shown) * 0.08;
    const prog = shown;
    let done = 0;
    pieces.forEach((p, i) => {
      // k : 0 = pas encore là, 1 = arrivée et vérifiée.
      const k = THREE.MathUtils.clamp((prog - p.at) / 0.08, 0, 1);
      const ease = 1 - Math.pow(1 - k, 3);
      if (k >= 1) done++;
      p3.lerpVectors(p.from, p.target, ease);
      e.set(p.spin.x * (1 - ease) + t * 0.0, p.spin.y * (1 - ease), p.spin.z * (1 - ease));
      q.setFromEuler(e);
      const size = k === 0 ? 0.0001 : 0.35 + ease * 0.65 + (k > 0 && k < 1 ? Math.sin(t * 20) * 0.08 : 0);
      sc.setScalar(size);
      m4.compose(p3, q, sc);
      mesh.setMatrixAt(i, m4);
      if (k >= 1) {
        col.copy(VIOLET).lerp(CYAN, (p.target.y + half) / (half * 2 || 1));
      } else {
        col.copy(dim).lerp(AMBER, ease);
      }
      mesh.setColorAt(i, col);
    });
    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
    const pct = Math.round((done / count) * 100);
    if (pct !== lastPct) { lastPct = pct; onPercent?.(pct); }

    group.rotation.y = t * 0.15 + prog * 2.5 + pointer.x * 0.3;
    group.rotation.x = 0.45 + pointer.y * 0.2;
    edge.material.opacity = 0.25 + 0.35 * (1 - prog) + Math.sin(t * 2) * 0.05;

    peers.forEach((m, i) => {
      const a = t * 0.4 + (i * Math.PI) / 2;
      m.position.set(Math.cos(a) * 9, Math.sin(a * 1.3) * 3, Math.sin(a) * 9 - 2);
    });
    beams.forEach((b, i) => {
      b.t += 0.012 + (b.peer * 0.002);
      if (b.t >= 1) { b.t = 0; b.target = (Math.random() * count) | 0; }
      const from = peers[b.peer].position;
      p3.copy(pieces[b.target].target).applyEuler(group.rotation);
      p3.lerpVectors(from, p3, b.t);
      const vis = prog > 0.02 && prog < 0.995 ? 1 : 0;
      bPos.set(vis ? [p3.x, p3.y, p3.z] : [0, 0, -999], i * 3);
    });
    bGeo.attributes.position.needsUpdate = true;
    // Sur téléphone, le cube monte dans la moitié haute ; les cartes passent dessous.
    camera.position.set(pointer.x * 1.5, (small ? -3.2 : 1) - pointer.y, small ? 21 : 16);
    camera.lookAt(0, small ? -3.2 : 0, 0);

  };
  return s;
}

// ===================================================================== sécurité : le bac à sable

export function sandbox(canvas) {
  const s = stage(canvas, { fov: 40, z: 10 });
  const { scene, camera } = s;
  scene.add(new THREE.AmbientLight(0xffffff, 0.6));
  const d = new THREE.PointLight(CYAN, 40, 30);
  d.position.set(3, 4, 6);
  scene.add(d);
  const group = new THREE.Group();
  scene.add(group);

  // La boîte de verre : le navigateur.
  const glass = new THREE.Mesh(new THREE.BoxGeometry(4, 4, 4), new THREE.MeshPhysicalMaterial({ color: 0x8b5cf6, transparent: true, opacity: 0.08, roughness: 0.1, metalness: 0, side: THREE.DoubleSide, depthWrite: false }));
  const edges = new THREE.LineSegments(new THREE.EdgesGeometry(new THREE.BoxGeometry(4, 4, 4)), new THREE.LineBasicMaterial({ color: CYAN }));
  group.add(glass, edges);

  // Au centre : le module WebAssembly.
  const gem = new THREE.Mesh(new THREE.IcosahedronGeometry(1.0, 0), new THREE.MeshStandardMaterial({ color: 0x2a1866, emissive: VIOLET, emissiveIntensity: 0.35, flatShading: true, metalness: 0.6, roughness: 0.15 }));
  gem.add(new THREE.LineSegments(new THREE.EdgesGeometry(new THREE.IcosahedronGeometry(1.25, 0)), new THREE.LineBasicMaterial({ color: CYAN, transparent: true, opacity: 0.7 })));
  const key = new THREE.PointLight(PINK, 25, 20);
  key.position.set(-3, 2, 3);
  scene.add(key);
  group.add(gem);
  const halo = new THREE.Sprite(new THREE.SpriteMaterial({ map: GLOW, color: VIOLET, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending }));
  halo.scale.setScalar(4);
  group.add(halo);

  // Des particules qui rebondissent sur les parois sans jamais sortir.
  const K = 80;
  const pos = new Float32Array(K * 3);
  const vel = [];
  for (let i = 0; i < K; i++) {
    pos.set([(Math.random() - 0.5) * 3.6, (Math.random() - 0.5) * 3.6, (Math.random() - 0.5) * 3.6], i * 3);
    vel.push(new THREE.Vector3().randomDirection().multiplyScalar(0.8 + Math.random()));
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute("position", new THREE.BufferAttribute(pos, 3));
  group.add(new THREE.Points(g, new THREE.PointsMaterial({ size: 0.28, map: GLOW, color: CYAN, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending })));

  // Une onde quand une particule touche la paroi.
  s.update = (t, dt) => {
    for (let i = 0; i < K; i++) {
      for (let a = 0; a < 3; a++) {
        const k = i * 3 + a;
        pos[k] += vel[i].getComponent(a) * dt;
        if (Math.abs(pos[k]) > 1.85) { pos[k] = Math.sign(pos[k]) * 1.85; vel[i].setComponent(a, -vel[i].getComponent(a)); }
      }
    }
    g.attributes.position.needsUpdate = true;
    gem.rotation.set(t * 0.6, t * 0.8, 0);
    halo.material.opacity = 0.5 + Math.sin(t * 3) * 0.2;
    group.rotation.set(0.4 + pointer.y * 0.2, t * 0.25 + pointer.x * 0.4, 0);
    edges.material.color.copy(CYAN).lerp(VIOLET, (Math.sin(t) + 1) / 2);
    camera.lookAt(0, 0, 0);
  };
  return s;
}

// ===================================================================== appel final : le nœud

export function knot(canvas) {
  const s = stage(canvas, { fov: 45, z: 12 });
  const { scene, camera } = s;
  const geo = new THREE.TorusKnotGeometry(3, 0.9, small ? 160 : 300, small ? 20 : 32, 2, 3);
  // Dégradé violet → cyan → rose selon la position, avec un éclairage de bord.
  const mat = new THREE.ShaderMaterial({
    uniforms: { uTime: { value: 0 } },
    vertexShader: `
      varying vec3 vN; varying vec3 vP; varying vec3 vV;
      uniform float uTime;
      void main() {
        vec3 p = position + normal * sin(position.x * 1.5 + uTime * 1.5) * 0.12;
        vec4 mv = modelViewMatrix * vec4(p, 1.0);
        vN = normalize(normalMatrix * normal); vP = position; vV = -mv.xyz;
        gl_Position = projectionMatrix * mv;
      }`,
    fragmentShader: `
      varying vec3 vN; varying vec3 vP; varying vec3 vV;
      uniform float uTime;
      void main() {
        vec3 a = vec3(0.545, 0.361, 0.965), b = vec3(0.133, 0.827, 0.933), c = vec3(0.925, 0.282, 0.6);
        float k = 0.5 + 0.5 * sin(vP.x * 0.5 + vP.y * 0.4 + uTime * 0.6);
        vec3 col = mix(mix(a, b, k), c, 0.5 + 0.5 * sin(vP.z * 0.7 - uTime * 0.4));
        float rim = pow(1.0 - abs(dot(normalize(vN), normalize(vV))), 2.0);
        gl_FragColor = vec4(col * (0.35 + rim * 1.2), 1.0);
      }`,
  });
  const mesh = new THREE.Mesh(geo, mat);
  scene.add(mesh);
  const pts = new THREE.Points(new THREE.TorusKnotGeometry(4.4, 1.6, 200, 12, 2, 3), new THREE.PointsMaterial({ size: 0.06, color: 0xbfb8ff, transparent: true, opacity: 0.6 }));
  scene.add(pts);
  s.update = (t) => {
    mat.uniforms.uTime.value = t;
    mesh.rotation.set(t * 0.15 + pointer.y * 0.3, t * 0.2 + pointer.x * 0.5, 0);
    pts.rotation.set(-t * 0.05, -t * 0.08, 0);
    camera.lookAt(0, 0, 0);
  };
  return s;
}

export function supportsWebGL() {
  try {
    const c = document.createElement("canvas");
    return !!(c.getContext("webgl2") || c.getContext("webgl"));
  } catch {
    return false;
  }
}

export const env = { small, coarse, reduced };
