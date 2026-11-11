//! Arena: herkesin tek başına olduğu (FFA) gerçek zamanlı ölüm maçı (FPS).
//!
//! Bu dosya ağdan ve arayüzden bağımsızdır: haritalar, silahlar, kurallar ve ev sahibinin
//! vuruşları doğrulamak için kullandığı geometri hesapları burada durur. Arayüz haritayı ve
//! silahları buradan (`info_json`) alır; aynı veriyle çizer ve hareketi hesaplar.
//!
//! Koordinatlar metre; y yukarı. Haritalar eksen hizalı kutulardan (`Block`) oluşur, zemin y = 0.
//!
//! Oda hamle listesi yalnızca oyundan çekilmeleri (`q<koltuk>`) taşır; maçın kendisi gerçek
//! zamanlı kanaldan oynanır (`node/arena.rs`), sonuç odaya ev sahibi tarafından yazılır.

use super::common::parse_quit;
use super::{Eval, Outcome};
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Maç süresi ve bitiren öldürme sayısı.
#[cfg(not(test))]
pub const MATCH_SECS: u64 = 300;
#[cfg(test)]
pub const MATCH_SECS: u64 = 30;
#[cfg(not(test))]
pub const KILL_LIMIT: u32 = 20;
#[cfg(test)]
pub const KILL_LIMIT: u32 = 2;
/// Ölen oyuncu bu kadar sonra yeniden doğar.
#[cfg(not(test))]
pub const RESPAWN_MS: u64 = 3000;
#[cfg(test)]
pub const RESPAWN_MS: u64 = 150;
pub const MAX_HP: f32 = 100.0;
/// Hasar almadan bu kadar geçince can saniyede `REGEN_PER_SEC` dolar.
pub const REGEN_DELAY_MS: u64 = 4000;
pub const REGEN_PER_SEC: f32 = 20.0;

/// Oyuncu ölçüleri (arayüzdeki fizik de aynılarını kullanır).
pub const PLAYER_RADIUS: f32 = 0.3;
pub const PLAYER_HEIGHT: f32 = 1.8;
pub const CROUCH_HEIGHT: f32 = 1.3;
pub const EYE: f32 = 1.6;
pub const CROUCH_EYE: f32 = 1.15;
/// Kafa bölgesi: boyun yüksekliği (ayaktan, ayakta).
pub const NECK: f32 = 1.42;
/// Vuruş doğrulamasında hedefin etrafındaki pay: gövde yarıçapı + gecikme payı.
pub const HIT_TOLERANCE: f32 = 1.05;
/// En büyük yatay hız (koşu + zıplama); bundan hızlı yer değiştirme reddedilir.
pub const MAX_SPEED: f32 = 11.0;

#[derive(Debug, Clone, Copy)]
pub struct Weapon {
    pub id: u8,
    pub name: &'static str,
    pub damage: f32,
    pub head_mult: f32,
    /// İki atış arası en kısa süre.
    pub interval_ms: u64,
    pub range: f32,
    /// Saçma sayısı (pompalı).
    pub pellets: u8,
    pub mag: u8,
    pub reload_ms: u64,
    /// Sapma (radyan): durağan, hareketli, nişan alırken.
    pub spread: f32,
    pub spread_move: f32,
    pub spread_aim: f32,
    /// Hasar bu mesafeden sonra azalır, menzilde `falloff_min` katına iner.
    pub falloff_start: f32,
    pub falloff_min: f32,
    pub auto: bool,
    /// Nişan alınca yakınlaştırma (sağ tık).
    pub zoom: f32,
    /// Geri tepme (radyan, yukarı).
    pub recoil: f32,
}

