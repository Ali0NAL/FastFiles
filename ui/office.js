// Sanal ofis: ofisteki masanın 3B dijital ikizi.
//
// Sahne yalnızca çizimden sorumludur; kim nerede oturuyor, kim kime ne gönderdi gibi bilgiler
// main.js'ten gelir (setPeople, message, typing, syncTransfers, setGames). Three.js uygulamanın
// içinde (ui/vendor) durur: internet gerekmez ve CSP dışarıdan betik yüklemeye izin vermez.

import * as THREE from './vendor/three.module.min.js';
import { OrbitControls } from './vendor/OrbitControls.js';

// ───────────────────────────── Ofis düzeni (metre) ─────────────────────────────

const TOP = 0.76;                       // masa üst yüzeyinin yüksekliği
const TABLE = { length: 5.6, width: 1.8 };
const SEAT_GAP = 0.46;                  // masa kenarından koltuk merkezine
const ROOM = { w: 15, d: 11, h: 2.9 };

// Çizimdeki masa: üstte 4, altta 3 koltuk. Numaralar soldan sağa, önce üst sıra.
export const SEATS = [
  { x: -1.85, side: -1 }, { x: -0.65, side: -1 }, { x: 1.05, side: -1 }, { x: 2.1, side: -1 },
  { x: -1.85, side: 1 }, { x: 0.4, side: 1 }, { x: 2.1, side: 1 },
].map((s, i) => ({ ...s, n: i + 1, z: s.side * (TABLE.width / 2 + SEAT_GAP) }));

// Masada yeri olmayan (koltuğu bilinmeyen) birinin olayları kapıdan gelir/kapıya gider.
const DOOR = new THREE.Vector3(6.4, 1.5, 4.2);

const CAM_HOME = { pos: new THREE.Vector3(0.8, 4.6, 6.9), target: new THREE.Vector3(0, 0.75, 0) };

// ───────────────────────────── Yardımcılar ─────────────────────────────

function hash(id) {
  let h = 2166136261;
  for (const c of String(id)) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return h >>> 0;
}
const ease = (t) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);
const clamp01 = (t) => Math.min(1, Math.max(0, t));

function roundedRect(w, h, r) {
  const s = new THREE.Shape();
  const x = -w / 2, y = -h / 2;
  s.moveTo(x + r, y);
  s.lineTo(x + w - r, y);
  s.quadraticCurveTo(x + w, y, x + w, y + r);
  s.lineTo(x + w, y + h - r);
  s.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  s.lineTo(x + r, y + h);
  s.quadraticCurveTo(x, y + h, x, y + h - r);
  s.lineTo(x, y + r);
  s.quadraticCurveTo(x, y, x + r, y);
  return s;
}

function canvasTexture(w, h, draw) {
  const c = document.createElement('canvas');
  c.width = w; c.height = h;
  draw(c.getContext('2d'), w, h);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.anisotropy = 4;
  return t;
}

const UP = new THREE.Vector3(0, 1, 0);
/** a → b arasında yuvarlak uçlu uzuv (kol, bacak). */
function limb(a, b, r, mat) {
  const dir = new THREE.Vector3().subVectors(b, a);
  const len = dir.length();
  const m = new THREE.Mesh(new THREE.CapsuleGeometry(r, Math.max(0.001, len), 4, 10), mat);
  m.position.copy(a).addScaledVector(dir, 0.5);
  m.quaternion.setFromUnitVectors(UP, dir.normalize());
  return m;
}

const v3 = (x, y, z) => new THREE.Vector3(x, y, z);

const STATUS_ICON = { busy: '📅', focus: '🎧', away: '☕' };
const STATUS_LABEL = { busy: 'Toplantıda', focus: 'Odakta', away: 'Dışarıda' };

// Kişi renkleri: parmak izinden türetilir, her bilgisayarda aynı kişi aynı görünür.
const SKINS = [0xf2c9a8, 0xe8b48f, 0xd49a6a, 0xa86f48, 0xf6d5bd, 0x8a5636];
const HAIRS = [0x2b1d16, 0x4a2f1f, 0x15110f, 0x8a5a2b, 0xc9a25a, 0x5c5c66, 0x6b2c1a];
const PANTS = [0x2d3550, 0x3a3a42, 0x24324a, 0x4b4038];

// ───────────────────────────── Sahne ─────────────────────────────

/**
 * @param {HTMLElement} root  sahnenin çizileceği kutu
 * @param {{ onPerson?: (fp: string) => void, onSeat?: (n: number) => void }} hooks
 */
