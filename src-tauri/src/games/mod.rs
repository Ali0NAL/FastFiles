//! Oyun kuralları (ağdan ve arayüzden bağımsız, saf fonksiyonlar).
//!
//! Her oyun yalnızca oyuncu sayısı ve hamle listesinden (`moves`) durumunu hesaplar: oda sahibi
//! her hamleyi `evaluate` ile doğrular, katılımcılar da sahipten gelen hamle listesini aynı
//! fonksiyonla yeniden oynatarak kontrol eder. Böylece kurallar tek yerde durur ve kimse
//! geçersiz hamle dayatamaz.
//!
//! **Yeni oyun eklemek:** `CATALOG`'a bir satır ekleyin; `evaluate`, `move_allowed`,
//! `move_actor` ve (çok oyunculuysa) `forfeit_move` içinde oyunun kurallarını bağlayın.
//! Oda yönetimi, davet, ağ ve kayıt (bkz. `node/game.rs`) oyundan bağımsızdır; arayüzde de
//! yalnızca oyun alanının çizimi oyuna özeldir.

pub mod basket;
pub mod chess;
mod common;
pub mod golf;

use serde::Serialize;
use serde_json::Value;

/// Bir hamle metninin en fazla uzunluğu (satrançta `e7e8q`, golfte `s1a1800p65`).
pub const MAX_MOVE_CHARS: usize = 16;
/// Bir oyundaki en fazla hamle sayısı. Satrançta 50 hamle kuralı bundan çok önce biter.
pub const MAX_MOVES: usize = 1200;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Oyuncu sayısı (oda sahibi dahil).
    pub min_players: usize,
    pub max_players: usize,
    /// Koltuk adları (ör. Beyaz, Siyah). Oyuncular bu sırayla oturur. Boşsa koltuklar adsızdır.
    pub seats: &'static [&'static str],
    /// Beraberlik teklif edilebilir mi (iki kişilik oyunlarda).
    pub draw_offers: bool,
    /// Takımlı oyun: çift koltuklar bir takım, tek koltuklar diğer takım; takımlar eşit sayıda.
    /// Sonuçtaki `winner` kazanan takımın ilk koltuğudur (0 ya da 1).
    pub teams: bool,
}

pub const CATALOG: &[GameDef] = &[
    GameDef {
        id: "chess",
        name: "Satranç",
        description: "İki kişilik klasik satranç. Rok, geçerken alma, terfi, şah mat, pat ve beraberlik kuralları dahil.",
        min_players: 2,
        max_players: 2,
        seats: &["Beyaz", "Siyah"],
        draw_offers: true,
        teams: false,
    },
    GameDef {
        id: "golf",
        name: "Mini golf",
        description: "2–6 kişilik mini golf: 25 delikten her oyunda rastgele 6 delik. Duvar, kum, su, engeller ve rakipleri yavaşlatan power-up'lar var. En az vuruşla bitiren kazanır.",
        min_players: 2,
        max_players: 6,
        seats: &[],
        draw_offers: false,
        teams: false,
    },
    GameDef {
        id: "basket",
        name: "Basket",
        description: "Takımlı atış yarışması: 1v1, 2v2 ya da 3v3. Her turda herkes aynı noktadan sırayla atar; rüzgâr, savunmacı, para topu, temiz sayı ve 🔥 seri bonusları var. Çok sayı atan takım kazanır.",
        min_players: 2,
        max_players: 6,
        seats: &[],
        draw_offers: false,
        teams: true,
    },
];

pub fn def(id: &str) -> Option<&'static GameDef> {
    CATALOG.iter().find(|g| g.id == id)
}

/// Oyun bitti: `winner` koltuk numarası (berabere ise `None`), `reason` makine okunur sebep
/// (ör. "checkmate", "stalemate", "repetition", "fifty", "material", "score", "tie", "forfeit").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    pub winner: Option<usize>,
    pub reason: &'static str,
}

/// Hamle listesinden hesaplanan oyun durumu.
#[derive(Debug, Clone)]
pub struct Eval {
    /// Sırası gelen koltuk (oyun bittiyse ya da herkes aynı anda oynuyorsa `None`).
    pub turn: Option<usize>,
    /// Şu an hamle yapabilecek koltuklar: sıralı oyunda yalnızca `turn`, golfte deliği henüz
    /// bitirmemiş herkes (aynı anda oynanır).
    pub movers: Vec<usize>,
    /// Sıradaki oyuncunun yapabileceği hamleler (sayılabiliyorsa; golfte boş).
    pub legal: Vec<String>,
    pub outcome: Option<Outcome>,
    /// Arayüze giden oyuna özel bilgi (satrançta FEN, golfte saha ve toplar...).
    pub info: Value,
}

