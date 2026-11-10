//! Oyunların ortak parçaları: atış/vuruş hamlesi biçimi ve herkeste aynı sonucu veren rastgelelik.
//!
//! Atışlı oyunlarda (golf, basket) hamle `s<koltuk>a<açı>p<güç>` biçimindedir: açı derecenin
//! onda biri (0–3599, ekran koordinatları: 0 = sağa, 900 = aşağı, 2700 = yukarı), güç yüzde
//! (1–100). Arayüz yalnızca `a<açı>p<güç>` gönderir; koltuk oda tarafından eklenir
//! (`seat_shot`). Oyundan çekilme `q<koltuk>`.

pub fn quit_move(seat: usize) -> String {
    format!("q{seat}")
}

/// `s<koltuk>a<açı>p<güç>` → (koltuk, açı, güç). Yalnızca kanonik yazım kabul edilir (baştaki
/// sıfır yok). Hamle kimin olduğunu kendisi taşır (golfte herkes aynı anda vurur).
pub fn parse_shot(mv: &str) -> Option<(usize, u32, u32)> {
    let rest = mv.strip_prefix('s')?;
    let (s, rest) = rest.split_once('a')?;
    let (a, p) = rest.split_once('p')?;
    let (s, a, p): (usize, u32, u32) = (s.parse().ok()?, a.parse().ok()?, p.parse().ok()?);
    (s < 6 && a < 3600 && (1..=100).contains(&p) && format!("s{s}a{a}p{p}") == mv).then_some((s, a, p))
}

/// Arayüzün gönderdiği `a<açı>p<güç>` vuruşunu oyuncunun koltuğuyla tamamlar (zaten koltuklu
/// ya da başka türden hamle olduğu gibi kalır; başkasının koltuğu yazılmışsa kurallar reddeder).
pub fn seat_shot(seat: usize, mv: &str) -> String {
    let full = format!("s{seat}{mv}");
    if mv.starts_with('a') && parse_shot(&full).is_some() {
        full
    } else {
        mv.to_string()
    }
}

pub fn parse_quit(mv: &str) -> Option<usize> {
    let seat: usize = mv.strip_prefix('q')?.parse().ok()?;
    (quit_move(seat) == mv).then_some(seat)
}

/// FNV-1a: sürümden ve platformdan bağımsız, kararlı özet (std'nin `DefaultHasher`'ı değil).
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// SplitMix64.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// [0, 1)
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}
