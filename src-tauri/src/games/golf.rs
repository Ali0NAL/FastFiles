//! Mini golf: 2–6 oyuncu, 25 delikten rastgele seçilen 6 delik, sırayla vuruş, power-up'lar.
//!
//! Hamleler:
//! - `a<açı>p<güç>`: vuruş. Açı derecenin onda biri (0–3599; 0 = sağa, 900 = aşağı, ekran
//!   koordinatları), güç yüzde (1–100). Ör. `a1800p65` = sola, %65 güçle.
//! - `q<koltuk>`: oyuncu oyundan çekildi (sıradan bağımsız; yalnızca oda sahibi ekler).
//!
//! Fizik sabit adımlı (1/240 sn) ve yalnızca bu dosyadadır: oda sahibi de diğer oyuncular da
//! aynı vuruşu aynı kodla oynatır, topun yolu arayüze buradan gider (`lastShot.path`).
//!
//! Delik seçimi: oyunun delikleri ve power-up türleri oda kimliğinden (`seed`) türetilir. Herkes
//! aynı oda kimliğini bildiği için herkes aynı sahayı görür; yine de her oyun farklıdır.
//!
//! Power-up'lar: her delikte sahaya serpilmiş birkaç power-up vardır. Topu üstünden geçen oyuncu
//! onu alır ve o delikte power-up kaybolur. Etkiler sonraki vuruşta işler:
//! - Rakiplere: `slow` (vuruşu %40 zayıf), `shaky` (vuruş birkaç derece sapar), `ice` (top kayar).
//! - Kendine: `turbo` (daha sert, kumdan etkilenmez), `ghost` (iç duvar ve direklerin içinden
//!   geçer), `magnet` (delik topu çeker). Kişide tek bir kendine-etki tutulur; yenisi eskisini siler.
//!
//! Sıra: her delikte sırayla birer vuruş; deliği bitiren (topu sokan ya da `MAX_STROKES`'a
//! ulaşan) atlanır. Deliğe ilk vuran her delikte bir sonraki oyuncuya kayar. Suya düşen top
//! vuruşun başladığı yere döner ve bir ceza vuruşu yazılır. Bütün delikler bitince en az toplam
//! vuruş kazanır; eşitlikte berabere. Bir kişi dışında herkes çekilirse kalan kazanır.

use super::{Eval, Outcome};
use serde_json::{json, Value};

pub const MAX_STROKES: u32 = 8;
/// Bir oyunda oynanan delik sayısı (`HOLES` içinden rastgele seçilir).
pub const HOLES_PER_GAME: usize = 6;
const WIDTH: f64 = 800.0;
const HEIGHT: f64 = 500.0;
const BALL_R: f64 = 7.0;
const CUP_R: f64 = 11.0;
/// Power-up yarıçapı (top merkezi `PUP_R + BALL_R`'den yakına gelince alınır).
const PUP_R: f64 = 13.0;
/// %100 güçte topun ilk hızı (birim/sn). Açık çimde ~1090 birim yol alır.
const VMAX: f64 = 1150.0;
const DT: f64 = 1.0 / 240.0;
const MAX_STEPS: usize = 240 * 25;
/// Yavaşlama: sabit (sürtünme) + hızla orantılı (sürüklenme).
const GRASS: (f64, f64) = (230.0, 0.55);
const SAND: (f64, f64) = (900.0, 2.2);
const STOP_SPEED: f64 = 5.0;
/// Bundan hızlı top deliğin üstünden geçer.
const CUP_SPEED: f64 = 420.0;
/// Duvardan sekme katsayısı.
const BOUNCE: f64 = 0.78;
/// Arayüze giden yolda kaç adımda bir nokta alınır (~40 nokta/sn).
const SAMPLE_EVERY: usize = 6;

/// Power-up etkilerinin gücü.
const SLOW_SPEED: f64 = 0.6;
const TURBO_SPEED: f64 = 1.35;
const ICE_FRICTION: f64 = 0.4;
const SHAKE_MIN: f64 = 4.0;
const SHAKE_MAX: f64 = 10.0;
const MAGNET_R: f64 = 120.0;
const MAGNET_ACC: f64 = 950.0;

type P = (f64, f64);
/// (x, y, genişlik, yükseklik)
type Rect = (f64, f64, f64, f64);

struct Hole {
    name: &'static str,
    par: u32,
    /// Sahanın sınırı (çokgen; kenarları duvardır).
    bounds: &'static [P],
    /// İçerideki duvar blokları (çokgen).
    blocks: &'static [&'static [P]],
    /// Yuvarlak direkler (x, y, yarıçap).
    posts: &'static [(f64, f64, f64)],
    sand: &'static [Rect],
    water: &'static [Rect],
    tee: P,
    cup: P,
    /// Power-up yerleri (türü oyun başında rastgele seçilir).
    pups: &'static [P],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pup {
    Slow,
    Shaky,
    Ice,
    Turbo,
    Ghost,
    Magnet,
}

const PUP_KINDS: [Pup; 6] = [Pup::Slow, Pup::Shaky, Pup::Ice, Pup::Turbo, Pup::Ghost, Pup::Magnet];

impl Pup {
    fn id(self) -> &'static str {
        match self {
            Pup::Slow => "slow",
            Pup::Shaky => "shaky",
            Pup::Ice => "ice",
            Pup::Turbo => "turbo",
            Pup::Ghost => "ghost",
            Pup::Magnet => "magnet",
        }
    }
    /// Rakiplere mi etki eder?
    fn hits_others(self) -> bool {
        matches!(self, Pup::Slow | Pup::Shaky | Pup::Ice)
    }
}

const fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> [P; 4] {
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

const FULL: [P; 4] = rect(60.0, 60.0, 740.0, 440.0);

const H1: [P; 4] = rect(80.0, 150.0, 720.0, 350.0);
const H2: [P; 4] = rect(60.0, 80.0, 740.0, 420.0);
const H2_WALL: [P; 4] = rect(385.0, 80.0, 415.0, 320.0);
const H3: [P; 6] = [(60.0, 60.0), (360.0, 60.0), (360.0, 290.0), (740.0, 290.0), (740.0, 440.0), (60.0, 440.0)];
const H4: [P; 4] = rect(60.0, 90.0, 740.0, 410.0);
const H5: [P; 4] = rect(60.0, 70.0, 740.0, 430.0);
const H6_A: [P; 4] = rect(230.0, 60.0, 260.0, 330.0);
const H6_B: [P; 4] = rect(480.0, 170.0, 510.0, 440.0);
const H8: [P; 4] = rect(40.0, 200.0, 760.0, 300.0);
const H9_WALL: [P; 4] = rect(60.0, 225.0, 600.0, 275.0);
const H10: [P; 12] = [
    (60.0, 60.0),
    (300.0, 60.0),
    (300.0, 170.0),
    (520.0, 170.0),
    (520.0, 280.0),
    (740.0, 280.0),
    (740.0, 440.0),
    (440.0, 440.0),
    (440.0, 330.0),
    (220.0, 330.0),
    (220.0, 220.0),
    (60.0, 220.0),
];
const H11: [P; 4] = rect(60.0, 80.0, 740.0, 420.0);
const H12: [P; 4] = [(400.0, 40.0), (760.0, 250.0), (400.0, 460.0), (40.0, 250.0)];
const H12_GEM: [P; 4] = [(400.0, 190.0), (460.0, 250.0), (400.0, 310.0), (340.0, 250.0)];
const H14_A: [P; 4] = rect(230.0, 60.0, 260.0, 320.0);
const H14_B: [P; 4] = rect(400.0, 180.0, 430.0, 440.0);
const H14_C: [P; 4] = rect(570.0, 60.0, 600.0, 320.0);
const H15_A: [P; 4] = rect(560.0, 130.0, 590.0, 300.0);
const H15_B: [P; 4] = rect(590.0, 270.0, 740.0, 300.0);
const H16_L: [P; 4] = rect(460.0, 150.0, 485.0, 350.0);
const H16_T: [P; 4] = rect(485.0, 150.0, 640.0, 175.0);
const H16_B: [P; 4] = rect(485.0, 325.0, 640.0, 350.0);
const H18: [P; 6] = [(60.0, 60.0), (500.0, 60.0), (740.0, 200.0), (740.0, 300.0), (500.0, 440.0), (60.0, 440.0)];
const H19_A: [P; 4] = rect(60.0, 180.0, 560.0, 210.0);
const H19_B: [P; 4] = rect(240.0, 310.0, 740.0, 340.0);
const H21: [P; 16] = [
    (60.0, 440.0),
    (60.0, 300.0),
    (120.0, 180.0),
    (230.0, 100.0),
    (400.0, 60.0),
    (570.0, 100.0),
    (680.0, 180.0),
    (740.0, 300.0),
    (740.0, 440.0),
    (600.0, 440.0),
    (600.0, 320.0),
    (540.0, 230.0),
    (400.0, 190.0),
    (260.0, 230.0),
    (200.0, 320.0),
    (200.0, 440.0),
];
const H22_A: [P; 4] = rect(250.0, 60.0, 270.0, 200.0);
const H22_B: [P; 4] = rect(250.0, 260.0, 270.0, 440.0);
const H22_C: [P; 4] = rect(420.0, 60.0, 440.0, 320.0);
const H22_D: [P; 4] = rect(420.0, 380.0, 440.0, 440.0);
const H22_E: [P; 4] = rect(590.0, 60.0, 610.0, 120.0);
const H22_F: [P; 4] = rect(590.0, 180.0, 610.0, 440.0);
const H25_A: [P; 4] = rect(140.0, 140.0, 660.0, 160.0);
const H25_B: [P; 4] = rect(640.0, 160.0, 660.0, 360.0);
const H25_C: [P; 4] = rect(220.0, 340.0, 640.0, 360.0);
const H25_D: [P; 4] = rect(220.0, 220.0, 240.0, 340.0);
const H25_E: [P; 4] = rect(240.0, 220.0, 560.0, 240.0);

/// Tüm delikler. Bir oyunda bunlardan `HOLES_PER_GAME` tanesi rastgele seçilir.
const HOLES: &[Hole] = &[
    Hole {
        name: "Isınma",
        par: 2,
        bounds: &H1,
        blocks: &[],
        posts: &[],
        sand: &[],
        water: &[],
        tee: (150.0, 250.0),
        cup: (650.0, 250.0),
        pups: &[(400.0, 195.0), (400.0, 305.0)],
    },
    Hole {
        name: "Duvar",
        par: 3,
        bounds: &H2,
        blocks: &[&H2_WALL],
        posts: &[],
        sand: &[],
        water: &[],
        tee: (160.0, 160.0),
        cup: (640.0, 160.0),
        pups: &[(400.0, 375.0), (250.0, 300.0)],
    },
    Hole {
        name: "Dönemeç",
        par: 3,
        bounds: &H3,
        blocks: &[],
        posts: &[],
        sand: &[(430.0, 380.0, 120.0, 60.0)],
        water: &[],
        tee: (200.0, 130.0),
        cup: (660.0, 365.0),
        pups: &[(210.0, 360.0), (560.0, 330.0)],
    },
    Hole {
        name: "Göl",
        par: 3,
        bounds: &H4,
        blocks: &[],
        posts: &[],
        sand: &[(330.0, 350.0, 140.0, 60.0)],
        water: &[(330.0, 90.0, 140.0, 200.0)],
        tee: (130.0, 250.0),
        cup: (670.0, 190.0),
        pups: &[(400.0, 320.0), (560.0, 330.0)],
    },
    Hole {
        name: "Direkler",
        par: 3,
        bounds: &H5,
        blocks: &[],
        posts: &[(300.0, 180.0, 22.0), (300.0, 320.0, 22.0), (450.0, 250.0, 28.0), (590.0, 165.0, 20.0), (590.0, 335.0, 20.0)],
        sand: &[],
        water: &[],
        tee: (120.0, 250.0),
        cup: (690.0, 250.0),
        pups: &[(450.0, 140.0), (450.0, 360.0)],
    },
    Hole {
        name: "Zikzak",
        par: 4,
        bounds: &FULL,
        blocks: &[&H6_A, &H6_B],
        posts: &[],
        sand: &[(560.0, 60.0, 180.0, 60.0)],
        water: &[],
        tee: (140.0, 120.0),
        cup: (650.0, 380.0),
        pups: &[(370.0, 120.0), (370.0, 400.0)],
    },
    Hole {
        name: "Ada",
        par: 4,
        bounds: &FULL,
        blocks: &[],
        posts: &[],
        sand: &[],
        water: &[(300.0, 120.0, 50.0, 260.0), (450.0, 120.0, 50.0, 260.0), (300.0, 120.0, 200.0, 40.0)],
        tee: (130.0, 250.0),
        cup: (400.0, 260.0),
        pups: &[(400.0, 410.0), (620.0, 130.0)],
    },
    Hole {
        name: "Koridor",
        par: 3,
        bounds: &H8,
        blocks: &[],
        posts: &[(250.0, 225.0, 14.0), (400.0, 275.0, 14.0), (550.0, 225.0, 14.0)],
        sand: &[],
        water: &[],
        tee: (90.0, 250.0),
        cup: (710.0, 250.0),
        pups: &[(325.0, 250.0), (475.0, 250.0)],
    },
    Hole {
        name: "U dönüşü",
        par: 3,
        bounds: &FULL,
        blocks: &[&H9_WALL],
        posts: &[],
        sand: &[(620.0, 180.0, 120.0, 140.0)],
        water: &[],
        tee: (120.0, 140.0),
        cup: (120.0, 360.0),
        pups: &[(400.0, 140.0), (400.0, 360.0)],
    },
    Hole {
        name: "Merdiven",
        par: 4,
        bounds: &H10,
        blocks: &[],
        posts: &[],
        sand: &[(330.0, 180.0, 70.0, 40.0)],
        water: &[],
        tee: (140.0, 140.0),
        cup: (650.0, 360.0),
        pups: &[(380.0, 270.0), (600.0, 320.0)],
    },
    Hole {
        name: "Kum havuzu",
        par: 3,
        bounds: &H11,
        blocks: &[],
        posts: &[(350.0, 170.0, 18.0), (350.0, 330.0, 18.0)],
        sand: &[(500.0, 150.0, 200.0, 40.0), (500.0, 310.0, 200.0, 40.0), (660.0, 190.0, 40.0, 120.0)],
        water: &[],
        tee: (130.0, 250.0),
        cup: (600.0, 250.0),
        pups: &[(250.0, 130.0), (250.0, 370.0)],
    },
    Hole {
        name: "Elmas",
        par: 3,
        bounds: &H12,
        blocks: &[&H12_GEM],
        posts: &[],
        sand: &[],
        water: &[],
        tee: (130.0, 250.0),
        cup: (670.0, 250.0),
        pups: &[(400.0, 125.0), (400.0, 375.0)],
    },
    Hole {
        name: "Nehir",
        par: 4,
        bounds: &FULL,
        blocks: &[],
        posts: &[],
        sand: &[(560.0, 330.0, 120.0, 60.0)],
        water: &[(360.0, 60.0, 80.0, 300.0)],
        tee: (150.0, 120.0),
        cup: (650.0, 120.0),
        pups: &[(400.0, 400.0), (200.0, 380.0)],
    },
    Hole {
        name: "Labirent",
        par: 5,
        bounds: &FULL,
        blocks: &[&H14_A, &H14_B, &H14_C],
        posts: &[],
        sand: &[],
        water: &[],
        tee: (145.0, 120.0),
        cup: (670.0, 120.0),
        pups: &[(330.0, 120.0), (500.0, 400.0)],
    },
    Hole {
        name: "Cep",
        par: 3,
        bounds: &FULL,
        blocks: &[&H15_A, &H15_B],
        posts: &[(300.0, 200.0, 25.0)],
        sand: &[],
        water: &[],
        tee: (130.0, 380.0),
        cup: (665.0, 190.0),
        pups: &[(300.0, 330.0), (450.0, 100.0)],
    },
    Hole {
        name: "Kale",
        par: 4,
        bounds: &FULL,
        blocks: &[&H16_L, &H16_T, &H16_B],
        posts: &[(300.0, 250.0, 22.0)],
        sand: &[],
        water: &[],
        tee: (130.0, 250.0),
        cup: (560.0, 250.0),
        pups: &[(300.0, 120.0), (300.0, 380.0)],
    },
    Hole {
        name: "Çifte göl",
        par: 3,
        bounds: &FULL,
        blocks: &[],
        posts: &[(480.0, 180.0, 20.0), (480.0, 320.0, 20.0)],
        sand: &[],
        water: &[(220.0, 60.0, 120.0, 170.0), (220.0, 270.0, 120.0, 170.0)],
        tee: (110.0, 250.0),
        cup: (670.0, 250.0),
        pups: &[(280.0, 250.0), (560.0, 110.0)],
    },
    Hole {
        name: "Huni",
        par: 3,
        bounds: &H18,
        blocks: &[],
        posts: &[(250.0, 150.0, 16.0), (250.0, 250.0, 16.0), (250.0, 350.0, 16.0), (400.0, 200.0, 16.0), (400.0, 300.0, 16.0), (540.0, 250.0, 16.0)],
        sand: &[],
        water: &[],
        tee: (110.0, 250.0),
        cup: (700.0, 250.0),
        pups: &[(330.0, 250.0), (480.0, 130.0)],
    },
    Hole {
        name: "Yılan",
        par: 5,
        bounds: &FULL,
        blocks: &[&H19_A, &H19_B],
        posts: &[],
        sand: &[(420.0, 345.0, 80.0, 90.0)],
        water: &[],
        tee: (110.0, 120.0),
        cup: (690.0, 400.0),
        pups: &[(650.0, 260.0), (150.0, 390.0)],
    },
    Hole {
        name: "Köprü",
        par: 3,
        bounds: &FULL,
        blocks: &[],
        posts: &[(400.0, 250.0, 18.0)],
        sand: &[],
        water: &[(250.0, 60.0, 300.0, 140.0), (250.0, 300.0, 300.0, 140.0)],
        tee: (120.0, 250.0),
        cup: (680.0, 250.0),
        pups: &[(330.0, 250.0), (150.0, 120.0)],
    },
    Hole {
        name: "Hilal",
        par: 4,
        bounds: &H21,
        blocks: &[],
        posts: &[],
        sand: &[(480.0, 105.0, 70.0, 40.0)],
        water: &[],
        tee: (130.0, 400.0),
        cup: (670.0, 400.0),
        pups: &[(400.0, 125.0), (160.0, 270.0)],
    },
    Hole {
        name: "Kapılar",
        par: 4,
        bounds: &FULL,
        blocks: &[&H22_A, &H22_B, &H22_C, &H22_D, &H22_E, &H22_F],
        posts: &[],
        sand: &[],
        water: &[],
        tee: (130.0, 250.0),
        cup: (690.0, 300.0),
        pups: &[(345.0, 330.0), (515.0, 200.0)],
    },
    Hole {
        name: "Bataklık",
        par: 4,
        bounds: &FULL,
        blocks: &[],
        posts: &[],
        sand: &[(180.0, 60.0, 120.0, 160.0), (180.0, 300.0, 120.0, 140.0), (430.0, 180.0, 120.0, 140.0)],
        water: &[(330.0, 60.0, 70.0, 110.0), (330.0, 330.0, 70.0, 110.0), (580.0, 60.0, 70.0, 130.0), (580.0, 310.0, 70.0, 130.0)],
        tee: (110.0, 250.0),
        cup: (690.0, 250.0),
        pups: &[(365.0, 250.0), (615.0, 250.0)],
    },
    Hole {
        name: "Langırt",
        par: 3,
        bounds: &FULL,
        blocks: &[],
        posts: &[
            (200.0, 150.0, 15.0),
            (200.0, 350.0, 15.0),
            (300.0, 250.0, 20.0),
            (400.0, 130.0, 15.0),
            (400.0, 370.0, 15.0),
            (500.0, 250.0, 20.0),
            (600.0, 150.0, 15.0),
            (600.0, 350.0, 15.0),
        ],
        sand: &[],
        water: &[],
        tee: (110.0, 250.0),
        cup: (690.0, 250.0),
        pups: &[(300.0, 140.0), (500.0, 360.0)],
    },
    Hole {
        name: "Sarmal",
        par: 4,
        bounds: &FULL,
        blocks: &[&H25_A, &H25_B, &H25_C, &H25_D, &H25_E],
        posts: &[],
        sand: &[],
        water: &[],
        tee: (100.0, 100.0),
        cup: (400.0, 290.0),
        pups: &[(600.0, 190.0), (100.0, 400.0)],
    },
];

pub fn quit_move(seat: usize) -> String {
    format!("q{seat}")
}

/// `a<açı>p<güç>` → (açı, güç). Yalnızca kanonik yazım kabul edilir (baştaki sıfır yok).
pub fn parse_shot(mv: &str) -> Option<(u32, u32)> {
    let rest = mv.strip_prefix('a')?;
    let (a, p) = rest.split_once('p')?;
    let (a, p): (u32, u32) = (a.parse().ok()?, p.parse().ok()?);
    (a < 3600 && (1..=100).contains(&p) && format!("a{a}p{p}") == mv).then_some((a, p))
}

pub fn parse_quit(mv: &str) -> Option<usize> {
    let seat: usize = mv.strip_prefix('q')?.parse().ok()?;
    (quit_move(seat) == mv).then_some(seat)
}

// ───────────── Rastgelelik (herkeste aynı) ─────────────

/// FNV-1a: sürümden ve platformdan bağımsız, kararlı özet (std'nin `DefaultHasher`'ı değil).
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// [0, 1)
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Bir oyunun sahası: seçilen delikler ve her deliğin power-up türleri.
struct Course {
    holes: Vec<usize>,
    kinds: Vec<Vec<Pup>>,
    seed: u64,
}

fn course(seed: &str) -> Course {
    let seed = fnv(seed.as_bytes());
    let mut rng = Rng(seed);
    let mut idx: Vec<usize> = (0..HOLES.len()).collect();
    for i in 0..HOLES_PER_GAME.min(idx.len()) {
        let j = i + rng.below(idx.len() - i);
        idx.swap(i, j);
    }
    idx.truncate(HOLES_PER_GAME);
    let kinds = idx
        .iter()
        .map(|&h| {
            let mut ks: Vec<Pup> = Vec::new();
            for _ in HOLES[h].pups {
                // Aynı delikte aynı tür iki kez çıkmasın.
                let mut k = PUP_KINDS[rng.below(PUP_KINDS.len())];
                while ks.contains(&k) {
                    k = PUP_KINDS[rng.below(PUP_KINDS.len())];
                }
                ks.push(k);
            }
            ks
        })
        .collect();
    Course { holes: idx, kinds, seed }
}

/// `shaky` etkisindeki vuruşun sapması (derece): oyun ve vuruş sırasına bağlı, herkeste aynı.
fn shake_for(seed: u64, shot_no: usize) -> f64 {
    let mut rng = Rng(seed ^ (shot_no as u64).wrapping_mul(0xa076_1d64_78bd_642f));
    let mag = SHAKE_MIN + (SHAKE_MAX - SHAKE_MIN) * rng.unit();
    if rng.next() & 1 == 0 {
        mag
    } else {
        -mag
    }
}

// ───────────── Fizik ─────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum ShotEnd {
    Stop,
    In,
    Water,
}