/// Hamle metni biçim olarak kabul edilebilir mi (kurallardan önceki ucuz kontrol)?
pub fn valid_move_text(m: &str) -> bool {
    !m.is_empty() && m.len() <= MAX_MOVE_CHARS && m.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Hamle listesini baştan oynatır. Geçersiz bir hamle varsa hata döner.
///
/// Sonuçlar küçük bir önbellekte tutulur: oyun listesi her değişiklikte yeniden çizilir ve
/// golfte her vuruşun fiziği yeniden hesaplanırdı.
///
/// `seed`: oda kimliği. Rastgelelik içeren oyunlar (golfte delik seçimi) bundan türetilir; herkes
/// aynı oda kimliğini bildiği için herkes aynı sonucu hesaplar.
pub fn evaluate(game: &str, seed: &str, players: usize, moves: &[String]) -> Result<Eval, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::collections::{HashMap, VecDeque};
    use std::hash::{Hash, Hasher};
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(HashMap<u64, Result<Eval, String>>, VecDeque<u64>)>> = Mutex::new(None);
    const CACHE_MAX: usize = 256;

    let mut h = DefaultHasher::new();
    (game, seed, players, moves).hash(&mut h);
    let key = h.finish();
    if let Some((map, _)) = CACHE.lock().unwrap().as_ref() {
        if let Some(hit) = map.get(&key) {
            return hit.clone();
        }
    }
    let result = evaluate_uncached(game, seed, players, moves);
    let mut guard = CACHE.lock().unwrap();
    let (map, order) = guard.get_or_insert_with(Default::default);
    if map.insert(key, result.clone()).is_none() {
        order.push_back(key);
        while order.len() > CACHE_MAX {
            if let Some(old) = order.pop_front() {
                map.remove(&old);
            }
        }
    }
    result
}

fn evaluate_uncached(game: &str, seed: &str, players: usize, moves: &[String]) -> Result<Eval, String> {
    if moves.len() > MAX_MOVES {
        return Err("çok fazla hamle".into());
    }
    if !moves.iter().all(|m| valid_move_text(m)) {
        return Err("geçersiz hamle".into());
    }
    match game {
        "chess" => chess::evaluate(moves),
        "golf" => golf::evaluate(seed, players, moves),
        "basket" => basket::evaluate(seed, players, moves),
        _ => Err("bilinmeyen oyun".into()),
    }
}

/// Oyuncunun gönderdiği hamlenin odaya yazılacak hâli. Atışlı oyunlarda (golf, basket) hamle
/// atanın koltuğunu taşır (`a900p50` → `s1a900p50`); diğer oyunlarda olduğu gibi kalır.
pub fn own_move(game: &str, seat: usize, mv: &str) -> String {
    match game {
        "golf" | "basket" => common::seat_shot(seat, mv),
        _ => mv.to_string(),
    }
}

/// Bu hamle kurallara uygun mu? (`before`: hamleden önceki durum; kimin yaptığı ayrıca
/// `move_actor` ve `Eval::movers` ile denetlenir.)
pub fn move_allowed(game: &str, before: &Eval, mv: &str) -> bool {
    match game {
        "golf" | "basket" => common::parse_shot(mv).is_some(),
        _ => before.legal.iter().any(|m| m == mv),
    }
}

/// Hamleyi hangi koltuk yaptı? Sıralı oyunda sırası gelen oyuncu; golfte vuruş ve oyundan
/// çekilme (`q<koltuk>`) koltuğu kendisi taşır.
pub fn move_actor(game: &str, before: &Eval, mv: &str) -> Option<usize> {
    match game {
        "golf" | "basket" => common::parse_quit(mv).or_else(|| common::parse_shot(mv).map(|s| s.0)),
        _ => before.turn,
    }
}

/// Çok oyunculu oyunda oyuncunun çekilmesini temsil eden hamle (oyun diğerleriyle sürer).
/// İki kişilik oyunlarda (satranç) `None`: pes eden kaybeder, oyun biter.
pub fn forfeit_move(game: &str, seat: usize) -> Option<String> {
    match game {
        "golf" | "basket" => Some(common::quit_move(seat)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_unknown_game() {
        assert_eq!(def("chess").unwrap().max_players, 2);
        assert_eq!(def("golf").unwrap().max_players, 6);
        assert!(def("tavla").is_none());
        assert!(evaluate("tavla", "r", 2, &[]).is_err());
        assert!(evaluate("chess", "r", 2, &["e2e4; rm -rf".into()]).is_err());
        let e = evaluate("chess", "r", 2, &[]).unwrap();
        assert_eq!(e.turn, Some(0));
        assert_eq!(e.legal.len(), 20);
        assert!(move_allowed("chess", &e, "e2e4"));
        assert!(!move_allowed("chess", &e, "e2e5"));
        assert_eq!(forfeit_move("chess", 0), None);
        assert_eq!(e.movers, vec![0]);
        let g = evaluate("golf", "r", 3, &[]).unwrap();
        assert_eq!(g.turn, None);
        assert_eq!(g.movers, vec![0, 1, 2], "golfte herkes aynı anda vurur");
        assert_eq!(own_move("golf", 1, "a900p50"), "s1a900p50");
        assert_eq!(own_move("golf", 1, "s2a900p50"), "s2a900p50");
        assert_eq!(own_move("chess", 1, "e2e4"), "e2e4");
        assert!(move_allowed("golf", &g, "s1a900p50"));
        assert!(!move_allowed("golf", &g, "a900p50"), "koltuksuz vuruş odaya yazılmaz");
        assert!(!move_allowed("golf", &g, "q1"), "çekilme bir vuruş değildir");
        assert_eq!(move_actor("golf", &g, "q2"), Some(2));
        assert_eq!(move_actor("golf", &g, "s2a900p50"), Some(2));
        let b = evaluate("basket", "r", 4, &[]).unwrap();
        assert!(def("basket").unwrap().teams);
        assert_eq!(b.movers, vec![0]);
        assert_eq!(own_move("basket", 0, "a2500p60"), "s0a2500p60");
        assert!(move_allowed("basket", &b, "s0a2500p60"));
        assert_eq!(move_actor("basket", &b, "s0a2500p60"), Some(0));
        assert_eq!(forfeit_move("basket", 3).as_deref(), Some("q3"));
        assert!(evaluate("basket", "r", 3, &[]).is_err());
    }
}
