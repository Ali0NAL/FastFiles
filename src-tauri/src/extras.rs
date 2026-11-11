//! Durum, sohbet olayları (tepki, düzenleme, silme, dürtme), "çevrimiçi olunca gönder" ve
//! ekran görüntüsü yardımcıları (`node` modülünün alt modülü).
//!
//! Güvenlik kuralları:
//! - Tepkiyi gönderen kişinin kendisi adına kaydedilir (TLS parmak izi); başkası adına tepki
//!   verilemez. Yalnızca sabit emoji listesi kabul edilir.
//! - Mesajı yalnızca yazan kişi düzenleyip silebilir: birebir sohbette karşı tarafın gelen
//!   mesajları, grupta `sender` alanı gönderenin parmak iziyle aynı olan mesajlar.
//! - Grup olayları yalnızca iki taraf da o grubun üyesiyse geçerlidir.
//! - Dürtme yalnızca birebir sohbette olur ve kişi başına 10 saniyede bir kabul edilir.
//! - Ekran animasyonları (hediye, kalp, kurşun) yalnızca sabit listeden olabilir; aynı sohbete
//!   en fazla 15 saniyede bir gönderilir, aynı kişiden en fazla 15 saniyede bir kabul edilir.

use super::*;
use crate::chat::{ChatEventKind, Reaction, ReplyRef, EFFECTS, REACTIONS, REPLY_SNIPPET_CHARS};
use std::sync::atomic::AtomicBool;

/// Bu kadar süre klavye/fare kullanılmazsa durum kendiliğinden "Dışarıda" olur.
const AUTO_AWAY: Duration = Duration::from_secs(10 * 60);
/// Aynı kişiden en fazla bu sıklıkta dürtme.
const NUDGE_GAP: Duration = Duration::from_secs(10);
/// Ekran animasyonları arasında en az bu kadar süre olmalı (art arda atılamaz).
pub const EFFECT_GAP: Duration = Duration::from_secs(15);
/// Mesaj gönderildikten sonra bu süre içinde düzenlenebilir.
const EDIT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
/// Sohbet olayları için hız sınırı (cihaz başına).
const EVENT_RATE_MAX: usize = 40;
const EVENT_RATE_WINDOW: Duration = Duration::from_secs(20);
/// Ekran görüntüsü seçimi bu kadar beklenir.
const CLIP_WAIT: Duration = Duration::from_secs(90);
/// Arayüzden gönderilen (üzerine çizilmiş) resmin en büyük boyutu.
pub const MAX_PNG_BYTES: usize = 40 * 1024 * 1024;

#[derive(Default)]
pub struct State {
    /// Şu anki (etkin) durum: elle seçilen ya da kendiliğinden "away".
    status: Mutex<String>,
    nudge_sent: Mutex<HashMap<String, Instant>>,
    nudge_seen: Mutex<HashMap<String, Instant>>,
    effect_sent: Mutex<HashMap<String, Instant>>,
    effect_seen: Mutex<HashMap<String, Instant>>,
    event_rate: Mutex<HashMap<String, VecDeque<Instant>>>,
    clip_cancel: AtomicBool,
    pub screen: screen::State,
    pub phone: phone::State,
    pub watch: watch::State,
    pub arena: arena::State,
}

