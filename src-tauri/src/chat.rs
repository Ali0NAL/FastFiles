//! Mesaj geçmişi. Sohbetler karşı cihazın sertifika parmak izine göre tutulur:
//! isim veya cihaz ID'si taklit edilse bile başkasının sohbetine mesaj eklenemez.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_PER_CONVERSATION: usize = 500;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MsgStatus {
    Sending,
    Sent,
    Failed,
    Received,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub outgoing: bool,
    pub text: String,
    /// Unix ms. Gelen mesajlarda alındığı an (gönderenin saatine güvenilmez).
    pub at: u64,
    pub status: MsgStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Grup mesajı: gönderenin parmak izi ve adı (gelen mesajlarda).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    /// Grup mesajı (giden): henüz iletilemeyen üyeler. Çevrimiçi olunca otomatik gönderilir.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    /// Grup mesajı (giden): toplam alıcı sayısı.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub recipients: u32,
    /// "Ahmet gruba eklendi" gibi bilgi satırı.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub system: bool,
    /// Dosya balonu: bu mesaj bir (veya grupta birden fazla) dosya aktarımını temsil eder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileRef>,
}

/// Sohbetteki dosya balonu. Durum ve ilerleme bağlı aktarım kayıtlarından okunur.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    pub title: String,
    pub count: u32,
    pub total: u64,
    /// Bu balona bağlı yerel aktarımlar (grupta üye başına bir tane).
    #[serde(default)]
    pub transfers: Vec<FileTransferRef>,
    /// (Gönderen, grup) çevrimdışı üyelere sonra göndermek için kaynak yollar.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferRef {
    /// Karşı cihazın parmak izi.
    pub to: String,
    /// Yerel aktarım kimliği.
    pub id: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Default for MsgStatus {
    fn default() -> Self {
        MsgStatus::Received
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    pub fingerprint: String,
    pub name: String,
}

/// Grup bilgisi. Yalnızca `admin` (oluşturan cihazın parmak izi) değiştirebilir;
/// her değişiklikte `rev` artar, eski sürümler yok sayılır.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    pub id: String,
    pub name: String,
    pub admin: String,
    pub members: Vec<GroupMember>,
    pub rev: u64,
    #[serde(default)]
    pub deleted: bool,
}

impl GroupInfo {
    pub fn has(&self, fp: &str) -> bool {
        self.members.iter().any(|m| m.fingerprint == fp)
    }
    pub fn member_name(&self, fp: &str) -> Option<&str> {
        self.members.iter().find(|m| m.fingerprint == fp).map(|m| m.name.as_str())
    }
}

/// Grup sohbetlerinin anahtarı. Birebir sohbetlerde anahtar karşı cihazın parmak izidir.
pub fn group_key(id: &str) -> String {
    format!("g:{id}")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    /// Sohbet anahtarı: karşı cihazın parmak izi ya da grup için "g:<id>".
    pub fingerprint: String,
    pub name: String,
    pub unread: u32,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub group: Option<GroupInfo>,
    /// Gruptan ayrıldım / çıkarıldım / grup silindi.
    #[serde(default)]
    pub left: bool,
    /// (Yönetici) son grup bilgisini henüz almamış üyeler.
    #[serde(default)]
    pub sync_pending: Vec<String>,
    /// (Üye) ayrıldığımı yöneticiye henüz bildiremedim.
    #[serde(default)]
    pub leave_pending: bool,
    /// Kullanıcı sildi ama bekleyen bildirimler (ayrılık/silme) var: listede gösterilmez,
    /// bildirimler gidince tamamen kaldırılır.
    #[serde(default)]
    pub hidden: bool,
}

/// Giden kutusu mesajı bekleyen alıcısı kalmadıysa "gönderiliyor" durumunda takılı kalmasın.
pub fn settle(m: &mut ChatMessage) {
    if m.outgoing && m.pending.is_empty() && m.status == MsgStatus::Sending {
        m.status = MsgStatus::Sent;
    }
}

/// En eski mesajları atarak sohbeti sınırda tutar; henüz iletilmemiş giden mesajlar atılmaz.
fn trim(messages: &mut Vec<ChatMessage>) {
    let mut extra = messages.len().saturating_sub(MAX_PER_CONVERSATION);
    if extra == 0 {
        return;
    }
    messages.retain(|m| {
        if extra > 0 && !(m.outgoing && !m.pending.is_empty()) {
            extra -= 1;
            false
        } else {
            true
        }
    });
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub fingerprint: String,
    pub name: String,
    pub unread: u32,
    pub last: Option<ChatMessage>,
    pub group: Option<GroupInfo>,
    pub left: bool,
}