pub const WEAPONS: [Weapon; 3] = [
    Weapon {
        id: 0,
        name: "Tüfek",
        damage: 18.0,
        head_mult: 1.6,
        interval_ms: 100,
        range: 90.0,
        pellets: 1,
        mag: 30,
        reload_ms: 1700,
        spread: 0.012,
        spread_move: 0.035,
        spread_aim: 0.004,
        falloff_start: 40.0,
        falloff_min: 0.7,
        auto: true,
        zoom: 1.35,
        recoil: 0.012,
    },
    Weapon {
        id: 1,
        name: "Keskin nişancı",
        damage: 85.0,
        head_mult: 1.8,
        interval_ms: 1100,
        range: 200.0,
        pellets: 1,
        mag: 5,
        reload_ms: 2200,
        spread: 0.07,
        spread_move: 0.12,
        spread_aim: 0.0,
        falloff_start: 200.0,
        falloff_min: 1.0,
        auto: false,
        zoom: 3.5,
        recoil: 0.06,
    },
    Weapon {
        id: 2,
        name: "Pompalı",
        damage: 13.0,
        head_mult: 1.5,
        interval_ms: 750,
        range: 32.0,
        pellets: 8,
        mag: 6,
        reload_ms: 2400,
        spread: 0.07,
        spread_move: 0.085,
        spread_aim: 0.055,
        falloff_start: 9.0,
        falloff_min: 0.3,
        auto: false,
        zoom: 1.2,
        recoil: 0.05,
    },
];

pub fn weapon(id: u8) -> Option<&'static Weapon> {
    WEAPONS.get(id as usize)
}

/// Mesafeye göre hasar katsayısı.
pub fn falloff(w: &Weapon, dist: f32) -> f32 {
    if dist <= w.falloff_start || w.range <= w.falloff_start {
        return 1.0;
    }
    let t = ((dist - w.falloff_start) / (w.range - w.falloff_start)).clamp(0.0, 1.0);
    1.0 - t * (1.0 - w.falloff_min)
}

/// Eksen hizalı kutu.
#[derive(Debug, Clone, Copy)]
pub struct Block {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub color: u32,
}

/// `x`, `z`: taban merkezi; `y`: taban yüksekliği; `w`, `h`, `d`: x, y, z boyutları.
fn bx(x: f32, y: f32, z: f32, w: f32, h: f32, d: f32, color: u32) -> Block {
    Block { min: [x - w / 2.0, y, z - d / 2.0], max: [x + w / 2.0, y + h, z + d / 2.0], color }
}

pub struct MapDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Oynanabilir alan: x ∈ [-half_x, half_x], z ∈ [-half_z, half_z].
    pub half_x: f32,
    pub half_z: f32,
    pub wall_height: f32,
    pub floor: u32,
    pub sky: u32,
    pub fog: u32,
    pub blocks: Vec<Block>,
    pub spawns: Vec<[f32; 3]>,
}

/// Merdiven: `n` basamak, her biri 0.4 m yükselir, 0.6 m derin. `dir`: çıkış yönü (+x, -x, +z, -z).
fn stairs(out: &mut Vec<Block>, x: f32, z: f32, top: f32, width: f32, dir: (f32, f32), color: u32) {
    let n = (top / 0.4).round() as i32;
    for k in 0..n {
        // k = 0 en üst basamak (hedefe en yakın).
        let h = top - 0.4 * k as f32;
        let off = 0.3 + 0.6 * k as f32;
        let (cx, cz) = (x - dir.0 * off, z - dir.1 * off);
        let (w, d) = if dir.0 != 0.0 { (0.6, width) } else { (width, 0.6) };
        out.push(bx(cx, 0.0, cz, w, h, d, color));
    }
}

fn walls(out: &mut Vec<Block>, hx: f32, hz: f32, h: f32, color: u32) {
    out.push(bx(0.0, 0.0, -hz - 0.5, hx * 2.0 + 2.0, h, 1.0, color));
    out.push(bx(0.0, 0.0, hz + 0.5, hx * 2.0 + 2.0, h, 1.0, color));
    out.push(bx(-hx - 0.5, 0.0, 0.0, 1.0, h, hz * 2.0 + 2.0, color));
    out.push(bx(hx + 0.5, 0.0, 0.0, 1.0, h, hz * 2.0 + 2.0, color));
}