impl State {
    pub fn status_now(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

/// Animasyonun sohbet notundaki simgesi ve adı.
fn effect_label(effect: &str) -> (&'static str, &'static str) {
    match effect {
        "gift" => ("🎁", "hediye"),
        "heart" => ("❤️", "kalp"),
        _ => ("🔫", "kurşun"),
    }
}

/// Ağdan gelen alıntı özetini temizler.
pub fn clean_reply(r: ReplyRef) -> ReplyRef {
    ReplyRef {
        id: if valid_message_id(&r.id) { r.id } else { String::new() },
        text: sanitize_text(&r.text, REPLY_SNIPPET_CHARS),
        name: sanitize_name(&r.name),
    }
}

/// Klavye/fare en son ne zaman kullanıldı?
#[cfg(windows)]
pub fn idle_time() -> Duration {
    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount() -> u32;
    }
    let mut info = LastInputInfo { cb_size: std::mem::size_of::<LastInputInfo>() as u32, dw_time: 0 };
    // SAFETY: yapı Windows'un beklediği düzende ve boyutu cb_size'da bildiriliyor.
    unsafe {
        if GetLastInputInfo(&mut info) == 0 {
            return Duration::ZERO;
        }
        Duration::from_millis(GetTickCount().wrapping_sub(info.dw_time) as u64)
    }
}

#[cfg(target_os = "macos")]
pub fn idle_time() -> Duration {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(source_state: i32, event_type: u32) -> f64;
    }
    // kCGEventSourceStateCombinedSessionState = 0, kCGAnyInputEventType = !0
    // SAFETY: parametreleri sabit, yan etkisiz CoreGraphics çağrısı.
    let secs = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    if secs.is_finite() && secs > 0.0 {
        Duration::from_secs_f64(secs)
    } else {
        Duration::ZERO
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn idle_time() -> Duration {
    Duration::ZERO
}

#[cfg(windows)]
fn clipboard_seq() -> u32 {
    #[link(name = "user32")]
    extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
    }
    // SAFETY: parametresiz, yan etkisiz Windows çağrısı.
    unsafe { GetClipboardSequenceNumber() }
}

/// RGBA görüntüyü bellekte PNG'ye çevirir.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

impl Node {
    // ───────────────────────────── Durum ─────────────────────────────

    /// Etkin durumu yeniden hesaplar (keşif döngüsü birkaç saniyede bir çağırır).
    pub fn status_tick(&self) {
        let manual = self.settings.lock().unwrap().status.clone();
        let effective = if !manual.is_empty() {
            manual.clone()
        } else if idle_time() >= AUTO_AWAY {
            "away".to_string()
        } else {
            String::new()
        };
        let changed = {
            let mut s = self.extras.status.lock().unwrap();
            if *s != effective {
                *s = effective.clone();
                true
            } else {
                false
            }
        };
        if changed {
            self.emit("my-status", json!({ "status": effective, "auto": manual.is_empty() && !effective.is_empty() }));
        }
    }

    pub fn set_status(&self, status: &str) -> Result<Settings, String> {
        if !valid_status(status) {
            return Err("Geçersiz durum".into());
        }
        let s = self.update_settings(|s| s.status = status.to_string());
        self.status_tick();
        Ok(s)
    }

    /// "Odakta" iken bildirimler susar (bahsedilme ve onay bekleyen dosyalar hariç).
    pub fn do_not_disturb(&self) -> bool {
        self.extras.status_now() == "focus"
    }

    // ───────────────────────────── Yanıt ─────────────────────────────

    /// Yanıtlanan mesajın özeti (yalnızca bu sohbetteki gerçek bir mesaj için).
    pub(super) fn reply_ref(&self, key: &str, id: &str) -> Option<ReplyRef> {
        let m = self.chats.lock().unwrap().get(key, id)?;
        if m.system || m.deleted {
            return None;
        }
        let name = if m.outgoing {
            self.settings.lock().unwrap().device_name.clone()
        } else if let Some(n) = m.sender_name.clone() {
            n
        } else {
            self.chats.lock().unwrap().conversation_meta(key).map(|x| x.0).unwrap_or_default()
        };
        Some(ReplyRef { id: m.id, text: m.text.chars().take(REPLY_SNIPPET_CHARS).collect(), name })
    }

    // ───────────────────────────── Sohbet olayları ─────────────────────────────

    fn emit_updated(&self, key: &str, msg: &ChatMessage) {
        if let Some((name, unread)) = self.chats.lock().unwrap().conversation_meta(key) {
            self.emit("chat", json!({ "fingerprint": key, "name": name, "unread": unread, "incoming": false, "message": msg }));
        }
    }