/// Vuruşa etki eden power-up'lar.
#[derive(Debug, Clone, Copy)]
struct Mods {
    speed: f64,
    no_sand: bool,
    ice: bool,
    ghost: bool,
    magnet: bool,
    /// Açıya eklenen sapma (derece).
    shake: f64,
}

const NO_MODS: Mods = Mods { speed: 1.0, no_sand: false, ice: false, ghost: false, magnet: false, shake: 0.0 };

struct Shot {
    end: P,
    result: ShotEnd,
    path: Vec<P>,
    /// Alınan power-up'lar: (deliğin power-up sırası, alındığı andaki yol noktası).
    picked: Vec<(usize, usize)>,
}

fn inside_rect(r: &Rect, x: f64, y: f64) -> bool {
    x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3
}

fn inside_poly(poly: &[P], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn edges(poly: &[P]) -> impl Iterator<Item = (P, P)> + '_ {
    (0..poly.len()).map(move |i| (poly[i], poly[(i + 1) % poly.len()]))
}

/// Top bir iç engelin (blok ya da direk) içinde mi?
fn in_obstacle(hole: &Hole, x: f64, y: f64) -> bool {
    hole.blocks.iter().any(|b| inside_poly(b, x, y)) || hole.posts.iter().any(|&(px, py, r)| (x - px).hypot(y - py) < r + BALL_R)
}

