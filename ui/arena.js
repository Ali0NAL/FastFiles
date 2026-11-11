// Arena: birinci şahıs nişancı (herkes tek başına, ölüm maçı).
//
// Bu modül yalnızca bu bilgisayarın oyuncusunu oynatır ve sahneyi çizer. Maçın yetkilisi oda
// sahibidir (arka uç: node/arena.rs). Akış:
// - Hareket burada hesaplanır (akıcı olsun diye) ve saniyede ~30 kez sahibe gönderilir.
// - Atışta vuruşu burada buluruz (ışın: harita + diğer oyuncuların kafa/gövde kutuları),
//   sahibe "şunu vurdum" deriz; sahip doğrular, canı o düşürür.
// - Sahipten gelen durumlar (`snap`) diğer oyuncuları, canımızı, ölüm/doğmayı ve skoru taşır.
// Harita, silahlar ve kurallar arka uçtan (`arena_info`) gelir; burada sabit değer yoktur.

import * as THREE from './vendor/three.module.min.js';

const SEND_HZ = 30;
const GRAVITY = 22;
const JUMP_V = 7.6;
const WALK = 6.4;
const CROUCH_WALK = 3.2;
const AIM_WALK = 4.2;
const STEP_UP = 0.55;
const FOV = 80;

const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const fmtTime = (ms) => {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

// ───────────────────────────── Ses (sentezlenmiş) ─────────────────────────────

function createAudio() {
  let ctx = null;
  let noise = null;
  const ensure = () => {
    if (ctx) return ctx;
    try {
      ctx = new AudioContext();
      noise = ctx.createBuffer(1, ctx.sampleRate * 0.6, ctx.sampleRate);
      const d = noise.getChannelData(0);
      for (let i = 0; i < d.length; i++) d[i] = Math.random() * 2 - 1;
    } catch { ctx = null; }
    return ctx;
  };
  const shot = (w, vol = 1) => {
    const c = ensure();
    if (!c || vol <= 0.01) return;
    const t = c.currentTime;
    const src = c.createBufferSource();
    src.buffer = noise;
    const f = c.createBiquadFilter();
    const g = c.createGain();
    const [type, freq, len, peak] = [['bandpass', 1800, 0.09, 0.45], ['lowpass', 1400, 0.45, 0.9], ['lowpass', 700, 0.3, 0.8]][w] || ['bandpass', 1500, 0.1, 0.4];
    f.type = type;
    f.frequency.value = freq;
    g.gain.setValueAtTime(peak * vol, t);
    g.gain.exponentialRampToValueAtTime(0.001, t + len);
    src.connect(f).connect(g).connect(c.destination);
    src.start(t);
    src.stop(t + len + 0.05);
    // Düşük gümbürtü (keskin nişancı ve pompalı).
    if (w !== 0) tone(80, 0.25, 0.35 * vol, 'sine', 40);
  };
  const tone = (freq, len, vol, type = 'sine', to = null) => {
    const c = ensure();
    if (!c) return;
    const t = c.currentTime;
    const o = c.createOscillator();
    const g = c.createGain();
    o.type = type;
    o.frequency.setValueAtTime(freq, t);
    if (to) o.frequency.exponentialRampToValueAtTime(to, t + len);
    g.gain.setValueAtTime(vol, t);
    g.gain.exponentialRampToValueAtTime(0.001, t + len);
    o.connect(g).connect(c.destination);
    o.start(t);
    o.stop(t + len + 0.02);
  };
  return {
    unlock: () => { try { ensure()?.resume?.()?.catch?.(() => {}); } catch { /* kapalı */ } },
    shot,
    hit: (head) => tone(head ? 1700 : 1150, 0.07, 0.25, 'square'),
    kill: () => { tone(880, 0.12, 0.3, 'triangle'); setTimeout(() => tone(1320, 0.18, 0.3, 'triangle'), 90); },
    hurt: () => tone(140, 0.18, 0.4, 'sawtooth', 70),
    reload: () => { tone(400, 0.05, 0.15, 'square'); setTimeout(() => tone(600, 0.05, 0.15, 'square'), 140); },
    empty: () => tone(300, 0.04, 0.15, 'square'),
    close: () => { try { ctx?.close(); } catch { /* kapalı */ } },
  };
}

// ───────────────────────────── Görseller ─────────────────────────────

function nameSprite(text, color) {
  const c = document.createElement('canvas');
  c.width = 256;
  c.height = 64;
  const x = c.getContext('2d');
  x.font = 'bold 30px "Segoe UI", sans-serif';
  x.textAlign = 'center';
  x.textBaseline = 'middle';
  const w = Math.min(250, x.measureText(text).width + 24);
  x.fillStyle = 'rgba(10,12,20,.55)';
  x.beginPath();
  x.roundRect(128 - w / 2, 10, w, 44, 12);
  x.fill();
  x.fillStyle = color;
  x.fillText(text, 128, 33);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, depthTest: false, transparent: true }));
  s.scale.set(1.6, 0.4, 1);
  s.renderOrder = 10;
  return s;
}