    /// Olayın gideceği kişiler: grupta diğer üyeler, birebir sohbette karşı cihaz.
    fn event_targets(&self, key: &str) -> Result<(Vec<String>, Option<String>), String> {
        let me = self.identity.fingerprint.clone();
        match self.group_of(key) {
            Some((g, left)) => {
                if left || g.deleted {
                    return Err("Bu grubun üyesi değilsiniz".into());
                }
                Ok((g.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect(), Some(g.id)))
            }
            None => Ok((vec![key.to_string()], None)),
        }
    }

    /// Olayı çevrimiçi kişilere iletir (çevrimdışı olanlar almaz; mesajın kendisi giden kutusundadır).
    pub(super) fn broadcast_event(self: &Arc<Self>, key: &str, event: ChatEventKind) -> Result<(), String> {
        let (targets, group_id) = self.event_targets(key)?;
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return Ok(()) };
        for peer in self.peers().into_iter().filter(|p| !p.blocked && targets.contains(&p.fingerprint)) {
            let node = self.clone();
            let (group_id, event) = (group_id.clone(), event.clone());
            rt.spawn(async move {
                let msg = Message::ChatEvent { device: node.device_info(), group_id, event };
                if let Ok(mut s) = node.connect(&node.candidates(&peer), Some(peer.fingerprint.clone())).await {
                    if write_msg(&mut s, &msg).await.is_ok() {
                        let _ = timeout(Duration::from_secs(5), read_msg_max(&mut s, 4096)).await;
                    }
                    let _ = s.shutdown().await;
                }
            });
        }
        Ok(())
    }

    fn apply_reaction(&self, key: &str, id: &str, fp: &str, emoji: &str) -> Option<ChatMessage> {
        let current = self.chats.lock().unwrap().get(key, id)?;
        if current.system || current.deleted {
            return None;
        }
        let m = self.chats.lock().unwrap().update_message(key, id, |m| {
            m.reactions.retain(|r| r.fp != fp);
            if !emoji.is_empty() {
                m.reactions.push(Reaction { fp: fp.to_string(), emoji: emoji.to_string() });
            }
        })?;
        self.emit_updated(key, &m);
        Some(m)
    }

    /// Mesaja tepki verir (aynı emoji tekrar seçilirse arayüz boş emoji gönderip kaldırır).
    pub fn chat_react(self: &Arc<Self>, key: &str, id: &str, emoji: &str) -> Result<ChatMessage, String> {
        if !emoji.is_empty() && !REACTIONS.contains(&emoji) {
            return Err("Geçersiz tepki".into());
        }
        let me = self.identity.fingerprint.clone();
        let m = self.apply_reaction(key, id, &me, emoji).ok_or("Mesaj bulunamadı")?;
        self.broadcast_event(key, ChatEventKind::React { id: id.to_string(), emoji: emoji.to_string() })?;
        Ok(m)
    }

    fn own_text_message(&self, key: &str, id: &str) -> Result<ChatMessage, String> {
        let m = self.chats.lock().unwrap().get(key, id).ok_or("Mesaj bulunamadı")?;
        if !m.outgoing || m.system || m.file.is_some() {
            return Err("Yalnızca kendi yazdığınız mesajlar değiştirilebilir".into());
        }
        if m.deleted {
            return Err("Mesaj silinmiş".into());
        }
        Ok(m)
    }