/// Çevrimiçi olunca yapılacak teslimatlar.
#[derive(Debug, Clone)]
pub enum OutboxItem {
    /// Yönetici → üye: güncel grup bilgisi
    GroupSync { key: String, to: String, group: GroupInfo },
    /// Grup mesajı → henüz almamış üye
    Message { key: String, to: String, group_id: String, msg: ChatMessage },
    /// Grup dosyası → henüz gönderilmemiş üye (aktarım başlatılır)
    File { key: String, to: String, group_id: String, msg: ChatMessage },
    /// Üye → yönetici: ayrılıyorum
    Leave { key: String, to: String, group_id: String },
}

/// Bekleyen grup mesajlarından bu kadar eskiyse vazgeçilir.
const OUTBOX_MAX_AGE_MS: u64 = 3 * 24 * 60 * 60 * 1000;

pub struct ChatStore {
    path: PathBuf,
    convs: HashMap<String, Conversation>,
}

impl ChatStore {
    pub fn load(path: &Path) -> Self {
        let list: Vec<Conversation> = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut convs = HashMap::new();
        for mut c in list {
            // Uygulama kapanırken yolda kalan birebir mesajlar gönderilememiş sayılır.
            // (Grup mesajları giden kutusundan teslim edilmeye devam eder.)
            for m in c.messages.iter_mut().filter(|m| m.status == MsgStatus::Sending && m.pending.is_empty()) {
                m.status = MsgStatus::Failed;
                m.error = Some("Uygulama kapandı".into());
            }
            convs.insert(c.fingerprint.clone(), c);
        }
        Self { path: path.to_path_buf(), convs }
    }

    fn save(&self) {
        let list: Vec<&Conversation> = self.convs.values().collect();
        if let Ok(json) = serde_json::to_vec(&list) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// Mesajı ekler; (okunmamış sayısı) döndürür.
    pub fn push(&mut self, fingerprint: &str, name: &str, msg: ChatMessage) -> u32 {
        self.push_inner(fingerprint, name, msg, true)
    }

    /// Okunmamış sayısını artırmadan ekler (dosya balonları: aktarım zaten bildirim gösterir).
    pub fn push_quiet(&mut self, fingerprint: &str, name: &str, msg: ChatMessage) -> u32 {
        self.push_inner(fingerprint, name, msg, false)
    }

    /// Bir mesajı değiştirir ve kaydeder.
    pub fn update_message(&mut self, key: &str, id: &str, f: impl FnOnce(&mut ChatMessage)) -> Option<ChatMessage> {
        let m = self.convs.get_mut(key)?.messages.iter_mut().find(|m| m.id == id)?;
        f(m);
        let out = m.clone();
        self.save();
        Some(out)
    }

    fn push_inner(&mut self, fingerprint: &str, name: &str, msg: ChatMessage, count_unread: bool) -> u32 {
        let conv = self.convs.entry(fingerprint.to_string()).or_insert_with(|| Conversation {
            fingerprint: fingerprint.to_string(),
            ..Default::default()
        });
        conv.name = name.to_string();
        if !msg.outgoing && count_unread {
            conv.unread += 1;
        }
        conv.hidden = false;
        conv.messages.push(msg);
        trim(&mut conv.messages);
        let unread = conv.unread;
        self.save();
        unread
    }

    pub fn contains(&self, fingerprint: &str, id: &str) -> bool {
        self.convs.get(fingerprint).map_or(false, |c| c.messages.iter().any(|m| m.id == id))
    }

    pub fn get(&self, fingerprint: &str, id: &str) -> Option<ChatMessage> {
        self.convs.get(fingerprint)?.messages.iter().find(|m| m.id == id).cloned()
    }

    pub fn conversation_meta(&self, fingerprint: &str) -> Option<(String, u32)> {
        self.convs.get(fingerprint).map(|c| (c.name.clone(), c.unread))
    }

    pub fn update_status(
        &mut self,
        fingerprint: &str,
        id: &str,
        status: MsgStatus,
        error: Option<String>,
    ) -> Option<ChatMessage> {
        let m = self.convs.get_mut(fingerprint)?.messages.iter_mut().find(|m| m.id == id)?;
        m.status = status;
        m.error = error;
        let out = m.clone();
        self.save();
        Some(out)
    }

    pub fn mark_read(&mut self, fingerprint: &str) {
        if let Some(c) = self.convs.get_mut(fingerprint) {
            if c.unread != 0 {
                c.unread = 0;
                self.save();
            }
        }
    }

    pub fn history(&self, fingerprint: &str) -> Vec<ChatMessage> {
        self.convs.get(fingerprint).map(|c| c.messages.clone()).unwrap_or_default()
    }

    pub fn clear(&mut self, fingerprint: &str) {
        if self.convs.remove(fingerprint).is_some() {
            self.save();
        }
    }

    fn summarize(c: &Conversation) -> ConversationSummary {
        ConversationSummary {
            fingerprint: c.fingerprint.clone(),
            name: c.name.clone(),
            unread: c.unread,
            last: c.messages.last().cloned(),
            group: c.group.clone(),
            left: c.left,
        }
    }

    pub fn summary(&self, key: &str) -> Option<ConversationSummary> {
        self.convs.get(key).map(Self::summarize)
    }

    pub fn conv(&self, key: &str) -> Option<&Conversation> {
        self.convs.get(key)
    }

    /// Sohbeti değiştirir ve kaydeder.
    pub fn with<R>(&mut self, key: &str, f: impl FnOnce(&mut Conversation) -> R) -> Option<R> {
        let r = f(self.convs.get_mut(key)?);
        self.save();
        Some(r)
    }

    pub fn insert(&mut self, conv: Conversation) {
        self.convs.insert(conv.fingerprint.clone(), conv);
        self.save();
    }

    /// Bilgi satırı ekler ("Ahmet gruba eklendi").
    pub fn push_system(&mut self, key: &str, text: String, at: u64) -> Option<ChatMessage> {
        let msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            text,
            at,
            system: true,
            ..Default::default()
        };
        let conv = self.convs.get_mut(key)?;
        conv.messages.push(msg.clone());
        trim(&mut conv.messages);
        self.save();
        Some(msg)
    }

