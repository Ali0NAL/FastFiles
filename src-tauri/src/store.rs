//! Kayıt dosyaları (sohbetler, ayarlar, oyunlar, oylamalar, ortak klasörler...) için ortak,
//! güvenli okuma/yazma.
//!
//! Eskiden dosyalar geçici dosyaya yazılıp diske işlenmesi beklenmeden yerine konuyordu.
//! Elektrik kesintisinde boş ya da yarım bir dosya kalabiliyor, açılışta okunamayan dosya "boş"
//! sayılıyor ve ilk kayıtta tüm geçmiş kalıcı olarak siliniyordu.

use serde::de::DeserializeOwned;
use std::io::Write;
use std::path::Path;

/// Önce `<dosya>.tmp`'ye yazar, diske işlenmesini bekler, sonra asıl dosyanın yerine koyar.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_name);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// JSON kayıt dosyasını okur. Dosya yoksa `None`. Dosya var ama okunamıyorsa (bozuk ya da yarım)
/// yanına `<dosya>.bozuk-<zaman>` olarak yedeklenir ve `None` döner: boş başlanır ama eski veri
/// silinmez, sonradan kurtarılabilir.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(e) => {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let mut backup = path.as_os_str().to_owned();
            backup.push(format!(".bozuk-{stamp}"));
            eprintln!("[fastfiles] {} okunamadı ({e}); yedeklendi", path.display());
            let _ = std::fs::rename(path, &backup);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_file_is_kept_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("chats.json");
        write_atomic(&p, b"[1,2,3]").unwrap();
        assert_eq!(read_json::<Vec<u32>>(&p), Some(vec![1, 2, 3]));
        assert!(!dir.path().join("chats.json.tmp").exists());
        std::fs::write(&p, b"[1,2,").unwrap(); // yarım yazılmış
        assert_eq!(read_json::<Vec<u32>>(&p), None);
        assert!(!p.exists(), "bozuk dosya yerinde kalmamalı (bir sonraki kayıt ezmesin)");
        let backups: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".bozuk-")).collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), b"[1,2,");
        assert_eq!(read_json::<Vec<u32>>(&dir.path().join("yok.json")), None);
    }
}
