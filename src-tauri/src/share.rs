//! Ortak klasörler (`node` modülünün alt modülü).
//!
//! Her üyede klasörün senkron bir kopyası vardır (İndirilenler\FastFiles\Ortak\<ad>).
//! FastFiles klasörü birkaç saniyede bir tarar; yeni/değişen/silinen dosyaları çevrimiçi üyelerle
//! karşılıklı TLS üzerinden eşitler. Sunucu yoktur.
//!
//! Eşitleme modeli
//! - Her dosya için bir kayıt tutulur: yol, boyut, dosyanın kendi değişim zamanı (mtime),
//!   sürüm (`version`, değişikliğin fark edildiği an) ve yazan cihaz. Aynı dosyada çakışmada
//!   (version, yazan) büyük olan kazanır. Silme bir "mezar taşı" kaydıdır, o da yayılır.
//! - Üzerine yazılacak veya silinecek yerel dosya önce `.fastfiles\silinen` klasörüne taşınır:
//!   yanlışlıkla silme/çakışmada hiçbir şey kaybolmaz.
//!
//! Güvenlik
//! - Üyelik ve şifre yalnızca klasörü oluşturan cihaz (yönetici, sertifika parmak izi) tarafından
//!   değiştirilebilir; her değişiklik `rev` ile sıralanır (gruplarla aynı model).
//! - Şifre yalnızca yöneticide PBKDF2-HMAC-SHA256 (100.000 tur, rastgele tuz) olarak saklanır,
//!   ağda yalnızca TLS içinde gider. Deneme sınırı: cihaz başına 5 dakikada 5 deneme.
//! - Şifreli klasörde, şifreyi girmemiş üye ne dosya listesi ne dosya alabilir; diğer üyeler de
//!   ona dosya göndermez.
//! - Ağdan gelen yollar temizlenir (`..` yasak), sembolik bağlantılar izlenmez/sunulmaz.

use super::*;
use crate::chat::GroupMember;
use ring::{pbkdf2, rand::SecureRandom};
use serde::Deserialize;
use std::num::NonZeroU32;

const SCAN_SETTLE_MS: u64 = 2000;
const PUSH_REFRESH: Duration = Duration::from_secs(60);
const SHARE_BACKOFF: Duration = Duration::from_secs(15);
const MAX_PARALLEL_DOWNLOADS: usize = 3;
const PBKDF2_ITERATIONS: u32 = 100_000;
const UNLOCK_MAX_ATTEMPTS: usize = 5;
const UNLOCK_WINDOW: Duration = Duration::from_secs(300);
const MIN_PASSWORD_CHARS: usize = 4;
/// Paylaşılan klasörün içinde FastFiles'ın kendi kullandığı (taranmayan) klasör.
pub const SHARE_META_DIR: &str = ".fastfiles";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShareInfo {
    pub id: String,
    pub name: String,
    pub admin: String,
    pub members: Vec<GroupMember>,
    pub rev: u64,
    #[serde(default)]
    pub deleted: bool,
    /// Şifreli mi? (Şifrenin kendisi/özeti asla paylaşılmaz.)
    #[serde(default)]
    pub protected: bool,
    /// Şifreyi doğru girmiş üyeler.
    #[serde(default)]
    pub unlocked: Vec<String>,
}

