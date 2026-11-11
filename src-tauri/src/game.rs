//! Oyun odaları (`node` modülünün alt modülü): davet, kabul/red, hamleler, beraberlik, pes etme.
//!
//! Akış: bir kişi oyun ve rakip(ler) seçip davet gönderir; bu kişi odanın **sahibi** olur.
//! Davet edilenler kabul edince oda açılır ve oyun başlar. Sunucu yoktur: oda sahibi tek
//! yetkilidir. Her hamle sahibine gider, sahip kurallarla (`crate::games`) doğrular, odayı
//! günceller ve yeni hâlini tüm oyunculara iletir. Sahibin kendi hamleleri doğrudan uygulanır.
//! Çevrimdışı olan oyuncuya bekleyen güncelleme/hamle, çevrimiçi olunca otomatik iletilir.
//!
//! Güvenlik
//! - Oda bilgisi yalnızca sahibinden kabul edilir (TLS sertifika parmak izi), `rev` ile sıralanır.
//! - Oyuncu, sahipten gelen hamle listesini kurallarla baştan oynatıp doğrular; geçmiş yeniden
//!   yazılamaz (yeni liste eskisinin devamı olmalı) ve oyuncunun kendi sırasındaki hamleler
//!   ancak oyuncunun gerçekten gönderdiği hamleler olabilir. Sahip, oyuncu adına "kabul etti",
//!   "pes etti" ya da "beraberliğe razı oldu" diyemez.
//! - Hamle yalnızca odanın oyuncularından, sırası gelen kişiden ve kurallara uygunsa kabul edilir.
//! - Aynı kişiden bekleyen davet sayısı sınırlıdır (davet yağmuruna karşı).

use super::*;
use crate::chat::GroupMember;
use crate::games;

/// Saklanan en fazla oda (bitenler en eskiden başlayarak silinir).
const MAX_ROOMS: usize = 200;
/// Bir kişiden aynı anda bekleyen en fazla davet.
const MAX_INVITES_PER_HOST: usize = 5;
/// Gönderilemeyen güncelleme/hamle bu kadar sonra yeniden denenir.
const GAME_RETRY: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoomStatus {
    /// Davet gönderildi, herkes henüz kabul etmedi.
    #[default]
    Invited,
    Active,
    Finished,
    /// Davet edilenlerden biri reddetti.
    Declined,
    /// Sahip daveti geri çekti.
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GameResult {
    /// Kazananın parmak izi (berabere ise boş).
    #[serde(default)]
    pub winner: Option<String>,
    /// "checkmate" | "stalemate" | "repetition" | "fifty" | "material" | "resign" | "agreement"
    pub reason: String,
    /// Sonucu doğuran oyuncu (pes eden ya da beraberliği kabul eden).
    #[serde(default)]
    pub by: Option<String>,
}

/// Oyunculara gönderilen oda.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GameRoom {
    pub id: String,
    /// Oyun türü (`games::CATALOG`), ör. "chess".
    pub game: String,
    /// Oda sahibi (daveti gönderen).
    pub host: String,
    /// Oyuncular, koltuk sırasıyla (satrançta 0 = beyaz, 1 = siyah).
    pub players: Vec<GroupMember>,
    /// Daveti kabul etmiş olanlar (sahip baştan kabul etmiş sayılır).
    pub joined: Vec<String>,
    pub status: RoomStatus,
    pub moves: Vec<String>,
    #[serde(default)]
    pub result: Option<GameResult>,
    /// Beraberlik teklif eden oyuncu (bekleyen teklif yoksa boş).
    #[serde(default)]
    pub draw_offer: Option<String>,
    pub rev: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// Daveti gönderenin seçtiği tür (arenada harita). Türü olmayan oyunlarda boş.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub variant: String,
    /// Gerçek zamanlı oyunun sonu: koltuk başına (öldürme, ölüm). Sahip maç bitince yazar.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<[u32; 2]>,
}

impl GameRoom {
    fn seat_of(&self, fp: &str) -> Option<usize> {
        self.players.iter().position(|p| p.fingerprint == fp)
    }
    fn name_of(&self, fp: &str) -> String {
        self.players.iter().find(|p| p.fingerprint == fp).map(|p| p.name.clone()).unwrap_or_else(|| "?".into())
    }
}

/// Oyuncunun odadaki hamlesi ya da kararı.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GameAction {
    /// Daveti kabul et.
    Join,
    /// Daveti reddet (sahip için: daveti geri çek).
    Decline,
    Move {
        mv: String,
        /// Hamlenin odadaki sırası (oyuncu gönderirken doldurur). Yanıt kaybolup aynı hamle yeniden
        /// gelirse sahip onu ikinci kez uygulamaz: golfte aynı oyuncu art arda vurabildiği için
        /// yinelenen vuruş eskiden iki kez işleniyor ve oyuncu kalıcı olarak eşzamansız kalıyordu.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<usize>,
    },
    Resign,
    OfferDraw,
    AcceptDraw,
    DeclineDraw,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct RoomState {
    room: GameRoom,
    host_name: String,
    /// (Sahip) güncel odayı henüz almamış oyuncular
    #[serde(default)]
    sync_pending: Vec<String>,
    /// (Oyuncu) sahibine henüz ulaşmamış hamle/kararlar, sırasıyla
    #[serde(default)]
    outbox: Vec<GameAction>,
    /// (Oyuncu) `outbox`'taki her işlemin odaya ekleyeceği ve henüz odada görülmemiş hamlem.
    /// Sahipten gelen odada benim adıma görünen her hamle sırayla bunlardan biri olmalı.
    #[serde(default)]
    outbox_moves: Vec<Option<String>>,
    /// (Oyuncu) sahibine bildirdiğim kararlar: "join", "resign", "draw"
    #[serde(default)]
    claims: Vec<String>,
    /// Listeden kaldırıldı; bekleyen iletimler bitince tamamen silinir.
    #[serde(default)]
    hidden: bool,
    /// Son hesaplanan durum: (oyuncu sayısı, hamle sayısı) → değerlendirme. Hamleler yalnızca
    /// sona eklenir (geçmiş değiştirilemez), bu yüzden bu ikisi değişmedikçe sonuç aynıdır.
    /// Golfte her değerlendirme tüm vuruşların fiziğini yeniden hesaplar; liste her olayda
    /// yeniden üretildiği için bu önbellek olmadan arayüz ve ağ işleri tıkanıyordu.
    #[serde(skip)]
    memo: Option<(usize, usize, Option<games::Eval>)>,
}

impl RoomState {
    /// Listeden kaldırılmış ve işi bitmiş oda tamamen silinebilir. Sahibi olduğum ve hâlâ süren
    /// bir oyun (ör. ben çekildikten sonra diğerlerinin oynamaya devam ettiği golf) silinmez:
    /// silinirse diğer oyuncuların hamleleri "bilinmeyen oyun" diye reddedilir ve oyun donar.
    fn removable(&self, me: &str) -> bool {
        self.hidden
            && self.sync_pending.is_empty()
            && self.outbox.is_empty()
            && !(self.room.host == me && matches!(self.room.status, RoomStatus::Active | RoomStatus::Invited))
    }