    /// Yapılması gereken teslimatlar. Çok eski bekleyen mesajlardan vazgeçilir.
    pub fn outbox(&mut self, now: u64, my_fp: &str) -> Vec<OutboxItem> {
        let mut items = Vec::new();
        // Silinmiş ve bekleyen bildirimi kalmamış sohbetler tamamen kaldırılır.
        let before = self.convs.len();
        self.convs.retain(|_, c| !(c.hidden && !c.leave_pending && c.sync_pending.is_empty()));
        let mut changed = self.convs.len() != before;
        for conv in self.convs.values_mut() {
            let Some(group) = conv.group.clone() else { continue };
            if group.admin == my_fp {
                for to in &conv.sync_pending {
                    items.push(OutboxItem::GroupSync { key: conv.fingerprint.clone(), to: to.clone(), group: group.clone() });
                }
            }
            if conv.leave_pending && !group.admin.is_empty() && group.admin != my_fp {
                items.push(OutboxItem::Leave { key: conv.fingerprint.clone(), to: group.admin.clone(), group_id: group.id.clone() });
            }
            for m in conv.messages.iter_mut().filter(|m| m.outgoing && !m.pending.is_empty()) {
                if now.saturating_sub(m.at) > OUTBOX_MAX_AGE_MS || conv.left {
                    if !conv.left {
                        m.status = MsgStatus::Failed;
                        m.error = Some(format!("{} kişiye 3 gün içinde ulaştırılamadı", m.pending.len()));
                    } else {
                        settle(m);
                    }
                    m.pending.clear();
                    changed = true;
                    continue;
                }
                for to in &m.pending {
                    if m.file.is_some() {
                        items.push(OutboxItem::File { key: conv.fingerprint.clone(), to: to.clone(), group_id: group.id.clone(), msg: m.clone() });
                        continue;
                    }
                    items.push(OutboxItem::Message {
                        key: conv.fingerprint.clone(),
                        to: to.clone(),
                        group_id: group.id.clone(),
                        msg: m.clone(),
                    });
                }
            }
        }
        if changed {
            self.save();
        }
        items
    }

    /// Tüm mesajları siler (PIN unutulunca). Grup üyelikleri korunur, yalnızca mesajlar gider.
    pub fn wipe_messages(&mut self) {
        self.convs.retain(|_, c| c.group.is_some());
        for c in self.convs.values_mut() {
            c.messages.clear();
            c.unread = 0;
        }
        self.save();
    }

    pub fn summaries(&self) -> Vec<ConversationSummary> {
        let mut list: Vec<ConversationSummary> = self.convs.values().filter(|c| !c.hidden).map(Self::summarize).collect();
        list.sort_by(|a, b| {
            let at = |s: &ConversationSummary| s.last.as_ref().map_or(0, |m| m.at);
            at(b).cmp(&at(a))
        });
        list
    }
}
