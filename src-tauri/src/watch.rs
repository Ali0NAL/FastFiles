//! Birlikte izle (`node` modülünün alt modülü): ekip arkadaşlarıyla aynı YouTube videosunu
//! eşzamanlı izleme.
//!
//! Akış
//! 1. Biri sohbetten (birebir ya da grup) bir YouTube bağlantısıyla oda açar; odanın **sahibi** olur.
//!    Sohbetteki herkes davet edilir (`WatchUpdate`); "Katıl" diyenler izlemeye başlar.
//! 2. Video ağdan geçmez: herkes videoyu kendi bilgisayarında YouTube'dan oynatır. Ağdan yalnızca
//!    küçük durum mesajları gider: hangi video, oynuyor mu, kaçıncı saniyede.
//! 3. Oynat/duraklat/sar/video değiştir istekleri sahibine gider (`WatchAction`). Sahip isteği
//!    uygular, sürümü (`rev`) artırır ve odanın yeni hâlini herkese iletir. Sahip isterse kontrolü
//!    yalnızca kendine alabilir (`host_only`).
//!
//! Saat eşitleme: bilgisayarların saatleri farklı olabileceği için duvar saati kullanılmaz. Oda
//! mesajı, **gönderildiği andaki** konumu taşır; alan taraf mesajı aldığı anı (`Instant`) kaydeder
//! ve oynuyorsa geçen süreyi ekler. Yerel ağda gecikme milisaniyeler mertebesindedir.
//!
//! Dayanıklılık: oda yalnızca bellekte tutulur (uygulama kapanınca biter). Sahip, değişiklikleri
//! ulaşamadığı üyelere kısa aralıklarla yeniden dener ve 15 saniyede bir herkese güncel hâli
//! gönderir (sonradan çevrimiçi olan da daveti böyle alır). Üyeler 45 saniye haber alamadıkları
//! odayı kapatır.
//!
//! Güvenlik
//! - Oda bilgisi yalnızca sahibinden kabul edilir (TLS sertifika parmak izi); eski sürüm (`rev`)
//!   yok sayılır. İstek yalnızca odanın üyelerinden kabul edilir; kontrol için izliyor olmak gerekir.
//! - Video kimliği 11 karakterlik YouTube kimliği olmalıdır; arayüz videoyu yalnızca
//!   `youtube.com/embed/<kimlik>` olarak açar (bağlantı ya da adres ağdan alınmaz).
//! - Başlık ve adlar temizlenir; istekler cihaz başına hız sınırına tabidir.

use super::*;
use crate::chat::GroupMember;

/// Sıradaki en fazla video.
pub const MAX_QUEUE: usize = 50;
const MAX_TITLE_CHARS: usize = 150;
/// Bir kişinin aynı anda açabileceği / bir kişiden kabul edilen en fazla oda.
const MAX_ROOMS_PER_HOST: usize = 3;
/// Sahip, bu aralıkla tüm üyelere odanın güncel hâlini yeniden gönderir.
const HEARTBEAT: Duration = Duration::from_secs(15);
/// Üye, sahipten bu kadar süre haber alamazsa odayı kapatır.
const HOST_SILENT: Duration = Duration::from_secs(45);
/// Ulaşılamayan üyeye güncel hâl bu kadar aralıkla, en fazla `MAX_ATTEMPTS` kez yeniden denenir.
const RETRY: Duration = Duration::from_secs(2);
const MAX_ATTEMPTS: u8 = 5;
/// İstek hız sınırı (cihaz başına).
const RATE_MAX: usize = 40;
const RATE_WINDOW: Duration = Duration::from_secs(10);
/// Konum üst sınırı (saniye): canlı yayınlar dahil makul her değer bunun altındadır.
const MAX_POSITION: f64 = 10_000_000.0;
/// İzlerken gönderilebilen tepkiler.
pub const WATCH_REACTIONS: [&str; 8] = ["👍", "❤️", "😂", "😮", "😢", "👏", "🔥", "🎉"];