    pub fn chat_edit(self: &Arc<Self>, key: &str, id: &str, text: &str) -> Result<ChatMessage, String> {
        let m = self.own_text_message(key, id)?;
        if now_ms().saturating_sub(m.at) > EDIT_WINDOW_MS {
            return Err("Mesaj yalnızca gönderildikten sonraki 24 saat içinde düzenlenebilir".into());
        }
        if text.chars().count() > MAX_CHAT_CHARS {
            return Err(format!("Mesaj çok uzun (en fazla {MAX_CHAT_CHARS} karakter)"));
        }
        let text = sanitize_text(text, MAX_CHAT_CHARS);
        if text.is_empty() {
            return Err("Mesaj boş olamaz; silmek için Sil'i kullanın".into());
        }
        if text == m.text {
            return Ok(m);
        }
        let updated = self
            .chats
            .lock()
            .unwrap()
            .update_message(key, id, |m| {
                m.text = text.clone();
                m.edited = true;
            })
            .ok_or("Mesaj bulunamadı")?;
        self.emit_updated(key, &updated);
        // Henüz ulaşmamış alıcılara (giden kutusu) zaten yeni metin gider.
        self.broadcast_event(key, ChatEventKind::Edit { id: id.to_string(), text })?;
        Ok(updated)
    }

    pub fn chat_delete(self: &Arc<Self>, key: &str, id: &str) -> Result<ChatMessage, String> {
        self.own_text_message(key, id)?;
        let updated = self
            .chats
            .lock()
            .unwrap()
            .update_message(key, id, |m| {
                m.text.clear();
                m.deleted = true;
                m.reactions.clear();
                m.reply = None;
                // Henüz ulaşmamış alıcılara artık gönderilmez.
                m.pending.clear();
                crate::chat::settle(m);
            })
            .ok_or("Mesaj bulunamadı")?;
        self.emit_updated(key, &updated);
        self.broadcast_event(key, ChatEventKind::Delete { id: id.to_string() })?;
        Ok(updated)
    }

    /// Birebir sohbette karşı tarafı dürter: penceresi titrer.
    pub fn chat_nudge(self: &Arc<Self>, fp: &str) -> Result<(), String> {
        if self.group_of(fp).is_some() {
            return Err("Grupta dürtme yapılamaz".into());
        }
        let peer = self.peers().into_iter().find(|p| p.fingerprint == fp).ok_or("Kişi çevrimdışı")?;
        if peer.blocked {
            return Err("Bu cihazı engellediniz".into());
        }
        {
            let mut sent = self.extras.nudge_sent.lock().unwrap();
            if sent.get(fp).is_some_and(|t| t.elapsed() < NUDGE_GAP) {
                return Err("Az önce dürttünüz, biraz bekleyin".into());
            }
            sent.insert(fp.to_string(), Instant::now());
        }
        self.push_note(fp, &peer.name, format!("👋 {} kişisini dürttünüz", peer.name));
        self.broadcast_event(fp, ChatEventKind::Nudge)?;
        Ok(())
    }

    /// Sohbete ekran animasyonu gönderir (hediye, kalp, kurşun): karşının tüm ekranında oynar.
    /// Birebir sohbette kişi çevrimiçi olmalı; grupta çevrimiçi üyelere gider.
    pub fn chat_effect(self: &Arc<Self>, key: &str, effect: &str) -> Result<(), String> {
        if !EFFECTS.contains(&effect) {
            return Err("Geçersiz animasyon".into());
        }
        let (targets, group_id) = self.event_targets(key)?;
        let online: Vec<Peer> = self.peers().into_iter().filter(|p| !p.blocked && targets.contains(&p.fingerprint)).collect();
        let name = match &group_id {
            Some(_) => {
                if online.is_empty() {
                    return Err("Grupta şu an çevrimiçi kimse yok".into());
                }
                String::new()
            }
            None => {
                if self.is_blocked(key) {
                    return Err("Bu cihazı engellediniz".into());
                }
                online.first().ok_or("Kişi çevrimdışı")?.name.clone()
            }
        };
        {
            let mut sent = self.extras.effect_sent.lock().unwrap();
            if let Some(left) = sent.get(key).and_then(|t| EFFECT_GAP.checked_sub(t.elapsed())) {
                return Err(format!("Art arda gönderilemez, {} sn bekleyin", left.as_secs() + 1));
            }
            sent.insert(key.to_string(), Instant::now());
        }
        let (icon, what) = effect_label(effect);
        let verb = if effect == "bullet" { "sıktınız" } else { "gönderdiniz" };
        let note = match group_id {
            Some(_) => format!("{icon} Gruba {what} {verb}"),
            None => format!("{icon} {name} kişisine {what} {verb}"),
        };
        self.push_note(key, &self.conv_name(key, &name), note);
        self.broadcast_event(key, ChatEventKind::Effect { effect: effect.to_string() })?;
        Ok(())
    }