fn depo() -> MapDef {
    const WALL: u32 = 0x6b7280;
    const CRATE: u32 = 0xb7894f;
    const CRATE2: u32 = 0x8f6a3d;
    const STEEL: u32 = 0x4b5563;
    const STEP: u32 = 0x9ca3af;
    const CONT: u32 = 0x2f7d6d;
    let (hx, hz) = (24.0, 24.0);
    let mut b = Vec::new();
    walls(&mut b, hx, hz, 8.0, WALL);
    // Orta platform (2.4 m) ve iki yandan merdiven, üstünde iki sandık.
    b.push(bx(0.0, 0.0, 0.0, 10.0, 2.4, 10.0, STEEL));
    stairs(&mut b, 0.0, -5.0, 2.4, 3.0, (0.0, 1.0), STEP);
    stairs(&mut b, 0.0, 5.0, 2.4, 3.0, (0.0, -1.0), STEP);
    b.push(bx(-3.0, 2.4, 0.0, 1.4, 1.1, 1.4, CRATE));
    b.push(bx(3.0, 2.4, 0.0, 1.4, 1.1, 1.4, CRATE));
    // Dört kolon.
    for (x, z) in [(-9.0, -9.0), (9.0, -9.0), (-9.0, 9.0), (9.0, 9.0)] {
        b.push(bx(x, 0.0, z, 1.2, 8.0, 1.2, STEEL));
    }
    // Batı duvarı boyunca yürüyüş yolu (3.2–3.6 m), güneyden merdiven, korkuluk.
    b.push(bx(-21.5, 3.2, -2.0, 5.0, 0.4, 26.0, STEEL));
    stairs(&mut b, -21.5, 11.0, 3.6, 3.0, (0.0, -1.0), STEP);
    b.push(bx(-19.1, 3.6, -4.0, 0.2, 0.9, 22.0, STEEL));
    // Doğuda konteyner (2.5 m) ve ona çıkan sandıklar.
    b.push(bx(19.5, 0.0, -12.0, 5.0, 2.5, 9.0, CONT));
    b.push(bx(15.9, 0.0, -9.0, 1.6, 1.3, 1.6, CRATE));
    b.push(bx(19.5, 0.0, 9.0, 5.0, 2.5, 7.0, 0x9a3b3b));
    b.push(bx(19.5, 0.0, 14.9, 1.6, 1.3, 1.6, CRATE2));
    // Sandıklar ve alçak siperler.
    for (x, z, w, h, d, c) in [
        (-12.0, -12.0, 2.0, 2.0, 2.0, CRATE),
        (-12.0, -12.0, 1.4, 1.2, 1.4, CRATE2),
        (-8.5, -15.0, 2.0, 1.4, 2.0, CRATE2),
        (12.0, 12.0, 2.0, 2.0, 2.0, CRATE),
        (13.5, 3.0, 2.4, 1.2, 2.4, CRATE2),
        (-14.0, 10.0, 3.0, 1.5, 1.5, CRATE),
        (9.0, -16.0, 1.5, 3.0, 1.5, CRATE),
        (0.0, -16.0, 6.0, 1.2, 1.0, STEEL),
        (0.0, 16.0, 6.0, 1.2, 1.0, STEEL),
        (-15.0, 0.0, 1.0, 1.2, 6.0, STEEL),
        (14.0, -3.0, 1.0, 1.2, 4.0, STEEL),
        (-5.0, 12.0, 2.0, 2.0, 2.0, CRATE),
        (6.0, 19.0, 2.0, 1.3, 2.0, CRATE2),
        (-4.0, -20.0, 2.0, 2.0, 2.0, CRATE),
    ] {
        // İkinci kat sandıkları: aynı noktada ikinci kutu üstte durur.
        let y = if (x, z) == (-12.0, -12.0) && w < 2.0 { 2.0 } else { 0.0 };
        b.push(bx(x, y, z, w, h, d, c));
    }
    let spawns = vec![
        [-20.0, 0.0, -20.0],
        [20.0, 0.0, 20.0],
        [-12.0, 0.0, 20.0],
        [20.0, 0.0, -21.0],
        [-21.0, 0.0, -8.0],
        [21.0, 0.0, 1.0],
        [0.0, 0.0, -21.0],
        [2.0, 0.0, 21.0],
        [-6.0, 0.0, 0.0],
        [6.0, 0.0, 0.0],
    ];
    MapDef {
        id: "depo",
        name: "Depo",
        description: "Kapalı depo: ortada merdivenli platform, sandıklar, konteynerler ve duvar boyunca yüksek yürüyüş yolu. Yakın ve orta mesafe.",
        half_x: hx,
        half_z: hz,
        wall_height: 8.0,
        floor: 0x3a3f47,
        sky: 0x1f2430,
        fog: 0x1f2430,
        blocks: b,
        spawns,
    }
}

