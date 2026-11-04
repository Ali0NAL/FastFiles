//! Cihazlar arası TCP/TLS protokolü.
//!
//! Kontrol mesajları `u32 (big-endian) uzunluk + JSON gövde` şeklinde çerçevelenir.
//! Gönderim kabul edildikten sonra dosyaların ham byte'ları, teklifte (Offer)
//! bildirilen sırayla ve boyutlarla, hiçbir sarmalama olmadan akıtılır.

use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Dosya alımı için dinlenen TCP portu (doluysa rastgele bir port seçilir).
pub const DEFAULT_PORT: u16 = 53317;
/// Cihaz keşfi için kullanılan UDP portu.
pub const DISCOVERY_PORT: u16 = 53318;
pub const PROTOCOL_VERSION: u32 = 1;

const MAX_FRAME: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub port: u16,
    pub fingerprint: String,
    pub os: String,
    /// Çalışan uygulama sürümü (eski sürümler göndermez → boş).
    #[serde(default)]
    pub version: String,
    /// Bu cihazın dağıtabildiği imzalı güncelleme paketinin sürümü (yoksa boş).
    #[serde(default)]
    pub pkg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// `/` ile ayrılmış göreli yol, ör. `Proje/assets/logo.png`
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Message {
    /// Bağlantı testi / elle eklenen cihazı tanıma. Sunucu kendi Hello'su ile yanıt verir.
    Hello { device: DeviceInfo },
    /// Gönderim teklifi.
    Offer {
        transfer_id: String,
        device: DeviceInfo,
        files: Vec<FileEntry>,
        total: u64,
    },
    Accept,
    /// `reason`: "declined" | "timeout" | "busy"
    Decline { reason: String },
    /// Alıcı tüm dosyaları diske yazdı.
    Done,
    /// Alıcı aktarımı iptal etti.
    Cancel,
    /// Metin mesajı. Gönderenin kimliği TLS istemci sertifikasıyla doğrulanır.
    Chat { device: DeviceInfo, id: String, text: String },
    ChatAck { id: String },
    /// `reason`: "blocked" | "rate" | "too_long" | "empty" | "invalid"
    ChatReject { reason: String },
    /// Grup bilgisi (oluşturma/değişiklik/silme). Yalnızca grubun yöneticisinden kabul edilir.
    GroupUpdate { device: DeviceInfo, group: crate::chat::GroupInfo },
    /// Grup mesajı. Yanıt: ChatAck ya da ChatReject ("unknown_group" | "not_member" | ...).
    GroupChat { device: DeviceInfo, group_id: String, id: String, text: String, sent_at: u64 },
    /// Üye gruptan ayrılıyor (yöneticiye gönderilir).
    GroupLeave { device: DeviceInfo, group_id: String },
    GroupAck,
    /// Güncelleme paketi isteği. Yanıt: UpdateOffer + ham paket baytları ya da UpdateNone.
    UpdateRequest,
    UpdateOffer { header: crate::update::PackageHeader },
    UpdateNone,
}

pub const MAX_CHAT_CHARS: usize = 4000;
pub const MAX_GROUP_MEMBERS: usize = 50;

/// SHA-256 parmak izi biçiminde mi (64 hex karakter)?
pub fn valid_fingerprint(fp: &str) -> bool {
    fp.len() == 64 && fp.chars().all(|c| c.is_ascii_hexdigit())
}
pub const MAX_NAME_CHARS: usize = 40;

/// Görünmez/yön değiştiren Unicode karakterleri (ör. U+202E ile "exe.txt" → "txt.exe" taklidi).
fn is_dangerous_char(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}')
}

/// Ağdan gelen metni temizler: kontrol ve tehlikeli Unicode karakterlerini atar,
/// satır sonlarını normalleştirir, uzunluğu sınırlar.
pub fn sanitize_text(s: &str, max_chars: usize) -> String {
    let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
    let cleaned: String = normalized
        .chars()
        .filter(|&c| (c == '\n' || c == '\t' || !c.is_control()) && !is_dangerous_char(c))
        .take(max_chars)
        .collect();
    cleaned.trim().to_string()
}

/// Cihaz adı: tek satır, en fazla 40 karakter.
pub fn sanitize_name(s: &str) -> String {
    let one_line: String = sanitize_text(s, MAX_NAME_CHARS)
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let name = one_line.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    if name.is_empty() { "Adsız cihaz".to_string() } else { name }
}

/// Mesaj kimliği: yalnızca kısa UUID benzeri değerler kabul edilir.
pub fn valid_message_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Message) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    w.write_all(&frame).await?;
    w.flush().await
}

pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Message> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "mesaj çok büyük"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Karşı taraftan gelen göreli yolu güvenli parçalara ayırır.
/// `..` içeren yollar reddedilir; Windows'ta geçersiz karakterler `_` ile değiştirilir.
pub fn sanitize_rel_path(p: &str) -> Option<Vec<String>> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut out = Vec::new();
    for raw in p.split(['/', '\\']) {
        if raw.is_empty() || raw == "." {
            continue;
        }
        if raw == ".." {
            return None;
        }
        let cleaned: String = raw
            .chars()
            .map(|c| {
                if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let trimmed = cleaned.trim_end_matches(['.', ' ']);
        let mut part = if trimmed.is_empty() { "_".to_string() } else { trimmed.to_string() };
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if RESERVED.contains(&stem.as_str()) {
            part = format!("_{part}");
        }
        out.push(part);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_traversal() {
        assert!(sanitize_rel_path("../evil.exe").is_none());
        assert!(sanitize_rel_path("a/../../b").is_none());
        assert!(sanitize_rel_path("").is_none());
        assert_eq!(sanitize_rel_path("/abs/path.txt").unwrap(), vec!["abs", "path.txt"]);
        assert_eq!(sanitize_rel_path("C:\\Windows\\x").unwrap(), vec!["C_", "Windows", "x"]);
        assert_eq!(sanitize_rel_path("dir/CON.txt").unwrap(), vec!["dir", "_CON.txt"]);
        assert_eq!(sanitize_rel_path("a/b?.txt").unwrap(), vec!["a", "b_.txt"]);
        assert_eq!(sanitize_rel_path("trail. ").unwrap(), vec!["trail"]);
    }

    #[test]
    fn sanitize_text_and_names() {
        // Yön değiştirme karakteri ve kontrol karakterleri atılır, satır sonları korunur.
        assert_eq!(sanitize_text("fatura\u{202E}fdp.exe", 100), "faturafdp.exe");
        assert_eq!(sanitize_text("  merhaba\r\ndünya\u{0007}  ", 100), "merhaba\ndünya");
        assert_eq!(sanitize_text("abcdef", 3), "abc");
        assert_eq!(sanitize_text("\u{200B}\u{FEFF} ", 10), "");
        assert_eq!(sanitize_name("  Ahmet\n\tYılmaz  "), "Ahmet Yılmaz");
        assert_eq!(sanitize_name("\u{202E}"), "Adsız cihaz");
        assert_eq!(sanitize_name(&"x".repeat(100)).chars().count(), MAX_NAME_CHARS);
        assert!(valid_message_id("3f2a-11aa-bb"));
        assert!(!valid_message_id("../../x"));
        assert!(!valid_message_id(&"a".repeat(65)));
    }
}