    fn eval(&mut self) -> Option<&games::Eval> {
        let key = (self.room.players.len(), self.room.moves.len());
        if self.memo.as_ref().map(|m| (m.0, m.1)) != Some(key) {
            let e = games::evaluate(&self.room.game, &self.room.id, key.0, &self.room.moves).ok();
            self.memo = Some((key.0, key.1, e));
        }
        self.memo.as_ref().and_then(|m| m.2.as_ref())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    #[serde(flatten)]
    pub room: GameRoom,
    /// Oyunun görünen adı, ör. "Satranç".
    pub title: String,
    pub host_name: String,
    pub is_host: bool,
    pub my_seat: Option<usize>,
    /// Sırası gelen koltuk (oyun sürmüyorsa boş).
    pub turn: Option<usize>,
    pub my_turn: bool,
    /// Sıra bendeyse yapabileceğim hamleler.
    pub legal: Vec<String>,
    /// Oyuna özel durum (satrançta FEN, şah, hamle notasyonu).
    pub info: Value,
    /// Hamlem/kararım sahibine iletiliyor.
    pub pending: bool,
    /// Benden bir şey bekleniyor (davet, hamle, beraberlik teklifi).
    pub needs_me: bool,
}

fn reject(reason: &str) -> Message {
    Message::GameReply { ok: false, reason: reason.into(), room: None, host_name: String::new() }
}

fn game_title(id: &str) -> &'static str {
    games::def(id).map(|d| d.name).unwrap_or("Oyun")
}

fn reason_text(reason: &str) -> &'static str {
    match reason {
        "checkmate" => "şah mat",
        "stalemate" => "pat",
        "repetition" => "üç kez tekrar",
        "fifty" => "50 hamle kuralı",
        "material" => "yetersiz malzeme",
        "resign" => "pes etme",
        "agreement" => "anlaşma",
        "score" => "en az vuruş",
        "tie" => "eşit vuruş",
        "forfeit" => "diğerleri çekildi",
        "kills" => "en çok öldürme",
        "kills_tie" => "eşit öldürme",
        "points" => "sayı",
        "tie_points" => "eşit sayı",
        "team_forfeit" => "rakip takım çekildi",
        _ => "oyun bitti",
    }
}

/// Kullanıcıya gösterilecek hata metni.
fn action_error(reason: &str) -> String {
    match reason {
        "not_player" => "Bu oyunda oyuncu değilsiniz",
        "closed" => "Oyun artık sürmüyor",
        "not_your_turn" => "Sıra sizde değil",
        "illegal" => "Kurallara aykırı hamle",
        "no_offer" => "Bekleyen beraberlik teklifi yok",
        _ => "Geçersiz işlem",
    }
    .into()
}

/// "Kazandınız (şah mat)" gibi kısa sonuç satırı.
fn result_line(room: &GameRoom, me: &str) -> String {
    let Some(r) = &room.result else { return "Oyun bitti".into() };
    let why = reason_text(&r.reason);
    if r.reason == "resign" {
        let by = r.by.as_deref().unwrap_or("");
        return if by == me { "Pes ettiniz".into() } else { format!("{} pes etti — kazandınız", room.name_of(by)) };
    }
    if games::def(&room.game).is_some_and(|d| d.teams) {
        // Takımlı oyun: kazanan, takımın ilk koltuğuyla yazılır (0 Kırmızı, 1 Mavi).
        let team = |fp: &str| room.seat_of(fp).map(|s| s % 2);
        return match &r.winner {
            Some(w) if team(w).is_some() && team(w) == team(me) => format!("Takımınız kazandı ({why})"),
            Some(w) => format!("{} takım kazandı ({why})", if team(w) == Some(0) { "Kırmızı" } else { "Mavi" }),
            None => format!("Berabere ({why})"),
        };
    }
    match &r.winner {
        Some(w) if w == me => format!("Kazandınız ({why})"),
        Some(w) => format!("{} kazandı ({why})", room.name_of(w)),
        None => format!("Berabere ({why})"),
    }
}

fn finish(room: &mut GameRoom, winner: Option<String>, reason: &str, by: Option<String>) {
    room.status = RoomStatus::Finished;
    room.result = Some(GameResult { winner, reason: reason.into(), by });
    room.draw_offer = None;
}