/// YouTube video kimliği: 11 karakter, `A-Z a-z 0-9 - _`.
pub fn valid_video(v: &str) -> bool {
    v.len() == 11 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn clean_title(t: &str) -> String {
    sanitize_text(t, MAX_TITLE_CHARS).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_position(p: f64) -> f64 {
    if p.is_finite() {
        p.clamp(0.0, MAX_POSITION)
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WatchItem {
    pub video: String,
    /// Oynatıcının bildirdiği başlık (henüz bilinmiyorsa boş).
    #[serde(default)]
    pub title: String,
    /// Ekleyen kişinin parmak izi.
    #[serde(default)]
    pub by: String,
}

/// Üyelere gönderilen oda.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WatchRoom {
    pub id: String,
    /// Oda sahibi.
    pub host: String,
    /// Oda bir grup sohbetinden açıldıysa grubun kimliği (alıcı kendi grup bilgisiyle doğrular).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Davet edilenler (sahip dahil).
    pub members: Vec<GroupMember>,
    /// Şu an izleyenler (sahip dahil).
    pub joined: Vec<String>,
    pub current: WatchItem,
    pub playing: bool,
    /// Mesajın gönderildiği andaki konum (saniye).
    pub position: f64,
    #[serde(default)]
    pub queue: Vec<WatchItem>,
    /// Oynat/duraklat/sar/video değiştir yalnızca sahipte.
    #[serde(default)]
    pub host_only: bool,
    /// Sahip odayı kapattı.
    #[serde(default)]
    pub ended: bool,
    pub rev: u64,
}

impl WatchRoom {
    fn is_member(&self, fp: &str) -> bool {
        self.members.iter().any(|m| m.fingerprint == fp)
    }
    fn can_control(&self, fp: &str) -> bool {
        fp == self.host || (!self.host_only && self.joined.iter().any(|j| j == fp))
    }
}

/// Üyenin isteği.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WatchAction {
    Join,
    Leave,
    Play { position: f64 },
    Pause { position: f64 },
    Seek { position: f64 },
    /// Hemen bu videoya geç.
    Load {
        video: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        position: f64,
    },
    /// Sıraya ekle.
    Enqueue {
        video: String,
        #[serde(default)]
        title: String,
    },
    /// Sıradan çıkar (`video`, aynı anda başka biri sırayı değiştirdiyse yanlış öğe silinmesin diye).
    Dequeue { index: usize, video: String },
    /// Sıradaki videoya geç. `from` şu anki video olmalı: video bitince birden çok kişinin oynatıcısı
    /// aynı anda bildirse de yalnızca bir kez ilerlenir.
    Next { from: String },
    React { emoji: String },
    /// Oynatıcının bildirdiği başlık (başlık henüz bilinmiyorsa kaydedilir).
    Title { video: String, title: String },
    /// Kontrolü yalnızca sahibe al / herkese aç (yalnızca sahip).
    HostOnly { on: bool },
}

impl WatchAction {
    fn kind(&self) -> &'static str {
        match self {
            WatchAction::Join => "join",
            WatchAction::Leave => "leave",
            WatchAction::Play { .. } => "play",
            WatchAction::Pause { .. } => "pause",
            WatchAction::Seek { .. } => "seek",
            WatchAction::Load { .. } => "load",
            WatchAction::Enqueue { .. } => "enqueue",
            WatchAction::Dequeue { .. } => "dequeue",
            WatchAction::Next { .. } => "next",
            WatchAction::React { .. } => "react",
            WatchAction::Title { .. } => "title",
            WatchAction::HostOnly { .. } => "hostOnly",
        }
    }
}

/// Odadaki değişikliğin kısa açıklaması ("Ayşe durdurdu", tepki baloncuğu…). Arayüz gösterir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WatchEvent {
    pub kind: String,
    /// İşlemi yapanın parmak izi.
    pub by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub emoji: String,
    #[serde(default)]
    pub position: f64,
}

/// Odaya isteği uygular. Oda konumu çağırmadan önce "şimdi"ye getirilmiş olmalı.
/// Dönüş: `Ok(true)` oda değişti, `Ok(false)` değişmedi (ör. tepki, yinelenen istek).
pub(super) fn apply(room: &mut WatchRoom, sender: &str, action: &WatchAction) -> Result<bool, &'static str> {
    if room.ended {
        return Err("closed");
    }
    if !room.is_member(sender) {
        return Err("not_member");
    }
    let joined = room.joined.iter().any(|j| j == sender);
    let control = |room: &WatchRoom| if room.can_control(sender) { Ok(()) } else if joined { Err("host_only") } else { Err("not_joined") };
    match action {
        WatchAction::Join => {
            if joined {
                return Ok(false);
            }
            room.joined.push(sender.to_string());
        }
        WatchAction::Leave => {
            if sender == room.host || !joined {
                return Ok(false);
            }
            room.joined.retain(|j| j != sender);
        }
        WatchAction::Play { position } => {
            control(room)?;
            room.playing = true;
            room.position = clean_position(*position);
        }
        WatchAction::Pause { position } => {
            control(room)?;
            room.playing = false;
            room.position = clean_position(*position);
        }
        WatchAction::Seek { position } => {
            control(room)?;
            room.position = clean_position(*position);
        }
        WatchAction::Load { video, title, position } => {
            control(room)?;
            if !valid_video(video) {
                return Err("invalid");
            }
            room.current = WatchItem { video: video.clone(), title: clean_title(title), by: sender.to_string() };
            room.position = clean_position(*position);
            room.playing = true;
            // Sırada bekleyen aynı video artık oynuyor.
            if let Some(i) = room.queue.iter().position(|q| q.video == *video) {
                room.queue.remove(i);
            }
        }
        WatchAction::Enqueue { video, title } => {
            if !joined {
                return Err("not_joined");
            }
            if !valid_video(video) {
                return Err("invalid");
            }
            if room.queue.len() >= MAX_QUEUE {
                return Err("queue_full");
            }
            room.queue.push(WatchItem { video: video.clone(), title: clean_title(title), by: sender.to_string() });
        }
        WatchAction::Dequeue { index, video } => {
            // Herkes kendi eklediğini çıkarabilir; başkasınınkini yalnızca kontrol yetkisi olan.
            match room.queue.get(*index) {
                Some(q) if q.video == *video => {
                    if q.by != sender {
                        control(room)?;
                    }
                }
                _ => return Ok(false),
            }
            room.queue.remove(*index);
        }
        WatchAction::Next { from } => {
            control(room)?;
            if room.current.video != *from {
                return Ok(false);
            }
            if room.queue.is_empty() {
                // Sırada video yok: bitti, durdur.
                if !room.playing {
                    return Ok(false);
                }
                room.playing = false;
            } else {
                room.current = room.queue.remove(0);
                room.position = 0.0;
                room.playing = true;
            }
        }
        WatchAction::React { emoji } => {
            if !joined {
                return Err("not_joined");
            }
            if !WATCH_REACTIONS.contains(&emoji.as_str()) {
                return Err("invalid");
            }
            return Ok(false);
        }
        WatchAction::Title { video, title } => {
            if !joined {
                return Err("not_joined");
            }
            let title = clean_title(title);
            if !valid_video(video) || title.is_empty() {
                return Err("invalid");
            }
            let mut changed = false;
            for item in std::iter::once(&mut room.current).chain(room.queue.iter_mut()) {
                if item.video == *video && item.title.is_empty() {
                    item.title = title.clone();
                    changed = true;
                }
            }
            return Ok(changed);
        }
        WatchAction::HostOnly { on } => {
            if sender != room.host {
                return Err("not_host");
            }
            if room.host_only == *on {
                return Ok(false);
            }
            room.host_only = *on;
        }
    }
    Ok(true)
}

