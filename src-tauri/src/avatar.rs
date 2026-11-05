//! Profil resimleri (`node` modülünün alt modülü).
//!
//! Her cihaz kendi resminin kısa özetini (hash) keşif duyurusuna koyar. Özet değişince diğer
//! cihazlar resmi yalnızca **o cihazın kendisinden** (TLS parmak iziyle doğrulanmış bağlantı)
//! ister ve özetin tuttuğunu kontrol eder. Böylece kimse başkasının resmini değiştiremez.
//! Boyut sınırı 96 KB; yalnızca gerçek JPEG/PNG/WebP içerik kabul edilir.

use super::*;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};

pub const MAX_AVATAR_BYTES: usize = 96 * 1024;
const AVATAR_RETRY: Duration = Duration::from_secs(60);

pub fn avatar_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))[..16].to_string()
}

pub fn valid_avatar_hash(h: &str) -> bool {
    h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit())
}

/// Dosyanın ilk baytlarına bakarak resim türünü bulur (uzantıya/beyana güvenilmez).
fn image_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn check_image(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
        return Err("Profil resmi en fazla 96 KB olabilir".into());
    }
    image_mime(bytes).ok_or_else(|| "Yalnızca JPEG, PNG veya WebP resim kullanılabilir".to_string())
}

fn data_url(bytes: &[u8]) -> Option<String> {
    image_mime(bytes).map(|mime| format!("data:{mime};base64,{}", B64.encode(bytes)))
}

impl Node {
    fn my_avatar_path(&self) -> PathBuf {
        self.avatar_dir.parent().unwrap_or(&self.avatar_dir).join("avatar.img")
    }

    fn cached_avatar_path(&self, fp: &str) -> Option<PathBuf> {
        valid_fingerprint(fp).then(|| self.avatar_dir.join(format!("{fp}.img")))
    }