/// Odanın kural makinesi (sahipte çalışır; oyuncu da göndermeden önce aynısıyla dener).
/// `Ok(Some(oda))`: yeni hâl, `Ok(None)`: zaten yapılmış (tekrar gelen istek), `Err`: reddedildi.
fn apply(room: &GameRoom, sender: &str, action: &GameAction) -> Result<Option<GameRoom>, &'static str> {
    let seat = room.seat_of(sender).ok_or("not_player")?;
    let mut r = room.clone();
    let n = r.players.len();
    let winner_fp = |r: &GameRoom, s: Option<usize>| s.and_then(|s| r.players.get(s)).map(|p| p.fingerprint.clone());
    let draws = games::def(&r.game).is_some_and(|d| d.draw_offers);
    if matches!(action, GameAction::OfferDraw | GameAction::AcceptDraw) && !draws {
        return Err("invalid");
    }
    match action {
        GameAction::Join => match r.status {
            RoomStatus::Invited if r.joined.iter().any(|f| f == sender) => return Ok(None),
            RoomStatus::Invited => {
                r.joined.push(sender.to_string());
                if r.players.iter().all(|p| r.joined.contains(&p.fingerprint)) {
                    r.status = RoomStatus::Active;
                }
            }
            RoomStatus::Active if r.joined.iter().any(|f| f == sender) => return Ok(None),
            _ => return Err("closed"),
        },
        GameAction::Decline => match r.status {
            RoomStatus::Invited if sender == r.host => r.status = RoomStatus::Cancelled,
            RoomStatus::Invited => {
                // Çok oyunculu oyunda yeterince kişi kalıyorsa reddeden çıkar, oyun diğerleriyle kurulur.
                // Takımlı oyunda takımlar bozulacağından reddeden olursa oyun kurulmaz.
                let min = games::def(&r.game).map_or(2, |d| d.min_players);
                let teams = games::def(&r.game).is_some_and(|d| d.teams);
                if n > min && !teams {
                    r.players.retain(|p| p.fingerprint != sender);
                    r.joined.retain(|f| f != sender);
                    if r.players.iter().all(|p| r.joined.contains(&p.fingerprint)) {
                        r.status = RoomStatus::Active;
                    }
                } else {
                    r.status = RoomStatus::Declined;
                }
            }
            RoomStatus::Declined | RoomStatus::Cancelled => return Ok(None),
            _ => return Err("closed"),
        },
        GameAction::Move { mv, at } => {
            // Golfte vuruşa vuranın koltuğu eklenir (herkes aynı anda vurur).
            let mv = &games::own_move(&r.game, seat, mv);
            // Gönderildiği sıradan bu yana bu oyuncunun aynı hamlesi işlendiyse istek yinelenmiştir
            // (arada başkası oyundan çekildiyse sıra kaymış olabilir).
            if let Some(at) = *at {
                for i in at..r.moves.len() {
                    if r.moves[i] == *mv {
                        let before = games::evaluate(&r.game, &r.id, n, &r.moves[..i]).map_err(|_| "invalid")?;
                        if games::move_actor(&r.game, &before, mv) == Some(seat) {
                            return Ok(None);
                        }
                    }
                }
            }
            if r.status != RoomStatus::Active {
                return Err("closed");
            }
            if !games::valid_move_text(mv) {
                return Err("illegal");
            }
            let now = games::evaluate(&r.game, &r.id, n, &r.moves).map_err(|_| "invalid")?;
            if !now.movers.contains(&seat) {
                return Err("not_your_turn");
            }
            if games::move_actor(&r.game, &now, mv) != Some(seat) || !games::move_allowed(&r.game, &now, mv) {
                return Err("illegal");
            }
            r.moves.push(mv.clone());
            // Hamle yapmak, rakibin bekleyen beraberlik teklifini reddetmek demektir.
            if r.draw_offer.as_deref().is_some_and(|d| d != sender) {
                r.draw_offer = None;
            }
            let after = games::evaluate(&r.game, &r.id, n, &r.moves).map_err(|_| "illegal")?;
            if let Some(o) = after.outcome {
                let winner = winner_fp(&r, o.winner);
                finish(&mut r, winner, o.reason, None);
            }
        }
        GameAction::Resign => match r.status {
            RoomStatus::Active => match games::forfeit_move(&r.game, seat) {
                // Çok oyunculu oyun: oyuncu çekilir, oyun diğerleriyle sürer.
                Some(q) => {
                    r.moves.push(q);
                    let after = games::evaluate(&r.game, &r.id, n, &r.moves).map_err(|_| "closed")?;
                    if let Some(o) = after.outcome {
                        let winner = winner_fp(&r, o.winner);
                        finish(&mut r, winner, o.reason, Some(sender.to_string()));
                    }
                }
                // İki kişilik oyunda diğer oyuncu kazanır.
                None => {
                    let winner = (n == 2).then(|| r.players[1 - seat].fingerprint.clone());
                    finish(&mut r, winner, "resign", Some(sender.to_string()));
                }
            },
            RoomStatus::Invited => return apply(room, sender, &GameAction::Decline),
            _ => return Err("closed"),
        },
        GameAction::OfferDraw => {
            if r.status != RoomStatus::Active {
                return Err("closed");
            }
            match r.draw_offer.clone() {
                Some(d) if d == sender => return Ok(None),
                // Karşı taraf zaten teklif etmişse bu bir kabuldür.
                Some(_) => finish(&mut r, None, "agreement", Some(sender.to_string())),
                None => r.draw_offer = Some(sender.to_string()),
            }
        }
        GameAction::AcceptDraw => {
            if r.status != RoomStatus::Active {
                return Err("closed");
            }
            match r.draw_offer.as_deref() {
                Some(d) if d != sender => finish(&mut r, None, "agreement", Some(sender.to_string())),
                _ => return Err("no_offer"),
            }
        }
        GameAction::DeclineDraw => {
            if r.status != RoomStatus::Active || !r.draw_offer.as_deref().is_some_and(|d| d != sender) {
                return Ok(None);
            }
            r.draw_offer = None;
        }
    }
    Ok(Some(r))
}

/// Ağdan gelen odayı doğrular ve temizler.
fn clean_room(mut r: GameRoom) -> Option<GameRoom> {
    if !valid_message_id(&r.id) || !valid_fingerprint(&r.host) {
        return None;
    }
    let def = games::def(&r.game)?;
    let mut seen = HashSet::new();
    r.players = r
        .players
        .into_iter()
        .filter(|p| valid_fingerprint(&p.fingerprint) && seen.insert(p.fingerprint.clone()))
        .map(|p| GroupMember { name: sanitize_name(&p.name), fingerprint: p.fingerprint })
        .collect();
    if r.players.len() < def.min_players || r.players.len() > def.max_players || r.seat_of(&r.host).is_none() {
        return None;
    }
    let mut seen = HashSet::new();
    let players: HashSet<String> = r.players.iter().map(|p| p.fingerprint.clone()).collect();
    r.joined.retain(|f| players.contains(f) && seen.insert(f.clone()));
    if r.moves.len() > games::MAX_MOVES || !r.moves.iter().all(|m| games::valid_move_text(m)) {
        return None;
    }
    if r.draw_offer.as_ref().is_some_and(|d| !players.contains(d)) {
        r.draw_offer = None;
    }
    // Tür, oyunun tanımladıklarından biri olmalı (türü olmayan oyunda boş).
    if def.variants.is_empty() {
        r.variant.clear();
    } else if !def.variants.iter().any(|v| v.id == r.variant) {
        return None;
    }
    if r.stats.len() > r.players.len() {
        return None;
    }
    if let Some(res) = &mut r.result {
        res.reason = sanitize_text(&res.reason, 24);
        if res.winner.as_ref().is_some_and(|w| !players.contains(w)) || res.by.as_ref().is_some_and(|b| !players.contains(b)) {
            return None;
        }
    }
    Some(r)
}

impl Node {
    pub(super) fn load_games(&self) {
        let list: Vec<RoomState> = crate::store::read_json(&self.games_path).unwrap_or_default();
        let mut map = self.games.lock().unwrap();
        for mut st in list {
            // Eski kayıtlarda `outbox_moves` yoktu: bekleyen hamleler işlemlerden çıkarılır.
            while st.outbox_moves.len() < st.outbox.len() {
                let i = st.outbox_moves.len();
                st.outbox_moves.push(match &st.outbox[i] {
                    GameAction::Move { mv, .. } => Some(mv.clone()),
                    _ => None,
                });
            }
            st.outbox_moves.truncate(st.outbox.len());
            map.insert(st.room.id.clone(), st);
        }
    }

    fn save_games(&self, map: &HashMap<String, RoomState>) {
        let list: Vec<&RoomState> = map.values().collect();
        if let Ok(json) = serde_json::to_vec(&list) {
            let _ = crate::store::write_atomic(&self.games_path, &json);
        }
    }

    fn game_view(&self, st: &mut RoomState) -> GameView {
        let me = &self.identity.fingerprint;
        let eval = st.eval().cloned();
        let room = &st.room;
        let seat = room.seat_of(me);
        let active = room.status == RoomStatus::Active;
        let turn = if active { eval.as_ref().and_then(|e| e.turn) } else { None };
        let pending = !st.outbox.is_empty();
        let can_move = active && seat.is_some_and(|s| eval.as_ref().is_some_and(|e| e.movers.contains(&s)));
        let my_turn = can_move && !pending;
        let invited = room.status == RoomStatus::Invited && seat.is_some() && !room.joined.contains(me) && !pending;
        let draw_for_me = active && !pending && room.draw_offer.as_ref().is_some_and(|d| d != me);
        GameView {
            room: room.clone(),
            title: game_title(&room.game).into(),
            host_name: st.host_name.clone(),
            is_host: room.host == *me,
            my_seat: seat,
            turn,
            my_turn,
            legal: if my_turn { eval.as_ref().map(|e| e.legal.clone()).unwrap_or_default() } else { Vec::new() },
            info: eval.map(|e| e.info).unwrap_or(Value::Null),
            pending,
            needs_me: invited || my_turn || draw_for_me,
        }
    }