    /// Sohbetin kayıtlı adı (grupta grup adı); yoksa verilen ad.
    fn conv_name(&self, key: &str, fallback: &str) -> String {
        self.chats.lock().unwrap().conversation_meta(key).map(|m| m.0).filter(|n| !n.is_empty()).unwrap_or_else(|| fallback.to_string())
    }

    /// Sohbete bilgi satırı ekler (sohbet yoksa oluşturur); okunmamış sayılmaz.
    fn push_note(&self, key: &str, name: &str, text: String) {
        let msg = ChatMessage { id: uuid::Uuid::new_v4().to_string(), text, at: now_ms(), system: true, ..Default::default() };
        self.chats.lock().unwrap().push_quiet(key, name, msg.clone());
        self.emit_updated(key, &msg);
    }

    fn event_rate_ok(&self, fp: &str) -> bool {
        rate_ok(&self.extras.event_rate, fp, EVENT_RATE_MAX, EVENT_RATE_WINDOW)
    }

    pub(super) fn accept_chat_event(&self, device: &DeviceInfo, ip: IpAddr, group_id: Option<&str>, event: ChatEventKind) {
        let fp = device.fingerprint.clone();
        if self.is_blocked(&fp) || !self.event_rate_ok(&fp) {
            return;
        }
        let key = match group_id {
            Some(gid) => match self.shared_group(gid, &fp) {
                Some((key, _)) => key,
                None => return,
            },
            None => fp.clone(),
        };
        // Mesajı gerçekten bu kişi mi yazdı?
        let authored = |m: &ChatMessage| {
            !m.system && m.file.is_none() && if group_id.is_some() { m.sender.as_deref() == Some(fp.as_str()) } else { !m.outgoing }
        };
        match event {
            ChatEventKind::React { id, emoji } => {
                if valid_message_id(&id) && (emoji.is_empty() || REACTIONS.contains(&emoji.as_str())) {
                    self.apply_reaction(&key, &id, &fp, &emoji);
                }
            }
            ChatEventKind::Edit { id, text } => {
                let Some(m) = self.chats.lock().unwrap().get(&key, &id) else { return };
                let text = sanitize_text(&text, MAX_CHAT_CHARS);
                if !authored(&m) || m.deleted || text.is_empty() {
                    return;
                }
                let mention = group_id.is_some() && crate::chat::mentions(&text, &self.settings.lock().unwrap().device_name);
                let updated = self.chats.lock().unwrap().update_message(&key, &id, |m| {
                    m.text = text;
                    m.edited = true;
                    m.mention = mention;
                });
                if let Some(u) = updated {
                    self.emit_updated(&key, &u);
                }
            }
            ChatEventKind::Delete { id } => {
                let Some(m) = self.chats.lock().unwrap().get(&key, &id) else { return };
                if !authored(&m) {
                    return;
                }
                let updated = self.chats.lock().unwrap().update_message(&key, &id, |m| {
                    m.text.clear();
                    m.deleted = true;
                    m.reactions.clear();
                    m.reply = None;
                    m.mention = false;
                });
                if let Some(u) = updated {
                    self.emit_updated(&key, &u);
                }
            }
            ChatEventKind::Nudge => {
                if group_id.is_some() {
                    return;
                }
                {
                    let mut seen = self.extras.nudge_seen.lock().unwrap();
                    if seen.get(&fp).is_some_and(|t| t.elapsed() < NUDGE_GAP) {
                        return;
                    }
                    seen.insert(fp.clone(), Instant::now());
                }
                let name = sanitize_name(&device.name);
                self.upsert_peer(device.clone(), ip, None);
                self.push_note(&fp, &name, format!("👋 {name} sizi dürttü"));
                self.emit("nudge", json!({ "fingerprint": fp, "name": name }));
            }
            ChatEventKind::Effect { effect } => {
                if !EFFECTS.contains(&effect.as_str()) {
                    return;
                }
                {
                    let mut seen = self.extras.effect_seen.lock().unwrap();
                    if seen.get(&fp).is_some_and(|t| t.elapsed() < EFFECT_GAP) {
                        return;
                    }
                    seen.insert(fp.clone(), Instant::now());
                }
                let name = sanitize_name(&device.name);
                let (icon, what) = effect_label(&effect);
                let verb = if effect == "bullet" { "sıktı" } else { "gönderdi" };
                let note = if group_id.is_some() {
                    format!("{icon} {name} gruba {what} {verb}")
                } else {
                    self.upsert_peer(device.clone(), ip, None);
                    format!("{icon} {name} size {what} {verb}")
                };
                self.push_note(&key, &self.conv_name(&key, &name), note);
                self.emit(
                    "effect",
                    json!({ "key": key, "fingerprint": fp, "name": name, "effect": effect, "group": group_id.is_some() }),
                );
            }
        }
    }

