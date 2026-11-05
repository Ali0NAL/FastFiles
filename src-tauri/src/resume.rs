//! Alıcı tarafında yarıda kalan aktarımların kaydı (`resume.json`).
//!
//! Bir gönderim kabul edildiğinde, gönderenin parmak izi + aktarım kimliğiyle birlikte
//! dosya listesinin özeti ve dosyaların yazıldığı klasör adları saklanır. Bağlantı koparsa
//! `.ffpart` dosyaları silinmez; aynı gönderen aynı aktarımı (aynı dosya listesiyle)
//! yeniden teklif ettiğinde tekrar onay sorulmaz ve kaldığı yerden devam edilir.
//!
//! Kayıt yalnızca aynı **sertifika parmak izinden** gelen teklifle eşleşir; başka bir cihaz
//! aktarım kimliğini bilse bile bu kayıtla onaysız dosya gönderemez.

use crate::protocol::FileEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Bu süreden eski yarım aktarımlar (ve `.ffpart` dosyaları) silinir.
pub const RESUME_TTL_MS: u64 = 3 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResumeEntry {
    /// Gönderenin verdiği aktarım kimliği.
    pub transfer_id: String,
    /// Gönderenin sertifika parmak izi.
    pub sender: String,
    /// Alıcıdaki aktarım kaydının kimliği (arayüzde aynı satır güncellensin diye).
    pub local_id: String,
    /// Dosya listesinin (yol + boyut) özeti.
    pub signature: String,
    /// Dosyaların yazıldığı klasör (o anki indirme klasörü).
    pub dest: String,
    /// Teklifteki üst düzey ad → diskte seçilen benzersiz ad (ör. "Proje" → "Proje (1)").
    pub tops: HashMap<String, String>,
    pub created_at: u64,
}

impl ResumeEntry {
    /// Bu kaydın diske yazdığı yarım (`.ffpart`) dosyaları.
    pub fn part_files(&self) -> Vec<PathBuf> {
        let dest = PathBuf::from(&self.dest);
        let mut out = Vec::new();
        for name in self.tops.values() {
            let top = dest.join(name);
            let single = dest.join(format!("{name}.ffpart"));
            if single.is_file() {
                out.push(single);
            }
            if top.is_dir() {
                for e in walkdir::WalkDir::new(&top).follow_links(false).into_iter().flatten() {
                    if e.file_type().is_file() && e.file_name().to_string_lossy().ends_with(".ffpart") {
                        out.push(e.into_path());
                    }
                }
            }
        }
        out
    }
}

/// Dosya listesinin kimliği: aynı yollar + aynı boyutlar → aynı özet.
pub fn files_signature(files: &[FileEntry]) -> String {
    let mut h = blake3::Hasher::new();
    for f in files {
        h.update(f.path.as_bytes());
        h.update(&[0]);
        h.update(&f.size.to_le_bytes());
        h.update(&f.mtime.unwrap_or(0).to_le_bytes());
    }
    h.finalize().to_hex().to_string()
}

pub struct ResumeStore {
    path: PathBuf,
    entries: Vec<ResumeEntry>,
}

impl ResumeStore {
    /// Kayıtları yükler; süresi dolanların yarım dosyalarını siler.
    pub fn load(path: &Path, now: u64) -> Self {
        let entries: Vec<ResumeEntry> = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut store = Self { path: path.to_path_buf(), entries };
        store.prune(now);
        store
    }

