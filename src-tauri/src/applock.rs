//! Uygulama kilidi: isteğe bağlı 4 haneli açılış PIN'i.
//!
//! - PIN yalnızca bu bilgisayarda, `applock.json` içinde PBKDF2-HMAC-SHA256 (100.000 tur,
//!   rastgele tuz) özeti olarak saklanır; PIN'in kendisi hiçbir yere yazılmaz.
//! - PIN tanımlıysa uygulama kilitli açılır. Kilitliyken bildirimler içeriksiz gösterilir.
//! - Tahmin denemelerine karşı: 5 yanlış denemeden sonra 30 sn bekleme, her yeni yanlışta
//!   süre ikiye katlanır (en fazla 5 dk).
//! - "PIN'imi unuttum": mesajlar ve aktarım geçmişi silinir, PIN kaldırılır. Bu düzeyde koruma
//!   bilerek seçildi: PIN'i bilmeyen biri eski mesajları göremez ama uygulamayı sıfırlayabilir.

use ring::{pbkdf2, rand::SecureRandom};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ITERATIONS: u32 = 100_000;
const FREE_ATTEMPTS: u32 = 5;
const FIRST_WAIT: Duration = Duration::from_secs(30);
const MAX_WAIT: Duration = Duration::from_secs(300);

#[derive(Serialize, Deserialize, Clone)]
struct PinRecord {
    salt: String,
    hash: String,
}

#[derive(Default)]
struct Attempts {
    fails: u32,
    until: Option<Instant>,
}

pub struct AppLock {
    path: PathBuf,
    record: Mutex<Option<PinRecord>>,
    locked: AtomicBool,
    attempts: Mutex<Attempts>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockState {
    pub has_pin: bool,
    pub locked: bool,
    /// Yeni deneme için beklenecek saniye (0 = hemen).
    pub retry_after: u64,
}

fn valid_pin(pin: &str) -> bool {
    pin.len() == 4 && pin.chars().all(|c| c.is_ascii_digit())
}

fn derive(pin: &str, salt: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ITERATIONS).unwrap(), salt, pin.as_bytes(), &mut out);
    out
}

impl AppLock {
    /// PIN tanımlıysa kilitli başlar.
    pub fn load(path: &Path) -> Self {
        let record: Option<PinRecord> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let locked = record.is_some();
        Self { path: path.to_path_buf(), record: Mutex::new(record), locked: AtomicBool::new(locked), attempts: Mutex::default() }
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    pub fn has_pin(&self) -> bool {
        self.record.lock().unwrap().is_some()
    }

    pub fn state(&self) -> LockState {
        let retry_after = self.attempts.lock().unwrap().until.map_or(0, |u| u.saturating_duration_since(Instant::now()).as_secs());
        LockState { has_pin: self.has_pin(), locked: self.is_locked(), retry_after }
    }

    fn check(&self, pin: &str) -> Result<bool, String> {
        {
            let a = self.attempts.lock().unwrap();
            if let Some(until) = a.until {
                let left = until.saturating_duration_since(Instant::now());
                if !left.is_zero() {
                    return Err(format!("Çok fazla yanlış deneme. {} sn sonra tekrar deneyin", left.as_secs().max(1)));
                }
            }
        }
        let Some(rec) = self.record.lock().unwrap().clone() else { return Ok(true) };
        let (Ok(salt), Ok(hash)) = (hex::decode(&rec.salt), hex::decode(&rec.hash)) else { return Ok(false) };
        let ok = valid_pin(pin)
            && pbkdf2::verify(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ITERATIONS).unwrap(), &salt, pin.as_bytes(), &hash).is_ok();
        let mut a = self.attempts.lock().unwrap();
        if ok {
            *a = Attempts::default();
        } else {
            a.fails += 1;
            if a.fails >= FREE_ATTEMPTS {
                let extra = (a.fails - FREE_ATTEMPTS).min(4);
                a.until = Some(Instant::now() + (FIRST_WAIT * 2u32.pow(extra)).min(MAX_WAIT));
            }
        }
        Ok(ok)
    }

    pub fn unlock(&self, pin: &str) -> Result<(), String> {
        if self.check(pin)? {
            self.locked.store(false, Ordering::SeqCst);
            Ok(())
        } else {
            Err("PIN yanlış".into())
        }
    }

    /// PIN tanımlıysa kilitler.
    pub fn lock(&self) -> Result<(), String> {
        if !self.has_pin() {
            return Err("Önce Ayarlar'dan bir PIN belirleyin".into());
        }
        self.locked.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// PIN belirler/değiştirir (`new_pin` = None ise kaldırır). PIN varsa önce mevcut PIN doğrulanır.
    pub fn set_pin(&self, new_pin: Option<&str>, current: Option<&str>) -> Result<(), String> {
        if self.has_pin() && !self.check(current.unwrap_or(""))? {
            return Err("Mevcut PIN yanlış".into());
        }
        match new_pin {
            Some(pin) => {
                if !valid_pin(pin) {
                    return Err("PIN 4 rakamdan oluşmalı".into());
                }
                let mut salt = [0u8; 16];
                ring::rand::SystemRandom::new().fill(&mut salt).map_err(|_| "Rastgele sayı üretilemedi".to_string())?;
                let rec = PinRecord { salt: hex::encode(salt), hash: hex::encode(derive(pin, &salt)) };
                let tmp = self.path.with_extension("json.tmp");
                std::fs::write(&tmp, serde_json::to_vec(&rec).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
                *self.record.lock().unwrap() = Some(rec);
            }
            None => {
                let _ = std::fs::remove_file(&self.path);
                *self.record.lock().unwrap() = None;
                self.locked.store(false, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    /// "PIN'imi unuttum": PIN kaldırılır ve kilit açılır. (Verilerin silinmesi çağıranın işidir.)
    pub fn forget(&self) {
        let _ = std::fs::remove_file(&self.path);
        *self.record.lock().unwrap() = None;
        *self.attempts.lock().unwrap() = Attempts::default();
        self.locked.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_lifecycle_and_rate_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("applock.json");
        let lock = AppLock::load(&path);
        assert!(!lock.is_locked() && !lock.has_pin());
        assert!(lock.lock().is_err(), "PIN yokken kilitlenemez");
        assert!(lock.set_pin(Some("12a4"), None).is_err());
        assert!(lock.set_pin(Some("12345"), None).is_err());
        lock.set_pin(Some("1234"), None).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("1234"), "PIN düz metin saklanmamalı");

        // Yeniden açılınca kilitli başlar.
        let lock = AppLock::load(&path);
        assert!(lock.is_locked());
        assert!(lock.unlock("0000").is_err());
        lock.unlock("1234").unwrap();
        assert!(!lock.is_locked());

        // Değiştirmek için mevcut PIN gerekir.
        assert!(lock.set_pin(Some("5678"), Some("0000")).is_err());
        lock.set_pin(Some("5678"), Some("1234")).unwrap();
        lock.lock().unwrap();
        assert!(lock.unlock("1234").is_err());

        // 5 yanlıştan sonra doğru PIN bile beklemeye takılır.
        for _ in 0..4 {
            let _ = lock.unlock("0000");
        }
        assert!(lock.state().retry_after > 0);
        assert!(lock.unlock("5678").unwrap_err().contains("sn sonra"));
        assert!(lock.is_locked());

        // Unuttum: PIN kalkar, kilit açılır.
        lock.forget();
        assert!(!lock.has_pin() && !lock.is_locked() && !path.exists());
        assert!(!AppLock::load(&path).is_locked());
    }
}