    // ───────────────────────────── Çevrimiçi olunca gönder ─────────────────────────────

    /// Çevrimdışı kişiye dosya bırakır: kişi çevrimiçi olunca (3 gün içinde) kendiliğinden gider.
    /// Kişi zaten çevrimiçiyse hemen gönderilir.
    pub async fn chat_queue_files(self: &Arc<Self>, fp: &str, name: &str, paths: Vec<String>) -> Result<(), String> {
        if !valid_fingerprint(fp) {
            return Err("Geçersiz kişi".into());
        }
        if self.is_blocked(fp) {
            return Err("Bu cihazı engellediniz".into());
        }
        if let Some(peer) = self.peers().into_iter().find(|p| p.fingerprint == fp) {
            self.send_paths(&[peer.id], paths).await?;
            return Ok(());
        }
        let (entries, title) = scan_sources(&paths).await?;
        let name = self.chats.lock().unwrap().conversation_meta(fp).map(|m| m.0).unwrap_or_else(|| sanitize_name(name));
        let msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            outgoing: true,
            text: format!("📎 {title}"),
            at: now_ms(),
            status: MsgStatus::Sending,
            pending: vec![fp.to_string()],
            recipients: 1,
            file: Some(FileRef {
                title,
                count: entries.len() as u32,
                total: entries.iter().map(|e| e.1.size).sum(),
                transfers: Vec::new(),
                sources: paths,
            }),
            ..Default::default()
        };
        self.chats.lock().unwrap().push(fp, &name, msg.clone());
        self.emit_updated(fp, &msg);
        Ok(())
    }

    // ───────────────────────────── Ekran görüntüsü ─────────────────────────────

    /// Windows ekran alıntısı aracını açar ve seçilen alanı PNG olarak döndürür.
    #[cfg(windows)]
    pub async fn screen_clip(&self) -> Result<Vec<u8>, String> {
        self.extras.clip_cancel.store(false, Ordering::Relaxed);
        let before = clipboard_seq();
        std::process::Command::new("explorer.exe")
            .arg("ms-screenclip:")
            .spawn()
            .map_err(|e| format!("Ekran alıntısı aracı açılamadı: {e}"))?;
        let start = Instant::now();
        while start.elapsed() < CLIP_WAIT {
            sleep(Duration::from_millis(200)).await;
            if self.extras.clip_cancel.load(Ordering::Relaxed) {
                return Err("İptal edildi".into());
            }
            if clipboard_seq() == before {
                continue;
            }
            // Pano değişti: görüntü hazır olana kadar kısa bir süre dene.
            for _ in 0..10 {
                let img = tokio::task::spawn_blocking(|| {
                    let mut cb = arboard::Clipboard::new().ok()?;
                    let img = cb.get().image().ok()?;
                    Some((img.width as u32, img.height as u32, img.bytes.into_owned()))
                })
                .await
                .ok()
                .flatten();
                if let Some((w, h, rgba)) = img {
                    if w > 0 && h > 0 {
                        return tokio::task::spawn_blocking(move || encode_png(w, h, &rgba)).await.map_err(|e| e.to_string())?;
                    }
                }
                sleep(Duration::from_millis(150)).await;
            }
            return Err("Panoya görüntü gelmedi".into());
        }
        Err("Ekran görüntüsü alınmadı".into())
    }

    /// macOS'un `screencapture` aracıyla alan seçtirir ve seçilen alanı PNG olarak döndürür.
    #[cfg(target_os = "macos")]
    pub async fn screen_clip(&self) -> Result<Vec<u8>, String> {
        self.extras.clip_cancel.store(false, Ordering::Relaxed);
        let out = std::env::temp_dir().join(format!("fastfiles-clip-{}.png", uuid::Uuid::new_v4()));
        // -i: alan/pencere seçimi, -x: deklanşör sesi yok. Esc ile vazgeçilirse dosya oluşmaz.
        let mut child = tokio::process::Command::new("/usr/sbin/screencapture")
            .arg("-i")
            .arg("-x")
            .arg(&out)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Ekran görüntüsü aracı açılamadı: {e}"))?;
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    let _ = std::fs::remove_file(&out);
                    return Err("Ekran görüntüsü alınmadı".into());
                }
                break;
            }
            if self.extras.clip_cancel.load(Ordering::Relaxed) || start.elapsed() >= CLIP_WAIT {
                let _ = child.kill().await;
                let _ = std::fs::remove_file(&out);
                return Err(if start.elapsed() >= CLIP_WAIT { "Ekran görüntüsü alınmadı" } else { "İptal edildi" }.into());
            }
            sleep(Duration::from_millis(150)).await;
        }
        let bytes = tokio::fs::read(&out).await.map_err(|_| "Ekran görüntüsü alınmadı".to_string());
        let _ = std::fs::remove_file(&out);
        bytes
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub async fn screen_clip(&self) -> Result<Vec<u8>, String> {
        Err("Bu özellik bu işletim sisteminde yok".into())
    }

    pub fn screen_clip_cancel(&self) {
        self.extras.clip_cancel.store(true, Ordering::Relaxed);
    }

    /// Arayüzde üzerine çizilen resmi kaydeder ve gönderir: grup sohbetine, birebir sohbete
    /// (kişi çevrimdışıysa çevrimiçi olunca) ya da seçilen cihazlara.
    pub async fn send_png(self: &Arc<Self>, data: &[u8], key: Option<&str>, peer_ids: &[String], stamp: &str) -> Result<(), String> {
        if data.len() > MAX_PNG_BYTES {
            return Err("Resim çok büyük".into());
        }
        if !data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Err("Geçersiz resim".into());
        }
        let path = crate::clipboard::save_png_bytes(&self.clip_dir, stamp, data)?.to_string_lossy().into_owned();
        match key {
            Some(k) if self.group_of(k).is_some() => self.group_send_files(k, vec![path]).await.map(|_| ()),
            Some(k) => {
                let name = self.chats.lock().unwrap().conversation_meta(k).map(|m| m.0).unwrap_or_default();
                self.chat_queue_files(k, &name, vec![path]).await
            }
            None if !peer_ids.is_empty() => self.send_paths(peer_ids, vec![path]).await.map(|_| ()),
            None => Err("Alıcı seçilmedi".into()),
        }
    }
}
