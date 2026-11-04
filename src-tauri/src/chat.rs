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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub fingerprint: String,
    pub name: String,
    pub unread: u32,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub fingerprint: String,
    pub name: String,
    pub unread: u32,
    pub last: Option<ChatMessage>,
}

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
            // Uygulama kapanırken yolda kalan mesajlar gönderilememiş sayılır.
            for m in c.messages.iter_mut().filter(|m| m.status == MsgStatus::Sending) {
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
        let conv = self.convs.entry(fingerprint.to_string()).or_insert_with(|| Conversation {
            fingerprint: fingerprint.to_string(),
            ..Default::default()
        });
        conv.name = name.to_string();
        if !msg.outgoing {
            conv.unread += 1;
        }
        conv.messages.push(msg);
        if conv.messages.len() > MAX_PER_CONVERSATION {
            let extra = conv.messages.len() - MAX_PER_CONVERSATION;
            conv.messages.drain(..extra);
        }
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

    pub fn summaries(&self) -> Vec<ConversationSummary> {
        let mut list: Vec<ConversationSummary> = self
            .convs
            .values()
            .map(|c| ConversationSummary {
                fingerprint: c.fingerprint.clone(),
                name: c.name.clone(),
                unread: c.unread,
                last: c.messages.last().cloned(),
            })
            .collect();
        list.sort_by(|a, b| {
            let at = |s: &ConversationSummary| s.last.as_ref().map_or(0, |m| m.at);
            at(b).cmp(&at(a))
        });
        list
    }
}