/// Topu (x, y) noktasından en yakın engel noktasına göre dışarı iter ve hızını yansıtır.
fn bounce_from(x: &mut f64, y: &mut f64, vx: &mut f64, vy: &mut f64, px: f64, py: f64, min_d: f64) {
    let (dx, dy) = (*x - px, *y - py);
    let d = dx.hypot(dy);
    if d >= min_d || d < 1e-9 {
        return;
    }
    let (nx, ny) = (dx / d, dy / d);
    let vn = *vx * nx + *vy * ny;
    if vn < 0.0 {
        *vx -= (1.0 + BOUNCE) * vn * nx;
        *vy -= (1.0 + BOUNCE) * vn * ny;
    }
    *x = px + nx * min_d;
    *y = py + ny * min_d;
}

/// Vuruşu oynatır. `angle` derece; `avail`: bu delikte henüz alınmamış power-up'lar.
fn shoot(hole: &Hole, start: P, angle: f64, power: u32, m: &Mods, avail: &[bool]) -> Shot {
    // Hayalet top iç duvarlardan geçer; sahanın kendi sınırından geçemez.
    let segs: Vec<(P, P)> = if m.ghost {
        edges(hole.bounds).collect()
    } else {
        edges(hole.bounds).chain(hole.blocks.iter().flat_map(|b| edges(b))).collect()
    };
    let a = (angle + m.shake).to_radians();
    let v0 = VMAX * power as f64 / 100.0 * m.speed;
    let (mut x, mut y) = start;
    let (mut vx, mut vy) = (v0 * a.cos(), v0 * a.sin());
    let mut path = vec![start];
    let mut picked: Vec<(usize, usize)> = Vec::new();
    let mut last_free = start;
    let mut slow_steps = 0usize;
    let cup_speed = if m.magnet { CUP_SPEED * 1.5 } else { CUP_SPEED };
    for step in 1..=MAX_STEPS {
        let in_sand = !m.no_sand && hole.sand.iter().any(|r| inside_rect(r, x, y));
        let (mut decel, mut drag) = if in_sand { SAND } else { GRASS };
        if m.ice {
            decel *= ICE_FRICTION;
            drag *= ICE_FRICTION;
        }
        let sp = vx.hypot(vy);
        if sp > 0.0 {
            let f = (sp - (decel + drag * sp) * DT).max(0.0) / sp;
            vx *= f;
            vy *= f;
        }
        // Mıknatıs: deliğe yaklaştıkça artan çekim.
        let mut pulling = false;
        if m.magnet {
            let (dx, dy) = (hole.cup.0 - x, hole.cup.1 - y);
            let d = dx.hypot(dy);
            if d < MAGNET_R && d > 1e-6 {
                let acc = MAGNET_ACC * (1.0 - d / MAGNET_R);
                vx += dx / d * acc * DT;
                vy += dy / d * acc * DT;
                pulling = acc > decel * 1.05;
            }
        }
        x += vx * DT;
        y += vy * DT;
        for &((ax, ay), (bx, by)) in &segs {
            let (ex, ey) = (bx - ax, by - ay);
            let t = (((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
            bounce_from(&mut x, &mut y, &mut vx, &mut vy, ax + ex * t, ay + ey * t, BALL_R);
        }
        if !m.ghost {
            for &(px, py, r) in hole.posts {
                let (dx, dy) = (x - px, y - py);
                let d = dx.hypot(dy);
                if d < r + BALL_R && d > 1e-9 {
                    // Direğin yüzeyindeki en yakın nokta.
                    bounce_from(&mut x, &mut y, &mut vx, &mut vy, px + dx / d * r, py + dy / d * r, BALL_R);
                }
            }
        } else if !in_obstacle(hole, x, y) {
            last_free = (x, y);
        }
        for (i, &(px, py)) in hole.pups.iter().enumerate() {
            if avail.get(i).copied().unwrap_or(false) && !picked.iter().any(|p| p.0 == i) && (x - px).hypot(y - py) < PUP_R + BALL_R {
                picked.push((i, path.len()));
            }
        }
        if hole.water.iter().any(|r| inside_rect(r, x, y)) {
            path.push((x, y));
            return Shot { end: start, result: ShotEnd::Water, path, picked };
        }
        let sp = vx.hypot(vy);
        if (x - hole.cup.0).hypot(y - hole.cup.1) < CUP_R && sp < cup_speed {
            path.push(hole.cup);
            return Shot { end: hole.cup, result: ShotEnd::In, path, picked };
        }
        if sp < STOP_SPEED {
            // Mıknatıs topu durmuşken de deliğe sürükler (duvara takılırsa kısa sürede bırakır).
            slow_steps += 1;
            if !pulling || slow_steps > 48 {
                break;
            }
        } else {
            slow_steps = 0;
        }
        if step % SAMPLE_EVERY == 0 {
            path.push((x, y));
        }
    }
    // Hayalet top bir engelin içinde durduysa engele girmeden önceki son yerine konur.
    if m.ghost && in_obstacle(hole, x, y) {
        (x, y) = last_free;
    }
    path.push((x, y));
    // Güvenlik: sayısal bir hatayla saha dışına çıkan top vuruşun başladığı yere döner.
    let outside = !inside_poly(hole.bounds, x, y) || hole.blocks.iter().any(|b| inside_poly(b, x, y));
    Shot { end: if outside { start } else { (x, y) }, result: ShotEnd::Stop, path, picked }
}

#[cfg(test)]
fn simulate(hole: &Hole, start: P, angle: u32, power: u32) -> Shot {
    shoot(hole, start, angle as f64 / 10.0, power, &NO_MODS, &[])
}

// ───────────── Oyun ─────────────

#[derive(Debug, Clone, Default)]
struct Ball {
    /// `None`: henüz vurmadı (başlangıç noktasında).
    pos: Option<P>,
    strokes: u32,
    done: bool,
}

/// Oyuncunun sonraki vuruşunu etkileyecek power-up'lar.
#[derive(Debug, Clone, Copy, Default)]
struct Effects {
    slow: bool,
    shaky: bool,
    ice: bool,
    /// Kendine aldığı etki (turbo / ghost / magnet).
    buff: Option<Pup>,
}

impl Effects {
    fn mods(&self, seed: u64, shot_no: usize) -> Mods {
        let mut m = NO_MODS;
        if self.slow {
            m.speed *= SLOW_SPEED;
        }
        if self.shaky {
            m.shake = shake_for(seed, shot_no);
        }
        m.ice = self.ice;
        match self.buff {
            Some(Pup::Turbo) => {
                m.speed *= TURBO_SPEED;
                m.no_sand = true;
            }
            Some(Pup::Ghost) => m.ghost = true,
            Some(Pup::Magnet) => m.magnet = true,
            _ => {}
        }
        m
    }
    fn list(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.slow {
            v.push("slow");
        }
        if self.shaky {
            v.push("shaky");
        }
        if self.ice {
            v.push("ice");
        }
        if let Some(b) = self.buff {
            v.push(b.id());
        }
        v
    }
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn hole_json(no: usize, h: &Hole, kinds: &[Pup]) -> Value {
    let pts = |p: &[P]| p.iter().map(|&(x, y)| json!([x, y])).collect::<Vec<_>>();
    json!({
        "no": no + 1,
        "name": h.name,
        "par": h.par,
        "bounds": pts(h.bounds),
        "blocks": h.blocks.iter().map(|b| pts(b)).collect::<Vec<_>>(),
        "posts": h.posts.iter().map(|&(x, y, r)| json!([x, y, r])).collect::<Vec<_>>(),
        "sand": h.sand.iter().map(|&(x, y, w, hh)| json!([x, y, w, hh])).collect::<Vec<_>>(),
        "water": h.water.iter().map(|&(x, y, w, hh)| json!([x, y, w, hh])).collect::<Vec<_>>(),
        "tee": [h.tee.0, h.tee.1],
        "cup": [h.cup.0, h.cup.1],
        "pups": h.pups.iter().zip(kinds).map(|(&(x, y), k)| json!([x, y, k.id()])).collect::<Vec<_>>(),
    })
}

/// `seed`: oda kimliği; oyunun deliklerini ve power-up türlerini belirler.
pub fn evaluate(seed: &str, players: usize, moves: &[String]) -> Result<Eval, String> {
    play(&course(seed), players, moves)
}

fn play(course: &Course, players: usize, moves: &[String]) -> Result<Eval, String> {
    let n = players;
    if !(2..=6).contains(&n) {
        return Err("geçersiz oyuncu sayısı".into());
    }
    let holes: Vec<&Hole> = course.holes.iter().map(|&i| &HOLES[i]).collect();
    let mut hole = 0usize;
    let mut balls = vec![Ball::default(); n];
    let mut quit = vec![false; n];
    let mut fx = vec![Effects::default(); n];
    let mut avail = vec![true; holes[0].pups.len()];
    let mut scores: Vec<Vec<Option<u32>>> = vec![Vec::new(); n];
    let mut last: Value = Value::Null;
    let mut outcome: Option<Outcome> = None;
    let mut shot_no = 0usize;
    // Sıradaki oyuncu: `from` dahil, çekilmemiş ve deliği bitirmemiş ilk koltuk.
    let pick = |from: usize, balls: &[Ball], quit: &[bool]| (0..n).map(|k| (from + k) % n).find(|&c| !quit[c] && !balls[c].done);
    let mut turn = pick(0, &balls, &quit);

    for mv in moves {
        if outcome.is_some() {
            return Err("oyun bitmişken hamle".into());
        }
        if let Some(seat) = parse_quit(mv) {
            if seat >= n || quit[seat] {
                return Err("geçersiz çekilme".into());
            }
            quit[seat] = true;
            if turn == Some(seat) {
                turn = pick((seat + 1) % n, &balls, &quit);
            }
            let left: Vec<usize> = (0..n).filter(|&s| !quit[s]).collect();
            if left.len() <= 1 {
                outcome = Some(Outcome { winner: left.first().copied(), reason: "forfeit" });
                turn = None;
                continue;
            }
        } else if let Some((angle, power)) = parse_shot(mv) {
            let s = turn.ok_or("sıra kimsede değil")?;
            let h = holes[hole];
            let start = balls[s].pos.unwrap_or(h.tee);
            let used = fx[s];
            let mods = used.mods(course.seed, shot_no);
            shot_no += 1;
            fx[s] = Effects::default();
            let before = avail.clone();
            let shot = shoot(h, start, angle as f64 / 10.0, power, &mods, &avail);
            let mut picked = Vec::new();
            for &(i, at) in &shot.picked {
                avail[i] = false;
                let kind = course.kinds[hole][i];
                if kind.hits_others() {
                    for o in (0..n).filter(|&o| o != s && !quit[o]) {
                        match kind {
                            Pup::Slow => fx[o].slow = true,
                            Pup::Shaky => fx[o].shaky = true,
                            _ => fx[o].ice = true,
                        }
                    }
                } else {
                    fx[s].buff = Some(kind);
                }
                picked.push(json!([i, kind.id(), at]));
            }
            let b = &mut balls[s];
            b.strokes += if shot.result == ShotEnd::Water { 2 } else { 1 };
            b.pos = Some(shot.end);
            if shot.result == ShotEnd::In || b.strokes >= MAX_STROKES {
                b.done = true;
                b.strokes = b.strokes.min(MAX_STROKES);
            }
            last = json!({
                "seat": s,
                "hole": hole,
                "result": match shot.result { ShotEnd::In => "in", ShotEnd::Water => "water", ShotEnd::Stop if b.done => "max", ShotEnd::Stop => "stop" },
                "strokes": b.strokes,
                "path": shot.path.iter().map(|&(x, y)| json!([r1(x), r1(y)])).collect::<Vec<_>>(),
                "end": [r1(shot.end.0), r1(shot.end.1)],
                "mods": used.list(),
                "shake": r1(mods.shake),
                "picked": picked,
                "avail": before,
            });
            turn = pick((s + 1) % n, &balls, &quit);
        } else {
            return Err(format!("geçersiz hamle: {mv}"));
        }
        // Delik bitti mi?
        if turn.is_none() {
            for s in 0..n {
                scores[s].push((!quit[s]).then_some(balls[s].strokes));
            }
            if hole + 1 == holes.len() {
                let totals: Vec<(usize, u32)> =
                    (0..n).filter(|&s| !quit[s]).map(|s| (s, scores[s].iter().flatten().sum())).collect();
                let best = totals.iter().map(|t| t.1).min().unwrap_or(0);
                let winners: Vec<usize> = totals.iter().filter(|t| t.1 == best).map(|t| t.0).collect();
                outcome = Some(if winners.len() == 1 {
                    Outcome { winner: Some(winners[0]), reason: "score" }
                } else {
                    Outcome { winner: None, reason: "tie" }
                });
            } else {
                hole += 1;
                balls = vec![Ball::default(); n];
                avail = vec![true; holes[hole].pups.len()];
                turn = pick(hole % n, &balls, &quit);
            }
        }
    }

    let h = holes[hole];
    Ok(Eval {
        turn: if outcome.is_none() { turn } else { None },
        legal: Vec::new(),
        info: json!({
            "width": WIDTH,
            "height": HEIGHT,
            "ballR": BALL_R,
            "cupR": CUP_R,
            "pupR": PUP_R,
            "maxStrokes": MAX_STROKES,
            "levels": HOLES.len(),
            "holes": course.holes.iter().zip(&course.kinds).map(|(&i, k)| hole_json(i, &HOLES[i], k)).collect::<Vec<_>>(),
            "hole": hole,
            "balls": balls.iter().map(|b| {
                let (x, y) = b.pos.unwrap_or(h.tee);
                json!({ "x": r1(x), "y": r1(y), "strokes": b.strokes, "done": b.done })
            }).collect::<Vec<_>>(),
            "pupAvail": avail,
            "effects": fx.iter().map(|f| f.list()).collect::<Vec<_>>(),
            "quit": quit,
            "scores": scores,
            "totals": scores.iter().map(|s| s.iter().flatten().sum::<u32>()).collect::<Vec<_>>(),
            "lastShot": last,
        }),
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// İlk altı delik sırayla ve verilen power-up türleriyle oynanan saha.
    fn fixed(kind: Pup) -> Course {
        Course { holes: (0..HOLES_PER_GAME).collect(), kinds: (0..HOLES_PER_GAME).map(|h| vec![kind; HOLES[h].pups.len()]).collect(), seed: 7 }
    }

    #[test]
    fn move_formats() {
        assert_eq!(parse_shot("a1800p65"), Some((1800, 65)));
        assert_eq!(parse_shot("a0p1"), Some((0, 1)));
        for bad in ["a3600p50", "a10p0", "a10p101", "a010p5", "ap5", "a10", "b10p5", "a10p5x"] {
            assert_eq!(parse_shot(bad), None, "{bad}");
        }
        assert_eq!(parse_quit("q3"), Some(3));
        assert_eq!(parse_quit("q03"), None);
        assert!(evaluate("r", 3, &shots(&["q5"])).is_err());
        assert!(evaluate("r", 2, &shots(&["q0", "q0"])).is_err());
        assert!(evaluate("r", 1, &[]).is_err());
        assert!(evaluate("r", 7, &[]).is_err());
    }

    #[test]
    fn twenty_five_distinct_holes() {
        assert_eq!(HOLES.len(), 25);
        for (i, a) in HOLES.iter().enumerate() {
            for b in &HOLES[i + 1..] {
                assert_ne!(a.name, b.name);
            }
        }
    }

    /// Tüm delikler: başlangıç, delik ve power-up'lar sahanın içinde, engel ve suyun dışında.
    #[test]
    fn holes_are_sane() {
        for h in HOLES {
            assert!((2..=5).contains(&h.par), "{}", h.name);
            assert!(!h.pups.is_empty() && h.pups.len() <= PUP_KINDS.len(), "{}", h.name);
            for (x, y) in [h.tee, h.cup].into_iter().chain(h.pups.iter().copied()) {
                assert!(inside_poly(h.bounds, x, y), "{} {x},{y}", h.name);
                assert!(!in_obstacle(h, x, y), "{} {x},{y}", h.name);
                assert!(!h.water.iter().any(|r| inside_rect(r, x, y)), "{} {x},{y}", h.name);
            }
            for &(x, y) in h.pups {
                assert!((x - h.tee.0).hypot(y - h.tee.1) > 40.0, "{}: power-up başlangıca çok yakın", h.name);
                assert!((x - h.cup.0).hypot(y - h.cup.1) > 40.0, "{}: power-up deliğe çok yakın", h.name);
            }
            assert!(!h.sand.iter().any(|r| inside_rect(r, h.cup.0, h.cup.1)), "{}", h.name);
        }
    }

    #[test]
    fn random_course_is_shared_and_varied() {
        let a = course("oda-1");
        assert_eq!(a.holes.len(), HOLES_PER_GAME);
        let mut uniq = a.holes.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), HOLES_PER_GAME, "aynı delik iki kez seçilmez");
        // Aynı oda herkeste aynı sahayı üretir.
        let b = course("oda-1");
        assert_eq!(a.holes, b.holes);
        assert_eq!(a.kinds, b.kinds);
        for (h, ks) in a.holes.iter().zip(&a.kinds) {
            assert_eq!(ks.len(), HOLES[*h].pups.len());
        }
        // Farklı odalar farklı sahalar; zamanla bütün delikler çıkar.
        let mut seen = vec![false; HOLES.len()];
        let mut differ = 0;
        for i in 0..400 {
            let c = course(&format!("{i}-oda"));
            if c.holes != a.holes {
                differ += 1;
            }
            for h in c.holes {
                seen[h] = true;
            }
        }
        assert!(differ > 390);
        assert!(seen.iter().all(|&s| s), "her delik bir gün çıkmalı");
        // Arayüze seçilen delikler gider.
        let e = evaluate("oda-1", 2, &[]).unwrap();
        assert_eq!(e.info["holes"].as_array().unwrap().len(), HOLES_PER_GAME);
        assert_eq!(e.info["holes"][0]["name"], HOLES[a.holes[0]].name);
        assert_eq!(e.info["levels"], 25);
    }

    #[test]
    fn straight_putt_can_hole_and_hard_shot_lips_out() {
        let h = &HOLES[0];
        let holed: Vec<u32> = (30..=90).filter(|&p| simulate(h, h.tee, 0, p).result == ShotEnd::In).collect();
        assert!(!holed.is_empty(), "ilk delik düz vuruşla girebilmeli");
        let hard = simulate(h, h.tee, 0, 100);
        assert_ne!(hard.result, ShotEnd::In, "çok hızlı top delikten geçer");
        // Duvara çarpıp sekmeli ama sahada kalmalı.
        assert!(inside_poly(h.bounds, hard.end.0, hard.end.1));
        assert!(hard.end.0 < 720.0 - BALL_R + 0.01);
    }

    #[test]
    fn walls_posts_and_water() {
        // 2. delik: doğrudan deliğe vurulan top ortadaki duvardan geri seker.
        let h = &HOLES[1];
        let s = simulate(h, h.tee, 0, 80);
        assert!(s.end.0 < 385.0, "duvarın içinden geçmemeli: {:?}", s.end);
        // 5. delik: ortadaki direğe vurulan top geri döner.
        let h = &HOLES[4];
        let s = simulate(h, (380.0, 250.0), 0, 40);
        assert!(s.end.0 < 450.0 - 28.0, "direğin içinden geçmemeli: {:?}", s.end);
        // 4. delik: göle vurulan top başladığı yere döner.
        let h = &HOLES[3];
        let s = simulate(h, h.tee, 3300, 60);
        assert_eq!(s.result, ShotEnd::Water);
        assert_eq!(s.end, h.tee);
        // Kum topu yavaşlatır.
        let grass = simulate(&HOLES[0], (100.0, 250.0), 0, 30);
        let sanded = simulate(&HOLES[2], (380.0, 410.0), 0, 30);
        assert!(sanded.end.0 - 380.0 < grass.end.0 - 100.0);
    }

    #[test]
    fn powerup_physics() {
        let h = &HOLES[1];
        let open = &HOLES[0];
        let base = shoot(open, (100.0, 250.0), 0.0, 25, &NO_MODS, &[]);
        let dist = |s: &Shot| s.end.0 - 100.0;
        // Yavaşlatma ve turbo gidilen yolu değiştirir; buz topu kaydırır.
        let slow = shoot(open, (100.0, 250.0), 0.0, 25, &Mods { speed: SLOW_SPEED, ..NO_MODS }, &[]);
        let turbo = shoot(open, (100.0, 250.0), 0.0, 25, &Mods { speed: TURBO_SPEED, no_sand: true, ..NO_MODS }, &[]);
        let ice = shoot(open, (100.0, 250.0), 0.0, 25, &Mods { ice: true, ..NO_MODS }, &[]);
        assert!(dist(&slow) < dist(&base) * 0.6, "{} {}", dist(&slow), dist(&base));
        assert!(dist(&turbo) > dist(&base) * 1.4);
        assert!(dist(&ice) > dist(&base) * 1.5);
        // Sapma: düz vuruş yukarı/aşağı kayar.
        let shaky = shoot(open, (100.0, 250.0), 0.0, 25, &Mods { shake: 8.0, ..NO_MODS }, &[]);
        assert!(shaky.end.1 > 250.0 + 10.0);
        // Hayalet top duvarın içinden geçer ama duvarın içinde durmaz.
        let ghost = shoot(h, h.tee, 0.0, 80, &Mods { ghost: true, ..NO_MODS }, &[]);
        assert!(ghost.end.0 > 415.0, "duvardan geçmeli: {:?}", ghost.end);
        for p in 30..=70 {
            let g = shoot(h, h.tee, 0.0, p, &Mods { ghost: true, ..NO_MODS }, &[]);
            assert!(!in_obstacle(h, g.end.0, g.end.1), "%{p}: {:?}", g.end);
        }
        // Mıknatıs: deliğin yanında duran top içeri çekilir.
        let near = (open.cup.0 - 60.0, open.cup.1 + 30.0);
        let soft = shoot(open, near, 0.0, 6, &NO_MODS, &[]);
        assert_ne!(soft.result, ShotEnd::In);
        let pulled = shoot(open, near, 0.0, 6, &Mods { magnet: true, ..NO_MODS }, &[]);
        assert_eq!(pulled.result, ShotEnd::In);
        // Sapma herkeste aynı, makul aralıkta ve vuruştan vuruşa değişir.
        let s: Vec<f64> = (0..50).map(|i| shake_for(42, i)).collect();
        assert!(s.iter().all(|v| (SHAKE_MIN..=SHAKE_MAX).contains(&v.abs())));
        assert!(s.iter().any(|&v| v > 0.0) && s.iter().any(|&v| v < 0.0));
        assert_eq!(s[3], shake_for(42, 3));
    }

    /// Power-up'ı alan oyuncunun rakipleri sonraki vuruşta etkilenir; power-up o delikte kaybolur.
    #[test]
    fn picking_powerups() {
        // 1. delik: (400,195)'teki power-up'a doğru vuruş.
        let h = &HOLES[0];
        let ang = ((195.0 - h.tee.1) as f64).atan2(400.0 - h.tee.0).to_degrees();
        let deg10 = ((ang * 10.0).round() as i64).rem_euclid(3600);
        let shot = format!("a{deg10}p45");
        assert!(parse_shot(&shot).is_some());

        let c = fixed(Pup::Slow);
        let e = play(&c, 3, &[shot.clone()]).unwrap();
        assert_eq!(e.info["pupAvail"], json!([false, true]));
        assert_eq!(e.info["lastShot"]["picked"][0][1], "slow");
        assert_eq!(e.info["effects"], json!([[], ["slow"], ["slow"]]));
        // Yavaşlatılan oyuncu aynı vuruşla daha kısa gider; etki bir vuruşta biter.
        let e = play(&c, 3, &[shot.clone(), "a0p40".into()]).unwrap();
        assert_eq!(e.info["lastShot"]["mods"], json!(["slow"]));
        let slowed_x = e.info["balls"][1]["x"].as_f64().unwrap();
        assert_eq!(e.info["effects"], json!([[], [], ["slow"]]));
        let e = play(&fixed(Pup::Turbo), 3, &[shot.clone(), "a0p40".into()]).unwrap();
        assert_eq!(e.info["effects"], json!([["turbo"], [], []]), "kendine etki yalnızca alana");
        let normal_x = e.info["balls"][1]["x"].as_f64().unwrap();
        assert!(slowed_x < normal_x - 50.0, "{slowed_x} {normal_x}");
        // Aynı power-up ikinci kez alınamaz.
        let e = play(&c, 3, &[shot.clone(), shot.clone()]).unwrap();
        assert_eq!(e.info["lastShot"]["picked"], json!([]));
        // Sapma etkisi oyunun tohumuyla herkeste aynı sonucu verir.
        let a = play(&fixed(Pup::Shaky), 2, &[shot.clone(), "a0p40".into()]).unwrap();
        let b = play(&fixed(Pup::Shaky), 2, &[shot.clone(), "a0p40".into()]).unwrap();
        assert_eq!(a.info, b.info);
        assert_ne!(a.info["lastShot"]["shake"], 0.0);
    }

    #[test]
    fn turns_holes_and_scoring() {
        // Herkes hep en hafif vuruşu yaparsa her delikte 8'e ulaşılır; berabere biter.
        let n = 3;
        let mut moves = Vec::new();
        let mut first_turns = Vec::new();
        for hole in 0..HOLES_PER_GAME {
            let e = evaluate("oda", n, &moves).unwrap();
            assert_eq!(e.info["hole"], hole);
            assert_eq!(e.info["pupAvail"].as_array().unwrap().len(), e.info["holes"][hole]["pups"].as_array().unwrap().len());
            first_turns.push(e.turn.unwrap());
            for _ in 0..(MAX_STROKES as usize * n) {
                moves.push("a0p1".to_string());
            }
        }
        assert_eq!(first_turns, vec![0, 1, 2, 0, 1, 2], "ilk vuruş her delikte sıradakine geçer");
        let e = evaluate("oda", n, &moves).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: None, reason: "tie" }));
        assert_eq!(e.info["totals"], json!([48, 48, 48]));
        assert!(evaluate("oda", n, &[moves.clone(), shots(&["a0p1"])].concat()).is_err(), "bitmiş oyuna vuruş olmaz");
    }

    #[test]
    fn quitting_and_turn_order() {
        let e = evaluate("r", 3, &shots(&["a0p1"])).unwrap();
        assert_eq!(e.turn, Some(1));
        assert_eq!(e.info["balls"][0]["strokes"], 1);
        // Sıradaki oyuncu çekilirse sıra bir sonrakine geçer.
        let e = evaluate("r", 3, &shots(&["a0p1", "q1"])).unwrap();
        assert_eq!(e.turn, Some(2));
        let e = evaluate("r", 3, &shots(&["a0p1", "q1", "a0p1"])).unwrap();
        assert_eq!(e.turn, Some(0));
        // Biri dışında herkes çekilirse kalan kazanır.
        let e = evaluate("r", 3, &shots(&["a0p1", "q1", "q0"])).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: Some(2), reason: "forfeit" }));
        // Deliği sokan oyuncu o delikte atlanır; en az vuruş kazanır.
        let c = fixed(Pup::Ghost);
        let p = (30..=90).find(|&p| simulate(&HOLES[0], HOLES[0].tee, 0, p).result == ShotEnd::In).unwrap();
        let e = play(&c, 2, &shots(&[&format!("a0p{p}"), "a0p1"])).unwrap();
        assert_eq!(e.info["balls"][0]["done"], true);
        assert_eq!(e.info["lastShot"]["seat"], 1);
        assert_eq!(e.turn, Some(1));
        // Aynı vuruş listesi her zaman aynı sonucu verir.
        assert_eq!(evaluate("r", 2, &shots(&["a450p70", "a3000p55"])).unwrap().info, evaluate("r", 2, &shots(&["a450p70", "a3000p55"])).unwrap().info);
    }

    /// Her delik makul vuruşla bitirilebilir mi? Deliğe olan yürüme mesafesini (engelleri
    /// dolaşarak) en çok azaltan vuruşları arayan basit bir oyuncu dener. Yavaş olduğundan
    /// yalnızca istenince çalışır: `cargo test --release -- --ignored holes_are_playable`.
    #[test]
    #[ignore]
    fn holes_are_playable() {
        const G: f64 = 5.0;
        let (gw, gh) = ((WIDTH / G) as usize, (HEIGHT / G) as usize);
        let mut report = Vec::new();
        for h in HOLES {
            // Delikten yürüme mesafesi (ızgara üzerinde, 8 yön).
            let free = |cx: usize, cy: usize| {
                let (x, y) = (cx as f64 * G + G / 2.0, cy as f64 * G + G / 2.0);
                inside_poly(h.bounds, x, y) && !in_obstacle(h, x, y) && !h.water.iter().any(|r| inside_rect(r, x, y))
            };
            let mut dist = vec![f64::INFINITY; gw * gh];
            let (cx, cy) = ((h.cup.0 / G) as usize, (h.cup.1 / G) as usize);
            dist[cy * gw + cx] = 0.0;
            let mut queue = std::collections::VecDeque::from([(cx, cy)]);
            while let Some((x, y)) = queue.pop_front() {
                let d = dist[y * gw + x];
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= gw as i32 || ny >= gh as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    let nd = d + if dx != 0 && dy != 0 { 1.414 } else { 1.0 };
                    if free(nx, ny) && nd < dist[ny * gw + nx] {
                        dist[ny * gw + nx] = nd;
                        queue.push_back((nx, ny));
                    }
                }
            }
            let walk = |p: P| dist[((p.1 / G) as usize).min(gh - 1) * gw + ((p.0 / G) as usize).min(gw - 1)];
            assert!(walk(h.tee).is_finite(), "{}: başlangıçtan deliğe yol yok", h.name);
            let mut beam = vec![h.tee];
            let mut solved = None;
            'strokes: for stroke in 1..=MAX_STROKES {
                let mut next: Vec<(f64, P)> = Vec::new();
                for &start in &beam {
                    for a in (0..3600).step_by(25) {
                        for p in [6, 10, 15, 20, 27, 35, 45, 55, 70, 85, 100] {
                            let s = simulate(h, start, a, p);
                            if s.result == ShotEnd::In {
                                solved = Some(stroke);
                                break 'strokes;
                            }
                            let penalty = if s.result == ShotEnd::Water { 1e6 } else { 0.0 };
                            next.push((walk(s.end) + penalty, s.end));
                        }
                    }
                }
                next.sort_by(|a, b| a.0.total_cmp(&b.0));
                beam.clear();
                for (_, p) in next {
                    if beam.iter().all(|&q: &P| (q.0 - p.0).hypot(q.1 - p.1) > 25.0) {
                        beam.push(p);
                    }
                    if beam.len() == 4 {
                        break;
                    }
                }
            }
            report.push(format!("{}: par {} · bulunan {:?}", h.name, h.par, solved));
            let s = solved.unwrap_or_else(|| panic!("{} bitirilemedi", h.name));
            assert!(s <= h.par + 1, "{}: par {} ama en iyi {s} vuruş", h.name, h.par);
        }
        println!("{}", report.join("\n"));
    }
}