function floorTexture(hex) {
  const c = document.createElement('canvas');
  c.width = c.height = 128;
  const x = c.getContext('2d');
  const base = new THREE.Color(hex);
  x.fillStyle = `#${base.getHexString()}`;
  x.fillRect(0, 0, 128, 128);
  x.strokeStyle = `#${base.clone().multiplyScalar(0.82).getHexString()}`;
  x.lineWidth = 3;
  x.strokeRect(1.5, 1.5, 125, 125);
  for (let i = 0; i < 260; i++) {
    x.fillStyle = Math.random() < 0.5 ? 'rgba(0,0,0,.05)' : 'rgba(255,255,255,.04)';
    x.fillRect(Math.random() * 128, Math.random() * 128, 2, 2);
  }
  const t = new THREE.CanvasTexture(c);
  t.wrapS = t.wrapT = THREE.RepeatWrapping;
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

/** Diğer oyuncunun basit insan modeli: bacak, gövde, kafa, silah; kafa ve gövde vuruş kutusudur. */
function buildPerson(color, name) {
  const g = new THREE.Group();
  const body = new THREE.MeshLambertMaterial({ color });
  const dark = new THREE.MeshLambertMaterial({ color: new THREE.Color(color).multiplyScalar(0.55) });
  const skin = new THREE.MeshLambertMaterial({ color: 0xe8c39e });
  const gunM = new THREE.MeshLambertMaterial({ color: 0x222831 });
  const box = (w, h, d, m, y, x = 0, z = 0) => {
    const mesh = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), m);
    mesh.position.set(x, y, z);
    g.add(mesh);
    return mesh;
  };
  const legL = box(0.2, 0.8, 0.25, dark, 0.4, -0.12);
  const legR = box(0.2, 0.8, 0.25, dark, 0.4, 0.12);
  const torso = box(0.56, 0.62, 0.32, body, 1.11);
  const head = box(0.34, 0.34, 0.34, skin, 1.6);
  box(0.36, 0.08, 0.36, dark, 1.79); // saç/şapka
  const arm = box(0.14, 0.14, 0.5, body, 1.22, 0.24, -0.2);
  const gun = box(0.08, 0.1, 0.62, gunM, 1.26, 0.24, -0.5);
  for (const m of [legL, legR, torso]) m.userData.hit = 'body';
  head.userData.hit = 'head';
  const tag = nameSprite(name, `#${new THREE.Color(color).getHexString()}`);
  tag.position.y = 2.15;
  g.add(tag);
  return { group: g, legL, legR, torso, head, arm, gun, tag, hitMeshes: [legL, legR, torso, head] };
}

/** Birinci şahıs silah modeli (kamera önünde). */
function buildViewModel(w) {
  const g = new THREE.Group();
  const metal = new THREE.MeshLambertMaterial({ color: 0x2b2f38 });
  const accent = new THREE.MeshLambertMaterial({ color: [0x4b6b8a, 0x6b4f2a, 0x7a3b2e][w] });
  const add = (geo, m, x, y, z) => { const mesh = new THREE.Mesh(geo, m); mesh.position.set(x, y, z); g.add(mesh); return mesh; };
  let muzzleZ = -0.7;
  if (w === 0) {
    add(new THREE.BoxGeometry(0.07, 0.09, 0.55), metal, 0, 0, -0.3);
    add(new THREE.BoxGeometry(0.05, 0.14, 0.08), accent, 0, -0.1, -0.28);
    add(new THREE.BoxGeometry(0.06, 0.07, 0.18), accent, 0, -0.02, 0.03);
    add(new THREE.CylinderGeometry(0.015, 0.015, 0.2, 8).rotateX(Math.PI / 2), metal, 0, 0.01, -0.65);
    muzzleZ = -0.76;
  } else if (w === 1) {
    add(new THREE.BoxGeometry(0.06, 0.08, 0.7), accent, 0, 0, -0.32);
    add(new THREE.CylinderGeometry(0.018, 0.018, 0.45, 8).rotateX(Math.PI / 2), metal, 0, 0.02, -0.85);
    add(new THREE.CylinderGeometry(0.035, 0.035, 0.24, 12).rotateX(Math.PI / 2), metal, 0, 0.08, -0.3);
    muzzleZ = -1.08;
  } else {
    add(new THREE.BoxGeometry(0.1, 0.1, 0.5), accent, 0, -0.01, -0.22);
    add(new THREE.CylinderGeometry(0.03, 0.03, 0.5, 10).rotateX(Math.PI / 2), metal, 0.02, 0.03, -0.55);
    add(new THREE.CylinderGeometry(0.03, 0.03, 0.42, 10).rotateX(Math.PI / 2), metal, -0.02, -0.03, -0.5);
    muzzleZ = -0.8;
  }
  const flash = new THREE.Mesh(new THREE.SphereGeometry(0.06, 8, 6), new THREE.MeshBasicMaterial({ color: 0xffd27a, transparent: true, opacity: 0.95 }));
  flash.position.set(0, 0.01, muzzleZ);
  flash.visible = false;
  g.add(flash);
  g.userData.flash = flash;
  g.userData.muzzleZ = muzzleZ;
  g.scale.setScalar(0.55);
  return g;
}

// ───────────────────────────── Oyun ─────────────────────────────

/**
 * @param {HTMLElement} root  oyunun çizileceği tam ekran kap
 * @param {{info:any, mapId:string, seat:number, players:{name:string, color:string}[], send:(p:any)=>void, onExit:()=>void}} opts
 */