/// Ağdan gelen odayı doğrular ve temizler.
fn clean_room(mut r: WatchRoom) -> Option<WatchRoom> {
    if !valid_message_id(&r.id) || !valid_fingerprint(&r.host) || r.members.len() > MAX_GROUP_MEMBERS + 1 {
        return None;
    }
    if r.group_id.as_deref().is_some_and(|g| !valid_message_id(g)) {
        return None;
    }
    let mut seen = HashSet::new();
    for m in &mut r.members {
        if !valid_fingerprint(&m.fingerprint) || !seen.insert(m.fingerprint.clone()) {
            return None;
        }
        m.name = sanitize_name(&m.name);
    }
    if !seen.contains(&r.host) {
        return None;
    }
    let mut joined = HashSet::new();
    r.joined.retain(|j| seen.contains(j) && joined.insert(j.clone()));
    if !valid_video(&r.current.video) || r.queue.len() > MAX_QUEUE || r.queue.iter().any(|q| !valid_video(&q.video)) {
        return None;
    }
    for item in std::iter::once(&mut r.current).chain(r.queue.iter_mut()) {
        item.title = clean_title(&item.title);
        if !valid_fingerprint(&item.by) {
            item.by.clear();
        }
    }
    r.position = clean_position(r.position);
    Some(r)
}

fn reject(reason: &str) -> Message {
    Message::ChatReject { reason: reason.into() }
}

fn action_error(reason: &str) -> String {
    match reason {
        "host_only" => "Kontrol şu an yalnızca oda sahibinde",
        "not_joined" => "Önce odaya katılın",
        "not_member" => "Bu odaya davetli değilsiniz",
        "not_host" => "Bunu yalnızca oda sahibi yapabilir",
        "closed" | "unknown_room" => "Bu oda kapandı",
        "queue_full" => "Sıra dolu (en fazla 50 video)",
        "rate" => "Çok hızlı istek gönderiliyor, biraz bekleyin",
        "blocked" => "İstek reddedildi",
        _ => "Geçersiz istek",
    }
    .into()
}

pub(super) struct RoomEntry {
    room: WatchRoom,
    host_name: String,
    /// `room.position`'ın geçerli olduğu an (sahipte son değişiklik, üyede mesajın alındığı an).
    anchor: Instant,
    /// Bu cihazda açılan sohbet: grup sohbeti ya da sahiple birebir sohbet.
    chat_key: String,
    /// (Üye) odaya katıldım mı?
    me_joined: bool,
    /// (Sahip) güncel hâli henüz alamamış üyeler → deneme sayısı.
    pending: HashMap<String, u8>,
    /// (Sahip) son toplu gönderim.
    last_beat: Instant,
}

impl RoomEntry {
    /// Şu anki konum (oynuyorsa geçen süre eklenir).
    fn position_now(&self) -> f64 {
        let p = if self.room.playing { self.room.position + self.anchor.elapsed().as_secs_f64() } else { self.room.position };
        clean_position(p)
    }
    /// Konumu "şimdi"ye getirir (değişiklikten önce).
    fn settle(&mut self) {
        self.room.position = self.position_now();
        self.anchor = Instant::now();
    }
    /// Gönderilecek anlık görüntü.
    fn snapshot(&self) -> WatchRoom {
        let mut r = self.room.clone();
        r.position = self.position_now();
        r
    }
}

#[derive(Default)]
pub struct State {
    rooms: Mutex<HashMap<String, RoomEntry>>,
    /// Kapattığım (ayrıldığım) odalar: sahibin sonraki güncellemeleri yok sayılır.
    dismissed: Mutex<HashSet<String>>,
    rate: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl State {
    /// Bir odada izliyor muyum? (Otomatik güncelleme bu sırada uygulamayı kapatmaz.)
    pub fn busy(&self) -> bool {
        self.rooms.lock().unwrap().values().any(|e| e.me_joined && !e.room.ended)
    }
}

/// Arayüze giden oda görünümü.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchView {
    pub room: WatchRoom,
    pub host_name: String,
    pub is_host: bool,
    /// Bu cihaz izliyor mu?
    pub joined: bool,
    pub can_control: bool,
    pub chat_key: String,
}