impl ShareInfo {
    pub fn has(&self, fp: &str) -> bool {
        self.members.iter().any(|m| m.fingerprint == fp)
    }
    /// Üye mi ve (şifreliyse) kilidi açmış mı?
    pub fn can_access(&self, fp: &str) -> bool {
        !self.deleted && self.has(fp) && (!self.protected || fp == self.admin || self.unlocked.iter().any(|u| u == fp))
    }
    fn member_name(&self, fp: &str) -> Option<&str> {
        self.members.iter().find(|m| m.fingerprint == fp).map(|m| m.name.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShareEntry {
    /// `/` ile ayrılmış göreli yol
    pub path: String,
    pub size: u64,
    /// Dosyanın kendi değişim zamanı (ms) — değişiklik algılama ve gösterim için.
    pub mtime: u64,
    /// Sıralama sürümü (ms). Çakışmada (version, author) büyük olan kazanır.
    pub version: u64,
    pub author: String,
    #[serde(default)]
    pub deleted: bool,
}

impl ShareEntry {
    fn newer_than(&self, other: &ShareEntry) -> bool {
        (self.version, &self.author) > (other.version, &other.author)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PasswordHash {
    salt: String,
    hash: String,
    iterations: u32,
}

/// Bir klasörün bu cihazdaki durumu (config\shares\<id>.json).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShareState {
    info: ShareInfo,
    /// Yerelde bulunan dosyalar + mezar taşları. Anahtar: küçük harfli yol.
    #[serde(default)]
    entries: HashMap<String, ShareEntry>,
    /// Yerel klasör yolu
    dir: String,
    #[serde(default)]
    left: bool,
    /// (Yönetici) son bilgiyi almamış üyeler
    #[serde(default)]
    sync_pending: Vec<String>,
    #[serde(default)]
    leave_pending: bool,
    /// (Yönetici) şifre özeti — asla ağa gönderilmez
    #[serde(default)]
    password: Option<PasswordHash>,
    /// Kullanıcı listeden sildi; bekleyen bildirimler gidince tamamen kaldırılır.
    #[serde(default)]
    forgotten: bool,
    /// İndirilmeyi bekleyenler: yol anahtarı → (kayıt, kimden)
    #[serde(skip)]
    wanted: HashMap<String, (ShareEntry, String)>,
    #[serde(skip)]
    digest: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSummary {
    pub id: String,
    pub name: String,
    pub admin: String,
    pub members: Vec<GroupMember>,
    pub protected: bool,
    /// Bu cihaz dosyalara erişebiliyor mu (şifreliyse kilit açık mı)?
    pub can_access: bool,
    pub left: bool,
    pub deleted: bool,
    pub dir: String,
    pub file_count: usize,
    pub total_size: u64,
    /// İndirilmeyi bekleyen dosya sayısı
    pub syncing: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFileView {
    pub path: String,
    pub size: u64,
    pub mtime: u64,
    pub author: String,
    pub author_name: String,
    pub full_path: String,
}

fn key_of(path: &str) -> String {
    path.to_lowercase()
}

/// Taramada yok sayılan dosyalar (geçici, kilit, sistem dosyaları).
fn ignored(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".ffpart")
        || lower.ends_with(".tmp")
        || lower.starts_with("~$")
        || lower == "desktop.ini"
        || lower == "thumbs.db"
        || lower == ".ds_store"
}

fn system_ms(t: std::time::SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn hash_password(password: &str) -> Result<PasswordHash, String> {
    let mut salt = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut salt).map_err(|_| "rastgele sayı üretilemedi".to_string())?;
    let mut out = [0u8; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(PBKDF2_ITERATIONS).unwrap(), &salt, password.as_bytes(), &mut out);
    Ok(PasswordHash { salt: hex::encode(salt), hash: hex::encode(out), iterations: PBKDF2_ITERATIONS })
}

fn verify_password(h: &PasswordHash, password: &str) -> bool {
    let (Ok(salt), Ok(hash), Some(iter)) = (hex::decode(&h.salt), hex::decode(&h.hash), NonZeroU32::new(h.iterations)) else {
        return false;
    };
    pbkdf2::verify(pbkdf2::PBKDF2_HMAC_SHA256, iter, &salt, password.as_bytes(), &hash).is_ok()
}

fn clean_share(mut s: ShareInfo) -> Option<ShareInfo> {
    if !valid_message_id(&s.id) || !valid_fingerprint(&s.admin) {
        return None;
    }
    s.name = sanitize_name(&s.name);
    let mut seen = HashSet::new();
    s.members = s
        .members
        .into_iter()
        .filter(|m| valid_fingerprint(&m.fingerprint) && seen.insert(m.fingerprint.clone()))
        .map(|m| GroupMember { name: sanitize_name(&m.name), fingerprint: m.fingerprint })
        .take(MAX_GROUP_MEMBERS)
        .collect();
    let members: HashSet<String> = s.members.iter().map(|m| m.fingerprint.clone()).collect();
    s.unlocked.retain(|f| members.contains(f));
    Some(s)
}

/// Ağdan gelen kaydı doğrular; yolu temizler.
fn clean_entry(mut e: ShareEntry) -> Option<ShareEntry> {
    let parts = sanitize_rel_path(&e.path)?;
    if parts.first().map_or(true, |p| p.eq_ignore_ascii_case(SHARE_META_DIR)) || ignored(parts.last()?) {
        return None;
    }
    if !valid_fingerprint(&e.author) {
        return None;
    }
    e.path = parts.join("/");
    Some(e)
}

fn digest_of(entries: &HashMap<String, ShareEntry>) -> String {
    use sha2::{Digest, Sha256};
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    let mut h = Sha256::new();
    for k in keys {
        let e = &entries[k];
        h.update(format!("{}|{}|{}|{}|{}|{}\n", e.path, e.size, e.mtime, e.version, e.author, e.deleted));
    }
    hex::encode(h.finalize())[..16].to_string()
}

/// Yerel dosyayı silmek/üzerine yazmak yerine `.fastfiles\silinen` içine taşır.
fn move_to_trash(dir: &Path, rel: &str) {
    let src = dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !src.exists() {
        return;
    }
    let trash = dir.join(SHARE_META_DIR).join("silinen");
    let _ = std::fs::create_dir_all(&trash);
    let stamp = chrono_stamp();
    let name = rel.replace('/', " - ");
    let dst = trash.join(format!("{stamp} {name}"));
    if std::fs::rename(&src, &dst).is_err() {
        let _ = std::fs::remove_file(&src);
    }
}

fn chrono_stamp() -> String {
    // Saat dilimi olmadan, dosya adına uygun sıralanabilir bir damga.
    format!("{}", now_ms())
}

/// Klasördeki dosyaları listeler (FastFiles'ın kendi klasörü ve geçici dosyalar hariç).
fn list_dir(dir: &Path) -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(dir).follow_links(false).into_iter().filter_entry(|e| {
        !(e.depth() == 1 && e.file_type().is_dir() && e.file_name().to_string_lossy().eq_ignore_ascii_case(SHARE_META_DIR))
    });
    for entry in walker.flatten() {
        if !entry.file_type().is_file() || ignored(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(dir) else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let rel = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        out.push((rel, meta.len(), meta.modified().map(system_ms).unwrap_or(0)));
    }
    out
}

impl Node {
    fn share_path(&self, id: &str) -> PathBuf {
        self.shares_dir.join(format!("{id}.json"))
    }

    fn save_share(&self, st: &ShareState) {
        if let Ok(json) = serde_json::to_vec(st) {
            let path = self.share_path(&st.info.id);
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub(super) fn load_shares(&self) {
        let mut map = self.shares.lock().unwrap();
        for entry in std::fs::read_dir(&self.shares_dir).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(mut st) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<ShareState>(&b).ok()) {
                st.digest = digest_of(&st.entries);
                map.insert(st.info.id.clone(), st);
            }
        }
    }

    fn summary_of(&self, st: &ShareState) -> ShareSummary {
        let me = &self.identity.fingerprint;
        let live: Vec<&ShareEntry> = st.entries.values().filter(|e| !e.deleted).collect();
        ShareSummary {
            id: st.info.id.clone(),
            name: st.info.name.clone(),
            admin: st.info.admin.clone(),
            members: st.info.members.clone(),
            protected: st.info.protected,
            can_access: !st.left && st.info.can_access(me),
            left: st.left,
            deleted: st.info.deleted,
            dir: st.dir.clone(),
            file_count: live.len(),
            total_size: live.iter().map(|e| e.size).sum(),
            syncing: st.wanted.len(),
        }
    }

    pub fn share_list(&self) -> Vec<ShareSummary> {
        let map = self.shares.lock().unwrap();
        let mut list: Vec<ShareSummary> = map.values().filter(|s| !s.forgotten).map(|s| self.summary_of(s)).collect();
        list.sort_by_key(|s| s.name.to_lowercase());
        list
    }

    fn emit_shares(&self) {
        self.emit("shares", json!(self.share_list()));
    }

    pub fn share_files(&self, id: &str) -> Result<Vec<ShareFileView>, String> {
        let map = self.shares.lock().unwrap();
        let st = map.get(id).ok_or("Klasör bulunamadı")?;
        if !st.info.can_access(&self.identity.fingerprint) && !st.left {
            return Err("Bu klasörün kilidi açılmadı".into());
        }
        let dir = PathBuf::from(&st.dir);
        let mut files: Vec<ShareFileView> = st
            .entries
            .values()
            .filter(|e| !e.deleted)
            .map(|e| ShareFileView {
                path: e.path.clone(),
                size: e.size,
                mtime: e.mtime,
                author: e.author.clone(),
                author_name: if e.author == self.identity.fingerprint {
                    "Siz".into()
                } else {
                    st.info.member_name(&e.author).map(str::to_string).or_else(|| self.known_name_any(&e.author)).unwrap_or_else(|| "Bilinmeyen".into())
                },
                full_path: dir.join(e.path.replace('/', std::path::MAIN_SEPARATOR_STR)).to_string_lossy().into_owned(),
            })
            .collect();
        files.sort_by(|a, b| b.mtime.cmp(&a.mtime));
        Ok(files)
    }

    fn known_name_any(&self, fp: &str) -> Option<String> {
        self.peers().into_iter().find(|p| p.fingerprint == fp).map(|p| p.name)
    }

    pub(super) fn name_for(&self, fp: &str) -> Option<String> {
        if let Some(n) = self.known_name_any(fp) {
            return Some(n);
        }
        self.chats.lock().unwrap().conv(fp).filter(|c| c.group.is_none()).map(|c| c.name.clone())
    }

    /// İndirme klasörü altında bu klasör için benzersiz bir yerel dizin seçer.
    fn new_share_dir(&self, name: &str) -> PathBuf {
        let root = PathBuf::from(self.settings().download_dir).join("Ortak");
        let base = sanitize_rel_path(name).map(|p| p.join(" ")).unwrap_or_else(|| "Klasör".into());
        let used: HashSet<String> = self.shares.lock().unwrap().values().map(|s| s.dir.to_lowercase()).collect();
        let mut candidate = root.join(&base);
        let mut i = 1;
        while used.contains(&candidate.to_string_lossy().to_lowercase()) {
            candidate = root.join(format!("{base} ({i})"));
            i += 1;
        }
        candidate
    }

    // ───────────── Kullanıcı işlemleri ─────────────

    pub fn share_create(self: &Arc<Self>, name: &str, member_fps: &[String], password: Option<String>) -> Result<ShareSummary, String> {
        let name = sanitize_name(name);
        if name == "Adsız cihaz" {
            return Err("Klasör adı boş olamaz".into());
        }
        let me = self.identity.fingerprint.clone();
        let mut members = vec![GroupMember { fingerprint: me.clone(), name: self.device_info().name }];
        for fp in member_fps {
            if *fp == me || !valid_fingerprint(fp) || members.iter().any(|m| m.fingerprint == *fp) {
                continue;
            }
            if let Some(n) = self.name_for(fp) {
                members.push(GroupMember { fingerprint: fp.clone(), name: n });
            }
        }
        if members.len() > MAX_GROUP_MEMBERS {
            return Err(format!("Bir klasörde en fazla {MAX_GROUP_MEMBERS} kişi olabilir"));
        }
        let password = match password.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
            Some(p) if p.chars().count() < MIN_PASSWORD_CHARS => return Err(format!("Şifre en az {MIN_PASSWORD_CHARS} karakter olmalı")),
            Some(p) => Some(hash_password(&p)?),
            None => None,
        };
        let info = ShareInfo {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone(),
            admin: me.clone(),
            members,
            rev: 1,
            deleted: false,
            protected: password.is_some(),
            unlocked: Vec::new(),
        };
        let dir = self.new_share_dir(&name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Klasör oluşturulamadı: {e}"))?;
        let st = ShareState {
            sync_pending: info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect(),
            info,
            dir: dir.to_string_lossy().into_owned(),
            password,
            ..Default::default()
        };
        let summary = self.summary_of(&st);
        self.save_share(&st);
        self.shares.lock().unwrap().insert(st.info.id.clone(), st);
        self.emit_shares();
        Ok(summary)
    }

    /// (Yalnızca yönetici) ad, üyeler.
    pub fn share_edit(&self, id: &str, name: Option<String>, add: &[String], remove: &[String]) -> Result<ShareSummary, String> {
        let me = self.identity.fingerprint.clone();
        let mut add_named = Vec::new();
        for fp in add {
            if *fp != me && valid_fingerprint(fp) {
                if let Some(n) = self.name_for(fp) {
                    add_named.push(GroupMember { fingerprint: fp.clone(), name: n });
                }
            }
        }
        let summary = {
            let mut map = self.shares.lock().unwrap();
            let st = map.get_mut(id).ok_or("Klasör bulunamadı")?;
            if st.info.admin != me {
                return Err("Klasörü yalnızca oluşturan kişi düzenleyebilir".into());
            }
            if st.left || st.info.deleted {
                return Err("Bu klasör artık etkin değil".into());
            }
            let old = st.info.clone();
            if let Some(n) = name {
                let n = sanitize_name(&n);
                if n != "Adsız cihaz" {
                    st.info.name = n;
                }
            }
            st.info.members.retain(|m| m.fingerprint == me || !remove.contains(&m.fingerprint));
            st.info.unlocked.retain(|f| !remove.contains(f));
            for m in add_named {
                if !st.info.has(&m.fingerprint) {
                    st.info.members.push(m);
                }
            }
            if st.info.members.len() > MAX_GROUP_MEMBERS {
                st.info = old;
                return Err(format!("Bir klasörde en fazla {MAX_GROUP_MEMBERS} kişi olabilir"));
            }
            if st.info != old {
                st.info.rev += 1;
                // Kalanlara ve çıkarılanlara bildir.
                let mut sync: Vec<String> = st.info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
                sync.extend(old.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me && !st.info.has(f)));
                st.sync_pending = sync;
                self.save_share(st);
            }
            self.summary_of(st)
        };
        self.emit_shares();
        Ok(summary)
    }

    /// (Yalnızca yönetici) şifre koy/değiştir/kaldır. Kilidi açmış üyeler açık kalır.
    pub fn share_set_password(&self, id: &str, password: Option<String>) -> Result<ShareSummary, String> {
        let me = self.identity.fingerprint.clone();
        let hashed = match password.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
            Some(p) if p.chars().count() < MIN_PASSWORD_CHARS => return Err(format!("Şifre en az {MIN_PASSWORD_CHARS} karakter olmalı")),
            Some(p) => Some(hash_password(&p)?),
            None => None,
        };
        let summary = {
            let mut map = self.shares.lock().unwrap();
            let st = map.get_mut(id).ok_or("Klasör bulunamadı")?;
            if st.info.admin != me {
                return Err("Şifreyi yalnızca klasörü oluşturan kişi değiştirebilir".into());
            }
            let was_protected = st.info.protected;
            st.info.protected = hashed.is_some();
            if st.info.protected && !was_protected {
                // Şifre sonradan konduysa, o ana kadar erişen üyeler erişmeye devam eder.
                st.info.unlocked = st.info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
            }
            if !st.info.protected {
                st.info.unlocked.clear();
            }
            st.password = hashed;
            st.info.rev += 1;
            st.sync_pending = st.info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
            self.save_share(st);
            self.summary_of(st)
        };
        self.emit_shares();
        Ok(summary)
    }

    /// Klasörden ayrıl. Yönetici ayrılırsa klasör herkes için kapanır (dosyalar herkesin bilgisayarında kalır).
    pub fn share_leave(&self, id: &str) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        {
            let mut map = self.shares.lock().unwrap();
            let st = map.get_mut(id).ok_or("Klasör bulunamadı")?;
            if st.info.admin == me {
                st.info.deleted = true;
                st.info.rev += 1;
                st.sync_pending = st.info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
            } else {
                st.leave_pending = true;
            }
            st.left = true;
            st.wanted.clear();
            self.save_share(st);
        }
        self.emit_shares();
        Ok(())
    }

    /// Klasörü sil: listeden kaldırır. Sahip silerse klasör önce herkes için kapatılır, üye silerse
    /// klasörden ayrılır. Çevrimdışı üyelere bildirim gidene kadar kayıt gizli tutulur, sonra
    /// tamamen kaldırılır. `delete_files` ise bu bilgisayardaki yerel kopya da silinir.
    pub fn share_delete(&self, id: &str, delete_files: bool) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        let dir = {
            let mut map = self.shares.lock().unwrap();
            let st = map.get_mut(id).ok_or("Klasör bulunamadı")?;
            if !st.left {
                if st.info.admin == me {
                    st.info.deleted = true;
                    st.info.rev += 1;
                    st.sync_pending = st.info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
                } else {
                    st.leave_pending = true;
                }
                st.left = true;
                st.wanted.clear();
            }
            st.forgotten = true;
            let dir = PathBuf::from(&st.dir);
            if st.sync_pending.is_empty() && !st.leave_pending {
                map.remove(id);
                let _ = std::fs::remove_file(self.share_path(id));
            } else {
                self.save_share(st);
            }
            dir
        };
        if delete_files {
            // Yalnızca FastFiles'ın oluşturduğu "...\Ortak\<ad>" klasörü silinir; başka bir yer asla.
            let in_share_root = dir.parent().and_then(|p| p.file_name()).map_or(false, |n| n == "Ortak");
            if in_share_root && dir.components().count() > 3 {
                std::fs::remove_dir_all(&dir).map_err(|e| format!("Klasör listeden kaldırıldı ama dosyalar silinemedi: {e}"))?;
            }
        }
        self.emit_shares();
        Ok(())
    }

    /// Silinmiş ve bildirimleri tamamlanmış klasör kayıtlarını temizler.
    fn cleanup_forgotten_shares(&self) {
        let mut map = self.shares.lock().unwrap();
        let done: Vec<String> = map
            .values()
            .filter(|s| s.forgotten && s.sync_pending.is_empty() && !s.leave_pending)
            .map(|s| s.info.id.clone())
            .collect();
        for id in done {
            map.remove(&id);
            let _ = std::fs::remove_file(self.share_path(&id));
        }
    }

    /// Dosya/klasörleri ortak klasöre kopyalar (sonraki taramada diğer üyelere gider).
    pub async fn share_add_files(self: &Arc<Self>, id: &str, paths: Vec<String>) -> Result<usize, String> {
        let dir = {
            let map = self.shares.lock().unwrap();
            let st = map.get(id).ok_or("Klasör bulunamadı")?;
            if st.left || !st.info.can_access(&self.identity.fingerprint) {
                return Err("Bu klasöre dosya ekleyemezsiniz".into());
            }
            PathBuf::from(&st.dir)
        };
        let copied = tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let entries = collect_entries(&paths.iter().map(PathBuf::from).collect::<Vec<_>>())?;
            let mut n = 0;
            for (src, e) in entries {
                let Some(parts) = sanitize_rel_path(&e.path) else { continue };
                let mut dst = dir.clone();
                for p in &parts {
                    dst.push(p);
                }
                if dst.starts_with(dir.join(SHARE_META_DIR)) {
                    continue;
                }
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                if dst.exists() {
                    move_to_trash(&dir, &parts.join("/"));
                }
                std::fs::copy(&src, &dst).map_err(|err| format!("{} kopyalanamadı: {err}", e.path))?;
                n += 1;
            }
            Ok(n)
        })
        .await
        .map_err(|e| e.to_string())??;
        self.share_scan(id).await;
        Ok(copied)
    }

    /// Şifreli klasörün kilidini açmak için şifreyi yöneticiye gönderir.
    pub async fn share_unlock(self: &Arc<Self>, id: &str, password: &str) -> Result<(), String> {
        let admin = {
            let map = self.shares.lock().unwrap();
            let st = map.get(id).ok_or("Klasör bulunamadı")?;
            if st.info.can_access(&self.identity.fingerprint) {
                return Ok(());
            }
            st.info.admin.clone()
        };
        let peer = self
            .peers()
            .into_iter()
            .find(|p| p.fingerprint == admin)
            .ok_or("Klasörün sahibi şu an çevrimdışı; o çevrimiçi olunca tekrar deneyin")?;
        let reply = self
            .share_request(&peer, &Message::ShareUnlock { device: self.device_info(), share_id: id.into(), password: password.into() })
            .await?;
        match reply {
            Message::ShareUnlockResult { ok: true, .. } => Ok(()),
            Message::ShareUnlockResult { message, .. } => Err(message),
            _ => Err("Beklenmeyen yanıt".into()),
        }
    }

    // ───────────── Tarama ─────────────

    /// Yerel klasörü tarar; yeni/değişen/silinen dosyaları kayda geçirir.
    pub async fn share_scan(&self, id: &str) -> bool {
        let me = self.identity.fingerprint.clone();
        let dir = {
            let map = self.shares.lock().unwrap();
            match map.get(id) {
                Some(st) if !st.left && st.info.can_access(&me) => PathBuf::from(&st.dir),
                _ => return false,
            }
        };
        let _ = tokio::fs::create_dir_all(&dir).await;
        let d2 = dir.clone();
        let Ok(listing) = tokio::task::spawn_blocking(move || list_dir(&d2)).await else { return false };
        let now = now_ms();
        let changed = {
            let mut map = self.shares.lock().unwrap();
            let Some(st) = map.get_mut(id) else { return false };
            let mut changed = false;
            let mut seen = HashSet::new();
            for (path, size, mtime) in listing {
                let key = key_of(&path);
                seen.insert(key.clone());
                // Yazılmakta olan dosyayı yarım yayınlamamak için birkaç saniye bekle.
                if now.saturating_sub(mtime) < SCAN_SETTLE_MS {
                    continue;
                }
                let current = st.entries.get(&key);
                let same = current.map_or(false, |e| !e.deleted && e.size == size && e.mtime == mtime);
                if !same {
                    let version = now.max(current.map_or(0, |e| e.version + 1));
                    st.entries.insert(key, ShareEntry { path, size, mtime, version, author: me.clone(), deleted: false });
                    changed = true;
                }
            }
            for (key, e) in st.entries.iter_mut() {
                if !e.deleted && !seen.contains(key) {
                    e.deleted = true;
                    e.version = now.max(e.version + 1);
                    e.author = me.clone();
                    changed = true;
                }
            }
            if changed {
                st.digest = digest_of(&st.entries);
                self.save_share(st);
            }
            changed
        };
        if changed {
            self.emit("share-files", json!({ "id": id }));
            self.emit_shares();
        }
        changed
    }

    /// Birkaç saniyede bir: tara, bilgileri/manifestoları gönder, eksikleri indir.
    pub async fn share_tick(self: &Arc<Self>) {
        self.cleanup_forgotten_shares();
        let ids: Vec<String> = self.shares.lock().unwrap().keys().cloned().collect();
        for id in &ids {
            self.share_scan(id).await;
        }
        let online: HashMap<String, Peer> =
            self.peers().into_iter().filter(|p| !p.blocked).map(|p| (p.fingerprint.clone(), p)).collect();
        let me = self.identity.fingerprint.clone();
        let mut jobs: Vec<(String, ShareJob)> = Vec::new();
        {
            let map = self.shares.lock().unwrap();
            for st in map.values() {
                let id = st.info.id.clone();
                for fp in &st.sync_pending {
                    jobs.push((fp.clone(), ShareJob::Sync(id.clone(), st.info.clone())));
                }
                if st.leave_pending {
                    jobs.push((st.info.admin.clone(), ShareJob::Leave(id.clone())));
                }
                if st.left || !st.info.can_access(&me) {
                    continue;
                }
                for m in &st.info.members {
                    if m.fingerprint != me && st.info.can_access(&m.fingerprint) && !st.sync_pending.contains(&m.fingerprint) {
                        let pushed = self.share_pushed.lock().unwrap().get(&(id.clone(), m.fingerprint.clone())).cloned();
                        let due = pushed.map_or(true, |(digest, at)| digest != st.digest || at.elapsed() > PUSH_REFRESH);
                        if due {
                            jobs.push((m.fingerprint.clone(), ShareJob::Manifest(id.clone())));
                        }
                    }
                }
                let downloading = self.share_fetching.lock().unwrap().iter().filter(|k| k.starts_with(&format!("{id}:"))).count();
                for (key, (_, from)) in st.wanted.iter().take(MAX_PARALLEL_DOWNLOADS.saturating_sub(downloading)) {
                    jobs.push((from.clone(), ShareJob::Fetch(id.clone(), key.clone())));
                }
            }
        }
        for (to, job) in jobs {
            let Some(peer) = online.get(&to).cloned() else { continue };
            let flight = job.key(&to);
            if self.share_backoff.lock().unwrap().get(&flight).map_or(false, |t| t.elapsed() < SHARE_BACKOFF) {
                continue;
            }
            if !self.share_fetching.lock().unwrap().insert(flight.clone()) {
                continue;
            }
            let node = self.clone();
            tokio::spawn(async move {
                let ok = node.run_share_job(job, &peer).await;
                if ok {
                    node.share_backoff.lock().unwrap().remove(&flight);
                } else {
                    node.share_backoff.lock().unwrap().insert(flight.clone(), Instant::now());
                }
                node.share_fetching.lock().unwrap().remove(&flight);
            });
        }
    }

    pub async fn run_share_loop(self: Arc<Self>) {
        loop {
            self.share_tick().await;
            tokio::select! {
                _ = sleep(Duration::from_secs(3)) => {}
                _ = self.shutdown.cancelled() => break,
            }
        }
    }

    async fn share_request(&self, peer: &Peer, msg: &Message) -> Result<Message, String> {
        let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut stream, msg).await.map_err(|e| e.to_string())?;
        let reply = timeout(Duration::from_secs(15), read_msg(&mut stream))
            .await
            .map_err(|_| "yanıt yok".to_string())?
            .map_err(|e| e.to_string())?;
        let _ = stream.shutdown().await;
        Ok(reply)
    }

    async fn run_share_job(&self, job: ShareJob, peer: &Peer) -> bool {
        let device = self.device_info();
        match job {
            ShareJob::Sync(id, info) => {
                let rev = info.rev;
                match self.share_request(peer, &Message::ShareUpdate { device, share: info }).await {
                    Ok(Message::GroupAck) | Ok(Message::ChatReject { .. }) => {
                        let mut map = self.shares.lock().unwrap();
                        if let Some(st) = map.get_mut(&id) {
                            if st.info.rev == rev {
                                st.sync_pending.retain(|f| *f != peer.fingerprint);
                                self.save_share(st);
                            }
                        }
                        true
                    }
                    _ => false,
                }
            }
            ShareJob::Leave(id) => match self.share_request(peer, &Message::ShareLeave { device, share_id: id.clone() }).await {
                Ok(Message::GroupAck) => {
                    let mut map = self.shares.lock().unwrap();
                    if let Some(st) = map.get_mut(&id) {
                        st.leave_pending = false;
                        self.save_share(st);
                    }
                    true
                }
                _ => false,
            },
            ShareJob::Manifest(id) => {
                let (entries, digest) = {
                    let map = self.shares.lock().unwrap();
                    let Some(st) = map.get(&id) else { return true };
                    (st.entries.values().cloned().collect::<Vec<_>>(), st.digest.clone())
                };
                match self.share_request(peer, &Message::ShareManifest { device, share_id: id.clone(), entries }).await {
                    Ok(Message::ShareManifest { entries, .. }) => {
                        self.share_pushed.lock().unwrap().insert((id.clone(), peer.fingerprint.clone()), (digest, Instant::now()));
                        self.merge_manifest(&id, &peer.fingerprint, entries);
                        true
                    }
                    _ => false,
                }
            }
            ShareJob::Fetch(id, key) => self.fetch_share_file(&id, &key, peer).await,
        }
    }

    /// Karşı tarafın kayıtlarını birleştirir: daha yeni silmeleri uygular, daha yeni dosyaları indirme listesine ekler.
    fn merge_manifest(&self, id: &str, from: &str, entries: Vec<ShareEntry>) {
        let mut changed = false;
        {
            let mut map = self.shares.lock().unwrap();
            let Some(st) = map.get_mut(id) else { return };
            let dir = PathBuf::from(&st.dir);
            for e in entries.into_iter().filter_map(clean_entry) {
                let key = key_of(&e.path);
                let local = st.entries.get(&key);
                if local.map_or(false, |l| !e.newer_than(l)) {
                    continue;
                }
                if e.deleted {
                    if local.map_or(false, |l| !l.deleted) {
                        move_to_trash(&dir, &local.unwrap().path);
                    }
                    st.wanted.remove(&key);
                    st.entries.insert(key, e);
                    changed = true;
                } else if st.wanted.get(&key).map_or(true, |(w, _)| e.newer_than(w)) {
                    st.wanted.insert(key, (e, from.to_string()));
                    changed = true;
                }
            }
            if changed {
                st.digest = digest_of(&st.entries);
                self.save_share(st);
            }
        }
        if changed {
            self.emit("share-files", json!({ "id": id }));
            self.emit_shares();
        }
    }

    async fn fetch_share_file(&self, id: &str, key: &str, peer: &Peer) -> bool {
        let (wanted, dir) = {
            let map = self.shares.lock().unwrap();
            let Some(st) = map.get(id) else { return true };
            let Some((w, _)) = st.wanted.get(key).cloned() else { return true };
            (w, PathBuf::from(&st.dir))
        };
        let result: Result<Option<ShareEntry>, String> = async {
            let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
            write_msg(&mut stream, &Message::ShareFetch { device: self.device_info(), share_id: id.into(), path: wanted.path.clone() })
                .await
                .map_err(|e| e.to_string())?;
            let reply = timeout(Duration::from_secs(15), read_msg(&mut stream)).await.map_err(|_| "yanıt yok".to_string())?.map_err(|e| e.to_string())?;
            let served = match reply {
                Message::ShareFileHeader { entry } => clean_entry(entry).ok_or("geçersiz kayıt")?,
                // Karşı tarafta artık yok/değişmiş: bir sonraki manifestoda düzelir.
                _ => return Ok(None),
            };
            if key_of(&served.path) != key || served.deleted {
                return Err("beklenmeyen dosya".into());
            }
            let tmp_dir = dir.join(SHARE_META_DIR).join("tmp");
            tokio::fs::create_dir_all(&tmp_dir).await.map_err(|e| e.to_string())?;
            let tmp = tmp_dir.join(format!("{}.ffpart", uuid::Uuid::new_v4()));
            let write: Result<(), String> = async {
                let mut w = BufWriter::with_capacity(CHUNK, tokio::fs::File::create(&tmp).await.map_err(|e| e.to_string())?);
                let mut remaining = served.size;
                let mut buf = vec![0u8; 256 * 1024];
                while remaining > 0 {
                    let want = remaining.min(buf.len() as u64) as usize;
                    let n = timeout(Duration::from_secs(30), stream.read(&mut buf[..want]))
                        .await
                        .map_err(|_| "indirme durdu".to_string())?
                        .map_err(|e| e.to_string())?;
                    if n == 0 {
                        return Err("bağlantı erken kapandı".into());
                    }
                    w.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
                    remaining -= n as u64;
                }
                w.flush().await.map_err(|e| e.to_string())?;
                let f = w.into_inner().into_std().await;
                // Değişiklik algılamanın tutması için dosyanın zamanı kaynakla aynı olsun.
                let _ = f.set_modified(UNIX_EPOCH + Duration::from_millis(served.mtime));
                drop(f);
                Ok(())
            }
            .await;
            if let Err(e) = write {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(e);
            }
            // Yerine koy (kilit altında, yerel kayıt bu arada daha yeni olmadıysa).
            let mut target = dir.clone();
            for p in served.path.split('/') {
                target.push(p);
            }
            let mut map = self.shares.lock().unwrap();
            let Some(st) = map.get_mut(id) else {
                let _ = std::fs::remove_file(&tmp);
                return Ok(None);
            };
            if st.entries.get(key).map_or(false, |l| !served.newer_than(l)) {
                let _ = std::fs::remove_file(&tmp);
                st.wanted.remove(key);
                return Ok(None);
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if target.exists() {
                move_to_trash(&dir, &served.path);
            }
            std::fs::rename(&tmp, &target).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("dosya yerleştirilemedi: {e}")
            })?;
            st.entries.insert(key.to_string(), served.clone());
            if st.wanted.get(key).map_or(false, |(w, _)| !w.newer_than(&served)) {
                st.wanted.remove(key);
            }
            st.digest = digest_of(&st.entries);
            self.save_share(st);
            Ok(Some(served))
        }
        .await;
        match result {
            Ok(Some(_)) => {
                self.emit("share-files", json!({ "id": id }));
                self.emit_shares();
                true
            }
            Ok(None) => {
                // Kaynakta artık yok: bekleme listesinden çıkar, sonraki manifestoda doğrusu gelir.
                let mut map = self.shares.lock().unwrap();
                if let Some(st) = map.get_mut(id) {
                    st.wanted.remove(key);
                }
                true
            }
            Err(e) => {
                eprintln!("[fastfiles] ortak klasör dosyası alınamadı ({}): {e}", wanted.path);
                false
            }
        }
    }

    // ───────────── Ağdan gelenler ─────────────

    /// İstek yapan bu klasöre erişebilir mi ve biz de erişebiliyor muyuz?
    fn share_access(&self, id: &str, requester: &str) -> Result<PathBuf, &'static str> {
        if self.is_blocked(requester) {
            return Err("blocked");
        }
        let map = self.shares.lock().unwrap();
        let st = map.get(id).ok_or("unknown_share")?;
        if st.left || !st.info.can_access(&self.identity.fingerprint) {
            return Err("not_member");
        }
        if !st.info.can_access(requester) {
            return Err("no_access");
        }
        Ok(PathBuf::from(&st.dir))
    }

    pub(super) fn accept_share_update(&self, sender: &str, info: ShareInfo) -> Message {
        if self.is_blocked(sender) {
            return Message::ChatReject { reason: "blocked".into() };
        }
        let Some(info) = clean_share(info) else { return Message::ChatReject { reason: "invalid".into() } };
        if info.admin != sender {
            return Message::ChatReject { reason: "not_admin".into() };
        }
        let me = self.identity.fingerprint.clone();
        let existing = self.shares.lock().unwrap().get(&info.id).map(|s| (s.info.admin.clone(), s.info.rev));
        match existing {
            Some((admin, rev)) => {
                if admin != info.admin {
                    return Message::ChatReject { reason: "not_admin".into() };
                }
                if info.rev <= rev {
                    return Message::GroupAck;
                }
                let mut map = self.shares.lock().unwrap();
                if let Some(st) = map.get_mut(&info.id) {
                    // Kullanıcı klasörü sildiyse (ayrılık bildirimi yolda) güncelleme onu geri katmaz.
                    st.left = st.forgotten || info.deleted || !info.has(&me);
                    st.info = info;
                    if st.left || !st.info.can_access(&me) {
                        st.wanted.clear();
                    }
                    self.save_share(st);
                }
            }
            None => {
                if info.deleted || !info.has(&me) {
                    return Message::GroupAck;
                }
                let dir = self.new_share_dir(&info.name);
                let _ = std::fs::create_dir_all(&dir);
                let st = ShareState { info, dir: dir.to_string_lossy().into_owned(), ..Default::default() };
                self.save_share(&st);
                self.shares.lock().unwrap().insert(st.info.id.clone(), st);
            }
        }
        self.emit_shares();
        Message::GroupAck
    }

    pub(super) fn accept_share_manifest(&self, sender: &str, id: &str, entries: Vec<ShareEntry>) -> Message {
        if let Err(reason) = self.share_access(id, sender) {
            return Message::ChatReject { reason: reason.into() };
        }
        let mine: Vec<ShareEntry> = self.shares.lock().unwrap().get(id).map(|s| s.entries.values().cloned().collect()).unwrap_or_default();
        self.merge_manifest(id, sender, entries);
        Message::ShareManifest { device: self.device_info(), share_id: id.to_string(), entries: mine }
    }

    pub(super) async fn serve_share_file(&self, sender: &str, id: &str, path: &str, wr: &mut SrvWrite) -> Result<(), String> {
        let dir = match self.share_access(id, sender) {
            Ok(d) => d,
            Err(r) => return write_msg(wr, &Message::ChatReject { reason: r.into() }).await.map_err(|e| e.to_string()),
        };
        let Some(parts) = sanitize_rel_path(path) else {
            return write_msg(wr, &Message::ChatReject { reason: "invalid".into() }).await.map_err(|e| e.to_string());
        };
        let entry = self.shares.lock().unwrap().get(id).and_then(|s| s.entries.get(&key_of(&parts.join("/"))).cloned());
        let mut full = dir.clone();
        for p in &parts {
            full.push(p);
        }
        let meta = tokio::fs::symlink_metadata(&full).await.ok();
        let ok = match (&entry, &meta) {
            (Some(e), Some(m)) => !e.deleted && m.is_file() && m.len() == e.size && !parts[0].eq_ignore_ascii_case(SHARE_META_DIR),
            _ => false,
        };
        if !ok {
            return write_msg(wr, &Message::ChatReject { reason: "missing".into() }).await.map_err(|e| e.to_string());
        }
        let entry = entry.unwrap();
        let f = tokio::fs::File::open(&full).await.map_err(|e| e.to_string())?;
        write_msg(wr, &Message::ShareFileHeader { entry: entry.clone() }).await.map_err(|e| e.to_string())?;
        let mut limited = f.take(entry.size);
        tokio::io::copy(&mut limited, wr).await.map_err(|e| e.to_string())?;
        wr.flush().await.map_err(|e| e.to_string())?;
        let _ = wr.shutdown().await;
        Ok(())
    }

    pub(super) fn accept_share_unlock(&self, sender: &str, id: &str, password: &str) -> Message {
        let fail = |m: &str| Message::ShareUnlockResult { ok: false, message: m.to_string() };
        if self.is_blocked(sender) {
            return fail("Erişim reddedildi");
        }
        // Kaba kuvvet denemelerine karşı sınır.
        {
            let mut attempts = self.unlock_attempts.lock().unwrap();
            let q = attempts.entry(sender.to_string()).or_default();
            while q.front().map_or(false, |t| t.elapsed() > UNLOCK_WINDOW) {
                q.pop_front();
            }
            if q.len() >= UNLOCK_MAX_ATTEMPTS {
                return fail("Çok fazla hatalı deneme. 5 dakika sonra tekrar deneyin");
            }
            q.push_back(Instant::now());
        }
        let me = self.identity.fingerprint.clone();
        let mut map = self.shares.lock().unwrap();
        let Some(st) = map.get_mut(id) else { return fail("Klasör bulunamadı") };
        if st.info.admin != me || st.info.deleted || !st.info.has(sender) {
            return fail("Bu klasöre erişiminiz yok");
        }
        let Some(hash) = &st.password else { return Message::ShareUnlockResult { ok: true, message: String::new() } };
        if password.chars().count() > 256 || !verify_password(hash, password) {
            return fail("Şifre yanlış");
        }
        if !st.info.unlocked.iter().any(|f| f == sender) {
            st.info.unlocked.push(sender.to_string());
            st.info.rev += 1;
            st.sync_pending = st.info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
            self.save_share(st);
        }
        self.unlock_attempts.lock().unwrap().remove(sender);
        drop(map);
        self.emit_shares();
        Message::ShareUnlockResult { ok: true, message: String::new() }
    }

    pub(super) fn accept_share_leave(&self, sender: &str, id: &str) {
        let me = self.identity.fingerprint.clone();
        {
            let mut map = self.shares.lock().unwrap();
            let Some(st) = map.get_mut(id) else { return };
            if st.info.admin != me || st.info.deleted || !st.info.has(sender) || sender == me {
                return;
            }
            st.info.members.retain(|m| m.fingerprint != sender);
            st.info.unlocked.retain(|f| f != sender);
            st.info.rev += 1;
            st.sync_pending = st.info.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect();
            self.save_share(st);
        }
        self.emit_shares();
    }
}