export function createArena(root, opts) {
  const { info, seat: mySeat, players, send, onExit } = opts;
  const map = info.maps.find((m) => m.id === opts.mapId) || info.maps[0];
  const W = info.weapons;
  const R = info.rules;
  const audio = createAudio();
  const disposables = [];

  // ── DOM ──
  root.innerHTML = `
    <canvas class="ar-canvas"></canvas>
    <div class="ar-vignette"></div>
    <div class="ar-scope" hidden><div class="ar-scope-ring"></div></div>
    <div class="ar-cross"><i></i><i></i><i></i><i></i></div>
    <div class="ar-hitmark"></div>
    <div class="ar-top"><div class="ar-time">5:00</div><div class="ar-sub"></div></div>
    <div class="ar-lead"></div>
    <div class="ar-feed"></div>
    <div class="ar-hp"><div class="ar-hp-bar"><i></i></div><b>100</b></div>
    <div class="ar-weapon"><div class="ar-ammo"><b>30</b><span>/ 30</span></div><div class="ar-wname"></div><div class="ar-slots"></div></div>
    <div class="ar-banner" hidden></div>
    <div class="ar-dead" hidden></div>
    <div class="ar-board" hidden></div>
    <div class="ar-pause">
      <div class="ar-pause-card">
        <h2>${esc(map.name)}</h2>
        <p>Oynamak için tıklayın</p>
        <div class="ar-keys">
          <span><kbd>W A S D</kbd> hareket</span><span><kbd>Boşluk</kbd> zıpla</span><span><kbd>C</kbd> çömel</span>
          <span><kbd>Sol tık</kbd> ateş</span><span><kbd>Sağ tık</kbd> nişan / dürbün</span><span><kbd>R</kbd> doldur</span>
          <span><kbd>1 2 3</kbd> / tekerlek silah</span><span><kbd>Tab</kbd> skor</span><span><kbd>Esc</kbd> duraklat</span>
        </div>
        <label class="ar-sens">Fare hassasiyeti <input type="range" min="0.2" max="3" step="0.05"></label>
        <div class="ar-pause-actions"><button class="ar-btn" data-act="play">Oyuna dön</button><button class="ar-btn ghost" data-act="exit">Maçtan çık</button></div>
      </div>
    </div>`;
  const $ = (s) => root.querySelector(s);
  const canvas = $('.ar-canvas');

  let sens = 1;
  try { sens = Number(localStorage.getItem('arena.sens')) || 1; } catch { /* yok */ }
  const sensInput = $('.ar-sens input');
  sensInput.value = sens;
  sensInput.oninput = () => { sens = Number(sensInput.value); try { localStorage.setItem('arena.sens', String(sens)); } catch { /* yok */ } };

  // ── Sahne ──
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, powerPreference: 'high-performance' });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  const scene = new THREE.Scene();
  scene.background = new THREE.Color(map.sky);
  scene.fog = new THREE.Fog(map.fog, 35, 110);
  const camera = new THREE.PerspectiveCamera(FOV, 1, 0.05, 300);
  scene.add(camera);
  scene.add(new THREE.HemisphereLight(0xffffff, 0x444455, 1.6));
  const sun = new THREE.DirectionalLight(0xffffff, 1.4);
  sun.position.set(18, 30, 10);
  scene.add(sun);

  const floorTex = floorTexture(map.floor);
  floorTex.repeat.set(map.halfX / 2, map.halfZ / 2);
  const floor = new THREE.Mesh(new THREE.PlaneGeometry(map.halfX * 2 + 2, map.halfZ * 2 + 2), new THREE.MeshLambertMaterial({ map: floorTex }));
  floor.rotation.x = -Math.PI / 2;
  scene.add(floor);
  disposables.push(floor.geometry, floor.material, floorTex);

  // Harita kutuları: çizim + çarpışma + mermi ışını.
  const boxes = map.blocks.map(([x0, y0, z0, x1, y1, z1, color]) => ({ min: [x0, y0, z0], max: [x1, y1, z1], color }));
  const mapMeshes = [floor];
  const mats = new Map();
  const edgeMat = new THREE.LineBasicMaterial({ color: 0x000000, transparent: true, opacity: 0.18 });
  disposables.push(edgeMat);
  for (const b of boxes) {
    const [w, h, d] = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
    if (!mats.has(b.color)) { const m = new THREE.MeshLambertMaterial({ color: b.color }); mats.set(b.color, m); disposables.push(m); }
    const geo = new THREE.BoxGeometry(w, h, d);
    const mesh = new THREE.Mesh(geo, mats.get(b.color));
    mesh.position.set((b.min[0] + b.max[0]) / 2, (b.min[1] + b.max[1]) / 2, (b.min[2] + b.max[2]) / 2);
    scene.add(mesh);
    mapMeshes.push(mesh);
    const eg = new THREE.EdgesGeometry(geo);
    const edges = new THREE.LineSegments(eg, edgeMat);
    edges.position.copy(mesh.position);
    scene.add(edges);
    disposables.push(geo, eg);
  }

  // ── Oyuncular ──
  const others = players.map((p, s) => {
    if (s === mySeat) return null;
    const person = buildPerson(p.color, p.name);
    person.group.visible = false;
    scene.add(person.group);
    for (const m of person.hitMeshes) m.userData.seat = s;
    return { ...person, target: new THREE.Vector3(), yaw: 0, crouch: false, alive: false, on: false, seen: false, walk: 0 };
  });
  const hitMeshes = () => others.filter((o) => o && o.group.visible).flatMap((o) => o.hitMeshes);

  // ── Yerel oyuncu ──
  const me = {
    pos: new THREE.Vector3(0, 0, 0), vel: new THREE.Vector3(), onGround: false, yaw: 0, pitch: 0,
    crouch: false, eye: R.eye, alive: false, sp: -1, hp: R.maxHp, w: 0, prevW: 1,
    ammo: W.map((w) => w.mag), reloadUntil: 0, lastShot: [0, 0, 0], kickPitch: 0, bob: 0,
  };
  const keys = new Set();
  let mouseDown = false;
  let aiming = false;
  let locked = false;
  let running = true;
  let started = false;
  let matchOver = false;
  let timeLeft = R.matchSecs * 1000;
  let lastSnap = null;
  let damageFlash = 0;
  let hitMarkUntil = 0;
  const feed = [];
  const tracers = [];

  let viewModel = buildViewModel(0);
  camera.add(viewModel);
  const setWeapon = (w) => {
    if (w === me.w || w < 0 || w >= W.length) return;
    me.prevW = me.w;
    me.w = w;
    me.reloadUntil = 0;
    camera.remove(viewModel);
    viewModel.traverse((o) => { o.geometry?.dispose(); if (o.material && !o.material.isLineBasicMaterial) o.material.dispose?.(); });
    viewModel = buildViewModel(w);
    camera.add(viewModel);
    aiming = false;
    renderHud();
  };

  // ── Çarpışma ──
  const height = () => (me.crouch ? R.crouchHeight : R.playerHeight);
  const r = R.playerRadius;
  const overlaps = (p, h, b) => p.x + r > b.min[0] + 1e-4 && p.x - r < b.max[0] - 1e-4 && p.z + r > b.min[2] + 1e-4 && p.z - r < b.max[2] - 1e-4
    && p.y + h > b.min[1] + 1e-4 && p.y < b.max[1] - 1e-4;
  const freeAt = (p, h) => !boxes.some((b) => overlaps(p, h, b));

  function moveAxis(axis, delta) {
    if (delta === 0) return;
    const h = height();
    me.pos[axis] += delta;
    for (const b of boxes) {
      if (!overlaps(me.pos, h, b)) continue;
      if (axis === 'y') {
        if (delta < 0) { me.pos.y = b.max[1]; me.onGround = true; } else me.pos.y = b.min[1] - h;
        me.vel.y = 0;
        continue;
      }
      // Basamak: ayak hizasındaki alçak engelin üstüne çık.
      const rise = b.max[1] - me.pos.y;
      if (rise > 0 && rise <= STEP_UP && (me.onGround || me.vel.y <= 0)) {
        const up = me.pos.clone();
        up.y = b.max[1];
        if (freeAt(up, h)) { me.pos.y = b.max[1]; continue; }
      }
      const i = axis === 'x' ? 0 : 2;
      me.pos[axis] = delta > 0 ? b.min[i] - r - 1e-3 : b.max[i] + r + 1e-3;
      me.vel[axis] = 0;
    }
  }

  function physics(dt) {
    const w = W[me.w];
    // Çömelme: kalkmak için üstte yer olmalı.
    const wantCrouch = keys.has('KeyC') || keys.has('ControlLeft');
    if (wantCrouch) me.crouch = true;
    else if (me.crouch) { const up = me.pos.clone(); me.crouch = false; if (!freeAt(up, R.playerHeight)) me.crouch = true; }
    const fwd = (keys.has('KeyW') || keys.has('ArrowUp') ? 1 : 0) - (keys.has('KeyS') || keys.has('ArrowDown') ? 1 : 0);
    const side = (keys.has('KeyD') || keys.has('ArrowRight') ? 1 : 0) - (keys.has('KeyA') || keys.has('ArrowLeft') ? 1 : 0);
    const sin = Math.sin(me.yaw);
    const cos = Math.cos(me.yaw);
    let wx = -sin * fwd + cos * side;
    let wz = -cos * fwd - sin * side;
    const l = Math.hypot(wx, wz);
    if (l > 0) { wx /= l; wz /= l; }
    const speed = me.crouch ? CROUCH_WALK : aiming && w.zoom > 1.3 ? AIM_WALK : WALK;
    const accel = me.onGround ? 60 : 14;
    const tx = wx * speed;
    const tz = wz * speed;
    const k = Math.min(1, accel * dt / Math.max(speed, 1));
    if (me.onGround || l > 0) {
      me.vel.x += (tx - me.vel.x) * k;
      me.vel.z += (tz - me.vel.z) * k;
    }
    if (keys.has('Space') && me.onGround) { me.vel.y = JUMP_V; me.onGround = false; }
    me.vel.y -= GRAVITY * dt;
    me.onGround = false;
    moveAxis('x', me.vel.x * dt);
    moveAxis('z', me.vel.z * dt);
    moveAxis('y', me.vel.y * dt);
    if (me.pos.y <= 0) { me.pos.y = 0; me.vel.y = Math.max(0, me.vel.y); me.onGround = true; }
    me.pos.x = clamp(me.pos.x, -map.halfX + r, map.halfX - r);
    me.pos.z = clamp(me.pos.z, -map.halfZ + r, map.halfZ - r);
    const eyeT = me.crouch ? R.crouchEye : R.eye;
    me.eye += (eyeT - me.eye) * Math.min(1, dt * 14);
    if (me.onGround && l > 0) me.bob += dt * (me.crouch ? 6 : 10);
  }

  // ── Atış ──
  const raycaster = new THREE.Raycaster();
  const tmpV = new THREE.Vector3();

  function spreadOf(w) {
    const moving = Math.hypot(me.vel.x, me.vel.z) > 1 || !me.onGround;
    if (aiming) return moving ? Math.max(w.spreadAim, w.spread * 0.6) : w.spreadAim;
    return moving ? w.spreadMove : w.spread;
  }

  function fire(now) {
    const w = W[me.w];
    if (!me.alive || matchOver || now < me.reloadUntil || now - me.lastShot[me.w] < w.interval) return;
    if (me.ammo[me.w] <= 0) { audio.empty(); reload(now); me.lastShot[me.w] = now; return; }
    me.lastShot[me.w] = now;
    me.ammo[me.w] -= 1;
    const origin = camera.getWorldPosition(new THREE.Vector3());
    const forward = camera.getWorldDirection(new THREE.Vector3());
    const spread = spreadOf(w);
    const right = new THREE.Vector3().crossVectors(forward, camera.up).normalize();
    const up = new THREE.Vector3().crossVectors(right, forward).normalize();
    const hits = new Map();
    const targets = [...mapMeshes, ...hitMeshes()];
    for (let i = 0; i < w.pellets; i++) {
      const a = Math.random() * Math.PI * 2;
      const rr = spread * Math.sqrt(Math.random());
      const dir = forward.clone().addScaledVector(right, Math.cos(a) * rr).addScaledVector(up, Math.sin(a) * rr).normalize();
      raycaster.set(origin, dir);
      raycaster.far = w.range;
      const hit = raycaster.intersectObjects(targets, false)[0];
      const end = hit ? hit.point : origin.clone().addScaledVector(dir, w.range);
      addTracer(muzzleWorld(), end, w.id, hit && hit.object.userData.seat == null);
      if (hit && hit.object.userData.seat != null) {
        const s = hit.object.userData.seat;
        const h = hits.get(s) || { s, h: false, n: 0, d: [dir.x, dir.y, dir.z] };
        h.n += 1;
        if (hit.object.userData.hit === 'head') h.h = true;
        hits.set(s, h);
      }
    }
    send({ t: 'fire', w: w.id, o: [origin.x, origin.y, origin.z], d: [forward.x, forward.y, forward.z], hits: [...hits.values()] });
    audio.shot(w.id, 0.9);
    // Geri tepme ve namlu alevi.
    me.kickPitch += w.recoil;
    me.pitch = clamp(me.pitch + w.recoil * 0.6, -1.5, 1.5);
    me.yaw += (Math.random() - 0.5) * w.recoil * 0.3;
    viewModel.userData.flash.visible = true;
    viewModel.userData.flashUntil = now + 45;
    viewModel.position.z += 0.05;
    if (me.ammo[me.w] === 0) setTimeout(() => reload(performance.now()), Math.min(250, w.interval));
    if (w.id === 1) aiming = false; // keskin nişancı her atıştan sonra dürbünden çıkar
    renderHud();
  }

  function reload(now) {
    const w = W[me.w];
    if (me.ammo[me.w] >= w.mag || now < me.reloadUntil || !me.alive) return;
    me.reloadUntil = now + w.reload;
    aiming = false;
    audio.reload();
    const which = me.w;
    setTimeout(() => {
      if (me.w === which && me.reloadUntil && performance.now() >= me.reloadUntil - 5) {
        me.ammo[which] = W[which].mag;
        me.reloadUntil = 0;
        renderHud();
      }
    }, w.reload);
    renderHud();
  }

  function muzzleWorld() {
    const p = new THREE.Vector3(0, 0.01, viewModel.userData.muzzleZ);
    return viewModel.localToWorld(p);
  }

  const tracerMat = new THREE.LineBasicMaterial({ color: 0xffe9a8, transparent: true, opacity: 0.8 });
  disposables.push(tracerMat);
  const sparkGeo = new THREE.SphereGeometry(0.06, 6, 4);
  const sparkMat = new THREE.MeshBasicMaterial({ color: 0xfff1c0 });
  disposables.push(sparkGeo, sparkMat);
  function addTracer(from, to, w, wall) {
    const geo = new THREE.BufferGeometry().setFromPoints([from, to]);
    const line = new THREE.Line(geo, tracerMat);
    scene.add(line);
    tracers.push({ obj: line, until: performance.now() + (w === 1 ? 140 : 60) });
    if (wall) {
      const s = new THREE.Mesh(sparkGeo, sparkMat);
      s.position.copy(to);
      scene.add(s);
      tracers.push({ obj: s, until: performance.now() + 90, shared: true });
    }
  }

  // ── Sunucudan gelenler ──
  function handle(msg) {
    if (!running || !msg) return;
    if (msg.t === 'snap') return applySnap(msg);
    if (msg.t === 'end') return showEnd(msg);
    if (msg.t === 'closed') return showBanner('Bağlantı koptu. Oda sahibi maçtan çıkmış olabilir.', 0, true);
  }

  function applySnap(snap) {
    lastSnap = snap;
    timeLeft = snap.left;
    if (snap.started && !started) { started = true; showBanner('Maç başladı!', 1600); }
    started = snap.started;
    for (const p of snap.players) {
      if (p.s === mySeat) {
        if (p.hp < me.hp - 0.5 && p.alive) { damageFlash = 1; audio.hurt(); }
        me.hp = p.hp;
        if (p.sp !== me.sp && p.alive) {
          // Yeniden doğdum: sahibin seçtiği yere ışınlan, şarjörler dolsun.
          me.sp = p.sp;
          me.pos.set(p.p[0], p.p[1], p.p[2]);
          me.vel.set(0, 0, 0);
          me.yaw = p.yaw;
          me.pitch = 0;
          me.ammo = W.map((w) => w.mag);
          me.reloadUntil = 0;
        }
        me.alive = p.alive;
        me.respawn = p.respawn;
        continue;
      }
      const o = others[p.s];
      if (!o) continue;
      o.on = p.on;
      o.alive = p.alive;
      o.target.set(p.p[0], p.p[1], p.p[2]);
      o.yaw = p.yaw;
      o.pitch = p.pitch;
      o.crouch = p.c;
      if (!o.seen || p.sp !== o.sp) { o.group.position.copy(o.target); o.seen = true; o.sp = p.sp; }
      o.group.visible = p.on && p.alive;
    }
    for (const e of snap.ev || []) onEvent(e);
    renderHud();
  }

  function nameOf(s) { return s === mySeat ? 'Siz' : players[s]?.name || '?'; }

  function onEvent(e) {
    const now = performance.now();
    if (e.e === 'fire' && e.s !== mySeat) {
      const o = others[e.s];
      const from = o ? o.gun.getWorldPosition(new THREE.Vector3()) : new THREE.Vector3(...e.o);
      raycaster.set(new THREE.Vector3(...e.o), new THREE.Vector3(...e.d));
      raycaster.far = W[e.w]?.range || 80;
      const hit = raycaster.intersectObjects(mapMeshes, false)[0];
      const to = hit ? hit.point : new THREE.Vector3(...e.o).addScaledVector(new THREE.Vector3(...e.d), raycaster.far);
      addTracer(from, to, e.w, !!hit);
      const dist = from.distanceTo(camera.getWorldPosition(tmpV));
      audio.shot(e.w, clamp(1 - dist / 70, 0, 1) * 0.7);
    } else if (e.e === 'hit') {
      if (e.by === mySeat && e.to !== mySeat) { hitMarkUntil = now + (e.head ? 260 : 160); $('.ar-hitmark').classList.toggle('head', !!e.head); audio.hit(e.head); }
    } else if (e.e === 'kill') {
      feed.unshift({ html: `<b style="color:${players[e.by]?.color}">${esc(nameOf(e.by))}</b> <span class="ar-w">${esc(W[e.w]?.name || '')}${e.head ? ' · kafa' : ''}</span> <b style="color:${players[e.to]?.color}">${esc(nameOf(e.to))}</b>`, until: now + 6000, me: e.by === mySeat || e.to === mySeat });
      feed.length = Math.min(feed.length, 6);
      if (e.by === mySeat && e.to !== mySeat) { audio.kill(); showBanner(`${e.head ? 'Kafadan! ' : ''}${nameOf(e.to)} vuruldu`, 1300); }
      if (e.to === mySeat) me.killedBy = e.by === mySeat ? null : e.by;
    } else if (e.e === 'join' && e.s !== mySeat) {
      feed.unshift({ html: `<span class="ar-w">${esc(nameOf(e.s))} maça girdi</span>`, until: now + 4000 });
    } else if (e.e === 'leave' && e.s !== mySeat) {
      feed.unshift({ html: `<span class="ar-w">${esc(nameOf(e.s))} maçtan çıktı</span>`, until: now + 4000 });
    }
  }

  function showBanner(text, ms, sticky = false) {
    const b = $('.ar-banner');
    b.textContent = text;
    b.hidden = false;
    clearTimeout(showBanner.t);
    if (!sticky) showBanner.t = setTimeout(() => { b.hidden = true; }, ms);
  }

  function scoreRows(stats) {
    return players.map((p, s) => ({ s, name: nameOf(s), color: p.color, k: stats?.[s]?.[0] ?? 0, d: stats?.[s]?.[1] ?? 0 }))
      .sort((a, b) => b.k - a.k || a.d - b.d);
  }

  function boardHtml(rows, title) {
    return `<h3>${esc(title)}</h3><table><tr><th>#</th><th>Oyuncu</th><th>Öldürme</th><th>Ölüm</th></tr>${rows.map((x, i) => `
      <tr class="${x.s === mySeat ? 'ar-me' : ''}"><td>${i + 1}</td><td><i style="background:${x.color}"></i>${esc(x.name)}</td><td>${x.k}</td><td>${x.d}</td></tr>`).join('')}</table>`;
  }

  function currentStats() {
    return (lastSnap?.players || []).reduce((a, p) => { a[p.s] = [p.k, p.d]; return a; }, []);
  }

  function showEnd(msg) {
    matchOver = true;
    document.exitPointerLock?.();
    const rows = scoreRows(msg.stats);
    const title = msg.winner == null ? 'Berabere!' : msg.winner === mySeat ? 'Kazandınız! 🏆' : `${nameOf(msg.winner)} kazandı`;
    const b = $('.ar-board');
    b.innerHTML = `${boardHtml(rows, title)}<div class="ar-pause-actions"><button class="ar-btn" data-act="exit">Oyunlara dön</button></div>`;
    b.hidden = false;
    b.classList.add('final');
    $('.ar-pause').hidden = true;
  }

  // ── Arayüz (HUD) ──
  function renderHud() {
    const w = W[me.w];
    const hp = Math.max(0, Math.round(me.hp));
    $('.ar-hp b').textContent = hp;
    $('.ar-hp-bar i').style.width = `${(hp / R.maxHp) * 100}%`;
    $('.ar-hp').classList.toggle('low', hp <= 35);
    const reloading = me.reloadUntil > performance.now();
    $('.ar-ammo').innerHTML = reloading ? '<b>…</b><span>doldruluyor</span>' : `<b>${me.ammo[me.w]}</b><span>/ ${w.mag}</span>`;
    $('.ar-wname').textContent = w.name;
    $('.ar-slots').innerHTML = W.map((x, i) => `<span class="${i === me.w ? 'on' : ''}">${i + 1} · ${esc(x.name)}</span>`).join('');
    $('.ar-time').textContent = started ? fmtTime(timeLeft) : 'Bekleniyor';
    $('.ar-sub').textContent = started ? `${map.name} · ${R.killLimit} öldürmede biter` : 'Maç ikinci oyuncu gelince başlar';
    const rows = scoreRows(currentStats());
    $('.ar-lead').innerHTML = rows.slice(0, 3).map((x, i) => `<div class="${x.s === mySeat ? 'ar-me' : ''}"><span>${i + 1}</span><i style="background:${x.color}"></i>${esc(x.name)}<b>${x.k}</b></div>`).join('');
    const dead = $('.ar-dead');
    if (!me.alive && me.sp >= 0 && !matchOver) {
      dead.hidden = false;
      dead.innerHTML = `<b>Vuruldunuz</b>${me.killedBy != null ? `<span>${esc(nameOf(me.killedBy))} sizi vurdu</span>` : ''}<span>${((me.respawn || 0) / 1000).toFixed(1)} sn sonra yeniden doğacaksınız</span>`;
    } else dead.hidden = true;
  }

  function renderFeed(now) {
    const live = feed.filter((f) => f.until > now);
    $('.ar-feed').innerHTML = live.map((f) => `<div class="${f.me ? 'ar-me' : ''}">${f.html}</div>`).join('');
  }

  // ── Giriş ──
  // Tuşlar pencerede, yakalama aşamasında dinlenir ve yutulur: oyun açıkken uygulamanın genel
  // kısayolları (Esc ile pencere kapatma, Ctrl+K, boşlukla video duraklatma...) çalışmasın.
  const onKeyDown = (e) => {
    if (!running) return;
    if (e.target.closest?.('input')) return;
    e.stopImmediatePropagation();
    if (e.code === 'Tab') {
      e.preventDefault();
      const b = $('.ar-board');
      if (!matchOver) { b.innerHTML = boardHtml(scoreRows(currentStats()), 'Skor'); b.hidden = false; }
      return;
    }
    if (!locked) return;
    keys.add(e.code);
    if (e.code === 'Digit1') setWeapon(0);
    if (e.code === 'Digit2') setWeapon(1);
    if (e.code === 'Digit3') setWeapon(2);
    if (e.code === 'KeyQ') setWeapon(me.prevW);
    if (e.code === 'KeyR') reload(performance.now());
    if (['Space', 'ArrowUp', 'ArrowDown', 'ControlLeft'].includes(e.code)) e.preventDefault();
  };
  const onKeyUp = (e) => {
    if (running) e.stopImmediatePropagation();
    keys.delete(e.code);
    if (e.code === 'Tab' && !matchOver) $('.ar-board').hidden = true;
  };
  const onMouseMove = (e) => {
    if (!locked) return;
    const k = 0.0022 * sens * (aiming ? 1 / Math.sqrt(W[me.w].zoom) : 1);
    me.yaw -= e.movementX * k;
    me.pitch = clamp(me.pitch - e.movementY * k, -1.5, 1.5);
  };
  const onMouseDown = (e) => {
    if (!locked) {
      if (!matchOver && e.target === canvas) lock();
      return;
    }
    if (e.button === 0) { mouseDown = true; fire(performance.now()); }
    if (e.button === 2 && performance.now() >= me.reloadUntil) aiming = true;
  };
  const onMouseUp = (e) => {
    if (e.button === 0) mouseDown = false;
    if (e.button === 2) aiming = false;
  };
  const onWheel = (e) => {
    if (!locked) return;
    setWeapon((me.w + (e.deltaY > 0 ? 1 : W.length - 1)) % W.length);
  };
  const onLockChange = () => {
    locked = document.pointerLockElement === canvas;
    $('.ar-pause').hidden = locked || matchOver;
    if (!locked) { keys.clear(); mouseDown = false; aiming = false; }
  };
  const onContext = (e) => e.preventDefault();
  function lock() {
    audio.unlock();
    // Yeni Chromium'da söz (promise) döner; kilit alınamazsa duraklatma ekranı açık kalır.
    try { canvas.requestPointerLock?.()?.catch?.(() => {}); } catch { /* desteklenmiyor */ }
  }
  // Tarayıcı önizlemesinde (fare kilidi yok) sınamak için iç durum; uygulamada kapalı.
  if (opts.debug) {
    window.__arenaDebug = {
      me, keys, others, setWeapon,
      fire: () => fire(performance.now()),
      aim: (v) => { aiming = v; },
      play: () => { locked = true; $('.ar-pause').hidden = true; },
    };
  }
  const onClick = (e) => {
    const act = e.target.closest('[data-act]')?.dataset.act;
    if (act === 'play') lock();
    if (act === 'exit') onExit();
  };
  root.addEventListener('click', onClick);
  window.addEventListener('keydown', onKeyDown, true);
  window.addEventListener('keyup', onKeyUp, true);
  document.addEventListener('mousemove', onMouseMove);
  document.addEventListener('mousedown', onMouseDown);
  document.addEventListener('mouseup', onMouseUp);
  document.addEventListener('wheel', onWheel, { passive: true });
  document.addEventListener('pointerlockchange', onLockChange);
  root.addEventListener('contextmenu', onContext);

  // ── Boyut ──
  const resize = () => {
    const w = root.clientWidth || 1;
    const h = root.clientHeight || 1;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  };
  const ro = new ResizeObserver(resize);
  ro.observe(root);
  resize();

  // ── Döngü ──
  let last = performance.now();
  let lastSend = 0;
  let raf = 0;
  function frame(now) {
    if (!running) return;
    raf = requestAnimationFrame(frame);
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    if (me.alive && !matchOver) {
      // Fizik küçük adımlarla (hızlı düşüşte kutunun içinden geçilmesin).
      const steps = Math.ceil(dt / (1 / 120));
      for (let i = 0; i < steps; i++) physics(dt / steps);
      if (mouseDown && W[me.w].auto) fire(now);
    }
    // Kamera.
    me.kickPitch *= Math.exp(-dt * 10);
    camera.position.set(me.pos.x, me.pos.y + me.eye, me.pos.z);
    camera.rotation.set(me.pitch, me.yaw, 0, 'YXZ');
    const w = W[me.w];
    const zoom = aiming ? w.zoom : 1;
    const fov = FOV / zoom;
    if (Math.abs(camera.fov - fov) > 0.05) { camera.fov += (fov - camera.fov) * Math.min(1, dt * 18); camera.updateProjectionMatrix(); }
    const scoped = aiming && w.id === 1 && camera.fov < FOV / 2;
    $('.ar-scope').hidden = !scoped;
    viewModel.visible = me.alive && !scoped;
    // Silah sallanması, nişan konumu, geri tepmeden dönüş, doldurma.
    const bobX = Math.sin(me.bob) * 0.012;
    const bobY = Math.abs(Math.cos(me.bob)) * 0.012;
    const reloading = me.reloadUntil > now;
    const target = aiming ? new THREE.Vector3(0, -0.1, -0.3) : new THREE.Vector3(0.24 + bobX, -0.22 - bobY - (reloading ? 0.1 : 0), -0.42);
    viewModel.position.lerp(target, Math.min(1, dt * 16));
    viewModel.rotation.x = reloading ? -0.5 : me.kickPitch * 3;
    if (viewModel.userData.flashUntil && now > viewModel.userData.flashUntil) viewModel.userData.flash.visible = false;
    // Nişangâh: sapmaya göre açılır.
    const gap = 6 + spreadOf(w) * 420;
    $('.ar-cross').style.setProperty('--gap', `${Math.min(60, gap)}px`);
    $('.ar-cross').hidden = scoped || (aiming && w.id !== 2);
    $('.ar-hitmark').classList.toggle('on', now < hitMarkUntil);
    damageFlash = Math.max(0, damageFlash - dt * 1.8);
    $('.ar-vignette').style.opacity = Math.max(damageFlash, me.alive ? clamp((45 - me.hp) / 60, 0, 0.5) : 0.7);
    // Diğer oyuncular: yumuşak geçiş, bakış, yürüme.
    const a = 1 - Math.exp(-dt * 16);
    for (const o of others) {
      if (!o || !o.group.visible) continue;
      const before = o.group.position.clone();
      o.group.position.lerp(o.target, a);
      const moved = before.distanceTo(o.group.position);
      o.walk += moved * 6;
      o.group.rotation.y = o.yaw;
      o.group.scale.y = o.crouch ? 0.74 : 1;
      o.legL.rotation.x = Math.sin(o.walk) * 0.6 * Math.min(1, moved * 40);
      o.legR.rotation.x = -o.legL.rotation.x;
      o.arm.rotation.x = o.pitch;
      o.gun.rotation.x = o.pitch;
    }
    // İzler.
    for (let i = tracers.length - 1; i >= 0; i--) {
      if (now > tracers[i].until) {
        scene.remove(tracers[i].obj);
        if (!tracers[i].shared) tracers[i].obj.geometry.dispose();
        tracers.splice(i, 1);
      }
    }
    renderFeed(now);
    if (now - lastSend >= 1000 / SEND_HZ && me.sp >= 0) {
      lastSend = now;
      send({ t: 'input', p: [me.pos.x, me.pos.y, me.pos.z], yaw: me.yaw, pitch: me.pitch, w: me.w, c: me.crouch, sp: me.sp });
      if (!me.alive && !matchOver) renderHud();
    }
    if (!started && !matchOver) $('.ar-time').textContent = 'Bekleniyor';
    else if (!matchOver) $('.ar-time').textContent = fmtTime(timeLeft);
    renderer.render(scene, camera);
  }
  raf = requestAnimationFrame(frame);
  renderHud();

  return {
    handle,
    lock,
    destroy() {
      running = false;
      cancelAnimationFrame(raf);
      if (document.pointerLockElement === canvas) document.exitPointerLock?.();
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mousedown', onMouseDown);
      document.removeEventListener('mouseup', onMouseUp);
      document.removeEventListener('wheel', onWheel);
      document.removeEventListener('pointerlockchange', onLockChange);
      ro.disconnect();
      root.removeEventListener('click', onClick);
      root.removeEventListener('contextmenu', onContext);
      clearTimeout(showBanner.t);
      scene.traverse((o) => {
        o.geometry?.dispose?.();
        const m = o.material;
        if (m) (Array.isArray(m) ? m : [m]).forEach((x) => { x.map?.dispose?.(); x.dispose?.(); });
      });
      disposables.forEach((d) => d.dispose?.());
      renderer.dispose();
      audio.close();
      root.innerHTML = '';
    },
  };
}