fn avlu() -> MapDef {
    const WALL: u32 = 0xd6c7a1;
    const STONE: u32 = 0xb9ab8a;
    const TOWER: u32 = 0xa0826d;
    const WOOD: u32 = 0x8b5a2b;
    const HEDGE: u32 = 0x3f7d3a;
    const LEAF: u32 = 0x4e9a46;
    const STEP: u32 = 0xc2b49a;
    let (hx, hz) = (30.0, 22.0);
    let mut b = Vec::new();
    walls(&mut b, hx, hz, 6.0, WALL);
    // İki kule: 4 köşe direği, 4.0–4.4 m döşeme, üstte siper duvarları; merdivenler dışarıda.
    for (tx, sdir) in [(-20.0f32, 1.0f32), (20.0, -1.0)] {
        for (dx, dz) in [(-3.6, -3.6), (3.6, -3.6), (-3.6, 3.6), (3.6, 3.6)] {
            b.push(bx(tx + dx, 0.0, dz, 0.8, 4.0, 0.8, TOWER));
        }
        b.push(bx(tx, 4.0, 0.0, 8.0, 0.4, 8.0, TOWER));
        // Siperler (köprü tarafı açık).
        b.push(bx(tx, 4.4, -3.8, 8.0, 1.1, 0.4, STONE));
        b.push(bx(tx, 4.4, 3.8, 8.0, 1.1, 0.4, STONE));
        b.push(bx(tx - 3.8 * sdir, 4.4, 0.0, 0.4, 1.1, 8.0, STONE));
        // Merdiven: batı kulede güneyden (+z), doğuda kuzeyden (-z) çıkılır.
        stairs(&mut b, tx + 2.0 * sdir, 4.0 * sdir, 4.4, 2.6, (0.0, -sdir), STEP);
    }
    // Kuleler arası köprü ve korkulukları.
    b.push(bx(0.0, 4.0, 0.0, 32.0, 0.4, 3.0, WOOD));
    b.push(bx(0.0, 4.4, -1.4, 32.0, 0.6, 0.2, WOOD));
    b.push(bx(0.0, 4.4, 1.4, 32.0, 0.6, 0.2, WOOD));
    // Köprünün altında çeşme: alçak halka ve ortada heykel.
    b.push(bx(0.0, 0.0, -3.5, 6.0, 0.9, 0.6, STONE));
    b.push(bx(0.0, 0.0, 3.5, 6.0, 0.9, 0.6, STONE));
    b.push(bx(-3.5, 0.0, 0.0, 0.6, 0.9, 6.0, STONE));
    b.push(bx(3.5, 0.0, 0.0, 0.6, 0.9, 6.0, STONE));
    b.push(bx(0.0, 0.0, 0.0, 1.0, 2.6, 1.0, 0x8c8c8c));
    // Çitler, pazar tezgâhları, alçak duvarlar.
    for (x, z, w, h, d, c) in [
        (-10.0, -12.0, 6.0, 1.3, 1.0, HEDGE),
        (10.0, 12.0, 6.0, 1.3, 1.0, HEDGE),
        (10.0, -12.0, 6.0, 1.3, 1.0, HEDGE),
        (-10.0, 12.0, 6.0, 1.3, 1.0, HEDGE),
        (0.0, -15.0, 5.0, 2.2, 2.5, WOOD),
        (0.0, 15.0, 5.0, 2.2, 2.5, WOOD),
        (-8.0, 0.0, 1.0, 1.2, 4.0, STONE),
        (8.0, 0.0, 1.0, 1.2, 4.0, STONE),
        (-27.0, -18.0, 3.0, 1.5, 1.5, WOOD),
        (27.0, 18.0, 3.0, 1.5, 1.5, WOOD),
        (16.0, 16.0, 2.0, 2.0, 2.0, WOOD),
        (-16.0, -16.0, 2.0, 2.0, 2.0, WOOD),
    ] {
        b.push(bx(x, 0.0, z, w, h, d, c));
    }
    // Ağaçlar: gövde ve taç (taç da mermiyi tutar).
    for (x, z) in [(-25.0, 17.0), (25.0, -17.0), (-6.0, -19.0), (6.0, 19.0)] {
        b.push(bx(x, 0.0, z, 0.6, 3.0, 0.6, 0x6b4423));
        b.push(bx(x, 3.0, z, 3.0, 2.4, 3.0, LEAF));
    }
    let spawns = vec![
        [-27.0, 0.0, 0.0],
        [27.0, 0.0, 0.0],
        [-14.0, 0.0, -19.0],
        [14.0, 0.0, 19.0],
        [14.0, 0.0, -19.0],
        [-14.0, 0.0, 19.0],
        [0.0, 0.0, -8.0],
        [0.0, 0.0, 8.0],
        [-26.0, 0.0, -10.0],
        [26.0, 0.0, 10.0],
    ];
    MapDef {
        id: "avlu",
        name: "Avlu",
        description: "Güneşli açık avlu: köprüyle bağlı iki kule, çeşme, çitler ve tezgâhlar. Uzun görüş hatları; keskin nişancıya uygun.",
        half_x: hx,
        half_z: hz,
        wall_height: 6.0,
        floor: 0xcfbf98,
        sky: 0x9fd3f0,
        fog: 0xbfe0f2,
        blocks: b,
        spawns,
    }
}