    pub fn game_list(&self) -> Vec<GameView> {
        let mut map = self.games.lock().unwrap();
        let mut list: Vec<GameView> = map.values_mut().filter(|s| !s.hidden).map(|s| self.game_view(s)).collect();
        list.sort_by(|a, b| b.room.updated_at.cmp(&a.room.updated_at));
        list
    }

    /// Tüm liste (oda silindiğinde). Diğer değişikliklerde yalnızca o oda gönderilir (`emit_game`).
    fn emit_games(&self) {
        self.emit("games", json!(self.game_list()));
    }

    /// Tek bir odanın güncel hâli; oda listeden kalktıysa tüm liste gider.
    fn emit_game(&self, id: &str) {
        let view = {
            let mut map = self.games.lock().unwrap();
            map.get_mut(id).filter(|s| !s.hidden).map(|s| self.game_view(s))
        };
        match view {
            Some(v) => self.emit("game", json!(v)),
            None => self.emit_games(),
        }
    }

    fn notify_game(&self, room_id: &str, title: &str, body: &str) {
        self.notify_to(title, body, json!({ "view": "games", "id": room_id }));
    }

    /// (Sahip) oda değişti: sürümü artırır, diğer oyunculara gönderilmek üzere işaretler.
    fn touch_room(&self, st: &mut RoomState) {
        let me = &self.identity.fingerprint;
        st.room.rev += 1;
        st.room.updated_at = now_ms();
        st.sync_pending = st.room.players.iter().map(|p| p.fingerprint.clone()).filter(|f| f != me).collect();
    }

    /// Oda sayısı sınırı: en eski bitmiş/kapalı odalar silinir.
    fn prune_games(map: &mut HashMap<String, RoomState>) {
        if map.len() <= MAX_ROOMS {
            return;
        }
        let mut closed: Vec<(u64, String)> = map
            .values()
            .filter(|s| !matches!(s.room.status, RoomStatus::Invited | RoomStatus::Active) && s.sync_pending.is_empty() && s.outbox.is_empty())
            .map(|s| (s.room.updated_at, s.room.id.clone()))
            .collect();
        closed.sort();
        for (_, id) in closed.into_iter().take(map.len() - MAX_ROOMS) {
            map.remove(&id);
        }
    }

    /// Odanın güncel hâli (gerçek zamanlı oyunlar için).
    pub(super) fn game_room(&self, id: &str) -> Option<GameRoom> {
        self.games.lock().unwrap().get(id).map(|s| s.room.clone())
    }

    /// (Sahip) gerçek zamanlı maç bitti: sonucu ve skorları odaya yazar, herkese iletir.
    pub(super) fn game_finish_realtime(self: &Arc<Self>, id: &str, winner: Option<usize>, reason: &str, stats: Vec<[u32; 2]>) {
        let me = self.identity.fingerprint.clone();
        let notice = {
            let mut map = self.games.lock().unwrap();
            let Some(st) = map.get_mut(id) else { return };
            if st.room.host != me || st.room.status != RoomStatus::Active {
                return;
            }
            let w = winner.and_then(|s| st.room.players.get(s)).map(|p| p.fingerprint.clone());
            st.room.stats = stats;
            finish(&mut st.room, w, reason, None);
            self.touch_room(st);
            self.save_games(&map);
            let st = &map[id];
            (!st.hidden).then(|| (format!("{} bitti", game_title(&st.room.game)), result_line(&st.room, &me)))
        };
        self.flush_games();
        self.emit_game(id);
        if let Some((t, b)) = notice {
            self.notify_game(id, &t, &b);
        }
    }

    // ───────────── Kullanıcı işlemleri ─────────────

    /// Oyun daveti gönderir. `my_seat`: sahibin koltuğu (satrançta 0 beyaz, 1 siyah); boşsa rastgele.
    /// Uygulama doğrudan `game_invite_variant`'ı çağırır; bu kısayol testler içindir.
    #[cfg(test)]
    pub fn game_invite(self: &Arc<Self>, game: &str, opponents: &[String], my_seat: Option<usize>) -> Result<GameView, String> {
        self.game_invite_variant(game, opponents, my_seat, None)
    }

    /// `game_invite` gibi; `variant`: oyunun türlerinden biri (arenada harita), boşsa ilki.
    pub fn game_invite_variant(
        self: &Arc<Self>,
        game: &str,
        opponents: &[String],
        my_seat: Option<usize>,
        variant: Option<&str>,
    ) -> Result<GameView, String> {
        let def = games::def(game).ok_or("Bilinmeyen oyun")?;
        let variant = match (def.variants.first(), variant) {
            (None, _) => String::new(),
            (Some(first), None) => first.id.to_string(),
            (Some(_), Some(v)) => def.variants.iter().find(|x| x.id == v).ok_or("Bilinmeyen harita")?.id.to_string(),
        };
        let me = self.identity.fingerprint.clone();
        let my_name = self.device_info().name;
        let mut others: Vec<GroupMember> = Vec::new();
        for fp in opponents {
            if *fp == me || !valid_fingerprint(fp) || others.iter().any(|o| o.fingerprint == *fp) {
                continue;
            }
            if self.is_blocked(fp) {
                return Err("Engellediğiniz bir kişiyi davet edemezsiniz".into());
            }
            let name = self.name_for(fp).ok_or("Davet edilen kişi bulunamadı")?;
            others.push(GroupMember { fingerprint: fp.clone(), name });
        }
        let total = others.len() + 1;
        if total < def.min_players || total > def.max_players {
            let need = if def.min_players == def.max_players {
                format!("{} rakip", def.min_players - 1)
            } else {
                format!("{}–{} rakip", def.min_players - 1, def.max_players - 1)
            };
            return Err(format!("{} için {need} seçin", def.name));
        }
        if def.teams && total % 2 != 0 {
            return Err(format!("{} için takımlar eşit olmalı (1v1, 2v2 ya da 3v3)", def.name));
        }
        let seat = match my_seat {
            Some(s) if s < total => s,
            _ => uuid::Uuid::new_v4().as_bytes()[0] as usize % total,
        };
        let mut players = others;
        players.insert(seat, GroupMember { fingerprint: me.clone(), name: my_name.clone() });
        let now = now_ms();
        let room = GameRoom {
            id: uuid::Uuid::new_v4().to_string(),
            game: def.id.into(),
            host: me.clone(),
            players,
            joined: vec![me.clone()],
            status: RoomStatus::Invited,
            moves: Vec::new(),
            result: None,
            draw_offer: None,
            rev: 1,
            created_at: now,
            updated_at: now,
            variant,
            stats: Vec::new(),
        };
        let mut st = RoomState {
            sync_pending: room.players.iter().map(|p| p.fingerprint.clone()).filter(|f| *f != me).collect(),
            room,
            host_name: my_name,
            ..Default::default()
        };
        let view = self.game_view(&mut st);
        let id = st.room.id.clone();
        {
            let mut map = self.games.lock().unwrap();
            map.insert(id.clone(), st);
            Self::prune_games(&mut map);
            self.save_games(&map);
        }
        self.emit_game(&id);
        self.flush_games();
        Ok(view)
    }