impl Node {
    fn watch_view(&self, e: &RoomEntry) -> WatchView {
        let me = &self.identity.fingerprint;
        let room = e.snapshot();
        WatchView {
            is_host: room.host == *me,
            can_control: e.me_joined && (room.host == *me || !room.host_only),
            joined: e.me_joined,
            host_name: e.host_name.clone(),
            chat_key: e.chat_key.clone(),
            room,
        }
    }

    pub fn watch_list(&self) -> Vec<WatchView> {
        self.extras.watch.rooms.lock().unwrap().values().map(|e| self.watch_view(e)).collect()
    }

    fn emit_watch(&self, id: &str, event: Option<WatchEvent>) {
        let view = self.extras.watch.rooms.lock().unwrap().get(id).map(|e| self.watch_view(e));
        if let Some(v) = view {
            self.emit("watch", json!({ "view": v, "event": event }));
        }
    }

    fn emit_watch_end(&self, id: &str, reason: &str, host_name: &str) {
        self.emit("watch-end", json!({ "id": id, "reason": reason, "hostName": host_name }));
    }

    fn watch_rate_ok(&self, fp: &str) -> bool {
        let mut map = self.extras.watch.rate.lock().unwrap();
        let q = map.entry(fp.to_string()).or_default();
        while q.front().is_some_and(|t| t.elapsed() > RATE_WINDOW) {
            q.pop_front();
        }
        if q.len() >= RATE_MAX {
            return false;
        }
        q.push_back(Instant::now());
        true
    }

    // ───────────── Kullanıcı işlemleri ─────────────

    /// Sohbetten (`key`: kişinin parmak izi ya da grup anahtarı) oda açar ve herkesi davet eder.
    /// Bu sohbet için zaten açtığım bir oda varsa yeni video o odada oynatılır.
    pub fn watch_start(self: &Arc<Self>, key: &str, video: &str, title: &str, position: f64) -> Result<WatchView, String> {
        if !valid_video(video) {
            return Err("Geçerli bir YouTube bağlantısı değil".into());
        }
        let me = self.identity.fingerprint.clone();
        let my_name = self.device_info().name;
        let (group_id, targets): (Option<String>, Vec<GroupMember>) = match self.group_of(key) {
            Some((g, left)) => {
                if left || g.deleted {
                    return Err("Bu grubun üyesi değilsiniz".into());
                }
                (Some(g.id.clone()), g.members.iter().filter(|m| m.fingerprint != me).cloned().collect())
            }
            None if valid_fingerprint(key) && key != me => {
                if self.is_blocked(key) {
                    return Err("Engellediğiniz bir kişiyi davet edemezsiniz".into());
                }
                let name = self.name_for(key).ok_or("Kişi bulunamadı")?;
                (None, vec![GroupMember { fingerprint: key.to_string(), name }])
            }
            None => return Err("Geçersiz sohbet".into()),
        };
        let online: HashSet<String> = self.peers().into_iter().filter(|p| !p.blocked).map(|p| p.fingerprint).collect();
        if !targets.iter().any(|t| online.contains(&t.fingerprint)) {
            return Err("Davet edilecek kimse çevrimiçi değil".into());
        }
        // Bu sohbette zaten açık odam varsa videoyu orada değiştir.
        let existing = self
            .extras
            .watch
            .rooms
            .lock()
            .unwrap()
            .values()
            .find(|e| e.room.host == me && e.chat_key == key && !e.room.ended)
            .map(|e| e.room.id.clone());
        if let Some(id) = existing {
            self.watch_act(&id, WatchAction::Load { video: video.into(), title: title.into(), position })?;
            let view = self.extras.watch.rooms.lock().unwrap().get(&id).map(|e| self.watch_view(e));
            return view.ok_or_else(|| "Oda bulunamadı".into());
        }
        let mut members = vec![GroupMember { fingerprint: me.clone(), name: my_name.clone() }];
        members.extend(targets.into_iter().filter(|t| !self.is_blocked(&t.fingerprint)));
        let room = WatchRoom {
            id: uuid::Uuid::new_v4().to_string(),
            host: me.clone(),
            group_id,
            members,
            joined: vec![me.clone()],
            current: WatchItem { video: video.into(), title: clean_title(title), by: me.clone() },
            playing: true,
            position: clean_position(position),
            queue: Vec::new(),
            host_only: false,
            ended: false,
            rev: 1,
        };
        let id = room.id.clone();
        {
            let mut rooms = self.extras.watch.rooms.lock().unwrap();
            let mine = rooms.values().filter(|e| e.room.host == me && !e.room.ended).count();
            if mine >= MAX_ROOMS_PER_HOST {
                return Err("En fazla 3 izleme odası açabilirsiniz; önce birini kapatın".into());
            }
            let entry = RoomEntry {
                pending: room.members.iter().filter(|m| m.fingerprint != me).map(|m| (m.fingerprint.clone(), 0)).collect(),
                room,
                host_name: my_name,
                anchor: Instant::now(),
                chat_key: key.to_string(),
                me_joined: true,
                last_beat: Instant::now(),
            };
            rooms.insert(id.clone(), entry);
        }
        self.emit_watch(&id, None);
        self.watch_broadcast(&id, None);
        let view = self.extras.watch.rooms.lock().unwrap().get(&id).map(|e| self.watch_view(e));
        view.ok_or_else(|| "Oda bulunamadı".into())
    }

