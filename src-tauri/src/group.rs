//! Grup mesajlaşması (`node` modülünün alt modülü).
//!
//! Sunucu yoktur: grup bilgisi her üyede ayrı tutulur ve her mesaj her üyeye ayrı ayrı,
//! karşılıklı TLS ile gönderilir. Çevrimdışı üyelere gidecekler "giden kutusunda" bekler ve
//! üye çevrimiçi olunca otomatik teslim edilir (en fazla 3 gün).
//!
//! Güvenlik kuralları:
//! - Grup bilgisini (ad, üyeler, silme) yalnızca grubu oluşturan cihaz (yönetici) değiştirebilir.
//!   Yönetici, isim ile değil TLS sertifika parmak iziyle tanınır; her değişiklikte `rev` artar.
//! - Grup mesajı yalnızca o grubun üyesi olan bir cihazdan kabul edilir.
//! - Engellenen cihazlar, hız sınırı, uzunluk sınırı ve metin temizliği birebir sohbetteki gibidir.

use super::*;
use crate::chat::{group_key, settle, Conversation, FileRef, FileTransferRef, GroupInfo, GroupMember, OutboxItem};

/// Bekleyen bildirim listesine ekler; henüz bildirilmemiş eski alıcılar (ör. az önce gruptan
/// çıkarılan üye) listeden düşmez.
fn merge_pending(list: &mut Vec<String>, add: Vec<String>) {
    for f in add {
        if !list.contains(&f) {
            list.push(f);
        }
    }
}

/// Başarısız teslimattan sonra aynı alıcıyı yeniden denemeden önce beklenecek süre.
const OUTBOX_BACKOFF: Duration = Duration::from_secs(20);
/// Alıcı grubu henüz öğrenmediyse (bilgi yolda) kısa süre sonra tekrar dene.
const OUTBOX_RETRY_SOON: Duration = Duration::from_secs(2);

enum Delivery {
    Done,
    RetrySoon,
    RetryLater,
}

fn clean_group_name(name: &str) -> Result<String, String> {
    let n = sanitize_text(name, MAX_NAME_CHARS).replace(['\n', '\t'], " ");
    let n = n.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        Err("Grup adı boş olamaz".into())
    } else {
        Ok(n)
    }
}

/// Ağdan gelen grup bilgisini doğrular ve temizler.
fn clean_group(mut g: GroupInfo) -> Option<GroupInfo> {
    if !valid_message_id(&g.id) || !valid_fingerprint(&g.admin) {
        return None;
    }
    g.name = clean_group_name(&g.name).ok()?;
    let mut seen = HashSet::new();
    g.members = g
        .members
        .into_iter()
        .filter(|m| valid_fingerprint(&m.fingerprint) && seen.insert(m.fingerprint.clone()))
        .map(|m| GroupMember { name: sanitize_name(&m.name), fingerprint: m.fingerprint })
        .take(MAX_GROUP_MEMBERS)
        .collect();
    Some(g)
}

fn join_names(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        _ => format!("{} ve {}", names[..names.len() - 1].join(", "), names[names.len() - 1]),
    }
}

impl Node {
    fn my_fp(&self) -> String {
        self.identity.fingerprint.clone()
    }

    /// Bir parmak izi için bilinen ad: önce çevrimiçi cihazlar, sonra sohbet geçmişi.
    fn known_name(&self, fp: &str) -> Option<String> {
        if let Some(p) = self.peers().into_iter().find(|p| p.fingerprint == fp) {
            return Some(p.name);
        }
        self.chats.lock().unwrap().conv(fp).filter(|c| c.group.is_none()).map(|c| c.name.clone())
    }

    fn emit_conversation(&self, key: &str) {
        let summary = self.chats.lock().unwrap().summary(key);
        if let Some(s) = summary {
            self.emit("conversation", json!(s));
        }
    }

    fn emit_chat_message(&self, key: &str, msg: &ChatMessage, incoming: bool) {
        let meta = self.chats.lock().unwrap().conversation_meta(key);
        if let Some((name, unread)) = meta {
            self.emit("chat", json!({ "fingerprint": key, "name": name, "unread": unread, "incoming": incoming, "message": msg }));
        }
    }

    fn system_note(&self, key: &str, text: String) {
        let msg = self.chats.lock().unwrap().push_system(key, text, now_ms());
        if let Some(m) = msg {
            self.emit_chat_message(key, &m, false);
        }
    }