    /// Kullanıcının hamlesi/kararı. Sahipte hemen uygulanır; oyuncuda sahibine gönderilir.
    pub fn game_act(self: &Arc<Self>, id: &str, action: GameAction) -> Result<(), String> {
        {
            let mut map = self.games.lock().unwrap();
            let st = map.get_mut(id).ok_or("Oyun bulunamadı")?;
            self.act_locked(st, action)?;
            self.save_games(&map);
        }
        // Önce ağa gönder, sonra arayüzü güncelle: hamle bir an önce karşıya ulaşsın.
        self.flush_games();
        self.emit_game(id);
        Ok(())
    }

    fn act_locked(&self, st: &mut RoomState, action: GameAction) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        if st.room.host == me {
            if let Some(next) = apply(&st.room, &me, &action).map_err(action_error)? {
                st.room = next;
                self.touch_room(st);
            }
            return Ok(());
        }
        // Önce aynı kurallarla dene (sahip de aynısını uygular); bekleyenler de hesaba katılır.
        let mut sim = st.room.clone();
        for a in &st.outbox {
            if let Ok(Some(n)) = apply(&sim, &me, a) {
                sim = n;
            }
        }
        let mut action = action;
        if let GameAction::Move { at, .. } = &mut action {
            *at = Some(sim.moves.len());
        }
        let Some(next) = apply(&sim, &me, &action).map_err(action_error)? else { return Ok(()) };
        match &action {
            GameAction::Join => st.claims.push("join".into()),
            GameAction::Resign => st.claims.push("resign".into()),
            GameAction::OfferDraw | GameAction::AcceptDraw => st.claims.push("draw".into()),
            _ => {}
        }
        // Bu işlemin odaya ekleyeceği hamle (vuruş, satranç hamlesi ya da golften çekilme).
        st.outbox_moves.push(next.moves.get(sim.moves.len()).cloned());
        st.outbox.push(action);
        Ok(())
    }

    /// Oyunu listeden kaldırır. Bekleyen davet reddedilir/geri çekilir, süren oyunda pes edilir.
    pub fn game_remove(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        {
            let mut map = self.games.lock().unwrap();
            let st = map.get_mut(id).ok_or("Oyun bulunamadı")?;
            if st.room.seat_of(&me).is_some() {
                let action = match st.room.status {
                    RoomStatus::Invited => Some(GameAction::Decline),
                    RoomStatus::Active => Some(GameAction::Resign),
                    _ => None,
                };
                if let Some(a) = action {
                    let _ = self.act_locked(st, a);
                }
            }
            st.hidden = true;
            if st.removable(&me) {
                map.remove(id);
            }
            self.save_games(&map);
        }
        self.emit_games();
        self.flush_games();
        Ok(())
    }

    // ───────────── Teslimat ─────────────

    /// Bekleyen oda güncellemelerini ve hamleleri çevrimiçi olanlara gönderir. Her değişiklikten
    /// hemen sonra ve keşif döngüsünde (yeniden deneme için) çağrılır.
    pub fn flush_games(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let me = self.identity.fingerprint.clone();
        let online: HashMap<String, Peer> =
            self.peers().into_iter().filter(|p| !p.blocked).map(|p| (p.fingerprint.clone(), p)).collect();
        let mut jobs: Vec<(Peer, String, Message)> = Vec::new();
        {
            let mut map = self.games.lock().unwrap();
            let before = map.len();
            map.retain(|_, s| !s.removable(&me));
            if map.len() != before {
                self.save_games(&map);
            }
            for st in map.values() {
                if st.room.host == me {
                    for fp in &st.sync_pending {
                        if let Some(p) = online.get(fp) {
                            let msg = Message::GameUpdate { device: self.device_info(), room: st.room.clone(), host_name: st.host_name.clone() };
                            jobs.push((p.clone(), format!("game:{}:sync:{fp}:{}", st.room.id, st.room.rev), msg));
                        }
                    }
                } else if let (Some(action), Some(p)) = (st.outbox.first(), online.get(&st.room.host)) {
                    let msg = Message::GameAction { device: self.device_info(), room_id: st.room.id.clone(), action: action.clone() };
                    jobs.push((p.clone(), format!("game:{}:act", st.room.id), msg));
                }
            }
        }
        for (peer, flight, msg) in jobs {
            if self.outbox_backoff.lock().unwrap().get(&flight).is_some_and(|until| Instant::now() < *until) {
                continue;
            }
            if !self.outbox_in_flight.lock().unwrap().insert(flight.clone()) {
                continue;
            }
            let node = self.clone();
            tokio::spawn(async move {
                let reply = node.game_request(&peer, &msg).await;
                let (delivered, again) = match (&msg, reply) {
                    (Message::GameUpdate { room, .. }, Ok(Message::GameReply { .. })) => {
                        // Diske yazılmaz: uygulama kapanırsa güncelleme yeniden gider (sürümüyle
                        // tanınır, zararsız). Her vuruşta tüm oyunları diske yazmak gereksiz yüktü.
                        let mut map = node.games.lock().unwrap();
                        if let Some(st) = map.get_mut(&room.id) {
                            if st.room.rev == room.rev {
                                st.sync_pending.retain(|f| *f != peer.fingerprint);
                            }
                        }
                        (true, false)
                    }
                    (Message::GameAction { room_id, action, .. }, Ok(Message::GameReply { ok, reason, room, host_name })) => {
                        let again = node.on_action_reply(&peer.fingerprint, room_id, action, ok, &reason, room, &host_name);
                        node.emit_game(room_id);
                        (true, again)
                    }
                    _ => (false, false),
                };
                if delivered {
                    node.outbox_backoff.lock().unwrap().remove(&flight);
                } else {
                    node.outbox_backoff.lock().unwrap().insert(flight.clone(), Instant::now() + GAME_RETRY);
                }
                node.outbox_in_flight.lock().unwrap().remove(&flight);
                if again {
                    node.flush_games();
                } else if !delivered {
                    // Keşif döngüsünü (2,5 sn) beklemeden kısa süre sonra yeniden dene.
                    let n2 = node.clone();
                    tokio::spawn(async move {
                        sleep(GAME_RETRY + Duration::from_millis(50)).await;
                        n2.flush_games();
                    });
                }
            });
        }
    }

    /// (Oyuncu) sahibin hamle/karar yanıtını işler. Gönderilecek başka iş kaldıysa `true`.
    #[allow(clippy::too_many_arguments)]
    fn on_action_reply(
        &self,
        host: &str,
        room_id: &str,
        action: &GameAction,
        ok: bool,
        reason: &str,
        room: Option<GameRoom>,
        host_name: &str,
    ) -> bool {
        // Önce sahibin gönderdiği güncel oda (kendi hamlem dahil) işlenir — aynı doğrulamalardan
        // geçer ve hamlem bekleyenlerden onu bulur; sonra işlem kuyruktan çıkar.
        if let Some(room) = room {
            let _ = self.accept_game_update(host, room, host_name);
        }
        let mut map = self.games.lock().unwrap();
        let Some(st) = map.get_mut(room_id) else { return false };
        if st.outbox.first() == Some(action) {
            st.outbox.remove(0);
            if !st.outbox_moves.is_empty() {
                // Onaylanmadıysa (reddedildi) bu işlemin hamlesi artık hiç gelmeyecek.
                st.outbox_moves.remove(0);
            }
        }
        // Reddettiğim davet: sahip beni oyunculardan çıkarmış olabilir.
        if !ok && reason == "not_player" && st.room.status == RoomStatus::Invited {
            st.room.status = RoomStatus::Declined;
        }
        let again = !st.outbox.is_empty();
        self.save_games(&map);
        drop(map);
        // Tekrar gelen istekler ("sıra sizde değil", "oyun bitti") oda güncellenince kendiliğinden
        // anlaşılır; yalnızca gerçek hatalar kullanıcıya gösterilir.
        if !ok && !matches!(reason, "closed" | "not_your_turn") {
            self.emit("game-error", json!({ "id": room_id, "message": action_error(reason) }));
        }
        again
    }

    async fn game_request(&self, peer: &Peer, msg: &Message) -> Result<Message, String> {
        let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut stream, msg).await.map_err(|e| e.to_string())?;
        let reply = timeout(Duration::from_secs(8), read_msg(&mut stream))
            .await
            .map_err(|_| "yanıt yok".to_string())?
            .map_err(|e| e.to_string())?;
        let _ = stream.shutdown().await;
        Ok(reply)
    }

    // ───────────── Ağdan gelenler ─────────────

    /// (Oyuncu) sahipten gelen oda.
    pub(super) fn accept_game_update(&self, sender: &str, room: GameRoom, host_name: &str) -> Message {
        if self.is_blocked(sender) {
            return reject("blocked");
        }
        let Some(room) = clean_room(room) else { return reject("invalid") };
        if room.host != sender {
            return reject("not_owner");
        }
        let me = self.identity.fingerprint.clone();
        let Some(my_seat) = room.seat_of(&me) else {
            // Oyunculardan çıkmışım (çok oyunculu davette reddettim): odayı bende kapat.
            let mut map = self.games.lock().unwrap();
            if let Some(st) = map.get_mut(&room.id) {
                if st.room.host == room.host && room.rev > st.room.rev && st.room.seat_of(&me).is_some() {
                    st.room.rev = room.rev;
                    st.room.status = RoomStatus::Declined;
                    st.outbox.clear();
                    st.outbox_moves.clear();
                    self.save_games(&map);
                    drop(map);
                    self.emit_game(&room.id);
                }
            }
            return Message::GameReply { ok: true, reason: String::new(), room: None, host_name: String::new() };
        };
        let n = room.players.len();
        let Ok(eval) = games::evaluate(&room.game, &room.id, n, &room.moves) else { return reject("invalid") };
        // Durum hamlelerle tutarlı olmalı: bitmiş oyun "sürüyor", başlamamış oyun "hamleli" olamaz.
        if (room.status == RoomStatus::Active && eval.outcome.is_some())
            || (room.status == RoomStatus::Invited && !room.moves.is_empty())
        {
            return reject("invalid");
        }
        let host_name = sanitize_name(host_name);
        let title = game_title(&room.game);
        let room_id = room.id.clone();
        let mut notify: Option<(String, String)> = None;
        {
            let mut map = self.games.lock().unwrap();
            match map.get_mut(&room.id) {
                Some(st) => {
                    if st.room.host != room.host {
                        return reject("not_owner");
                    }
                    // Oyun türü ve harita sonradan değiştirilemez (ör. satranç odası golfe çevrilemez).
                    if st.room.game != room.game || st.room.variant != room.variant {
                        return reject("invalid");
                    }
                    if room.rev <= st.room.rev {
                        return Message::GameReply { ok: true, reason: String::new(), room: None, host_name: String::new() };
                    }
                    if !room.moves.starts_with(&st.room.moves) {
                        return reject("invalid");
                    }
                    // Benim adıma görünen yeni hamleler, sırasıyla benim gönderdiklerim olmalı.
                    let mut mine = st.outbox_moves.iter().enumerate().filter_map(|(i, m)| m.as_ref().map(|m| (i, m)));
                    let mut confirmed = Vec::new();
                    for ply in st.room.moves.len()..room.moves.len() {
                        let Ok(before) = games::evaluate(&room.game, &room.id, n, &room.moves[..ply]) else { return reject("invalid") };
                        if games::move_actor(&room.game, &before, &room.moves[ply]) == Some(my_seat) {
                            match mine.next() {
                                Some((i, m)) if *m == room.moves[ply] => confirmed.push(i),
                                _ => return reject("forged"),
                            }
                        }
                    }
                    let claimed = |c: &str| st.claims.iter().any(|x| x == c);
                    if room.joined.contains(&me) && !st.room.joined.contains(&me) && !claimed("join") {
                        return reject("forged");
                    }
                    if let Some(r) = &room.result {
                        if (r.reason == "resign" && r.by.as_deref() == Some(&me) && !claimed("resign"))
                            || (r.reason == "agreement" && !claimed("draw"))
                        {
                            return reject("forged");
                        }
                    }
                    if !st.hidden {
                        notify = self.update_notice(&st.room, &room, &me, &eval);
                    }
                    for i in confirmed {
                        st.outbox_moves[i] = None;
                    }
                    st.room = room;
                    st.host_name = host_name;
                }
                None => {
                    // Yalnızca yeni davetler kabul edilir (silinmiş oda geri gelmez).
                    if room.status != RoomStatus::Invited || room.joined.contains(&me) {
                        return Message::GameReply { ok: true, reason: String::new(), room: None, host_name: String::new() };
                    }
                    let waiting = map
                        .values()
                        .filter(|s| s.room.host == sender && s.room.status == RoomStatus::Invited && !s.room.joined.contains(&me))
                        .count();
                    if waiting >= MAX_INVITES_PER_HOST {
                        return reject("rate");
                    }
                    notify = Some((format!("Oyun daveti · {host_name}"), format!("{title} oynamak istiyor")));
                    map.insert(room.id.clone(), RoomState { room, host_name, ..Default::default() });
                    Self::prune_games(&mut map);
                }
            }
            self.save_games(&map);
        }
        self.emit_game(&room_id);
        if let Some((t, b)) = notify {
            self.notify_game(&room_id, &t, &b);
        }
        Message::GameReply { ok: true, reason: String::new(), room: None, host_name: String::new() }
    }

    /// Oyuncu tarafında, sahipten gelen değişiklik için bildirim metni.
    fn update_notice(&self, old: &GameRoom, new: &GameRoom, me: &str, eval: &games::Eval) -> Option<(String, String)> {
        let title = game_title(&new.game);
        let host = new.name_of(&new.host);
        if new.status == RoomStatus::Finished && old.status != RoomStatus::Finished {
            return Some((format!("{title} bitti"), result_line(new, me)));
        }
        if new.status == RoomStatus::Cancelled && old.status == RoomStatus::Invited {
            return Some((format!("{title} daveti geri çekildi"), host));
        }
        if new.status == RoomStatus::Active && old.status == RoomStatus::Invited {
            let others: Vec<String> = new.players.iter().filter(|p| p.fingerprint != me).map(|p| p.name.clone()).collect();
            return Some((format!("{title} başladı"), format!("Oyuncular: {}", others.join(", "))));
        }
        if new.draw_offer.is_some() && new.draw_offer != old.draw_offer && new.draw_offer.as_deref() != Some(me) {
            return Some(("Beraberlik teklifi".into(), format!("{host} beraberlik teklif ediyor")));
        }
        if new.moves.len() > old.moves.len() && new.status == RoomStatus::Active {
            return my_move_notice(old, new, eval, me);
        }
        None
    }

    /// (Sahip) oyuncudan gelen hamle/karar. Yanıt her durumda odanın güncel hâlini taşır.
    pub(super) fn accept_game_action(self: &Arc<Self>, sender: &str, room_id: &str, action: GameAction) -> Message {
        if self.is_blocked(sender) {
            return reject("blocked");
        }
        let me = self.identity.fingerprint.clone();
        let mut changed = false;
        let mut notify: Option<(String, String)> = None;
        let reply = {
            let mut map = self.games.lock().unwrap();
            let Some(st) = map.get_mut(room_id) else { return reject("unknown_game") };
            if st.room.host != me {
                return reject("not_owner");
            }
            if st.room.seat_of(sender).is_none() {
                return reject("not_player");
            }
            let (ok, reason) = match apply(&st.room, sender, &action) {
                Ok(Some(next)) => {
                    let old = std::mem::replace(&mut st.room, next);
                    self.touch_room(st);
                    // Yanıtla birlikte gidecek; ayrıca göndermeye gerek yok.
                    st.sync_pending.retain(|f| f != sender);
                    changed = true;
                    if !st.hidden {
                        notify = self.action_notice(&old, &st.room, sender, &action, &me);
                    }
                    (true, String::new())
                }
                Ok(None) => (true, String::new()),
                Err(r) => (false, r.to_string()),
            };
            // Yanıttan önce kaydedilir: sahip yanıttan hemen sonra kapanırsa yeniden açıldığında
            // aynı sürüm numarasını farklı içerikle kullanıp oyuncuyu eşzamansız bırakmasın.
            if changed {
                self.save_games(&map);
            }
            let st = &map[room_id];
            Message::GameReply { ok, reason, room: Some(st.room.clone()), host_name: st.host_name.clone() }
        };
        if changed {
            // Yanıt hemen gitsin: diğer oyunculara iletim, bildirim ve arayüz güncellemesi
            // yanıtı bekletmez.
            self.flush_games();
            let node = self.clone();
            let id = room_id.to_string();
            tokio::spawn(async move {
                if let Some((t, b)) = notify {
                    node.notify_game(&id, &t, &b);
                }
                node.emit_game(&id);
            });
        }
        reply
    }

    /// Sahip tarafında, oyuncunun işlemi için bildirim metni.
    fn action_notice(&self, old: &GameRoom, new: &GameRoom, sender: &str, action: &GameAction, me: &str) -> Option<(String, String)> {
        let title = game_title(&new.game);
        // Reddeden oyuncu yeni odada olmayabilir; adı eski odadan alınır.
        let who = old.name_of(sender);
        if new.status == RoomStatus::Finished && old.status != RoomStatus::Finished {
            return Some((format!("{title} bitti"), result_line(new, me)));
        }
        match action {
            GameAction::Join if new.status == RoomStatus::Active => Some((format!("{who} davetinizi kabul etti"), format!("{title} başladı"))),
            GameAction::Decline if new.status == RoomStatus::Active => {
                Some((format!("{who} davetinizi reddetti"), format!("{title} diğer oyuncularla başladı")))
            }
            GameAction::Decline => Some((format!("{who} davetinizi reddetti"), title.into())),
            GameAction::Resign => Some((format!("{who} oyundan çekildi"), title.into())),
            GameAction::OfferDraw => Some(("Beraberlik teklifi".into(), format!("{who} beraberlik teklif ediyor"))),
            GameAction::Move { .. } => {
                let eval = games::evaluate(&new.game, &new.id, new.players.len(), &new.moves).ok()?;
                my_move_notice(old, new, &eval, me)
            }
            _ => None,
        }
    }
}