    /// Kullanıcının isteği. Sahipte hemen uygulanır ve herkese gider; üyede sahibine gönderilir.
    pub fn watch_act(self: &Arc<Self>, id: &str, action: WatchAction) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        let host = {
            let mut rooms = self.extras.watch.rooms.lock().unwrap();
            let e = rooms.get_mut(id).ok_or("Bu oda kapandı")?;
            if e.room.host == me {
                e.settle();
                let changed = apply(&mut e.room, &me, &action).map_err(action_error)?;
                if changed {
                    e.room.rev += 1;
                    e.pending = e.room.members.iter().filter(|m| m.fingerprint != me).map(|m| (m.fingerprint.clone(), 0)).collect();
                }
                None
            } else {
                match action {
                    WatchAction::Join => e.me_joined = true,
                    WatchAction::Leave => e.me_joined = false,
                    _ => {}
                }
                Some(e.room.host.clone())
            }
        };
        match host {
            None => {
                let event = WatchEvent { kind: action.kind().into(), by: me, emoji: react_emoji(&action), position: action_position(&action) };
                self.emit_watch(id, Some(event.clone()));
                self.watch_broadcast(id, Some(event));
            }
            Some(host) => {
                self.emit_watch(id, None);
                let node = self.clone();
                let id = id.to_string();
                tokio::spawn(async move { node.watch_send_action(&host, &id, action).await });
            }
        }
        Ok(())
    }

    /// (Üye) isteği sahibine iletir; reddedilirse arayüze hata gider.
    async fn watch_send_action(self: Arc<Self>, host: &str, id: &str, action: WatchAction) {
        let Some(peer) = self.peers().into_iter().find(|p| p.fingerprint == host) else {
            self.emit("watch-error", json!({ "id": id, "message": "Oda sahibi çevrimdışı" }));
            return;
        };
        let msg = Message::WatchAction { device: self.device_info(), room_id: id.to_string(), action: action.clone() };
        let reply = async {
            let mut s = self.connect(&self.candidates(&peer), Some(peer.fingerprint.clone())).await?;
            write_msg(&mut s, &msg).await.map_err(|e| e.to_string())?;
            let r = timeout(Duration::from_secs(6), read_msg_max(&mut s, 4096)).await.map_err(|_| "yanıt yok".to_string())?.map_err(|e| e.to_string())?;
            let _ = s.shutdown().await;
            Ok::<_, String>(r)
        }
        .await;
        match reply {
            Ok(Message::GroupAck) => {}
            Ok(Message::ChatReject { reason }) => {
                if matches!(reason.as_str(), "unknown_room" | "closed" | "not_member") {
                    let name = self.extras.watch.rooms.lock().unwrap().remove(id).map(|e| e.host_name).unwrap_or_default();
                    self.emit_watch_end(id, "gone", &name);
                } else if !matches!(action, WatchAction::Title { .. } | WatchAction::Leave) {
                    self.emit("watch-error", json!({ "id": id, "message": action_error(&reason) }));
                }
            }
            _ => {
                if !matches!(action, WatchAction::Title { .. } | WatchAction::Leave) {
                    self.emit("watch-error", json!({ "id": id, "message": "Oda sahibine ulaşılamadı" }));
                }
            }
        }
    }

    /// Odayı kapatır: sahipse herkes için biter, üyeyse odadan ayrılır.
    pub fn watch_close(self: &Arc<Self>, id: &str) {
        let me = self.identity.fingerprint.clone();
        let Some(mut e) = self.extras.watch.rooms.lock().unwrap().remove(id) else { return };
        if e.room.host == me {
            e.room.ended = true;
            e.room.rev += 1;
            let room = e.snapshot();
            for peer in self.peers().into_iter().filter(|p| !p.blocked && p.fingerprint != me && room.is_member(&p.fingerprint)) {
                let node = self.clone();
                let msg = Message::WatchUpdate { device: self.device_info(), room: room.clone(), host_name: e.host_name.clone(), event: None };
                tokio::spawn(async move {
                    let _ = node.watch_request(&peer, &msg).await;
                });
            }
        } else {
            self.extras.watch.dismissed.lock().unwrap().insert(id.to_string());
            if e.me_joined {
                let node = self.clone();
                let host = e.room.host.clone();
                let id = id.to_string();
                tokio::spawn(async move { node.watch_send_action(&host, &id, WatchAction::Leave).await });
            }
        }
        self.emit_watch_end(id, "closed", &e.host_name);
    }

    // ───────────── Teslimat (sahip) ─────────────

    /// Odanın güncel hâlini çevrimiçi üyelere gönderir. Tepki gibi oda değiştirmeyen olaylar
    /// yalnızca izleyenlere gider.
    fn watch_broadcast(self: &Arc<Self>, id: &str, event: Option<WatchEvent>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let me = self.identity.fingerprint.clone();
        let only_viewers = event.as_ref().is_some_and(|e| e.kind == "react");
        let targets: Vec<String> = {
            let rooms = self.extras.watch.rooms.lock().unwrap();
            let Some(e) = rooms.get(id) else { return };
            e.room
                .members
                .iter()
                .map(|m| m.fingerprint.clone())
                .filter(|f| *f != me && (!only_viewers || e.room.joined.contains(f)))
                .collect()
        };
        for peer in self.peers().into_iter().filter(|p| !p.blocked && targets.contains(&p.fingerprint)) {
            self.watch_push(id, peer, event.clone());
        }
    }

    /// Bir üyeye odanın o anki hâlini gönderir (konum bağlantı kurulduktan sonra hesaplanır).
    fn watch_push(self: &Arc<Self>, id: &str, peer: Peer, event: Option<WatchEvent>) {
        let node = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let result = async {
                let mut s = node.connect(&node.candidates(&peer), Some(peer.fingerprint.clone())).await?;
                let Some((room, host_name)) = node.extras.watch.rooms.lock().unwrap().get(&id).map(|e| (e.snapshot(), e.host_name.clone())) else {
                    return Ok(None);
                };
                let rev = room.rev;
                let msg = Message::WatchUpdate { device: node.device_info(), room, host_name, event };
                write_msg(&mut s, &msg).await.map_err(|e| e.to_string())?;
                let r = timeout(Duration::from_secs(6), read_msg_max(&mut s, 4096)).await.map_err(|_| "yanıt yok".to_string())?.map_err(|e| e.to_string())?;
                let _ = s.shutdown().await;
                Ok::<_, String>(Some((rev, r)))
            }
            .await;
            let mut rooms = node.extras.watch.rooms.lock().unwrap();
            let Some(e) = rooms.get_mut(&id) else { return };
            match result {
                Ok(Some((rev, Message::GroupAck))) => {
                    if e.room.rev == rev {
                        e.pending.remove(&peer.fingerprint);
                    }
                }
                Ok(Some((_, Message::ChatReject { reason }))) => {
                    // Üye odayı istemiyor ya da kabul edemiyor (engelledi, çok oda): yeniden deneme.
                    e.pending.remove(&peer.fingerprint);
                    if e.room.joined.contains(&peer.fingerprint) && reason != "rate" {
                        e.room.joined.retain(|j| *j != peer.fingerprint);
                    }
                }
                Ok(None) => {}
                _ => {
                    if let Some(n) = e.pending.get_mut(&peer.fingerprint) {
                        *n += 1;
                        if *n >= MAX_ATTEMPTS {
                            e.pending.remove(&peer.fingerprint);
                        }
                    }
                }
            }
        });
    }

    async fn watch_request(&self, peer: &Peer, msg: &Message) -> Result<Message, String> {
        let mut s = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut s, msg).await.map_err(|e| e.to_string())?;
        let r = timeout(Duration::from_secs(6), read_msg_max(&mut s, 4096)).await.map_err(|_| "yanıt yok".to_string())?.map_err(|e| e.to_string())?;
        let _ = s.shutdown().await;
        Ok(r)
    }

    /// Keşif döngüsünden (2,5 sn) çağrılır: ulaşılamayan üyelere yeniden gönderir, düzenli
    /// güncel hâl gönderir, çevrimdışı kalan izleyicileri ve sahibinden haber alınamayan odaları kapatır.
    pub fn watch_tick(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let me = self.identity.fingerprint.clone();
        let online: HashMap<String, Peer> = self.peers().into_iter().filter(|p| !p.blocked).map(|p| (p.fingerprint.clone(), p)).collect();
        let mut pushes: Vec<(String, Peer)> = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        let mut gone: Vec<(String, String)> = Vec::new();
        {
            let mut rooms = self.extras.watch.rooms.lock().unwrap();
            rooms.retain(|id, e| {
                if e.room.host != me {
                    if e.anchor.elapsed() > HOST_SILENT {
                        gone.push((id.clone(), e.host_name.clone()));
                        return false;
                    }
                    return true;
                }
                // Çevrimdışı kalan izleyiciler odadan çıkar.
                let before = e.room.joined.len();
                e.room.joined.retain(|j| *j == me || online.contains_key(j));
                if e.room.joined.len() != before {
                    e.settle();
                    e.room.rev += 1;
                    e.pending = e.room.members.iter().filter(|m| m.fingerprint != me).map(|m| (m.fingerprint.clone(), 0)).collect();
                    changed.push(id.clone());
                    return true;
                }
                let beat = e.last_beat.elapsed() >= HEARTBEAT;
                if beat {
                    e.last_beat = Instant::now();
                }
                for m in &e.room.members {
                    if m.fingerprint == me {
                        continue;
                    }
                    let Some(p) = online.get(&m.fingerprint) else { continue };
                    if beat || e.pending.contains_key(&m.fingerprint) {
                        pushes.push((id.clone(), p.clone()));
                    }
                }
                true
            });
        }
        for (id, name) in gone {
            self.emit_watch_end(&id, "gone", &name);
        }
        for id in changed {
            self.emit_watch(&id, None);
            self.watch_broadcast(&id, None);
        }
        for (id, peer) in pushes {
            let flight = format!("watch:{id}:{}", peer.fingerprint);
            if self.outbox_backoff.lock().unwrap().get(&flight).is_some_and(|until| Instant::now() < *until) {
                continue;
            }
            self.outbox_backoff.lock().unwrap().insert(flight, Instant::now() + RETRY);
            self.watch_push(&id, peer, None);
        }
    }

    // ───────────── Ağdan gelenler ─────────────

    /// (Üye) sahipten gelen oda.
    pub(super) fn accept_watch_update(&self, sender: &str, room: WatchRoom, host_name: &str, event: Option<WatchEvent>) -> Message {
        if self.is_blocked(sender) {
            return reject("blocked");
        }
        let Some(room) = clean_room(room) else { return reject("invalid") };
        if room.host != sender {
            return reject("not_owner");
        }
        let me = self.identity.fingerprint.clone();
        if room.host == me || !room.is_member(&me) {
            return reject("not_member");
        }
        let id = room.id.clone();
        let host_name = sanitize_name(host_name);
        if room.ended {
            let existed = self.extras.watch.rooms.lock().unwrap().remove(&id).is_some();
            self.extras.watch.dismissed.lock().unwrap().remove(&id);
            if existed {
                self.emit_watch_end(&id, "ended", &host_name);
            }
            return Message::GroupAck;
        }
        if self.extras.watch.dismissed.lock().unwrap().contains(&id) {
            return Message::GroupAck;
        }
        let fresh = {
            let mut rooms = self.extras.watch.rooms.lock().unwrap();
            match rooms.get_mut(&id) {
                Some(e) => {
                    if e.room.host != sender || room.rev < e.room.rev {
                        return Message::GroupAck;
                    }
                    e.room = room;
                    e.host_name = host_name.clone();
                    e.anchor = Instant::now();
                    false
                }
                None => {
                    if rooms.values().filter(|e| e.room.host == sender).count() >= MAX_ROOMS_PER_HOST {
                        return reject("too_many");
                    }
                    let chat_key = room
                        .group_id
                        .as_deref()
                        .and_then(|g| self.shared_group(g, sender))
                        .map(|(key, _)| key)
                        .unwrap_or_else(|| sender.to_string());
                    rooms.insert(
                        id.clone(),
                        RoomEntry {
                            room,
                            host_name: host_name.clone(),
                            anchor: Instant::now(),
                            chat_key,
                            me_joined: false,
                            pending: HashMap::new(),
                            last_beat: Instant::now(),
                        },
                    );
                    true
                }
            }
        };
        if fresh {
            let (title, chat_key) = self
                .extras
                .watch
                .rooms
                .lock()
                .unwrap()
                .get(&id)
                .map(|e| (e.room.current.title.clone(), e.chat_key.clone()))
                .unwrap_or_default();
            self.emit("watch-invite", json!({ "id": id, "hostName": host_name, "title": title, "chatKey": chat_key }));
        }
        self.emit_watch(&id, event.filter(|e| e.by != me));
        Message::GroupAck
    }

    /// (Sahip) üyeden gelen istek.
    pub(super) fn accept_watch_action(self: &Arc<Self>, sender: &str, room_id: &str, action: WatchAction) -> Message {
        if self.is_blocked(sender) {
            return reject("blocked");
        }
        if !self.watch_rate_ok(sender) {
            return reject("rate");
        }
        let me = self.identity.fingerprint.clone();
        let changed = {
            let mut rooms = self.extras.watch.rooms.lock().unwrap();
            let Some(e) = rooms.get_mut(room_id).filter(|e| e.room.host == me) else { return reject("unknown_room") };
            e.settle();
            match apply(&mut e.room, sender, &action) {
                Ok(true) => {
                    e.room.rev += 1;
                    e.pending = e.room.members.iter().filter(|m| m.fingerprint != me).map(|m| (m.fingerprint.clone(), 0)).collect();
                    true
                }
                Ok(false) => false,
                Err(reason) => return reject(reason),
            }
        };
        let event = WatchEvent { kind: action.kind().into(), by: sender.to_string(), emoji: react_emoji(&action), position: action_position(&action) };
        // Yinelenen istek (aynı başlık, zaten katılmış) sessizce yanıtlanır; tepki yine iletilir.
        if changed || matches!(action, WatchAction::React { .. }) {
            self.emit_watch(room_id, Some(event.clone()));
            self.watch_broadcast(room_id, Some(event));
        }
        Message::GroupAck
    }
}