    pub(super) fn group_of(&self, key: &str) -> Option<(GroupInfo, bool)> {
        let chats = self.chats.lock().unwrap();
        let c = chats.conv(key)?;
        Some((c.group.clone()?, c.left))
    }

    // ───────────── Kullanıcı işlemleri ─────────────

    /// Yeni grup oluşturur; bu cihaz grubun yöneticisi olur.
    pub fn group_create(self: &Arc<Self>, name: &str, member_fps: &[String]) -> Result<ConversationSummary, String> {
        let name = clean_group_name(name)?;
        let me = self.my_fp();
        let mut members = vec![GroupMember { fingerprint: me.clone(), name: self.device_info().name }];
        for fp in member_fps {
            if *fp == me || !valid_fingerprint(fp) || members.iter().any(|m| m.fingerprint == *fp) {
                continue;
            }
            if let Some(n) = self.known_name(fp) {
                members.push(GroupMember { fingerprint: fp.clone(), name: n });
            }
        }
        if members.len() < 2 {
            return Err("Gruba en az bir kişi ekleyin".into());
        }
        if members.len() > MAX_GROUP_MEMBERS {
            return Err(format!("Bir grupta en fazla {MAX_GROUP_MEMBERS} kişi olabilir"));
        }
        let group = GroupInfo { id: uuid::Uuid::new_v4().to_string(), name: name.clone(), admin: me.clone(), members, rev: 1, deleted: false };
        let key = group_key(&group.id);
        let others: Vec<String> = group.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
        self.chats.lock().unwrap().insert(Conversation {
            fingerprint: key.clone(),
            name,
            group: Some(group),
            sync_pending: others,
            ..Default::default()
        });
        self.system_note(&key, "Grubu oluşturdunuz".into());
        self.emit_conversation(&key);
        self.flush_outbox();
        self.chats.lock().unwrap().summary(&key).ok_or("Grup oluşturulamadı".into())
    }