/// Hamle yapma sırası bana yeni geldiyse bildirim: satrançta "Sıra sizde", golfte (herkes aynı
/// anda oynadığı için) yalnızca yeni delik başlarken.
fn my_move_notice(old: &GameRoom, new: &GameRoom, eval: &games::Eval, me: &str) -> Option<(String, String)> {
    let seat = new.seat_of(me)?;
    if !eval.movers.contains(&seat) {
        return None;
    }
    let was = games::evaluate(&old.game, &old.id, old.players.len(), &old.moves).ok();
    if was.as_ref().is_some_and(|w| w.movers.contains(&seat)) {
        return None;
    }
    let title = game_title(&new.game);
    if eval.turn.is_none() {
        let hole = eval.info["hole"].as_u64().unwrap_or(0) + 1;
        return Some((format!("{title} · {hole}. delik başladı"), "Herkes aynı anda vuruyor".into()));
    }
    Some((format!("Sıra sizde · {title}"), last_move_text(new, eval)))
}

/// Bildirimde son hamlenin kısa anlatımı: satrançta "Ali: Vxf7", golfte "Ali vurdu · 3. delik".
fn last_move_text(room: &GameRoom, eval: &games::Eval) -> String {
    if let Some(san) = eval.info["san"].as_array().and_then(|a| a.last()).and_then(|s| s.as_str()) {
        let mover = room.players.get(1 - room.moves.len() % 2).map(|p| p.name.clone()).unwrap_or_default();
        return format!("{mover}: {san}");
    }
    let shot = &eval.info["lastShot"];
    let who = shot["seat"].as_u64().and_then(|s| room.players.get(s as usize)).map(|p| p.name.clone()).unwrap_or_default();
    if room.game == "basket" {
        let round = shot["round"].as_u64().unwrap_or(0) + 1;
        let what = if shot["made"].as_bool() == Some(true) { format!("+{} sayı", shot["points"]) } else { "kaçırdı".into() };
        return format!("{who} {what} · {round}. tur");
    }
    let hole = eval.info["hole"].as_u64().unwrap_or(0) + 1;
    format!("{who} vurdu · {hole}. delik")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(fp: &str, name: &str) -> GroupMember {
        GroupMember { fingerprint: fp.into(), name: name.into() }
    }

    fn room() -> GameRoom {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        GameRoom {
            id: "r1".into(),
            game: "chess".into(),
            host: a.clone(),
            players: vec![member(&a, "Ali"), member(&b, "Banu")],
            joined: vec![a],
            status: RoomStatus::Invited,
            rev: 1,
            ..Default::default()
        }
    }

    #[test]
    fn room_state_machine() {
        let (a, b, c) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let r = room();
        assert_eq!(apply(&r, &c, &GameAction::Join), Err("not_player"));
        assert_eq!(apply(&r, &b, &GameAction::Move { mv: "e2e4".into(), at: None }), Err("closed"));
        let r = apply(&r, &b, &GameAction::Join).unwrap().unwrap();
        assert_eq!(r.status, RoomStatus::Active);
        assert_eq!(apply(&r, &b, &GameAction::Join), Ok(None), "tekrar gelen kabul");
        // Beyaz (koltuk 0) Ali başlar.
        assert_eq!(apply(&r, &b, &GameAction::Move { mv: "e7e5".into(), at: None }), Err("not_your_turn"));
        assert_eq!(apply(&r, &a, &GameAction::Move { mv: "e2e5".into(), at: None }), Err("illegal"));
        let r = apply(&r, &a, &GameAction::Move { mv: "e2e4".into(), at: None }).unwrap().unwrap();
        // Beraberlik: teklif → hamle teklifi düşürür → yeni teklif → kabul.
        let r = apply(&r, &a, &GameAction::OfferDraw).unwrap().unwrap();
        assert_eq!(r.draw_offer.as_deref(), Some(a.as_str()));
        assert_eq!(apply(&r, &a, &GameAction::AcceptDraw), Err("no_offer"), "kendi teklifini kabul edemez");
        let r = apply(&r, &b, &GameAction::Move { mv: "e7e5".into(), at: None }).unwrap().unwrap();
        assert!(r.draw_offer.is_none());
        let r2 = apply(&r, &b, &GameAction::OfferDraw).unwrap().unwrap();
        let done = apply(&r2, &a, &GameAction::AcceptDraw).unwrap().unwrap();
        assert_eq!(done.status, RoomStatus::Finished);
        assert_eq!(done.result.as_ref().unwrap().reason, "agreement");
        // Pes etme: diğer oyuncu kazanır.
        let done = apply(&r, &b, &GameAction::Resign).unwrap().unwrap();
        assert_eq!(done.result.as_ref().unwrap().winner.as_deref(), Some(a.as_str()));
        assert_eq!(result_line(&done, &a), "Banu pes etti — kazandınız");
        assert_eq!(apply(&done, &a, &GameAction::Move { mv: "d2d4".into(), at: None }), Err("closed"));
        // Mat ile biten hamle oyunu bitirir.
        let mut r = r;
        for (who, mv) in [(&a, "d1h5"), (&b, "b8c6"), (&a, "f1c4"), (&b, "g8f6"), (&a, "h5f7")] {
            r = apply(&r, who, &GameAction::Move { mv: mv.into(), at: None }).unwrap().unwrap();
        }
        assert_eq!(r.status, RoomStatus::Finished);
        assert_eq!(result_line(&r, &b), "Ali kazandı (şah mat)");
        // Davet reddi / geri çekme.
        assert_eq!(apply(&room(), &b, &GameAction::Decline).unwrap().unwrap().status, RoomStatus::Declined);
        assert_eq!(apply(&room(), &a, &GameAction::Decline).unwrap().unwrap().status, RoomStatus::Cancelled);
    }

    /// Golfte herkes aynı anda vurur. Yanıtı kaybolan vuruş yeniden gelirse, arada başkaları
    /// vurmuş olsa bile ikinci kez işlenmez; kimse başkası adına vuramaz.
    #[test]
    fn simultaneous_golf_shots() {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let mut r = room();
        r.game = "golf".into();
        r.status = RoomStatus::Active;
        r.joined = vec![a.clone(), b.clone()];
        let shot = |at| GameAction::Move { mv: "a0p1".into(), at: Some(at) };
        // İkisi de sıra beklemeden vurur; vuruş vuranın koltuğuyla yazılır.
        let r = apply(&r, &b, &shot(0)).unwrap().unwrap();
        let r = apply(&r, &a, &shot(0)).unwrap().unwrap();
        assert_eq!(r.moves, vec!["s1a0p1", "s0a0p1"]);
        assert_eq!(apply(&r, &b, &shot(0)), Ok(None), "aynı vuruş yinelendi, yeniden işlenmemeli");
        assert_eq!(apply(&r, &b, &shot(1)).unwrap().unwrap().moves.len(), 3, "yeni vuruş işlenir");
        let forged = GameAction::Move { mv: "s0a0p1".into(), at: Some(2) };
        assert_eq!(apply(&r, &b, &forged), Err("illegal"), "başkası adına vuruş");
        // Banu deliği bitirince (8 vuruş) Ali bitirene kadar bekler.
        let mut r = r;
        for _ in 0..7 {
            r = apply(&r, &b, &shot(r.moves.len())).unwrap().unwrap();
        }
        let e = games::evaluate("golf", &r.id, 2, &r.moves).unwrap();
        assert_eq!(e.movers, vec![0]);
        assert_eq!(e.info["hole"], 0);
        assert_eq!(apply(&r, &b, &shot(r.moves.len())), Err("not_your_turn"));
        for _ in 0..7 {
            r = apply(&r, &a, &shot(r.moves.len())).unwrap().unwrap();
        }
        // Herkes bitirdi: 2. delik ikisi için birlikte başlar.
        let e = games::evaluate("golf", &r.id, 2, &r.moves).unwrap();
        assert_eq!(e.info["hole"], 1);
        assert_eq!(e.movers, vec![0, 1]);
    }

    /// Takımlı oyunda reddeden olursa takımlar bozulacağından oyun kurulmaz; sonuç takımca yazılır.
    #[test]
    fn team_game_rules() {
        let (a, b, c, d) = ("a".repeat(64), "b".repeat(64), "c".repeat(64), "d".repeat(64));
        let mut r = room();
        r.game = "basket".into();
        r.players = vec![member(&a, "Ali"), member(&b, "Banu"), member(&c, "Can"), member(&d, "Deniz")];
        let declined = apply(&r, &c, &GameAction::Decline).unwrap().unwrap();
        assert_eq!(declined.status, RoomStatus::Declined);
        assert_eq!(declined.players.len(), 4);
        // Mavi (1, 3) kazandı.
        let mut done = r.clone();
        finish(&mut done, Some(b.clone()), "points", None);
        assert_eq!(result_line(&done, &d), "Takımınız kazandı (sayı)");
        assert_eq!(result_line(&done, &a), "Mavi takım kazandı (sayı)");
        // Sırası gelmeyen atamaz; atış atanın koltuğuyla yazılır.
        r.status = RoomStatus::Active;
        r.joined = r.players.iter().map(|p| p.fingerprint.clone()).collect();
        let shot = GameAction::Move { mv: "a900p1".into(), at: Some(0) };
        assert_eq!(apply(&r, &b, &shot), Err("not_your_turn"));
        let r = apply(&r, &a, &shot).unwrap().unwrap();
        assert_eq!(r.moves, vec!["s0a900p1"]);
        assert_eq!(apply(&r, &a, &shot), Ok(None), "yinelenen atış");
        assert!(apply(&r, &b, &GameAction::Move { mv: "a900p1".into(), at: Some(1) }).is_ok());
    }

    #[test]
    fn clean_room_rejects_bad_input() {
        assert!(clean_room(room()).is_some());
        let mut r = room();
        r.game = "tavla".into();
        assert!(clean_room(r).is_none());
        let mut r = room();
        r.players.pop();
        assert!(clean_room(r).is_none(), "satranç iki kişilik");
        let mut r = room();
        r.host = "c".repeat(64);
        assert!(clean_room(r).is_none(), "sahip oyuncu olmalı");
        let mut r = room();
        r.result = Some(GameResult { winner: Some("c".repeat(64)), reason: "resign".into(), by: None });
        assert!(clean_room(r).is_none());
        let mut r = room();
        r.players[1].name = "Banu\u{202E}exe".into();
        assert_eq!(clean_room(r).unwrap().players[1].name, "Banuexe");
    }
}
