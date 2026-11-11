//! Cihaz kimliği (kalıcı ID + TLS sertifikası) ve kullanıcı ayarları.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub struct Identity {
    pub id: String,
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
    /// Sertifikanın SHA-256 özeti (hex). Cihazı kriptografik olarak tanımlar.
    pub fingerprint: String,
}

pub fn fingerprint(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

impl Identity {
    pub fn load_or_create(dir: &Path) -> Result<Self, String> {
        let id_path = dir.join("device-id");
        let cert_path = dir.join("cert.der");
        let key_path = dir.join("key.der");

        if let (Ok(id), Ok(cert_der), Ok(key_der)) =
            (fs::read_to_string(&id_path), fs::read(&cert_path), fs::read(&key_path))
        {
            let id = id.trim().to_string();
            if !id.is_empty() && !cert_der.is_empty() && !key_der.is_empty() {
                let fingerprint = fingerprint(&cert_der);
                return Ok(Self { id, cert_der, key_der, fingerprint });
            }
        }

        let ck = rcgen::generate_simple_self_signed(vec!["fastfiles.local".to_string()])
            .map_err(|e| format!("Sertifika oluşturulamadı: {e}"))?;
        let cert_der = ck.cert.der().to_vec();
        let key_der = ck.key_pair.serialize_der();
        let id = uuid::Uuid::new_v4().to_string();

        // Yarım yazılmış kimlik dosyası (elektrik kesintisi) uygulamayı açılmaz hale getirmesin.
        let write_atomic = |path: &Path, data: &[u8]| -> Result<(), String> {
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, data).map_err(|e| e.to_string())?;
            fs::rename(&tmp, path).map_err(|e| e.to_string())
        };
        write_atomic(&key_path, &key_der)?;
        write_atomic(&cert_path, &cert_der)?;
        write_atomic(&id_path, id.as_bytes())?;

        let fingerprint = fingerprint(&cert_der);
        Ok(Self { id, cert_der, key_der, fingerprint })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    pub fingerprint: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub device_name: String,
    pub download_dir: String,
    /// Pencere kapatılınca uygulama sistem tepsisinde çalışmaya devam etsin mi?
    pub close_to_tray: bool,
    /// Bu cihazlardan gelen gönderimler sormadan kabul edilir.
    pub trusted: Vec<TrustedDevice>,
    /// Bu cihazlardan gelen dosya ve mesajlar reddedilir.
    pub blocked: Vec<TrustedDevice>,
    /// Keşifle bulunamayan cihazlar için elle eklenen adresler (ör. "192.168.1.40").
    pub manual_peers: Vec<String>,
    /// İlk açılış karşılama ekranı gösterildi mi?
    pub onboarded: bool,
    /// En son çalışan sürüm ("güncellendi" bildirimi için).
    pub last_version: String,
    /// Uygun dosyalar gönderilirken zstd ile sıkıştırılsın mı (metin, log, kaynak kod...).
    pub compress: bool,
    /// Toplam gönderim hızı sınırı (MB/sn). 0 = sınırsız.
    pub speed_limit_mb: u32,
    /// Gezgin'de sağ tık → "FastFiles ile gönder" menüsü.
    pub shell_menu: bool,
    /// Renk teması: mor | okyanus | orman | gunbatimi | gul | grafit
    pub theme: String,
    /// Görünüm: system | light | dark
    pub appearance: String,
    /// Bu kadar dakika işlem yapılmazsa kilitlenir (PIN tanımlıysa). 0 = kapalı.
    pub lock_minutes: u32,
    /// Son otomatik kurulum denemesi (sürüm, Unix ms). Kurulum başarısız olup eski sürüm
    /// yeniden açılırsa sonsuz "kur → yeniden başlat" döngüsüne girilmesin.
    pub install_attempt: String,
    pub install_attempt_at: u64,
    /// Sanal ofiste oturulan koltuk (0 = seçilmedi). Keşif duyurusuyla diğer cihazlara gider.
    pub seat: u8,
    /// İş modundaki hazır profil resmi (1..=WORK_AVATARS). 0 = seçilmedi: arayüz parmak izinden
    /// birini seçer. Keşif duyurusuyla diğer cihazlara gider.
    pub work_avatar: u8,
    /// Elle seçilen durum: "" (müsait) | "busy" | "focus" | "away". Boşken 10 dk işlem
    /// yapılmazsa kendiliğinden "away" olur.
    pub status: String,
    /// Genel kısayollar: Ctrl+Shift+Space hızlı gönderim, Ctrl+Shift+PrtSc ekran görüntüsü.
    pub hotkeys: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            download_dir: String::new(),
            close_to_tray: true,
            trusted: Vec::new(),
            blocked: Vec::new(),
            manual_peers: Vec::new(),
            onboarded: false,
            last_version: String::new(),
            compress: true,
            speed_limit_mb: 0,
            shell_menu: true,
            theme: "mor".into(),
            appearance: "system".into(),
            lock_minutes: 0,
            install_attempt: String::new(),
            install_attempt_at: 0,
            seat: 0,
            work_avatar: 0,
            status: String::new(),
            hotkeys: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path, default_download: &Path) -> Self {
        let mut s: Settings = crate::store::read_json(path).unwrap_or_default();
        if s.device_name.trim().is_empty() {
            s.device_name = default_device_name();
        }
        if s.download_dir.trim().is_empty() {
            s.download_dir = default_download.join("FastFiles").to_string_lossy().into_owned();
        }
        s
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        crate::store::write_atomic(path, &json).map_err(|e| e.to_string())
    }
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "FastFiles".to_string())
}