pub fn maps() -> &'static [MapDef] {
    static MAPS: OnceLock<Vec<MapDef>> = OnceLock::new();
    MAPS.get_or_init(|| vec![depo(), avlu()])
}

pub fn map(id: &str) -> Option<&'static MapDef> {
    maps().iter().find(|m| m.id == id)
}

// ───────────── Geometri ─────────────

pub type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub fn finite(a: V3) -> bool {
    a.iter().all(|v| v.is_finite())
}
pub fn normalize(a: V3) -> Option<V3> {
    let l = len(a);
    (l.is_finite() && l > 1e-6).then(|| mul(a, 1.0 / l))
}

/// Işın `o + d·t` (`d` birim) kutuya `t ∈ [0, max_t]` aralığında giriyorsa giriş `t`'si.
fn ray_box(o: V3, d: V3, b: &Block, max_t: f32) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, max_t);
    for i in 0..3 {
        if d[i].abs() < 1e-8 {
            if o[i] < b.min[i] || o[i] > b.max[i] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[i];
        let (mut a, mut c) = ((b.min[i] - o[i]) * inv, (b.max[i] - o[i]) * inv);
        if a > c {
            std::mem::swap(&mut a, &mut c);
        }
        t0 = t0.max(a);
        t1 = t1.min(c);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// `from`'dan `to`'ya görüş hattını bir kutu (ya da zemin) kesiyor mu?
pub fn blocked(m: &MapDef, from: V3, to: V3) -> bool {
    let Some(d) = normalize(sub(to, from)) else { return false };
    let dist = len(sub(to, from));
    if from[1] < 0.0 || to[1] < 0.0 {
        return true;
    }
    m.blocks.iter().any(|b| ray_box(from, d, b, dist).is_some_and(|t| t > 0.01 && t < dist - 0.01))
}

/// Işınla bir doğru parçası arasındaki en kısa uzaklık: (uzaklık, ışın üzerindeki t, parça
/// üzerindeki s ∈ [0, 1]).
pub fn ray_segment(o: V3, d: V3, a: V3, b: V3) -> (f32, f32, f32) {
    let u = sub(b, a);
    let w = sub(o, a);
    let (aa, bb, cc, dd, ee) = (dot(d, d), dot(d, u), dot(u, u), dot(d, w), dot(u, w));
    let den = aa * cc - bb * bb;
    let mut s = if den.abs() < 1e-8 { 0.0 } else { ((aa * ee - bb * dd) / den).clamp(0.0, 1.0) };
    let mut t = (bb * s - dd) / aa;
    if t < 0.0 {
        t = 0.0;
        s = if cc > 1e-8 { (dot(u, sub(o, a)) / cc).clamp(0.0, 1.0) } else { 0.0 };
    }
    let p = add(o, mul(d, t));
    let q = add(a, mul(u, s));
    (len(sub(p, q)), t, s)
}

/// Vuruş doğrulaması (ev sahibinde): `o`'dan `d` yönündeki atış, ayakları `feet`'te olan hedefe
/// gerçekten değebilir mi? Değebiliyorsa (mesafe, kafa bölgesine yakın mı).
pub fn check_hit(m: &MapDef, w: &Weapon, o: V3, d: V3, feet: V3, crouch: bool) -> Option<(f32, bool)> {
    let top = if crouch { CROUCH_HEIGHT } else { PLAYER_HEIGHT };
    let a = add(feet, [0.0, 0.15, 0.0]);
    let b = add(feet, [0.0, top - 0.1, 0.0]);
    let (dist, t, s) = ray_segment(o, d, a, b);
    if dist > HIT_TOLERANCE || t > w.range + HIT_TOLERANCE {
        return None;
    }
    // Hedefin önündeki noktaya kadar görüş açık olmalı.
    let hit = add(o, mul(d, (t - 0.5).max(0.0)));
    if blocked(m, o, hit) {
        return None;
    }
    let neck = if crouch { NECK * CROUCH_HEIGHT / PLAYER_HEIGHT } else { NECK };
    let h = 0.15 + s * (top - 0.25);
    Some((t, h >= neck - 0.3))
}

/// Yeniden doğma noktası: diğer oyunculara en uzak olan (eşitlikte `pick` belirler).
pub fn pick_spawn(m: &MapDef, others: &[V3], pick: usize) -> V3 {
    if others.is_empty() {
        return m.spawns[pick % m.spawns.len()];
    }
    let score = |s: &V3| others.iter().map(|o| len(sub(*s, *o))).fold(f32::MAX, f32::min);
    let mut best: Vec<(f32, V3)> = m.spawns.iter().map(|s| (score(s), *s)).collect();
    best.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    // En iyi iki noktadan biri: hep aynı yere doğulmasın.
    best[pick % best.len().min(2)].1
}

/// Konumu harita sınırlarına sıkıştırır.
pub fn clamp_to_map(m: &MapDef, p: V3) -> V3 {
    let r = PLAYER_RADIUS;
    [p[0].clamp(-m.half_x + r, m.half_x - r), p[1].clamp(0.0, 30.0), p[2].clamp(-m.half_z + r, m.half_z - r)]
}

/// Arayüze giden statik veri: haritalar, silahlar ve kurallar.
pub fn info_json() -> Value {
    json!({
        "maps": maps().iter().map(|m| json!({
            "id": m.id,
            "name": m.name,
            "description": m.description,
            "halfX": m.half_x,
            "halfZ": m.half_z,
            "wallHeight": m.wall_height,
            "floor": m.floor,
            "sky": m.sky,
            "fog": m.fog,
            "blocks": m.blocks.iter().map(|b| json!([b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2], b.color])).collect::<Vec<_>>(),
            "spawns": m.spawns,
        })).collect::<Vec<_>>(),
        "weapons": WEAPONS.iter().map(|w| json!({
            "id": w.id, "name": w.name, "damage": w.damage, "headMult": w.head_mult, "interval": w.interval_ms,
            "range": w.range, "pellets": w.pellets, "mag": w.mag, "reload": w.reload_ms, "spread": w.spread,
            "spreadMove": w.spread_move, "spreadAim": w.spread_aim, "auto": w.auto, "zoom": w.zoom, "recoil": w.recoil,
        })).collect::<Vec<_>>(),
        "rules": {
            "matchSecs": MATCH_SECS, "killLimit": KILL_LIMIT, "respawnMs": RESPAWN_MS, "maxHp": MAX_HP,
            "playerRadius": PLAYER_RADIUS, "playerHeight": PLAYER_HEIGHT, "crouchHeight": CROUCH_HEIGHT,
            "eye": EYE, "crouchEye": CROUCH_EYE, "neck": NECK,
        },
    })
}

/// Oda durumu: hamleler yalnızca oyundan çekilmelerdir. Biri dışında herkes çekilirse kalan kazanır.
pub fn evaluate(players: usize, moves: &[String]) -> Result<Eval, String> {
    if !(2..=8).contains(&players) {
        return Err("geçersiz oyuncu sayısı".into());
    }
    let mut quit = vec![false; players];
    let mut outcome = None;
    for mv in moves {
        if outcome.is_some() {
            return Err("oyun bitmişken hamle".into());
        }
        let seat = parse_quit(mv).ok_or_else(|| format!("geçersiz hamle: {mv}"))?;
        if seat >= players || quit[seat] {
            return Err("geçersiz çekilme".into());
        }
        quit[seat] = true;
        let left: Vec<usize> = (0..players).filter(|&s| !quit[s]).collect();
        if left.len() <= 1 {
            outcome = Some(Outcome { winner: left.first().copied(), reason: "forfeit" });
        }
    }
    Ok(Eval { turn: None, movers: Vec::new(), legal: Vec::new(), outcome, info: json!({ "quit": quit }) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(b: &Block, p: V3) -> bool {
        (0..3).all(|i| p[i] > b.min[i] && p[i] < b.max[i])
    }

    #[test]
    fn maps_are_sane() {
        assert_eq!(maps().len(), 2);
        for m in maps() {
            assert!(m.spawns.len() >= 8, "{}", m.id);
            for s in &m.spawns {
                // Oyuncunun gövdesi hiçbir kutunun içinde olmamalı ve sahada olmalı.
                for h in [0.1, 0.9, 1.7] {
                    for (dx, dz) in [(0.0, 0.0), (PLAYER_RADIUS, 0.0), (-PLAYER_RADIUS, 0.0), (0.0, PLAYER_RADIUS), (0.0, -PLAYER_RADIUS)] {
                        let p = [s[0] + dx, h, s[2] + dz];
                        assert!(!m.blocks.iter().any(|b| inside(b, p)), "{}: doğma noktası kutuda: {s:?}", m.id);
                    }
                }
                assert!(s[0].abs() < m.half_x && s[2].abs() < m.half_z, "{}: {s:?}", m.id);
            }
            for b in &m.blocks {
                assert!((0..3).all(|i| b.min[i] < b.max[i]), "{}: boş kutu", m.id);
            }
        }
        assert!(map("depo").is_some() && map("avlu").is_some() && map("yok").is_none());
    }

    #[test]
    fn geometry() {
        let m = map("depo").unwrap();
        // Orta platform (y ≤ 2.4) iki uç arasındaki görüşü keser; üstünden geçen hat açık.
        assert!(blocked(m, [-8.0, 1.6, 0.0], [8.0, 1.6, 0.0]));
        assert!(!blocked(m, [-8.0, 6.5, 0.0], [8.0, 6.5, 0.0]));
        // Işın-parça uzaklığı.
        let (d, t, s) = ray_segment([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [5.0, 0.0, 0.5], [5.0, 2.0, 0.5]);
        assert!((d - 0.5).abs() < 1e-4 && (t - 5.0).abs() < 1e-4 && (s - 0.5).abs() < 1e-4);
        assert!((falloff(&WEAPONS[2], 5.0) - 1.0).abs() < 1e-6);
        assert!((falloff(&WEAPONS[2], 32.0) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn hit_validation() {
        let m = map("depo").unwrap();
        let rifle = &WEAPONS[0];
        let feet = [-18.0, 0.0, 20.0];
        let o = [-18.0, 1.6, 12.0];
        // Gövdeye doğru.
        let d = normalize(sub([-18.0, 1.0, 20.0], o)).unwrap();
        let (dist, head) = check_hit(m, rifle, o, d, feet, false).unwrap();
        assert!((dist - 8.0).abs() < 0.2 && !head);
        // Kafaya doğru.
        let d = normalize(sub([-18.0, 1.65, 20.0], o)).unwrap();
        assert!(check_hit(m, rifle, o, d, feet, false).unwrap().1);
        // Tamamen ıska.
        let d = normalize(sub([-14.0, 1.0, 20.0], o)).unwrap();
        assert!(check_hit(m, rifle, o, d, feet, false).is_none());
        // Menzil dışı (pompalı 32 m).
        let far_o = [-18.0, 1.6, -22.0];
        let d = normalize(sub([-18.0, 1.0, 20.0], far_o)).unwrap();
        assert!(check_hit(m, &WEAPONS[2], far_o, d, feet, false).is_none());
        // Duvarın arkası: platformun öbür yanındaki hedef görünmez.
        let o2 = [-8.0, 1.6, 0.0];
        let feet2 = [8.0, 0.0, 0.0];
        let d = normalize(sub([8.0, 1.0, 0.0], o2)).unwrap();
        assert!(check_hit(m, rifle, o2, d, feet2, false).is_none());
    }

    #[test]
    fn spawn_choice_avoids_players() {
        let m = map("avlu").unwrap();
        let s = pick_spawn(m, &[[-27.0, 0.0, 0.0]], 0);
        assert!(len(sub(s, [-27.0, 0.0, 0.0])) > 30.0);
    }

    #[test]
    fn room_rules() {
        assert!(evaluate(1, &[]).is_err());
        assert!(evaluate(9, &[]).is_err());
        let e = evaluate(3, &[]).unwrap();
        assert!(e.movers.is_empty() && e.outcome.is_none());
        let e = evaluate(3, &["q0".into(), "q2".into()]).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: Some(1), reason: "forfeit" }));
        assert!(evaluate(3, &["a0p1".into()]).is_err());
    }
}