    /// (Yalnızca yönetici) Grup adını değiştirir, üye ekler/çıkarır.
    pub fn group_edit(
        self: &Arc<Self>,
        key: &str,
        name: Option<String>,
        add: &[String],
        remove: &[String],
    ) -> Result<ConversationSummary, String> {
        let me = self.my_fp();
        let (old, left) = self.group_of(key).ok_or("Grup bulunamadı")?;
        if old.admin != me {
            return Err("Grubu yalnızca oluşturan kişi düzenleyebilir".into());
        }
        if left || old.deleted {
            return Err("Bu grup artık etkin değil".into());
        }
        let mut g = old.clone();
        let mut notes = Vec::new();
        if let Some(n) = name {
            let n = clean_group_name(&n)?;
            if n != g.name {
                notes.push(format!("Grup adını “{n}” olarak değiştirdiniz"));
                g.name = n;
            }
        }
        let mut removed = Vec::new();
        for fp in remove {
            if *fp != me && g.has(fp) {
                removed.push(g.member_name(fp).unwrap_or("?").to_string());
                g.members.retain(|m| m.fingerprint != *fp);
            }
        }
        let mut added = Vec::new();
        for fp in add {
            if *fp == me || !valid_fingerprint(fp) || g.has(fp) {
                continue;
            }
            if let Some(n) = self.known_name(fp) {
                added.push(n.clone());
                g.members.push(GroupMember { fingerprint: fp.clone(), name: n });
            }
        }
        if g.members.len() > MAX_GROUP_MEMBERS {
            return Err(format!("Bir grupta en fazla {MAX_GROUP_MEMBERS} kişi olabilir"));
        }
        if g == old {
            return self.chats.lock().unwrap().summary(key).ok_or("Grup bulunamadı".into());
        }
        if !added.is_empty() {
            notes.push(format!("{} gruba eklendi", join_names(&added)));
        }
        if !removed.is_empty() {
            notes.push(format!("{} gruptan çıkarıldı", join_names(&removed)));
        }
        g.rev += 1;
        // Güncel bilgi: kalan üyelere ve çıkarılanlara (çıkarıldıklarını öğrensinler).
        let mut sync: Vec<String> = g.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
        sync.extend(old.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me && !g.has(f)));
        let new_group = g.clone();
        self.chats.lock().unwrap().with(key, |c| {
            c.name = new_group.name.clone();
            c.group = Some(new_group.clone());
            merge_pending(&mut c.sync_pending, sync);
            for m in c.messages.iter_mut() {
                m.pending.retain(|fp| new_group.has(fp));
                settle(m);
            }
        });
        for n in notes {
            self.system_note(key, n);
        }
        self.emit_conversation(key);
        self.flush_outbox();
        self.chats.lock().unwrap().summary(key).ok_or("Grup bulunamadı".into())
    }

    /// Gruptan ayrılır. Yönetici ayrılırsa grup herkes için silinir.
    pub fn group_leave(self: &Arc<Self>, key: &str) -> Result<(), String> {
        let me = self.my_fp();
        let (g, left) = self.group_of(key).ok_or("Grup bulunamadı")?;
        if left {
            return Ok(());
        }
        if g.admin == me {
            let mut deleted = g.clone();
            deleted.deleted = true;
            deleted.rev += 1;
            let others: Vec<String> = g.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
            self.chats.lock().unwrap().with(key, |c| {
                c.group = Some(deleted);
                c.left = true;
                merge_pending(&mut c.sync_pending, others);
            });
            self.system_note(key, "Grubu herkes için sildiniz".into());
        } else {
            self.chats.lock().unwrap().with(key, |c| {
                c.left = true;
                c.leave_pending = true;
            });
            self.system_note(key, "Gruptan ayrıldınız".into());
        }
        self.emit_conversation(key);
        self.flush_outbox();
        Ok(())
    }

    /// Gruba mesaj gönderir. Çevrimdışı üyelere sonra otomatik iletilir.
    pub fn group_send(self: &Arc<Self>, key: &str, text: &str, reply_to: Option<&str>) -> Result<ChatMessage, String> {
        let me = self.my_fp();
        let (g, left) = self.group_of(key).ok_or("Grup bulunamadı")?;
        if left || g.deleted || !g.has(&me) {
            return Err("Bu grubun üyesi değilsiniz".into());
        }
        if text.chars().count() > MAX_CHAT_CHARS {
            return Err(format!("Mesaj çok uzun (en fazla {MAX_CHAT_CHARS} karakter)"));
        }
        let text = sanitize_text(text, MAX_CHAT_CHARS);
        if text.is_empty() {
            return Err("Mesaj boş".into());
        }
        let others: Vec<String> = g.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
        if others.is_empty() {
            return Err("Grupta başka üye yok".into());
        }
        let msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            outgoing: true,
            text,
            at: now_ms(),
            status: MsgStatus::Sending,
            recipients: others.len() as u32,
            pending: others,
            reply: reply_to.and_then(|id| self.reply_ref(key, id)),
            ..Default::default()
        };
        self.chats.lock().unwrap().push(key, &g.name, msg.clone());
        self.emit_chat_message(key, &msg, false);
        self.flush_outbox();
        Ok(msg)
    }

    /// Gruba dosya/klasör gönderir: çevrimiçi üyelere hemen, çevrimdışı olanlara geri
    /// geldiklerinde (en fazla 3 gün) ayrı ayrı aktarım başlar. Her üye kendi ekranında onaylar.
    pub async fn group_send_files(self: &Arc<Self>, key: &str, paths: Vec<String>) -> Result<ChatMessage, String> {
        let me = self.my_fp();
        let (g, left) = self.group_of(key).ok_or("Grup bulunamadı")?;
        if left || g.deleted || !g.has(&me) {
            return Err("Bu grubun üyesi değilsiniz".into());
        }
        let others: Vec<String> = g.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
        if others.is_empty() {
            return Err("Grupta başka üye yok".into());
        }
        let (entries, title) = scan_sources(&paths).await?;
        let file = FileRef {
            title: title.clone(),
            count: entries.len() as u32,
            total: entries.iter().map(|e| e.1.size).sum(),
            transfers: Vec::new(),
            sources: paths,
        };
        let msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            outgoing: true,
            text: format!("📎 {title}"),
            at: now_ms(),
            status: MsgStatus::Sending,
            recipients: others.len() as u32,
            pending: others,
            file: Some(file),
            ..Default::default()
        };
        self.chats.lock().unwrap().push(key, &g.name, msg.clone());
        self.emit_chat_message(key, &msg, false);
        self.flush_outbox();
        Ok(msg)
    }

    // ───────────── Giden kutusu ─────────────

    /// Bekleyen grup teslimatlarını çevrimiçi üyelere gönderir. Keşif döngüsü bunu birkaç
    /// saniyede bir çağırır; böylece çevrimdışı üyeler geri gelince mesajlarını alır.
    pub fn flush_outbox(self: &Arc<Self>) {
        let items = self.chats.lock().unwrap().outbox(now_ms(), &self.identity.fingerprint);
        if items.is_empty() {
            return;
        }
        let online: HashMap<String, Peer> =
            self.peers().into_iter().filter(|p| !p.blocked).map(|p| (p.fingerprint.clone(), p)).collect();
        // Bir üye grubun güncel bilgisini almadıysa önce o gitsin, mesajlar sonraki turda.
        let syncing: HashSet<(String, String)> = items
            .iter()
            .filter_map(|i| match i {
                OutboxItem::GroupSync { key, to, .. } => Some((key.clone(), to.clone())),
                _ => None,
            })
            .collect();
        // Aynı alıcıya aynı sohbetten mesajlar sırayla gitsin (eşzamanlı gönderim sırayı bozar
        // ve alıcının hız sınırına takılır): her turda alıcı başına en eski bekleyen mesaj.
        let busy_pairs: HashSet<(String, String)> = self
            .outbox_in_flight
            .lock()
            .unwrap()
            .iter()
            .filter_map(|f| f.strip_prefix("msg:").and_then(|r| r.rsplit_once(':')).and_then(|(kt, _)| kt.rsplit_once(':')))
            .map(|(k, t)| (k.to_string(), t.to_string()))
            .collect();
        let mut queued_pairs: HashSet<(String, String)> = HashSet::new();
        for item in items {
            if let OutboxItem::Message { key, to, .. } = &item {
                let pair = (key.clone(), to.clone());
                if busy_pairs.contains(&pair) || !queued_pairs.insert(pair) {
                    continue;
                }
            }
            let (to, flight) = match &item {
                OutboxItem::GroupSync { key, to, group } => (to.clone(), format!("sync:{key}:{to}:{}", group.rev)),
                OutboxItem::Message { key, to, msg, .. } => {
                    if syncing.contains(&(key.clone(), to.clone())) {
                        continue;
                    }
                    (to.clone(), format!("msg:{key}:{to}:{}", msg.id))
                }
                OutboxItem::File { key, to, msg, .. } => {
                    if syncing.contains(&(key.clone(), to.clone())) {
                        continue;
                    }
                    (to.clone(), format!("file:{key}:{to}:{}", msg.id))
                }
                OutboxItem::Leave { key, to, .. } => (to.clone(), format!("leave:{key}:{to}")),
                OutboxItem::DirectFile { key, to, msg } => (to.clone(), format!("file:{key}:{to}:{}", msg.id)),
            };
            let Some(peer) = online.get(&to).cloned() else { continue };
            if self.outbox_backoff.lock().unwrap().get(&flight).map_or(false, |until| Instant::now() < *until) {
                continue;
            }
            if !self.outbox_in_flight.lock().unwrap().insert(flight.clone()) {
                continue;
            }
            let node = self.clone();
            tokio::spawn(async move {
                let result = node.deliver(item, &peer).await;
                {
                    let mut backoff = node.outbox_backoff.lock().unwrap();
                    match result {
                        Delivery::Done => backoff.remove(&flight),
                        Delivery::RetrySoon => backoff.insert(flight.clone(), Instant::now() + OUTBOX_RETRY_SOON),
                        Delivery::RetryLater => backoff.insert(flight.clone(), Instant::now() + OUTBOX_BACKOFF),
                    };
                }
                node.outbox_in_flight.lock().unwrap().remove(&flight);
                // Bir teslimat başarılıysa (ör. grup bilgisi ulaştı) arkasında bekleyenleri hemen gönder.
                if matches!(result, Delivery::Done) {
                    node.flush_outbox();
                }
            });
        }
    }

    async fn group_request(&self, peer: &Peer, msg: &Message) -> Result<Message, String> {
        let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut stream, msg).await.map_err(|e| e.to_string())?;
        let reply = timeout(Duration::from_secs(8), read_msg(&mut stream))
            .await
            .map_err(|_| "yanıt yok".to_string())?
            .map_err(|e| e.to_string())?;
        let _ = stream.shutdown().await;
        Ok(reply)
    }

    /// Tek bir teslimatı yapar.
    async fn deliver(self: &Arc<Self>, item: OutboxItem, peer: &Peer) -> Delivery {
        let device = self.device_info();
        match item {
            OutboxItem::GroupSync { key, to, group } => {
                let rev = group.rev;
                match self.group_request(peer, &Message::GroupUpdate { device, group }).await {
                    Ok(Message::GroupAck) | Ok(Message::ChatReject { .. }) => {
                        self.chats.lock().unwrap().with(&key, |c| {
                            if c.group.as_ref().map(|g| g.rev) == Some(rev) {
                                c.sync_pending.retain(|f| *f != to);
                            }
                        });
                        Delivery::Done
                    }
                    _ => Delivery::RetryLater,
                }
            }
            OutboxItem::Message { key, to, group_id, msg } => {
                let request = Message::GroupChat {
                    device,
                    group_id,
                    id: msg.id.clone(),
                    text: msg.text.clone(),
                    sent_at: msg.at,
                    reply: msg.reply.clone(),
                };
                let (done, unknown_group) = match self.group_request(peer, &request).await {
                    Ok(Message::ChatAck { id }) if id == msg.id => (true, false),
                    Ok(Message::ChatReject { reason }) => match reason.as_str() {
                        // Alıcı grubu henüz bilmiyor: yöneticiysek önce grup bilgisini gönder.
                        "unknown_group" => (false, true),
                        // Kalıcı ret: bu alıcı için vazgeç.
                        "not_member" | "sender_not_member" | "blocked" | "invalid" | "too_long" | "empty" => (true, false),
                        // "rate" vb.: sonra tekrar dene.
                        _ => (false, false),
                    },
                    _ => (false, false),
                };
                let me = self.my_fp();
                let mut am_admin = false;
                let updated = self.chats.lock().unwrap().with(&key, |c| {
                    am_admin = c.group.as_ref().map_or(false, |g| g.admin == me);
                    if unknown_group && am_admin && !c.sync_pending.contains(&to) {
                        c.sync_pending.push(to.clone());
                    }
                    let m = c.messages.iter_mut().find(|m| m.id == msg.id)?;
                    if done {
                        m.pending.retain(|f| *f != to);
                        if m.pending.is_empty() {
                            m.status = MsgStatus::Sent;
                            m.error = None;
                        }
                    }
                    Some(m.clone())
                });
                if let Some(Some(m)) = updated {
                    self.emit_chat_message(&key, &m, false);
                }
                if done {
                    Delivery::Done
                } else if unknown_group && am_admin {
                    // Grup bilgisi bir sonraki turda gidecek; ardından mesaj hemen denensin.
                    Delivery::RetrySoon
                } else {
                    // Yönetici değilsek grup bilgisini biz gönderemeyiz: sık deneme yapma.
                    Delivery::RetryLater
                }
            }
            OutboxItem::File { key, to, group_id, msg } => {
                // Üye çevrimiçi: ona ayrı bir aktarım başlar (onay, devam, doğrulama normal akışta).
                let Some(file) = msg.file.clone() else { return Delivery::Done };
                let started = match scan_sources(&file.sources).await {
                    Ok((entries, title)) => Ok(self.start_send(peer.clone(), entries, title, file.sources.clone(), None, Some(group_id))),
                    Err(e) => Err(e),
                };
                let updated = self.chats.lock().unwrap().update_message(&key, &msg.id, |m| {
                    match &started {
                        Ok(tid) => {
                            m.pending.retain(|f| *f != to);
                            if let Some(f) = m.file.as_mut() {
                                f.transfers.push(FileTransferRef { to: to.clone(), id: tid.clone() });
                            }
                        }
                        // Kaynak dosyalar silinmiş/taşınmış: kalan üyelere gönderilemez.
                        Err(e) => {
                            m.pending.clear();
                            m.error = Some(e.clone());
                        }
                    }
                    if m.pending.is_empty() && started.is_ok() {
                        m.status = MsgStatus::Sent;
                    } else if m.pending.is_empty() {
                        m.status = MsgStatus::Failed;
                    }
                });
                if let Some(m) = updated {
                    self.emit_chat_message(&key, &m, false);
                }
                Delivery::Done
            }
            OutboxItem::DirectFile { key, to, msg } => {
                // Kişi çevrimiçi oldu: bekleyen dosya normal bir gönderim olarak başlar.
                let Some(file) = msg.file.clone() else { return Delivery::Done };
                let started = match scan_sources(&file.sources).await {
                    Ok((entries, title)) => Ok(self.start_send(peer.clone(), entries, title, file.sources.clone(), None, None)),
                    Err(e) => Err(e),
                };
                match started {
                    // Aktarım kendi dosya balonunu ekler: bekleyen balon kaldırılır.
                    Ok(_) => {
                        self.chats.lock().unwrap().with(&key, |c| c.messages.retain(|m| m.id != msg.id));
                        self.emit("chat-remove", json!({ "fingerprint": key, "id": msg.id }));
                        let _ = to;
                    }
                    Err(e) => {
                        let updated = self.chats.lock().unwrap().update_message(&key, &msg.id, |m| {
                            m.pending.clear();
                            m.status = MsgStatus::Failed;
                            m.error = Some(e.clone());
                        });
                        if let Some(m) = updated {
                            self.emit_chat_message(&key, &m, false);
                        }
                    }
                }
                Delivery::Done
            }
            OutboxItem::Leave { key, group_id, .. } => match self.group_request(peer, &Message::GroupLeave { device, group_id }).await {
                Ok(Message::GroupAck) => {
                    self.chats.lock().unwrap().with(&key, |c| c.leave_pending = false);
                    Delivery::Done
                }
                _ => Delivery::RetryLater,
            },
        }
    }

    // ───────────── Ağdan gelenler ─────────────

    /// Yöneticiden gelen grup bilgisi.
    pub(super) fn accept_group_update(&self, sender_fp: &str, group: GroupInfo) -> Message {
        if self.is_blocked(sender_fp) {
            return Message::ChatReject { reason: "blocked".into() };
        }
        let Some(group) = clean_group(group) else {
            return Message::ChatReject { reason: "invalid".into() };
        };
        if group.admin != sender_fp {
            return Message::ChatReject { reason: "not_admin".into() };
        }
        let me = self.my_fp();
        let key = group_key(&group.id);
        let admin_name = group.member_name(&group.admin).unwrap_or("Yönetici").to_string();
        let existing = self.group_of(&key);
        let mut notes = Vec::new();
        match existing {
            Some((old, _)) => {
                // Başka biri aynı grup kimliğiyle yöneticiymiş gibi davranamaz.
                if old.admin != group.admin {
                    return Message::ChatReject { reason: "not_admin".into() };
                }
                if group.rev <= old.rev {
                    return Message::GroupAck;
                }
                // Kendi isteğimle ayrıldıysam (ayrılık bildirimi yolda ya da sohbeti sildim),
                // yöneticinin eski üye listesi beni gruba geri katmaz.
                let i_left = self.chats.lock().unwrap().conv(&key).map_or(false, |c| c.left && (c.leave_pending || c.hidden));
                let left = i_left || group.deleted || !group.has(&me);
                if i_left {
                    // Bilgi güncellenir ama not düşülmez.
                } else if group.deleted {
                    notes.push(format!("{admin_name} grubu sildi"));
                } else if !group.has(&me) {
                    notes.push(format!("{admin_name} sizi gruptan çıkardı"));
                } else {
                    if group.name != old.name {
                        notes.push(format!("{admin_name} grup adını “{}” olarak değiştirdi", group.name));
                    }
                    let added: Vec<String> = group.members.iter().filter(|m| !old.has(&m.fingerprint)).map(|m| m.name.clone()).collect();
                    let removed: Vec<String> = old.members.iter().filter(|m| !group.has(&m.fingerprint)).map(|m| m.name.clone()).collect();
                    if !added.is_empty() {
                        notes.push(format!("{} gruba eklendi", join_names(&added)));
                    }
                    if !removed.is_empty() {
                        notes.push(format!("{} gruptan ayrıldı", join_names(&removed)));
                    }
                }
                let g = group.clone();
                self.chats.lock().unwrap().with(&key, |c| {
                    // Sürüm aynı kilit altında yeniden denetlenir: art arda iki güncelleme aynı anda
                    // gelirse eskisi yenisinin üzerine yazılmasın.
                    if c.group.as_ref().is_some_and(|cur| cur.rev >= g.rev) {
                        return;
                    }
                    c.name = g.name.clone();
                    c.group = Some(g);
                    c.left = left;
                });
            }
            None => {
                // Üyesi olmadığımız veya silinmiş bir grubu kaydetmeyiz.
                if group.deleted || !group.has(&me) {
                    return Message::GroupAck;
                }
                notes.push(format!("{admin_name} sizi “{}” grubuna ekledi", group.name));
                self.chats.lock().unwrap().insert(Conversation {
                    fingerprint: key.clone(),
                    name: group.name.clone(),
                    group: Some(group),
                    ..Default::default()
                });
            }
        }
        for n in notes {
            self.system_note(&key, n);
        }
        self.emit_conversation(&key);
        Message::GroupAck
    }

    /// Bir üyeden gelen grup mesajı.
    pub(super) fn accept_group_chat(
        &self,
        device: DeviceInfo,
        ip: IpAddr,
        group_id: &str,
        id: &str,
        text: &str,
        sent_at: u64,
        reply: Option<crate::chat::ReplyRef>,
    ) -> Result<(), &'static str> {
        let sender = device.fingerprint.clone();
        if self.is_blocked(&sender) {
            return Err("blocked");
        }
        if !valid_message_id(id) || !valid_message_id(group_id) {
            return Err("invalid");
        }
        if text.chars().count() > MAX_CHAT_CHARS {
            return Err("too_long");
        }
        let key = group_key(group_id);
        let (group, left) = self.group_of(&key).ok_or("unknown_group")?;
        if left || group.deleted || !group.has(&self.my_fp()) {
            return Err("not_member");
        }
        if !group.has(&sender) {
            return Err("sender_not_member");
        }
        // Zaten alınmış mesajın yeniden gönderimi hız sınırından düşmez.
        if self.chats.lock().unwrap().contains(&key, id) {
            return Ok(());
        }
        if !self.chat_rate_ok(&sender) {
            return Err("rate");
        }
        let text = sanitize_text(text, MAX_CHAT_CHARS);
        if text.is_empty() {
            return Err("empty");
        }
        let sender_name = group.member_name(&sender).map(str::to_string).unwrap_or_else(|| sanitize_name(&device.name));
        self.upsert_peer(device, ip, None);
        if self.chats.lock().unwrap().contains(&key, id) {
            return Ok(());
        }
        // Gecikmeli teslimatta gönderilme saati gösterilir; gelecekteki saatlere güvenilmez.
        let now = now_ms();
        let at = if sent_at > 0 && sent_at <= now { sent_at } else { now };
        let mention = crate::chat::mentions(&text, &self.settings().device_name);
        let msg = ChatMessage {
            id: id.to_string(),
            text,
            at,
            status: MsgStatus::Received,
            sender: Some(sender),
            sender_name: Some(sender_name),
            reply: reply.map(extras::clean_reply),
            mention,
            ..Default::default()
        };
        let unread = self.chats.lock().unwrap().push(&key, &group.name, msg.clone());
        self.emit("chat", json!({ "fingerprint": key, "name": group.name, "unread": unread, "incoming": true, "message": msg, "mention": mention }));
        Ok(())
    }

    /// Bir üye ayrılıyor; yöneticiysek gruptan çıkarıp herkese bildiririz.
    pub(super) fn accept_group_leave(&self, sender_fp: &str, group_id: &str) {
        let me = self.my_fp();
        let key = group_key(group_id);
        let Some((g, left)) = self.group_of(&key) else { return };
        if g.admin != me || left || g.deleted || !g.has(sender_fp) || sender_fp == me {
            return;
        }
        let name = g.member_name(sender_fp).unwrap_or("Bir üye").to_string();
        let mut ng = g.clone();
        ng.members.retain(|m| m.fingerprint != sender_fp);
        ng.rev += 1;
        let sync: Vec<String> = ng.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
        self.chats.lock().unwrap().with(&key, |c| {
            for m in c.messages.iter_mut() {
                m.pending.retain(|f| f != sender_fp);
                settle(m);
            }
            c.group = Some(ng);
            merge_pending(&mut c.sync_pending, sync);
        });
        self.system_note(&key, format!("{name} gruptan ayrıldı"));
        self.emit_conversation(&key);
    }
}
