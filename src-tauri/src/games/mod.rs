//! Oyun kuralları (ağdan ve arayüzden bağımsız, saf fonksiyonlar).
//!
//! Her oyun yalnızca hamle listesinden (`moves`) durumunu hesaplar: oda sahibi her hamleyi
//! `evaluate` ile doğrular, katılımcılar da sahipten gelen hamle listesini aynı fonksiyonla
//! yeniden oynatarak kontrol eder. Böylece kurallar tek yerde durur ve kimse geçersiz hamle
//! dayatamaz.
//!
//! **Yeni oyun eklemek:** `CATALOG`'a bir satır ekleyin ve `evaluate` içinde kural
//! fonksiyonunu çağırın. Oda yönetimi, davet, ağ ve kayıt (bkz. `node/game.rs`) oyundan
//! bağımsızdır; arayüzde de yalnızca tahtanın çizimi oyuna özeldir.

pub mod chess;

use serde::Serialize;
use serde_json::Value;

/// Bir hamle metninin en fazla uzunluğu (satrançta UCI biçimi: `e2e4`, `e7e8q`).
pub const MAX_MOVE_CHARS: usize = 16;
/// Bir oyundaki en fazla hamle sayısı (yarım hamle). Satrançta 50 hamle kuralı bundan çok önce biter.
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
    /// Koltuk adları (ör. Beyaz, Siyah). Oyuncular bu sırayla oturur.
    pub seats: &'static [&'static str],
}

pub const CATALOG: &[GameDef] = &[GameDef {
    id: "chess",
    name: "Satranç",
    description: "İki kişilik klasik satranç. Rok, geçerken alma, terfi, şah mat, pat ve beraberlik kuralları dahil.",
    min_players: 2,
    max_players: 2,
    seats: &["Beyaz", "Siyah"],
}];

pub fn def(id: &str) -> Option<&'static GameDef> {
    CATALOG.iter().find(|g| g.id == id)
}

/// Oyun bitti: `winner` koltuk numarası (berabere ise `None`), `reason` makine okunur sebep
/// (ör. "checkmate", "stalemate", "repetition", "fifty", "material").
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
    /// Sıradaki oyuncunun yapabileceği hamleler.
    pub legal: Vec<String>,
    pub outcome: Option<Outcome>,
    /// Arayüze giden oyuna özel bilgi (satrançta FEN, şah durumu, hamle notasyonu...).
    pub info: Value,
}

/// Hamle metni biçim olarak kabul edilebilir mi (kurallardan önceki ucuz kontrol)?
pub fn valid_move_text(m: &str) -> bool {
    !m.is_empty() && m.len() <= MAX_MOVE_CHARS && m.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Hamle listesini baştan oynatır. Geçersiz bir hamle varsa hata döner.
pub fn evaluate(game: &str, moves: &[String]) -> Result<Eval, String> {
    if moves.len() > MAX_MOVES {
        return Err("çok fazla hamle".into());
    }
    if !moves.iter().all(|m| valid_move_text(m)) {
        return Err("geçersiz hamle".into());
    }
    match game {
        "chess" => chess::evaluate(moves),
        _ => Err("bilinmeyen oyun".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_unknown_game() {
        assert_eq!(def("chess").unwrap().max_players, 2);
        assert!(def("tavla").is_none());
        assert!(evaluate("tavla", &[]).is_err());
        assert!(evaluate("chess", &["e2e4; rm -rf".into()]).is_err());
        let e = evaluate("chess", &[]).unwrap();
        assert_eq!(e.turn, Some(0));
        assert_eq!(e.legal.len(), 20);
    }
}