fn react_emoji(a: &WatchAction) -> String {
    match a {
        WatchAction::React { emoji } => emoji.clone(),
        _ => String::new(),
    }
}

fn action_position(a: &WatchAction) -> f64 {
    match a {
        WatchAction::Play { position } | WatchAction::Pause { position } | WatchAction::Seek { position } | WatchAction::Load { position, .. } => {
            clean_position(*position)
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const A: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const X: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn room() -> WatchRoom {
        WatchRoom {
            id: "r1".into(),
            host: H.into(),
            group_id: None,
            members: vec![GroupMember { fingerprint: H.into(), name: "Host".into() }, GroupMember { fingerprint: A.into(), name: "Ayşe".into() }],
            joined: vec![H.into()],
            current: WatchItem { video: "dQw4w9WgXcQ".into(), title: String::new(), by: H.into() },
            playing: true,
            position: 10.0,
            queue: vec![],
            host_only: false,
            ended: false,
            rev: 1,
        }
    }

    #[test]
    fn video_ids() {
        assert!(valid_video("dQw4w9WgXcQ"));
        assert!(valid_video("a-_b0123456"));
        assert!(!valid_video("dQw4w9WgXc"));
        assert!(!valid_video("dQw4w9WgXcQQ"));
        assert!(!valid_video("dQw4w9WgX/Q"));
        assert!(!valid_video("javascript:"));
    }

    #[test]
    fn controls_need_join_and_respect_host_only() {
        let mut r = room();
        // Katılmadan kontrol edilemez, yabancı hiçbir şey yapamaz.
        assert_eq!(apply(&mut r, A, &WatchAction::Pause { position: 3.0 }), Err("not_joined"));
        assert_eq!(apply(&mut r, X, &WatchAction::Join), Err("not_member"));
        assert_eq!(apply(&mut r, A, &WatchAction::Join), Ok(true));
        assert_eq!(apply(&mut r, A, &WatchAction::Join), Ok(false));
        assert_eq!(apply(&mut r, A, &WatchAction::Pause { position: 42.5 }), Ok(true));
        assert!(!r.playing);
        assert_eq!(r.position, 42.5);
        assert_eq!(apply(&mut r, A, &WatchAction::Seek { position: f64::NAN }), Ok(true));
        assert_eq!(r.position, 0.0);
        // Kontrol yalnızca sahipte.
        assert_eq!(apply(&mut r, A, &WatchAction::HostOnly { on: true }), Err("not_host"));
        assert_eq!(apply(&mut r, H, &WatchAction::HostOnly { on: true }), Ok(true));
        assert_eq!(apply(&mut r, A, &WatchAction::Play { position: 1.0 }), Err("host_only"));
        assert_eq!(apply(&mut r, H, &WatchAction::Play { position: 1.0 }), Ok(true));
        // Sıraya eklemek ve tepki herkese açık.
        assert_eq!(apply(&mut r, A, &WatchAction::Enqueue { video: "aaaaaaaaaaa".into(), title: "x".into() }), Ok(true));
        assert_eq!(apply(&mut r, A, &WatchAction::React { emoji: "🔥".into() }), Ok(false));
        assert_eq!(apply(&mut r, A, &WatchAction::React { emoji: "<b>".into() }), Err("invalid"));
        // Kendi eklediğini çıkarabilir; yanlış video kimliğiyle hiçbir şey silinmez.
        assert_eq!(apply(&mut r, A, &WatchAction::Dequeue { index: 0, video: "bbbbbbbbbbb".into() }), Ok(false));
        assert_eq!(apply(&mut r, A, &WatchAction::Dequeue { index: 0, video: "aaaaaaaaaaa".into() }), Ok(true));
        assert!(r.queue.is_empty());
        // Ayrılan izleyici kontrol edemez; sahip ayrılamaz.
        assert_eq!(apply(&mut r, A, &WatchAction::Leave), Ok(true));
        assert_eq!(apply(&mut r, H, &WatchAction::Leave), Ok(false));
        assert_eq!(apply(&mut r, A, &WatchAction::Enqueue { video: "aaaaaaaaaaa".into(), title: String::new() }), Err("not_joined"));
    }

    #[test]
    fn next_is_idempotent_and_titles_fill_once() {
        let mut r = room();
        apply(&mut r, A, &WatchAction::Join).unwrap();
        apply(&mut r, A, &WatchAction::Enqueue { video: "aaaaaaaaaaa".into(), title: String::new() }).unwrap();
        assert_eq!(apply(&mut r, A, &WatchAction::Title { video: "aaaaaaaaaaa".into(), title: "  Sıradaki\n video ".into() }), Ok(true));
        assert_eq!(r.queue[0].title, "Sıradaki video");
        assert_eq!(apply(&mut r, H, &WatchAction::Title { video: "aaaaaaaaaaa".into(), title: "Başka".into() }), Ok(false));
        // İki oynatıcı aynı anda "bitti" der: yalnızca bir kez ilerlenir.
        assert_eq!(apply(&mut r, H, &WatchAction::Next { from: "dQw4w9WgXcQ".into() }), Ok(true));
        assert_eq!(apply(&mut r, A, &WatchAction::Next { from: "dQw4w9WgXcQ".into() }), Ok(false));
        assert_eq!(r.current.video, "aaaaaaaaaaa");
        assert_eq!(r.position, 0.0);
        assert!(r.playing && r.queue.is_empty());
        // Sıra boşken video bitince durur.
        assert_eq!(apply(&mut r, A, &WatchAction::Next { from: "aaaaaaaaaaa".into() }), Ok(true));
        assert!(!r.playing);
        // Geçersiz video yüklenemez.
        assert_eq!(apply(&mut r, A, &WatchAction::Load { video: "../../etc".into(), title: String::new(), position: 0.0 }), Err("invalid"));
    }

    #[test]
    fn clean_room_rejects_bad_input() {
        assert!(clean_room(room()).is_some());
        let mut r = room();
        r.current.video = "not-a-video-id".into();
        assert!(clean_room(r).is_none());
        let mut r = room();
        r.host = X.into();
        assert!(clean_room(r).is_none(), "sahip üyelerde olmalı");
        let mut r = room();
        r.members.push(r.members[1].clone());
        assert!(clean_room(r).is_none(), "yinelenen üye");
        let mut r = room();
        r.joined = vec![H.into(), X.into(), H.into()];
        r.current.title = "a\u{202E}b\n c".into();
        r.position = f64::INFINITY;
        let c = clean_room(r).unwrap();
        assert_eq!(c.joined, vec![H.to_string()]);
        assert_eq!(c.current.title, "ab c");
        assert_eq!(c.position, 0.0);
    }
}
