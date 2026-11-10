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

pub mod chess;
pub mod golf;

use serde::Serialize;
use serde_json::Value;

/// Bir hamle metninin en fazla uzunluğu (satrançta `e7e8q`, golfte `a1800p65`).
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
    },
    GameDef {
        id: "golf",
        name: "Mini golf",
        description: "2–6 kişilik mini golf: 25 delikten her oyunda rastgele 6 delik. Duvar, kum, su, engeller ve rakipleri yavaşlatan power-up'lar var. En az vuruşla bitiren kazanır.",
        min_players: 2,
        max_players: 6,
        seats: &[],
        draw_offers: false,
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
    /// Sırası gelen koltuk (oyun bittiyse `None`).
    pub turn: Option<usize>,
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
        _ => Err("bilinmeyen oyun".into()),
    }
}

/// Sırası gelen oyuncu bu hamleyi yapabilir mi? (`before`: hamleden önceki durum)
pub fn move_allowed(game: &str, before: &Eval, mv: &str) -> bool {
    match game {
        "golf" => golf::parse_shot(mv).is_some(),
        _ => before.legal.iter().any(|m| m == mv),
    }
}

/// Hamleyi hangi koltuk yaptı? Çoğu hamle sırası gelen oyuncunundur; golfte oyundan çekilme
/// (`q<koltuk>`) sıradan bağımsızdır.
pub fn move_actor(game: &str, before: &Eval, mv: &str) -> Option<usize> {
    match game {
        "golf" => golf::parse_quit(mv).or(before.turn),
        _ => before.turn,
    }
}

/// Çok oyunculu oyunda oyuncunun çekilmesini temsil eden hamle (oyun diğerleriyle sürer).
/// İki kişilik oyunlarda (satranç) `None`: pes eden kaybeder, oyun biter.
pub fn forfeit_move(game: &str, seat: usize) -> Option<String> {
    match game {
        "golf" => Some(golf::quit_move(seat)),
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
        let g = evaluate("golf", "r", 3, &[]).unwrap();
        assert!(move_allowed("golf", &g, "a900p50"));
        assert!(!move_allowed("golf", &g, "q1"), "çekilme bir vuruş değildir");
        assert_eq!(move_actor("golf", &g, "q2"), Some(2));
        assert_eq!(move_actor("golf", &g, "a900p50"), Some(0));
    }
}