#[derive(Debug, Clone)]
enum ShareJob {
    Sync(String, ShareInfo),
    Leave(String),
    Manifest(String),
    Fetch(String, String),
}

impl ShareJob {
    fn key(&self, to: &str) -> String {
        match self {
            ShareJob::Sync(id, info) => format!("{id}:sync:{to}:{}", info.rev),
            ShareJob::Leave(id) => format!("{id}:leave:{to}"),
            ShareJob::Manifest(id) => format!("{id}:manifest:{to}"),
            ShareJob::Fetch(id, key) => format!("{id}:fetch:{key}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hashing() {
        let h = hash_password("gizli-şifre").unwrap();
        assert!(verify_password(&h, "gizli-şifre"));
        assert!(!verify_password(&h, "yanlis"));
        let h2 = hash_password("gizli-şifre").unwrap();
        assert_ne!(h.salt, h2.salt, "her şifre farklı tuzla saklanmalı");
    }

    #[test]
    fn entry_paths_are_sanitized() {
        let fp = "a".repeat(64);
        let mk = |p: &str| ShareEntry { path: p.into(), size: 1, mtime: 1, version: 1, author: fp.clone(), deleted: false };
        assert!(clean_entry(mk("../../Windows/x.dll")).is_none());
        assert!(clean_entry(mk(".fastfiles/tmp/x")).is_none());
        assert!(clean_entry(mk("a/~$rapor.docx")).is_none());
        assert_eq!(clean_entry(mk("Belgeler\\rapor.pdf")).unwrap().path, "Belgeler/rapor.pdf");
    }
}