    /// Kendi resmini ve önbellekteki diğer resimleri yükler.
    pub(super) fn load_avatars(&self) {
        if let Ok(b) = std::fs::read(self.my_avatar_path()) {
            if check_image(&b).is_ok() {
                *self.my_avatar.lock().unwrap() = Some((avatar_hash(&b), b));
            }
        }
        let mut map = self.avatar_hashes.lock().unwrap();
        for entry in std::fs::read_dir(&self.avatar_dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(fp) = name.strip_suffix(".img") else { continue };
            if !valid_fingerprint(fp) {
                continue;
            }
            match std::fs::read(entry.path()) {
                Ok(b) if check_image(&b).is_ok() => {
                    map.insert(fp.to_string(), avatar_hash(&b));
                }
                _ => {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    pub(super) fn my_avatar_hash(&self) -> String {
        self.my_avatar.lock().unwrap().as_ref().map(|(h, _)| h.clone()).unwrap_or_default()
    }

    /// Bilinen tüm resimler: parmak izi → data URL (kendi resmimiz dahil).
    pub fn avatars(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if let Some((_, b)) = self.my_avatar.lock().unwrap().as_ref() {
            if let Some(u) = data_url(b) {
                out.insert(self.identity.fingerprint.clone(), u);
            }
        }
        let fps: Vec<String> = self.avatar_hashes.lock().unwrap().keys().cloned().collect();
        for fp in fps {
            if let Some(path) = self.cached_avatar_path(&fp) {
                if let Some(u) = std::fs::read(path).ok().as_deref().and_then(data_url) {
                    out.insert(fp, u);
                }
            }
        }
        out
    }

    fn emit_avatar(&self, fp: &str, url: Option<String>) {
        self.emit("avatar", json!({ "fingerprint": fp, "url": url }));
    }

    /// Kullanıcının seçtiği resmi kaydeder (arayüz önceden kırpıp küçültür).
    pub fn set_avatar(&self, url: &str) -> Result<(), String> {
        let b64 = url.split_once(";base64,").map(|(_, d)| d).unwrap_or(url);
        let bytes = B64.decode(b64.trim()).map_err(|_| "Resim okunamadı".to_string())?;
        check_image(&bytes)?;
        let path = self.my_avatar_path();
        std::fs::write(&path, &bytes).map_err(|e| format!("Resim kaydedilemedi: {e}"))?;
        let u = data_url(&bytes);
        *self.my_avatar.lock().unwrap() = Some((avatar_hash(&bytes), bytes));
        self.emit_avatar(&self.identity.fingerprint, u);
        Ok(())
    }

    pub fn remove_avatar(&self) {
        let _ = std::fs::remove_file(self.my_avatar_path());
        *self.my_avatar.lock().unwrap() = None;
        self.emit_avatar(&self.identity.fingerprint, None);
    }

    pub(super) fn serve_avatar(&self) -> Message {
        match self.my_avatar.lock().unwrap().as_ref() {
            Some((_, b)) => Message::AvatarData { data: B64.encode(b) },
            None => Message::AvatarNone,
        }
    }

    /// Resmi değişen çevrimiçi cihazların yeni resmini getirir (keşif döngüsünden çağrılır).
    pub fn check_avatars(self: &Arc<Self>) {
        for p in self.peers() {
            if p.blocked || !valid_fingerprint(&p.fingerprint) {
                continue;
            }
            let cached = self.avatar_hashes.lock().unwrap().get(&p.fingerprint).cloned();
            if p.avatar.is_empty() {
                // Eski sürümler resim bildirmez; bildiren bir cihaz resmini kaldırdıysa biz de sileriz.
                if cached.is_some() && !p.version.is_empty() {
                    self.avatar_hashes.lock().unwrap().remove(&p.fingerprint);
                    if let Some(path) = self.cached_avatar_path(&p.fingerprint) {
                        let _ = std::fs::remove_file(path);
                    }
                    self.emit_avatar(&p.fingerprint, None);
                }
                continue;
            }
            if cached.as_deref() == Some(p.avatar.as_str()) {
                continue;
            }
            let key = format!("{}@{}", p.fingerprint, p.avatar);
            if self.avatar_failures.lock().unwrap().get(&key).map_or(false, |t| t.elapsed() < AVATAR_RETRY) {
                continue;
            }
            if !self.avatar_fetching.lock().unwrap().insert(p.fingerprint.clone()) {
                continue;
            }
            let node = self.clone();
            tokio::spawn(async move {
                if let Err(e) = node.fetch_avatar(&p).await {
                    eprintln!("[fastfiles] profil resmi alınamadı ({}): {e}", p.name);
                    node.avatar_failures.lock().unwrap().insert(key, Instant::now());
                }
                node.avatar_fetching.lock().unwrap().remove(&p.fingerprint);
            });
        }
    }

    async fn fetch_avatar(&self, peer: &Peer) -> Result<(), String> {
        let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut stream, &Message::AvatarRequest).await.map_err(|e| e.to_string())?;
        // Resim en fazla ~96 KB; yanıt okunmadan önce boyutu sınırlanır (64 MB'lık genel sınır yerine).
        let reply = timeout(Duration::from_secs(8), read_msg_max(&mut stream, 256 * 1024))
            .await
            .map_err(|_| "yanıt yok".to_string())?
            .map_err(|e| e.to_string())?;
        let _ = stream.shutdown().await;
        let Message::AvatarData { data } = reply else { return Err("resim yok".into()) };
        if data.len() > MAX_AVATAR_BYTES * 4 / 3 + 8 {
            return Err("resim çok büyük".into());
        }
        let bytes = B64.decode(data).map_err(|_| "bozuk resim".to_string())?;
        check_image(&bytes)?;
        if avatar_hash(&bytes) != peer.avatar {
            return Err("resim duyurulanla eşleşmiyor".into());
        }
        let path = self.cached_avatar_path(&peer.fingerprint).ok_or("geçersiz kimlik")?;
        tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
        self.avatar_hashes.lock().unwrap().insert(peer.fingerprint.clone(), peer.avatar.clone());
        self.emit_avatar(&peer.fingerprint, data_url(&bytes));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_validation() {
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_mime(b"<svg onload=alert(1)>"), None);
        assert_eq!(image_mime(b"MZ\x90\0"), None);
        assert!(check_image(&vec![0xFF; MAX_AVATAR_BYTES + 1]).is_err());
    }
}