export function createOffice(root, hooks = {}) {
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, powerPreference: 'high-performance' });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  renderer.toneMapping = THREE.NeutralToneMapping;
  renderer.domElement.className = 'office-gl';
  root.appendChild(renderer.domElement);

  const labelsEl = document.createElement('div');
  labelsEl.className = 'office-labels';
  root.appendChild(labelsEl);

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(40, 1, 0.1, 120);
  camera.position.copy(CAM_HOME.pos).multiplyScalar(1.6).add(v3(4, 3, 0));

  const controls = new OrbitControls(camera, renderer.domElement);
  controls.target.copy(CAM_HOME.target);
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.enablePan = false;
  controls.minDistance = 3;
  controls.maxDistance = 14;
  controls.minPolarAngle = 0.2;
  controls.maxPolarAngle = 1.4;
  controls.rotateSpeed = 0.7;

  // ── Malzemeler (renkleri temaya göre applyPalette belirler)
  const M = {
    floor: new THREE.MeshStandardMaterial({ roughness: 0.92 }),
    rug: new THREE.MeshStandardMaterial({ roughness: 1 }),
    wall: new THREE.MeshStandardMaterial({ roughness: 0.95 }),
    wood: new THREE.MeshStandardMaterial({ roughness: 0.55, metalness: 0.02 }),
    metal: new THREE.MeshStandardMaterial({ color: 0x2b2e3a, roughness: 0.45, metalness: 0.6 }),
    chair: new THREE.MeshStandardMaterial({ color: 0x343748, roughness: 0.7 }),
    cushion: new THREE.MeshStandardMaterial({ color: 0x4a4e66, roughness: 0.85 }),
    laptop: new THREE.MeshStandardMaterial({ color: 0xb8bcc8, roughness: 0.35, metalness: 0.7 }),
    pot: new THREE.MeshStandardMaterial({ color: 0xe9e4da, roughness: 0.8 }),
    leaf: new THREE.MeshStandardMaterial({ color: 0x3f9b5b, roughness: 0.8, flatShading: true }),
    leaf2: new THREE.MeshStandardMaterial({ color: 0x5bb56d, roughness: 0.8, flatShading: true }),
    lamp: new THREE.MeshStandardMaterial({ color: 0x24262f, roughness: 0.5, metalness: 0.4 }),
    lampGlow: new THREE.MeshBasicMaterial({ color: 0xfff1d6 }),
    window: new THREE.MeshBasicMaterial({ color: 0xcfe8ff }),
    windowFrame: new THREE.MeshStandardMaterial({ color: 0xffffff, roughness: 0.6 }),
    accent: new THREE.MeshStandardMaterial({ roughness: 0.5 }),
    ring: new THREE.MeshBasicMaterial({ transparent: true, opacity: 0.8, depthWrite: false, side: THREE.DoubleSide }),
    headset: new THREE.MeshStandardMaterial({ color: 0x1f2130, roughness: 0.4, metalness: 0.3 }),
    paper: new THREE.MeshStandardMaterial({ color: 0xffffff, roughness: 0.6, side: THREE.DoubleSide, emissive: 0x9aa4ff, emissiveIntensity: 0.15 }),
  };

  // ── Işıklar
  const hemi = new THREE.HemisphereLight(0xffffff, 0x8a8577, 1.5);
  scene.add(hemi);
  const sun = new THREE.DirectionalLight(0xffffff, 1.9);
  sun.position.set(-5, 9, 6);
  sun.castShadow = true;
  sun.shadow.mapSize.set(2048, 2048);
  sun.shadow.camera.left = -8; sun.shadow.camera.right = 8;
  sun.shadow.camera.top = 6; sun.shadow.camera.bottom = -6;
  sun.shadow.camera.near = 1; sun.shadow.camera.far = 25;
  sun.shadow.bias = -0.0005;
  sun.shadow.normalBias = 0.02;
  sun.shadow.radius = 4;
  scene.add(sun);
  const lampLights = [-1.4, 1.4].map((x) => {
    const l = new THREE.PointLight(0xffd9a0, 0, 7, 1.6);
    l.position.set(x, 1.9, 0);
    scene.add(l);
    return l;
  });

  // ── Zemin, halı, duvarlar
  const floor = new THREE.Mesh(new THREE.PlaneGeometry(ROOM.w, ROOM.d), M.floor);
  floor.rotation.x = -Math.PI / 2;
  floor.receiveShadow = true;
  scene.add(floor);

  const rugGeo = new THREE.ShapeGeometry(roundedRect(TABLE.length + 2.4, TABLE.width + 3.0, 0.5));
  const rug = new THREE.Mesh(rugGeo, M.rug);
  rug.rotation.x = -Math.PI / 2;
  rug.position.y = 0.004;
  rug.receiveShadow = true;
  scene.add(rug);

  // Duvarlar yalnızca odanın içinden görünür (arka yüz çizilmez): kamera hangi taraftaysa
  // o taraftaki duvar kendiliğinden kaybolur, masa her açıdan görünür kalır.
  const walls = [
    { pos: [0, ROOM.h / 2, -ROOM.d / 2], rot: 0, len: ROOM.w },
    { pos: [0, ROOM.h / 2, ROOM.d / 2], rot: Math.PI, len: ROOM.w },
    { pos: [-ROOM.w / 2, ROOM.h / 2, 0], rot: Math.PI / 2, len: ROOM.d },
    { pos: [ROOM.w / 2, ROOM.h / 2, 0], rot: -Math.PI / 2, len: ROOM.d },
  ].map((w) => {
    const m = new THREE.Mesh(new THREE.PlaneGeometry(w.len, ROOM.h), M.wall);
    m.position.set(...w.pos);
    m.rotation.y = w.rot;
    m.receiveShadow = true;
    scene.add(m);
    return m;
  });

  // Sol duvarda pencereler
  for (const z of [-2.6, 0, 2.6]) {
    const frame = new THREE.Mesh(new THREE.PlaneGeometry(2.1, 1.7), M.windowFrame);
    frame.position.set(-ROOM.w / 2 + 0.01, 1.6, z);
    frame.rotation.y = Math.PI / 2;
    scene.add(frame);
    for (const dz of [-0.49, 0.49]) {
      const pane = new THREE.Mesh(new THREE.PlaneGeometry(0.9, 1.5), M.window);
      pane.position.set(-ROOM.w / 2 + 0.02, 1.6, z + dz);
      pane.rotation.y = Math.PI / 2;
      scene.add(pane);
    }
  }

  // Arka duvarda canlı pano (beyaz tahta)
  const boardCanvas = document.createElement('canvas');
  boardCanvas.width = 1024; boardCanvas.height = 512;
  const boardTex = new THREE.CanvasTexture(boardCanvas);
  boardTex.colorSpace = THREE.SRGBColorSpace;
  boardTex.anisotropy = 4;
  const board = new THREE.Mesh(new THREE.PlaneGeometry(3.4, 1.7), new THREE.MeshBasicMaterial({ map: boardTex, toneMapped: false }));
  board.position.set(0, 1.62, -ROOM.d / 2 + 0.02);
  scene.add(board);
  let boardSig = '';

  // ── Masa
  const tableTop = new THREE.Mesh(
    new THREE.ExtrudeGeometry(roundedRect(TABLE.length, TABLE.width, 0.14), {
      depth: 0.045, bevelEnabled: true, bevelThickness: 0.012, bevelSize: 0.012, bevelSegments: 3, curveSegments: 10,
    }),
    M.wood,
  );
  tableTop.geometry.rotateX(-Math.PI / 2);
  tableTop.position.y = TOP - 0.057;
  tableTop.castShadow = true;
  tableTop.receiveShadow = true;
  scene.add(tableTop);
  for (const x of [-TABLE.length / 2 + 0.45, TABLE.length / 2 - 0.45]) {
    const leg = new THREE.Mesh(new THREE.BoxGeometry(0.07, TOP - 0.06, TABLE.width - 0.5), M.metal);
    leg.position.set(x, (TOP - 0.06) / 2, 0);
    leg.castShadow = true;
    scene.add(leg);
    const foot = new THREE.Mesh(new THREE.BoxGeometry(0.12, 0.03, TABLE.width - 0.3), M.metal);
    foot.position.set(x, 0.015, 0);
    scene.add(foot);
  }
  const rail = new THREE.Mesh(new THREE.BoxGeometry(TABLE.length - 0.9, 0.12, 0.05), M.metal);
  rail.position.set(0, TOP - 0.16, 0);
  scene.add(rail);
  // Masanın ortasında kablo kutusu
  const hub = new THREE.Mesh(new THREE.BoxGeometry(0.9, 0.025, 0.16), M.metal);
  hub.position.set(0.2, TOP + 0.012, 0);
  scene.add(hub);

  // ── Sarkıt lambalar
  for (const x of [-1.4, 1.4]) {
    const shade = new THREE.Mesh(new THREE.CylinderGeometry(0.16, 0.36, 0.2, 32, 1, true), M.lamp);
    shade.position.set(x, 2.05, 0);
    shade.material.side = THREE.DoubleSide;
    scene.add(shade);
    const glow = new THREE.Mesh(new THREE.CircleGeometry(0.34, 32), M.lampGlow);
    glow.rotation.x = Math.PI / 2;
    glow.position.set(x, 1.96, 0);
    scene.add(glow);
    const cable = new THREE.Mesh(new THREE.CylinderGeometry(0.006, 0.006, ROOM.h - 2.15, 6), M.lamp);
    cable.position.set(x, 2.15 + (ROOM.h - 2.15) / 2, 0);
    scene.add(cable);
  }

  // ── Bitkiler
  function plant(x, z, s = 1) {
    const g = new THREE.Group();
    const pot = new THREE.Mesh(new THREE.CylinderGeometry(0.26, 0.2, 0.5, 24), M.pot);
    pot.position.y = 0.25;
    pot.castShadow = true;
    g.add(pot);
    const r = hash(`${x}${z}`);
    for (let i = 0; i < 6; i++) {
      const leaf = new THREE.Mesh(new THREE.IcosahedronGeometry(0.22 + ((r >> i) & 3) * 0.04, 0), i % 2 ? M.leaf : M.leaf2);
      const a = i * 1.1 + (r % 7);
      leaf.position.set(Math.cos(a) * 0.14, 0.62 + i * 0.13, Math.sin(a) * 0.14);
      leaf.rotation.set(a, a * 0.7, 0);
      leaf.castShadow = true;
      g.add(leaf);
    }
    g.position.set(x, 0, z);
    g.scale.setScalar(s);
    scene.add(g);
  }
  plant(-6.7, -4.7, 1.2);
  plant(6.7, -4.7, 1.0);
  plant(-6.7, 4.7, 0.9);

  // ── Ortak geometriler (her kişi aynılarını kullanır)
  const G = {
    head: new THREE.SphereGeometry(0.13, 24, 18),
    hair: new THREE.SphereGeometry(0.137, 24, 12, 0, Math.PI * 2, 0, Math.PI * 0.56),
    torso: new THREE.CapsuleGeometry(0.17, 0.24, 6, 16),
    shoe: new THREE.BoxGeometry(0.1, 0.07, 0.2),
    seat: new THREE.BoxGeometry(0.48, 0.07, 0.46),
    back: new THREE.BoxGeometry(0.46, 0.5, 0.06),
    pole: new THREE.CylinderGeometry(0.025, 0.025, 0.36, 10),
    spoke: new THREE.BoxGeometry(0.3, 0.03, 0.05),
    lapBase: new THREE.BoxGeometry(0.34, 0.016, 0.23),
    lapLid: new THREE.BoxGeometry(0.34, 0.22, 0.01),
    screen: new THREE.PlaneGeometry(0.31, 0.19),
    logo: new THREE.CircleGeometry(0.025, 20),
    seatRing: new THREE.RingGeometry(0.42, 0.5, 48),
    pop: new THREE.TorusGeometry(0.2, 0.012, 8, 40),
    plane: (() => {
      const g = new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.Float32BufferAttribute([
        0, 0, 0.17, -0.12, 0.012, -0.09, 0, 0, -0.06,
        0, 0, 0.17, 0, 0, -0.06, 0.12, 0.012, -0.09,
        0, 0, 0.17, 0, -0.04, -0.075, 0, 0, -0.06,
      ], 3));
      g.computeVertexNormals();
      return g;
    })(),
    bit: new THREE.SphereGeometry(0.022, 10, 8),
    box: new THREE.BoxGeometry(0.15, 0.12, 0.15),
    boxBand: new THREE.BoxGeometry(0.155, 0.125, 0.04),
    board: new THREE.BoxGeometry(0.36, 0.02, 0.36),
    pawn: new THREE.CylinderGeometry(0.012, 0.018, 0.045, 10),
    cup: new THREE.CylinderGeometry(0.05, 0.05, 0.035, 18),
  };

  // Kişi pozu (yerel eksen: kişi +z yönüne, masaya bakar; koltuğun ortası orijin)
  const POSE = {
    thighs: [[v3(-0.1, 0.53, -0.02), v3(-0.1, 0.53, 0.36)], [v3(0.1, 0.53, -0.02), v3(0.1, 0.53, 0.36)]],
    shins: [[v3(-0.1, 0.53, 0.36), v3(-0.1, 0.09, 0.4)], [v3(0.1, 0.53, 0.36), v3(0.1, 0.09, 0.4)]],
    upper: [[v3(-0.21, 1.02, -0.02), v3(-0.25, 0.83, 0.2)], [v3(0.21, 1.02, -0.02), v3(0.25, 0.83, 0.2)]],
    fore: [[v3(-0.25, 0.83, 0.2), v3(-0.13, TOP + 0.035, 0.5)], [v3(0.25, 0.83, 0.2), v3(0.13, TOP + 0.035, 0.5)]],
  };
  const limbGeos = {};
  for (const [k, pairs] of Object.entries(POSE)) {
    limbGeos[k] = pairs.map(([a, b]) => {
      const m = limb(a, b, k === 'thighs' || k === 'shins' ? 0.065 : 0.05, null);
      return { geo: m.geometry, pos: m.position.clone(), quat: m.quaternion.clone() };
    });
  }
  const HEAD_Y = 1.26;
  const LAPTOP_Z = SEAT_GAP + 0.3;

  // ── Koltuklar
  const slots = SEATS.map((seat) => {
    const g = new THREE.Group();
    g.position.set(seat.x, 0, seat.z);
    g.rotation.y = seat.side > 0 ? Math.PI : 0;
    g.userData.slot = seat.n - 1;
    scene.add(g);

    const chair = new THREE.Group();
    const cushionMat = M.cushion.clone();
    const seatM = new THREE.Mesh(G.seat, cushionMat);
    seatM.position.y = 0.45;
    const back = new THREE.Mesh(G.back, M.chair);
    back.position.set(0, 0.84, -0.25);
    back.rotation.x = -0.1;
    const pole = new THREE.Mesh(G.pole, M.metal);
    pole.position.y = 0.24;
    chair.add(seatM, back, pole);
    for (let i = 0; i < 5; i++) {
      const s = new THREE.Mesh(G.spoke, M.metal);
      s.rotation.y = (i / 5) * Math.PI * 2;
      s.position.set(Math.cos(-s.rotation.y) * 0.13, 0.04, Math.sin(-s.rotation.y) * 0.13);
      chair.add(s);
    }
    chair.traverse((o) => { if (o.isMesh) { o.castShadow = true; o.userData.slot = seat.n - 1; } });
    g.add(chair);

    const ringMat = M.ring.clone();
    ringMat.opacity = 0;
    const ring = new THREE.Mesh(G.seatRing, ringMat);
    ring.rotation.x = -Math.PI / 2;
    ring.position.y = 0.012;
    g.add(ring);

    // Dizüstü bilgisayar: çevrimiçiyken açık ve ekranı yanar, çevrimdışıyken kapalı durur.
    const laptop = new THREE.Group();
    laptop.position.set(0, TOP, LAPTOP_Z);
    const base = new THREE.Mesh(G.lapBase, M.laptop);
    base.position.y = 0.008;
    base.castShadow = true;
    const hinge = new THREE.Group();
    hinge.position.set(0, 0.016, 0.115);
    const lid = new THREE.Mesh(G.lapLid, M.laptop);
    lid.position.y = 0.11;
    lid.castShadow = true;
    const screenMat = new THREE.MeshBasicMaterial({ color: 0x111322, toneMapped: false });
    const screen = new THREE.Mesh(G.screen, screenMat);
    screen.position.set(0, 0.11, -0.0056);
    screen.rotation.y = Math.PI;
    const logoMat = new THREE.MeshBasicMaterial({ color: 0x777777, toneMapped: false });
    const logo = new THREE.Mesh(G.logo, logoMat);
    logo.position.set(0, 0.12, 0.0056);
    hinge.add(lid, screen, logo);
    laptop.add(base, hinge);
    laptop.visible = false;
    laptop.traverse((o) => { if (o.isMesh) o.userData.slot = seat.n - 1; });
    g.add(laptop);

    const label = document.createElement('div');
    label.className = 'ol';
    label.hidden = true;
    labelsEl.appendChild(label);

    return {
      seat, g, chair, cushionMat, ring, ringMat, laptop, hinge, screenMat, logoMat,
      lidOpen: 0, lidTarget: 0, person: null, head: null, torso: null,
      occ: null, sig: null, label, bubbles: null, statusEl: null,
      typingUntil: 0, flash: 0, lookAt: null, lookUntil: 0, phase: (seat.n * 1.7) % 6,
    };
  });

  // ── Tema
  let pal = null;
  function applyPalette() {
    const cs = getComputedStyle(document.documentElement);
    const css = (n, d) => cs.getPropertyValue(n).trim() || d;
    const bg = new THREE.Color(css('--bg', '#f4f5fa'));
    const hsl = {};
    bg.getHSL(hsl);
    const dark = hsl.l < 0.3;
    pal = {
      dark,
      accent: new THREE.Color(css('--accent', '#5b55f6')),
      accent2: new THREE.Color(css('--accent-2', '#22c5e0')),
      success: new THREE.Color(css('--success', '#16a35f')),
      danger: new THREE.Color(css('--danger', '#e0424a')),
      text: css('--text', '#171a2b'),
      muted: css('--muted', '#6b7088'),
      accentCss: css('--accent', '#5b55f6'),
    };
    scene.background = new THREE.Color(dark ? 0x0c0d1a : 0xe9ebf3);
    scene.fog = new THREE.Fog(scene.background, 18, 34);
    M.floor.color.set(dark ? 0x2a2622 : 0xcfb491);
    M.wall.color.set(dark ? 0x1d2036 : 0xf4f1ea);
    M.wood.color.set(dark ? 0x9c7a56 : 0xe2c7a0);
    M.rug.color.copy(pal.accent).lerp(new THREE.Color(dark ? 0x101020 : 0xffffff), dark ? 0.65 : 0.72);
    M.accent.color.copy(pal.accent);
    M.window.color.set(dark ? 0x1d2f55 : 0xd8ecff);
    M.windowFrame.color.set(dark ? 0x2c3050 : 0xffffff);
    M.lampGlow.color.set(dark ? 0xffe2b0 : 0xfff6e6);
    hemi.intensity = dark ? 0.95 : 1.45;
    hemi.color.set(dark ? 0x9aa0d8 : 0xffffff);
    hemi.groundColor.set(dark ? 0x2a2420 : 0x8a8577);
    sun.intensity = dark ? 0.85 : 1.9;
    lampLights.forEach((l) => { l.intensity = dark ? 5 : 1.2; });
    renderer.toneMappingExposure = dark ? 1.05 : 1;
    for (const s of slots) s.logoMat.color.copy(pal.accent);
    tubes.forEach((t) => t.recolor());
    boardSig = '';
  }
  const themeObs = new MutationObserver(applyPalette);
  themeObs.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'data-mode'] });
  const mql = window.matchMedia('(prefers-color-scheme: dark)');
  mql.addEventListener?.('change', applyPalette);

  // ── Kişiler
  function buildPerson(fp) {
    const h = hash(fp);
    const shirt = new THREE.Color().setHSL((h % 360) / 360, 0.55, 0.55);
    const shirtMat = new THREE.MeshStandardMaterial({ color: shirt, roughness: 0.8 });
    const skinMat = new THREE.MeshStandardMaterial({ color: SKINS[(h >>> 9) % SKINS.length], roughness: 0.7 });
    const hairMat = new THREE.MeshStandardMaterial({ color: HAIRS[(h >>> 13) % HAIRS.length], roughness: 0.9 });
    const pantsMat = new THREE.MeshStandardMaterial({ color: PANTS[(h >>> 17) % PANTS.length], roughness: 0.85 });
    const shoeMat = new THREE.MeshStandardMaterial({ color: 0x1d1e24, roughness: 0.6 });

    const p = new THREE.Group();
    const mk = (geo, mat, pos, quat) => {
      const m = new THREE.Mesh(geo, mat);
      if (pos) m.position.copy(pos);
      if (quat) m.quaternion.copy(quat);
      m.castShadow = true;
      p.add(m);
      return m;
    };
    for (const l of limbGeos.thighs) mk(l.geo, pantsMat, l.pos, l.quat);
    for (const l of limbGeos.shins) mk(l.geo, pantsMat, l.pos, l.quat);
    for (const x of [-0.1, 0.1]) mk(G.shoe, shoeMat, v3(x, 0.035, 0.45));
    const torso = mk(G.torso, shirtMat, v3(0, 0.86, -0.02));
    torso.rotation.x = 0.1;
    for (const l of limbGeos.upper) mk(l.geo, shirtMat, l.pos, l.quat);
    for (const l of limbGeos.fore) mk(l.geo, skinMat, l.pos, l.quat);
    const head = new THREE.Group();
    head.position.set(0, HEAD_Y, 0.03);
    const skull = new THREE.Mesh(G.head, skinMat);
    skull.castShadow = true;
    const hair = new THREE.Mesh(G.hair, hairMat);
    hair.rotation.x = -0.2;
    hair.position.set(0, 0.014, -0.01);
    hair.castShadow = true;
    head.add(skull, hair);
    // Kulaklık: "Odakta" durumunda görünür.
    const phones = new THREE.Group();
    const band = new THREE.Mesh(new THREE.TorusGeometry(0.15, 0.014, 8, 24, Math.PI), M.headset);
    band.position.y = 0.02;
    band.rotation.z = 0;
    const cupL = new THREE.Mesh(G.cup, M.headset);
    const cupR = new THREE.Mesh(G.cup, M.headset);
    cupL.rotation.z = Math.PI / 2;
    cupR.rotation.z = Math.PI / 2;
    cupL.position.set(-0.145, 0.0, 0);
    cupR.position.set(0.145, 0.0, 0);
    phones.add(band, cupL, cupR);
    phones.visible = false;
    head.add(phones);
    p.add(head);
    p.traverse((o) => { if (o.isMesh) o.userData.person = true; });
    return { group: p, head, torso, phones, mats: [shirtMat, skinMat, hairMat, pantsMat, shoeMat] };
  }

  function disposePerson(s) {
    if (!s.person) return;
    s.g.remove(s.person.group);
    s.person.mats.forEach((m) => m.dispose());
    // Kişiye özel oluşturulan geometriler (ör. kulaklık bandı) de bırakılır; ortak olanlar (G) kalır.
    const shared = new Set(Object.values(G));
    s.person.group.traverse((o) => { if (o.geometry && !shared.has(o.geometry)) o.geometry.dispose(); });
    s.person = null;
  }

  function renderLabel(s) {
    const o = s.occ;
    const el = s.label;
    if (!o) {
      el.className = 'ol empty';
      el.innerHTML = `<div class="ol-tag"><span class="ol-name">Koltuk ${s.seat.n} · boş</span></div>`;
      el.dataset.officePeer = '';
      s.bubbles = null; s.statusEl = null;
      return;
    }
    el.className = `ol${o.online ? '' : ' offline'}${o.me ? ' me' : ''}${o.auto ? ' auto' : ''}${o.status ? ` st-${o.status}` : ''}`;
    el.dataset.officePeer = o.online && !o.me ? o.fp : '';
    el.innerHTML = `
      <div class="ol-bubbles"></div>
      <div class="ol-tag" title="${o.me ? 'Siz' : o.online ? 'Sohbet açmak için tıklayın · dosya bırakabilirsiniz' : 'Çevrimdışı'}">
        <span class="avatar xs" style="background:${o.faceBg}">${o.faceHtml}</span>
        <span class="ol-name"></span>${o.online && STATUS_ICON[o.status] ? `<span class="ol-st" title="${STATUS_LABEL[o.status]}">${STATUS_ICON[o.status]}</span>` : ''}
      </div>
      <div class="ol-status"></div>`;
    el.querySelector('.ol-name').textContent = o.me ? `${o.name} (siz)` : o.name;
    s.bubbles = el.querySelector('.ol-bubbles');
    s.statusEl = el.querySelector('.ol-status');
    s.statusText = null;
  }

  let people = new Map();   // parmak izi → koltuk indeksi
  let overflow = [];
  let pickMode = false;

  /**
   * Masadakileri günceller.
   * @param {{fp:string,name:string,seat:number,online:boolean,me:boolean,faceHtml:string,faceBg:string}[]} list
   * @returns {{ placed: Map<string, number>, overflow: object[] }}
   */
  function setPeople(list) {
    const valid = (n) => Number.isInteger(n) && n >= 1 && n <= SEATS.length;
    // Her bilgisayarda aynı sonuç çıksın: çevrimiçi olan önce, sonra parmak izi sırası.
    const order = [...list].sort((a, b) => (b.online - a.online) || (a.fp < b.fp ? -1 : a.fp > b.fp ? 1 : 0));
    const at = new Array(SEATS.length).fill(null);
    const rest = [];
    for (const p of order) {
      if (valid(p.seat) && !at[p.seat - 1]) at[p.seat - 1] = { ...p, auto: false };
      else if (p.online) rest.push(p);
    }
    // Koltuk seçmemiş (ya da koltuğu dolu) çevrimiçi kişiler boş koltuklara oturtulur.
    overflow = [];
    for (const p of rest) {
      const free = at.findIndex((x) => !x);
      if (free < 0) overflow.push(p);
      else at[free] = { ...p, auto: true };
    }
    people = new Map();
    at.forEach((o, i) => {
      const s = slots[i];
      if (o) people.set(o.fp, i);
      const sig = o ? JSON.stringify([o.fp, o.name, o.online, o.me, o.auto, o.faceHtml.length, o.faceBg, o.status || '']) : '';
      if (sig === s.sig) { s.occ = o; return; }
      const samePerson = s.occ && o && s.occ.fp === o.fp && s.occ.online === o.online;
      s.sig = sig;
      s.occ = o;
      if (!samePerson) {
        disposePerson(s);
        if (o?.online) {
          s.person = buildPerson(o.fp);
          s.person.group.traverse((m) => { m.userData.slot = i; });
          s.g.add(s.person.group);
          s.person.group.scale.setScalar(0.01);
          s.grow = 0;
        }
      }
      s.laptop.visible = !!o;
      const away = o?.online && o.status === 'away';
      s.lidTarget = o?.online ? (away ? 0.35 : 1) : 0;
      if (s.person) {
        // Dışarıdaysa koltuk boş görünür; odaktaysa kulaklık takılı.
        s.person.group.visible = !away;
        s.person.phones.visible = o.status === 'focus';
      }
      if (!o) s.lidOpen = 0;
      s.cushionMat.color.set(o?.me ? pal.accent : 0x4a4e66);
      renderLabel(s);
    });
    updateBoard();
    return { placed: new Map(people), overflow };
  }

  // ── Konumlar
  const _v = new THREE.Vector3();
  function headPos(fp, out = new THREE.Vector3()) {
    const i = people.get(fp);
    if (i == null || !slots[i].occ?.online) return out.copy(DOOR);
    return slots[i].g.localToWorld(out.set(0, HEAD_Y + 0.05, 0.08));
  }
  function deskPos(fp, out = new THREE.Vector3()) {
    const i = people.get(fp);
    if (i == null) return out.copy(DOOR);
    return slots[i].g.localToWorld(out.set(0, TOP + 0.14, LAPTOP_Z - 0.02));
  }
  const slotOf = (fp) => { const i = people.get(fp); return i == null ? null : slots[i]; };

  // ── Efektler
  const fx = new Set();
  function addFx(update, dispose) { fx.add({ update, dispose }); }

  function pop(pos, color, scale = 1) {
    const mat = new THREE.MeshBasicMaterial({ color, transparent: true, opacity: 0.9, depthWrite: false, toneMapped: false });
    const m = new THREE.Mesh(G.pop, mat);
    m.position.copy(pos);
    m.rotation.x = Math.PI / 2;
    scene.add(m);
    let t = 0;
    addFx((dt) => {
      t += dt / 0.7;
      m.scale.setScalar(scale * (1 + ease(clamp01(t)) * 2.4));
      mat.opacity = 0.9 * (1 - clamp01(t));
      return t < 1;
    }, () => { scene.remove(m); mat.dispose(); });
  }

  function bubble(fp, html, kind = '') {
    const s = slotOf(fp);
    if (!s?.bubbles) return;
    const b = document.createElement('div');
    b.className = `ol-bubble ${kind}`;
    b.innerHTML = html;
    s.bubbles.appendChild(b);
    while (s.bubbles.children.length > 3) s.bubbles.firstElementChild.remove();
    setTimeout(() => { b.classList.add('out'); setTimeout(() => b.remove(), 300); }, 3200);
  }

  function look(fromFp, toFp) {
    const s = slotOf(fromFp);
    if (!s?.person) return;
    s.lookAt = headPos(toFp);
    s.lookUntil = performance.now() + 2600;
  }

  /** Kâğıt uçak: mesaj bir kişiden diğerine uçar. */
  function flyPlane(fromFp, toFp, delay = 0, onArrive) {
    const a = headPos(fromFp);
    const b = headPos(toFp);
    const mid = a.clone().lerp(b, 0.5);
    mid.y += 0.7 + a.distanceTo(b) * 0.22;
    const curve = new THREE.QuadraticBezierCurve3(a, mid, b);
    const m = new THREE.Mesh(G.plane, M.paper);
    m.castShadow = true;
    m.visible = false;
    scene.add(m);
    const dur = 1.2 + a.distanceTo(b) * 0.1;
    let t = -delay;
    let trailT = 0;
    const p = new THREE.Vector3();
    const q = new THREE.Vector3();
    addFx((dt) => {
      t += dt;
      if (t < 0) return true;
      m.visible = true;
      const k = ease(clamp01(t / dur));
      curve.getPoint(k, p);
      curve.getPoint(Math.min(1, k + 0.02), q);
      m.position.copy(p);
      if (k < 0.995) m.lookAt(q);
      m.rotateZ(Math.sin(k * Math.PI) * 0.5);
      trailT += dt;
      if (trailT > 0.035 && k < 0.97) { trailT = 0; trail(p); }
      if (t >= dur) { onArrive?.(); return false; }
      return true;
    }, () => scene.remove(m));
  }

  function trail(pos) {
    const mat = new THREE.MeshBasicMaterial({ color: pal.accent, transparent: true, opacity: 0.55, depthWrite: false, toneMapped: false });
    const m = new THREE.Mesh(G.bit, mat);
    m.position.copy(pos);
    m.scale.setScalar(0.7);
    scene.add(m);
    let t = 0;
    addFx((dt) => {
      t += dt / 0.6;
      mat.opacity = 0.55 * (1 - t);
      m.scale.setScalar(0.7 * (1 - t * 0.6));
      return t < 1;
    }, () => { scene.remove(m); mat.dispose(); });
  }

  /**
   * Mesaj: gönderenden her alıcıya bir kâğıt uçak.
   * @param {string} from  gönderenin parmak izi
   * @param {string[]} to  alıcıların parmak izleri
   * @param {string} [label]  balonda görünecek kısa yazı
   */
  function message(from, to, label = 'Mesaj') {
    look(from, to[0]);
    const sp = slotOf(from);
    if (sp) sp.flash = 1;
    to.forEach((fp, i) => flyPlane(from, fp, i * 0.12, () => {
      pop(headPos(fp), pal.accent);
      bubble(fp, `💬 ${escapeHtml(label)}`);
      look(fp, from);
    }));
  }

  /** "Yazıyor…" göstergesi. */
  function typing(fp) {
    const s = slotOf(fp);
    if (s) s.typingUntil = performance.now() + 6000;
  }

  // ── Dosya aktarımları: iki masa arasında akan veri yayı ve ilerleyen paket
  const tubes = new Map();
  const stripeTex = canvasTexture(64, 8, (x, w, h) => {
    x.fillStyle = 'rgba(255,255,255,0.15)';
    x.fillRect(0, 0, w, h);
    x.fillStyle = 'rgba(255,255,255,1)';
    x.fillRect(0, 0, w * 0.45, h);
  });
  stripeTex.wrapS = stripeTex.wrapT = THREE.RepeatWrapping;

  function makeTube(t) {
    const a = deskPos(t.from);
    const b = deskPos(t.to);
    const mid = a.clone().lerp(b, 0.5);
    mid.y += 0.55 + a.distanceTo(b) * 0.16;
    const curve = new THREE.QuadraticBezierCurve3(a, mid, b);
    const len = curve.getLength();
    const map = stripeTex.clone();
    map.needsUpdate = true;
    map.repeat.set(Math.max(4, Math.round(len * 6)), 1);
    const tubeMat = new THREE.MeshBasicMaterial({ map, transparent: true, opacity: 0.55, depthWrite: false, toneMapped: false });
    const tube = new THREE.Mesh(new THREE.TubeGeometry(curve, 80, 0.013, 8), tubeMat);
    scene.add(tube);

    const boxMat = new THREE.MeshStandardMaterial({ roughness: 0.45, emissiveIntensity: 0.35 });
    const bandMat = new THREE.MeshStandardMaterial({ color: 0xffffff, roughness: 0.5 });
    const pkg = new THREE.Group();
    const box = new THREE.Mesh(G.box, boxMat);
    const band = new THREE.Mesh(G.boxBand, bandMat);
    box.castShadow = true;
    pkg.add(box, band);
    const size = Math.min(1.5, 0.75 + Math.log10(Math.max(1, t.total)) / 12);
    pkg.scale.setScalar(size);
    scene.add(pkg);

    const bitMat = new THREE.MeshBasicMaterial({ transparent: true, opacity: 0.9, toneMapped: false });
    const bits = Array.from({ length: 4 }, () => { const m = new THREE.Mesh(G.bit, bitMat); scene.add(m); return m; });

    const o = {
      id: t.id, t, curve, tube, tubeMat, map, pkg, boxMat, bits, bitMat,
      shown: 0, flow: 0, ending: null, endT: 0, alive: true,
      recolor() {
        const c = o.ending === 'bad' ? pal.danger : o.ending === 'good' ? pal.success : pal.accent;
        tubeMat.color.copy(c);
        boxMat.color.copy(c);
        boxMat.emissive.copy(c);
        bitMat.color.copy(o.ending ? c : pal.accent2);
      },
      dispose() {
        scene.remove(tube, pkg, ...bits);
        tube.geometry.dispose();
        [tubeMat, boxMat, bandMat, bitMat, map].forEach((x) => x.dispose());
      },
    };
    o.recolor();
    return o;
  }

  const ENDED = { done: 'good', failed: 'bad', declined: 'bad', cancelled: 'bad' };

  /**
   * Aktarımları eşitler.
   * @param {{id:string,from:string,to:string,status:string,done:number,total:number,speed:number,title:string}[]} list
   */
  function syncTransfers(list) {
    const seen = new Set();
    for (const t of list) {
      seen.add(t.id);
      let o = tubes.get(t.id);
      const end = ENDED[t.status];
      if (!o) {
        if (end) continue; // eski, bitmiş aktarım: çizilmez
        o = makeTube(t);
        tubes.set(t.id, o);
        look(t.from, t.to);
        const s = slotOf(t.from);
        if (s) s.flash = 1;
      }
      o.t = t;
      if (end && !o.ending) {
        o.ending = end;
        o.endT = 0;
        o.recolor();
        if (end === 'good') {
          o.arrived = false;
        } else {
          bubble(t.from, t.status === 'declined' ? '✕ Reddedildi' : t.status === 'cancelled' ? '✕ İptal' : '⚠ Başarısız', 'bad');
        }
      }
    }
    // Listeden çıkan (silinen) aktarımlar sessizce kaybolur.
    for (const [id, o] of tubes) {
      if (!seen.has(id) && !o.ending) { o.ending = 'bad'; o.endT = 0.6; o.recolor(); }
    }
  }

  const _p = new THREE.Vector3();
  function updateTubes(dt, now) {
    for (const [id, o] of tubes) {
      const t = o.t;
      const frac = t.total > 0 ? clamp01(t.done / t.total) : 0;
      const target = o.ending === 'good' ? 1 : frac;
      o.shown += (target - o.shown) * Math.min(1, dt * (o.ending === 'good' ? 4 : 2.5));
      const active = t.status === 'active' && !o.ending;
      // Veri akışı: hızlı aktarımda şeritler de hızlı akar.
      const speed = active ? 0.6 + Math.min(3, Math.log10(Math.max(1, t.speed)) / 3) : t.status === 'waiting' ? 0.08 : 0;
      o.map.offset.x -= dt * speed;
      o.flow = (o.flow + dt * speed * 0.5) % 1;

      o.curve.getPoint(o.shown, _p);
      o.pkg.position.copy(_p);
      const bob = t.status === 'waiting' && !o.ending ? Math.sin(now / 250) * 0.03 : 0;
      o.pkg.position.y += 0.03 + bob;
      o.pkg.rotation.y += dt * (active ? 1.6 : 0.4);

      o.bits.forEach((b, i) => {
        const k = (o.flow + i / o.bits.length) % 1;
        b.visible = active && o.shown > 0.03;
        if (b.visible) { o.curve.getPoint(k * o.shown, _p); b.position.copy(_p); }
      });

      if (o.ending) {
        if (o.ending === 'good' && !o.arrived && o.shown > 0.97) {
          o.arrived = true;
          pop(deskPos(t.to), pal.success, 1.2);
          bubble(t.to, `📎 ${escapeHtml(shorten(t.title, 26))}`, 'good');
          look(t.to, t.from);
        }
        if (o.ending === 'bad' || o.arrived) {
          o.endT += dt;
          const f = clamp01(o.endT / 1.4);
          o.tubeMat.opacity = 0.55 * (1 - f);
          o.pkg.scale.setScalar(Math.max(0.001, o.pkg.scale.x * (1 - dt * 2.5)));
          if (o.ending === 'bad') o.pkg.position.y -= f * 0.2;
          if (f >= 1) { o.dispose(); tubes.delete(id); }
        }
      }
    }
  }

  function transferStatus(s) {
    const fp = s.occ?.fp;
    if (!fp) return '';
    for (const o of tubes.values()) {
      if (o.ending) continue;
      const t = o.t;
      if (t.from !== fp && t.to !== fp) continue;
      const pct = t.total > 0 ? Math.floor((t.done / t.total) * 100) : 0;
      const verb = t.from === fp ? 'gönderiyor' : 'alıyor';
      if (t.status === 'waiting') return `<span class="os-file">⏳ ${t.from === fp ? 'onay bekliyor' : 'dosya geliyor'}</span>`;
      if (t.status === 'paused') return `<span class="os-file">⏸ duraklatıldı · %${pct}</span>`;
      return `<span class="os-file">${t.from === fp ? '↑' : '↓'} ${verb} · %${pct}</span>`;
    }
    return '';
  }

  // ── Oyunlar: iki oyuncunun arasında masada küçük bir satranç tahtası
  const boards = new Map();
  const chessTex = canvasTexture(256, 256, (x, w) => {
    const c = w / 8;
    for (let i = 0; i < 64; i++) {
      x.fillStyle = (i + Math.floor(i / 8)) % 2 ? '#7a5a3a' : '#f0dcc0';
      x.fillRect((i % 8) * c, Math.floor(i / 8) * c, c, c);
    }
  });
  const pawnW = new THREE.MeshStandardMaterial({ color: 0xf6f2ea, roughness: 0.4 });
  const pawnB = new THREE.MeshStandardMaterial({ color: 0x24242c, roughness: 0.4 });
  const boardMats = [M.wood, M.wood, new THREE.MeshStandardMaterial({ map: chessTex, roughness: 0.5 }), M.wood, M.wood, M.wood];

  /** @param {{id:string,a:string,b:string}[]} list  süren oyunlar */
  function setGames(list) {
    const want = new Set();
    for (const g of list) {
      const sa = slotOf(g.a), sb = slotOf(g.b);
      if (!sa || !sb) continue;
      want.add(g.id);
      const pa = sa.g.localToWorld(v3(0, 0, LAPTOP_Z));
      const pb = sb.g.localToWorld(v3(0, 0, LAPTOP_Z));
      const mid = pa.lerp(pb, 0.5);
      // Aynı sıradaysa tahta masanın ortasına doğru çekilir.
      if (sa.seat.side === sb.seat.side) mid.z = sa.seat.side * 0.3;
      mid.x = Math.max(-TABLE.length / 2 + 0.4, Math.min(TABLE.length / 2 - 0.4, mid.x));
      let b = boards.get(g.id);
      if (!b) {
        b = new THREE.Group();
        const top = new THREE.Mesh(G.board, boardMats);
        top.castShadow = true;
        b.add(top);
        const c = 0.36 / 8;
        [[1, 6, pawnB], [3, 5, pawnB], [4, 4, pawnW], [2, 1, pawnW], [6, 2, pawnW], [5, 6, pawnB]].forEach(([fx_, fz, mat]) => {
          const p = new THREE.Mesh(G.pawn, mat);
          p.position.set(-0.18 + c * (fx_ + 0.5), 0.032, -0.18 + c * (fz + 0.5));
          p.castShadow = true;
          b.add(p);
        });
        b.scale.setScalar(0.01);
        scene.add(b);
        boards.set(g.id, b);
      }
      b.position.set(mid.x, TOP + 0.01, mid.z);
      b.lookAt(pb.x, TOP + 0.01, pb.z);
    }
    for (const [id, b] of boards) if (!want.has(id)) { scene.remove(b); boards.delete(id); }
  }

  // ── Pano
  function updateBoard() {
    if (!pal) return;
    const online = slots.filter((s) => s.occ?.online).length + overflow.length;
    const moving = [...tubes.values()].filter((o) => !o.ending).length;
    const clock = new Date().toLocaleTimeString('tr-TR', { hour: '2-digit', minute: '2-digit' });
    const sig = [online, moving, clock, pal.accentCss, pal.dark].join('|');
    if (sig === boardSig) return;
    boardSig = sig;
    const x = boardCanvas.getContext('2d');
    const w = boardCanvas.width, h = boardCanvas.height;
    x.fillStyle = pal.dark ? '#151729' : '#ffffff';
    x.fillRect(0, 0, w, h);
    x.strokeStyle = pal.dark ? '#2b2e4c' : '#d9dbe6';
    x.lineWidth = 14;
    x.strokeRect(7, 7, w - 14, h - 14);
    x.fillStyle = pal.accentCss;
    x.fillRect(60, 70, 14, 64);
    x.font = '700 58px "Segoe UI", system-ui, sans-serif';
    x.fillStyle = pal.dark ? '#eceefa' : '#171a2b';
    x.textBaseline = 'middle';
    x.fillText('FastFiles Ofis', 96, 104);
    x.font = '500 44px "Segoe UI", system-ui, sans-serif';
    x.fillStyle = pal.dark ? '#9196b5' : '#6b7088';
    x.textAlign = 'right';
    x.fillText(clock, w - 64, 104);
    x.textAlign = 'left';
    const line = (y, dot, text) => {
      x.fillStyle = dot;
      x.beginPath(); x.arc(84, y, 12, 0, Math.PI * 2); x.fill();
      x.fillStyle = pal.dark ? '#eceefa' : '#171a2b';
      x.font = '500 48px "Segoe UI", system-ui, sans-serif';
      x.fillText(text, 116, y);
    };
    line(240, '#16a35f', `Masada ${online} kişi çevrimiçi`);
    line(340, moving ? pal.accentCss : '#b5b8c8', moving ? `${moving} dosya aktarımı sürüyor` : 'Şu an aktarım yok');
    boardTex.needsUpdate = true;
  }

  // ── Seçim (tıklama / üzerine gelme / dosya bırakma)
  const ray = new THREE.Raycaster();
  const ndc = new THREE.Vector2();
  function slotAt(clientX, clientY) {
    const r = renderer.domElement.getBoundingClientRect();
    if (clientX < r.left || clientX > r.right || clientY < r.top || clientY > r.bottom) return null;
    ndc.set(((clientX - r.left) / r.width) * 2 - 1, -((clientY - r.top) / r.height) * 2 + 1);
    ray.setFromCamera(ndc, camera);
    const hits = ray.intersectObjects(slots.map((s) => s.g), true);
    for (const h of hits) {
      let o = h.object;
      let shown = true;
      for (let p = o; p; p = p.parent) if (!p.visible) shown = false;
      if (!shown) continue;
      while (o && o.userData.slot == null) o = o.parent;
      if (o) return slots[o.userData.slot];
    }
    return null;
  }
  /** Ekrandaki noktada (çevrimiçi, kendisi olmayan) kim var? Etiketler de sayılır. */
  function fpAt(clientX, clientY) {
    const el = document.elementFromPoint(clientX, clientY)?.closest?.('[data-office-peer]');
    if (el?.dataset.officePeer) return el.dataset.officePeer;
    const s = slotAt(clientX, clientY);
    return s?.occ?.online && !s.occ.me ? s.occ.fp : null;
  }
  let dropFp = null;
  function setDropTarget(fp) {
    dropFp = fp || null;
    slots.forEach((s) => s.label.classList.toggle('drop', !!dropFp && s.occ?.fp === dropFp));
  }

  let down = null;
  renderer.domElement.addEventListener('pointerdown', (e) => { down = { x: e.clientX, y: e.clientY }; });
  renderer.domElement.addEventListener('pointerup', (e) => {
    if (!down || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 6) { down = null; return; }
    down = null;
    const s = slotAt(e.clientX, e.clientY);
    if (!s) return;
    if (pickMode) { if (!s.occ || s.occ.me || !s.occ.online) hooks.onSeat?.(s.seat.n); return; }
    if (s.occ?.online && !s.occ.me) hooks.onPerson?.(s.occ.fp);
  });
  let hoverSlot = null;
  renderer.domElement.addEventListener('pointermove', (e) => {
    if (e.buttons) return;
    const s = slotAt(e.clientX, e.clientY);
    hoverSlot = s;
    const clickable = s && (pickMode ? (!s.occ || s.occ.me || !s.occ.online) : (s.occ?.online && !s.occ.me));
    renderer.domElement.style.cursor = clickable ? 'pointer' : '';
    slots.forEach((x) => x.label.classList.toggle('hover', x === s && !!clickable));
  });
  renderer.domElement.addEventListener('pointerleave', () => {
    hoverSlot = null;
    slots.forEach((x) => x.label.classList.remove('hover'));
  });
  labelsEl.addEventListener('click', (e) => {
    const el = e.target.closest('.ol');
    const s = slots.find((x) => x.label === el);
    if (!s) return;
    if (pickMode) { if (!s.occ || s.occ.me || !s.occ.online) hooks.onSeat?.(s.seat.n); return; }
    if (s.occ?.online && !s.occ.me) hooks.onPerson?.(s.occ.fp);
  });

  function setPickMode(on) {
    pickMode = !!on;
    root.classList.toggle('picking', pickMode);
  }

  // ── Kamera
  let camAnim = null;
  function resetCamera(dur = 0.9) {
    camAnim = { from: camera.position.clone(), fromT: controls.target.clone(), t: 0, dur };
  }

  // ── Döngü
  const clock = new THREE.Clock();
  let raf = 0;
  let running = false;
  let lastBoard = 0;
  const _w = new THREE.Vector3();

  function update(dt) {
    const now = performance.now();
    if (camAnim) {
      camAnim.t += dt / camAnim.dur;
      const k = ease(clamp01(camAnim.t));
      camera.position.lerpVectors(camAnim.from, CAM_HOME.pos, k);
      controls.target.lerpVectors(camAnim.fromT, CAM_HOME.target, k);
      if (camAnim.t >= 1) camAnim = null;
    }
    const breath = now / 1000;
    for (const s of slots) {
      // Kapak açılıp kapanır
      s.lidOpen += (s.lidTarget - s.lidOpen) * Math.min(1, dt * 4);
      s.hinge.rotation.x = -Math.PI / 2 + s.lidOpen * (Math.PI / 2 + 0.3);
      const typingNow = s.typingUntil > now;
      s.flash = Math.max(0, s.flash - dt * 1.5);
      const glow = s.occ?.online && s.occ.status !== 'away' ? 0.55 + (typingNow ? 0.3 + Math.sin(now / 90) * 0.15 : 0) + s.flash * 0.4 : 0.04;
      s.screenMat.color.copy(pal.accent).multiplyScalar(glow).lerp(new THREE.Color(0x0a0b14), s.occ?.online ? 0.25 : 0.9);
      s.logoMat.color.copy(pal.accent).multiplyScalar(s.occ?.online ? 1 : 0.25);

      // Kendi koltuğum ve koltuk seçme modundaki boş koltuklar parlar
      const free = !s.occ || !s.occ.online;
      const ringOn = (s.occ?.me && !pickMode) || (pickMode && (free || s.occ?.me));
      const target = ringOn ? (pickMode ? 0.55 + Math.sin(now / 260) * 0.3 : 0.75) : 0;
      s.ringMat.opacity += (target - s.ringMat.opacity) * Math.min(1, dt * 6);
      s.ringMat.color.copy(pickMode && free ? pal.accent2 : pal.accent);
      s.ring.visible = s.ringMat.opacity > 0.01;
      s.cushionMat.emissive.copy(pal.accent).multiplyScalar(pickMode && free ? 0.25 + Math.sin(now / 260) * 0.15 : 0);

      if (s.person) {
        if (s.grow < 1) { s.grow = Math.min(1, s.grow + dt * 2.5); s.person.group.scale.setScalar(Math.max(0.01, ease(s.grow))); }
        s.person.torso.scale.y = 1 + Math.sin(breath * 1.7 + s.phase) * 0.015;
        // Baş: biriyle etkileşimdeyse ona döner, yoksa hafifçe sağa sola bakar
        let yaw = Math.sin(breath * 0.35 + s.phase) * 0.25;
        let pitch = typingNow ? 0.28 : 0.05;
        if (s.lookAt && s.lookUntil > now) {
          s.g.worldToLocal(_w.copy(s.lookAt));
          yaw = Math.atan2(_w.x, _w.z);
          yaw = Math.max(-1.2, Math.min(1.2, yaw));
          pitch = -0.15;
        }
        s.person.head.rotation.y += (yaw - s.person.head.rotation.y) * Math.min(1, dt * 5);
        s.person.head.rotation.x += (pitch - s.person.head.rotation.x) * Math.min(1, dt * 5);
      }
    }
    updateTubes(dt, now);
    for (const f of fx) if (!f.update(dt)) { f.dispose?.(); fx.delete(f); }
    for (const b of boards.values()) if (b.scale.x < 1) b.scale.setScalar(Math.min(1, b.scale.x + dt * 3));
    if (now - lastBoard > 1000) { lastBoard = now; updateBoard(); }
  }

  const _proj = new THREE.Vector3();
  function updateLabels() {
    const w = renderer.domElement.clientWidth;
    const h = renderer.domElement.clientHeight;
    const now = performance.now();
    for (const s of slots) {
      const show = s.occ || pickMode;
      if (!show) { s.label.hidden = true; continue; }
      const seated = s.occ?.online && s.occ.status !== 'away';
      s.g.localToWorld(_proj.set(0, seated ? HEAD_Y + 0.42 : TOP + 0.45, seated ? 0.03 : 0.25));
      _proj.project(camera);
      if (_proj.z > 1) { s.label.hidden = true; continue; }
      s.label.hidden = false;
      const x = (_proj.x * 0.5 + 0.5) * w;
      const y = (-_proj.y * 0.5 + 0.5) * h;
      s.label.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) translate(-50%, -100%)`;
      s.label.style.zIndex = String(1000 - Math.round(_proj.z * 1000));
      if (s.statusEl) {
        let st = '';
        if (s.typingUntil > now) st = '<span class="os-typing">yazıyor<i>.</i><i>.</i><i>.</i></span>';
        else st = transferStatus(s) || (!s.occ.online ? 'çevrimdışı' : STATUS_LABEL[s.occ.status] ? STATUS_LABEL[s.occ.status].toLocaleLowerCase('tr') : s.occ.auto && s.occ.me ? 'koltuk seçmediniz' : '');
        if (st !== s.statusText) { s.statusText = st; s.statusEl.innerHTML = st; s.statusEl.hidden = !st; }
      }
    }
  }

  function resize() {
    const w = root.clientWidth, h = root.clientHeight;
    if (!w || !h) return;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    // Dar pencerede masanın tamamı görünsün
    camera.fov = w / h < 1.2 ? 52 : 40;
    camera.updateProjectionMatrix();
  }
  const ro = new ResizeObserver(resize);
  ro.observe(root);

  function loop() {
    raf = requestAnimationFrame(loop);
    const dt = Math.min(clock.getDelta(), 0.05);
    update(dt);
    controls.update();
    renderer.render(scene, camera);
    updateLabels();
  }

  function start() {
    if (running) return;
    running = true;
    resize();
    clock.getDelta();
    loop();
  }
  function stop() {
    running = false;
    cancelAnimationFrame(raf);
  }

  applyPalette();
  resetCamera(2.2); // açılışta kamera yukarıdan süzülerek masaya yaklaşır

  return {
    start, stop, setPeople, message, typing, syncTransfers, setGames, setPickMode, resetCamera, fpAt, setDropTarget,
    seats: SEATS.length,
    get pickMode() { return pickMode; },
  };
}

function escapeHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
const shorten = (s, n) => (String(s).length > n ? `${String(s).slice(0, n - 1)}…` : String(s));