    fn save(&self) {
        let write = || -> std::io::Result<()> {
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(&self.entries)?)?;
            std::fs::rename(&tmp, &self.path)
        };
        if let Err(e) = write() {
            eprintln!("[fastfiles] devam kaydı yazılamadı: {e}");
        }
    }

    /// Süresi dolan kayıtları ve yarım dosyalarını kaldırır.
    pub fn prune(&mut self, now: u64) {
        let (old, keep): (Vec<_>, Vec<_>) =
            self.entries.drain(..).partition(|e| now.saturating_sub(e.created_at) > RESUME_TTL_MS);
        self.entries = keep;
        if !old.is_empty() {
            for e in &old {
                for p in e.part_files() {
                    let _ = std::fs::remove_file(p);
                }
            }
            self.save();
        }
    }

    pub fn find(&self, sender: &str, transfer_id: &str, signature: &str) -> Option<ResumeEntry> {
        self.entries
            .iter()
            .find(|e| e.sender == sender && e.transfer_id == transfer_id && e.signature == signature)
            .cloned()
    }

    /// Gönderenin iptal bildirimi için: dosya listesine bakmadan bulur.
    pub fn find_by_id(&self, sender: &str, transfer_id: &str) -> Option<ResumeEntry> {
        self.entries.iter().find(|e| e.sender == sender && e.transfer_id == transfer_id).cloned()
    }

    pub fn insert(&mut self, entry: ResumeEntry) {
        self.entries.retain(|e| !(e.sender == entry.sender && e.transfer_id == entry.transfer_id));
        self.entries.push(entry);
        self.save();
    }

    pub fn remove(&mut self, sender: &str, transfer_id: &str) {
        let before = self.entries.len();
        self.entries.retain(|e| !(e.sender == sender && e.transfer_id == transfer_id));
        if self.entries.len() != before {
            self.save();
        }
    }

    /// Yarım aktarımların ayırdığı adlar (yeni aktarımlar bu adları kullanmasın).
    pub fn reserved_names(&self, dest: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.dest == dest)
            .flat_map(|e| e.tops.values().map(|n| n.to_lowercase()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dest: &Path, created_at: u64) -> ResumeEntry {
        ResumeEntry {
            transfer_id: "t1".into(),
            sender: "fp".into(),
            local_id: "l1".into(),
            signature: "sig".into(),
            dest: dest.to_string_lossy().into_owned(),
            tops: HashMap::from([("Proje".to_string(), "Proje (1)".to_string()), ("a.bin".into(), "a.bin".into())]),
            created_at,
        }
    }

    #[test]
    fn signature_depends_on_paths_and_sizes() {
        let f = |path: &str, size, mtime| FileEntry { path: path.into(), size, mtime };
        let a = [f("x/a", 1, Some(5)), f("x/b", 2, Some(5))];
        let b = [f("x/a", 1, Some(5)), f("x/b", 3, Some(5))];
        let c = [f("x/a", 1, Some(5)), f("x/b", 2, Some(6))];
        assert_eq!(files_signature(&a), files_signature(&a.clone()));
        assert_ne!(files_signature(&a), files_signature(&b));
        assert_ne!(files_signature(&a), files_signature(&c), "değişen dosyadan devam edilmemeli");
    }

    #[test]
    fn store_persists_and_expires_with_part_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dl");
        std::fs::create_dir_all(dest.join("Proje (1)/alt")).unwrap();
        std::fs::write(dest.join("Proje (1)/alt/b.txt.ffpart"), b"yarim").unwrap();
        std::fs::write(dest.join("Proje (1)/tamam.txt"), b"tamam").unwrap();
        std::fs::write(dest.join("a.bin.ffpart"), b"yarim").unwrap();
        let file = tmp.path().join("resume.json");

        let mut s = ResumeStore::load(&file, 1000);
        s.insert(entry(&dest, 1000));
        assert_eq!(s.reserved_names(&dest.to_string_lossy()).len(), 2);

        // Yeniden yüklenince kayıt duruyor; yanlış parmak izi veya liste eşleşmiyor.
        let s = ResumeStore::load(&file, 2000);
        assert!(s.find("fp", "t1", "sig").is_some());
        assert!(s.find("baska", "t1", "sig").is_none());
        assert!(s.find("fp", "t1", "baska").is_none());
        assert_eq!(s.find("fp", "t1", "sig").unwrap().part_files().len(), 2);

        // Süre dolunca yarım dosyalar silinir, tamamlanmış dosyalar kalır.
        let s = ResumeStore::load(&file, 1000 + RESUME_TTL_MS + 1);
        assert!(s.find("fp", "t1", "sig").is_none());
        assert!(!dest.join("a.bin.ffpart").exists());
        assert!(!dest.join("Proje (1)/alt/b.txt.ffpart").exists());
        assert!(dest.join("Proje (1)/tamam.txt").exists());
    }
}
