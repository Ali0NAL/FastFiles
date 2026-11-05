//! Uygulamanın çekirdeği: cihaz listesi, aktarımlar, TLS sunucusu, gönderme/alma.
//!
//! Tauri'den bağımsızdır; arayüze olaylar `Emitter` geri çağrısı ile iletilir.
//! Bu sayede aynı kod testlerde iki "sanal cihaz" ile uçtan uca denenebilir.

#[path = "group.rs"]
mod group;
#[path = "avatar.rs"]
mod avatar;
#[path = "share.rs"]
pub mod share;
#[path = "poll.rs"]
pub mod poll;

use crate::chat::{ChatMessage, ChatStore, ConversationSummary, MsgStatus};
use crate::config::{fingerprint, Identity, Settings, TrustedDevice};
use crate::protocol::*;
use crate::tls;
use crate::update::{self, PackageHeader, CURRENT_VERSION};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufWriter, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use tokio_rustls::{server::TlsStream as ServerTls, TlsAcceptor, TlsConnector};
use tokio_util::sync::CancellationToken;

pub type Emitter = Arc<dyn Fn(&str, Value) + Send + Sync>;

const CHUNK: usize = 1024 * 1024;
const PEER_TTL: Duration = Duration::from_secs(10);
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const EMIT_INTERVAL: Duration = Duration::from_millis(150);
/// Bir cihazdan kabul edilen en fazla mesaj sayısı / zaman penceresi (spam koruması).
const CHAT_RATE_MAX: usize = 30;
const CHAT_RATE_WINDOW: Duration = Duration::from_secs(30);

type SrvRead = ReadHalf<ServerTls<TcpStream>>;
type SrvWrite = WriteHalf<ServerTls<TcpStream>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub os: String,
    pub ip: String,
    pub port: u16,
    /// Elle eklenen adres (varsa).
    pub manual_addr: Option<String>,
    pub trusted: bool,
    pub blocked: bool,
    /// Karşı cihazın FastFiles sürümü (eski sürümlerde boş).
    pub version: String,
    /// Karşı cihazın dağıttığı güncelleme paketi sürümü.
    pub pkg: String,
    /// Karşı cihazın profil resmi özeti (yoksa boş).
    pub avatar: String,
    #[serde(skip)]
    last_seen: Option<Instant>,
    /// Cihazın son görüldüğü tüm adresler (Wi-Fi + ethernet + VPN gibi birden fazla olabilir).
    #[serde(skip)]
    addrs: Vec<(IpAddr, Instant)>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Waiting,
    Active,
    Done,
    Declined,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub id: String,
    pub direction: Direction,
    pub peer_id: String,
    pub peer_name: String,
    pub title: String,
    pub file_count: usize,
    pub total: u64,
    pub done: u64,
    /// byte/sn
    pub speed: f64,
    pub status: Status,
    pub error: Option<String>,
    pub current_file: Option<String>,
    pub save_path: Option<String>,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    #[serde(skip)]
    last_emit: Option<Instant>,
    #[serde(skip)]
    sample: Option<(Instant, u64)>,
}

impl Transfer {
    fn new(
        id: String,
        direction: Direction,
        peer_id: String,
        peer_name: String,
        title: String,
        file_count: usize,
        total: u64,
        status: Status,
    ) -> Self {
        Self {
            id,
            direction,
            peer_id,
            peer_name,
            title,
            file_count,
            total,
            done: 0,
            speed: 0.0,
            status,
            error: None,
            current_file: None,
            save_path: None,
            created_at: now_ms(),
            finished_at: None,
            last_emit: None,
            sample: None,
        }
    }

    fn is_finished(&self) -> bool {
        !matches!(self.status, Status::Waiting | Status::Active)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Decision {
    pub accept: bool,
    pub trust: bool,
}

enum SendOutcome {
    Done,
    Declined(String),
    Cancelled,
    PeerCancelled,
}

enum RecvError {
    Cancelled,
    Failed(String),
}

pub struct Node {
    pub identity: Identity,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    port: AtomicU16,
    peers: Mutex<HashMap<String, Peer>>,
    transfers: Mutex<HashMap<String, Transfer>>,
    cancels: Mutex<HashMap<String, CancellationToken>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Decision>>>,
    emit: Emitter,
    acceptor: TlsAcceptor,
    /// Yerel alt ağlar (ağ adresi, maske) — en uygun karşı adresi seçmek için.
    subnets: Mutex<Vec<(u32, u32)>>,
    last_peers: Mutex<String>,
    chats: Mutex<ChatStore>,
    chat_rate: Mutex<HashMap<String, VecDeque<Instant>>>,
    update_dir: PathBuf,
    update_key: Mutex<Vec<u8>>,
    update_pkg: Mutex<Option<PackageHeader>>,
    update_fetching: AtomicBool,
    update_failures: Mutex<HashMap<String, Instant>>,
    outbox_in_flight: Mutex<HashSet<String>>,
    outbox_backoff: Mutex<HashMap<String, Instant>>,
    avatar_dir: PathBuf,
    my_avatar: Mutex<Option<(String, Vec<u8>)>>,
    avatar_hashes: Mutex<HashMap<String, String>>,
    avatar_fetching: Mutex<HashSet<String>>,
    avatar_failures: Mutex<HashMap<String, Instant>>,
    shares: Mutex<HashMap<String, share::ShareState>>,
    shares_dir: PathBuf,
    share_pushed: Mutex<HashMap<(String, String), (String, Instant)>>,
    share_fetching: Mutex<HashSet<String>>,
    share_backoff: Mutex<HashMap<String, Instant>>,
    unlock_attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
    polls: Mutex<HashMap<String, poll::PollState>>,
    polls_path: PathBuf,
    pub shutdown: CancellationToken,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Node {
    pub fn new(config_dir: &Path, default_download: &Path, emit: Emitter) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
        let identity = Identity::load_or_create(config_dir)?;
        let settings_path = config_dir.join("settings.json");
        let settings = Settings::load(&settings_path, default_download);
        let acceptor = TlsAcceptor::from(tls::server_config(&identity)?);
        let chats = ChatStore::load(&config_dir.join("chats.json"));
        let update_dir = config_dir.join("updates");
        std::fs::create_dir_all(&update_dir).map_err(|e| e.to_string())?;
        let node = Arc::new(Self {
            identity,
            settings: Mutex::new(settings),
            settings_path,
            port: AtomicU16::new(0),
            peers: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            emit,
            acceptor,
            subnets: Mutex::new(Vec::new()),
            last_peers: Mutex::new(String::new()),
            chats: Mutex::new(chats),
            chat_rate: Mutex::new(HashMap::new()),
            update_dir,
            update_key: Mutex::new(update::public_key()),
            update_pkg: Mutex::new(None),
            update_fetching: AtomicBool::new(false),
            update_failures: Mutex::new(HashMap::new()),
            outbox_in_flight: Mutex::new(HashSet::new()),
            outbox_backoff: Mutex::new(HashMap::new()),
            avatar_dir: config_dir.join("avatars"),
            my_avatar: Mutex::new(None),
            avatar_hashes: Mutex::new(HashMap::new()),
            avatar_fetching: Mutex::new(HashSet::new()),
            avatar_failures: Mutex::new(HashMap::new()),
            shares: Mutex::new(HashMap::new()),
            shares_dir: config_dir.join("shares"),
            share_pushed: Mutex::new(HashMap::new()),
            share_fetching: Mutex::new(HashSet::new()),
            share_backoff: Mutex::new(HashMap::new()),
            unlock_attempts: Mutex::new(HashMap::new()),
            polls: Mutex::new(HashMap::new()),
            polls_path: config_dir.join("polls.json"),
            shutdown: CancellationToken::new(),
        });
        node.load_cached_package();
        let _ = std::fs::create_dir_all(&node.avatar_dir);
        node.load_avatars();
        let _ = std::fs::create_dir_all(&node.shares_dir);
        node.load_shares();
        node.load_polls();
        Ok(node)
    }

    pub fn set_local_subnets(&self, nets: Vec<(u32, u32)>) {
        *self.subnets.lock().unwrap() = nets;
    }

    /// Adres tercihi: bizimle aynı alt ağda > özel ağ > diğer > link-local.
    fn addr_score(&self, ip: &IpAddr) -> u8 {
        match ip {
            IpAddr::V4(v4) => {
                let n = u32::from(*v4);
                if self.subnets.lock().unwrap().iter().any(|(net, mask)| n & mask == net & mask) {
                    3
                } else if v4.is_private() {
                    2
                } else if v4.is_link_local() {
                    0
                } else {
                    1
                }
            }
            IpAddr::V6(_) => 1,
        }
    }

    /// Bağlanılacak adresler, en olası olandan başlayarak.
    fn candidates(&self, peer: &Peer) -> Vec<SocketAddr> {
        let mut ips: Vec<IpAddr> = peer.addrs.iter().map(|(a, _)| *a).collect();
        if let Ok(ip) = peer.ip.parse() {
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        }
        let current = peer.ip.clone();
        ips.sort_by_key(|a| std::cmp::Reverse((self.addr_score(a), a.to_string() == current)));
        ips.into_iter().map(|ip| SocketAddr::new(ip, peer.port)).collect()
    }

    fn emit(&self, event: &str, payload: Value) {
        (self.emit)(event, payload)
    }

    /// Bildirim; tıklanınca arayüzün açacağı yer 	arget ile verilir (ör. {"view":"polls"}).
    fn notify_to(&self, title: &str, body: &str, target: Value) {
        self.emit("notify", json!({ "title": title, "body": body, "target": target }));
    }

    /// Windows bildirimi isteği. Pencere öndeyse kabuk (lib.rs) bildirimi göstermez.
    fn notify(&self, title: &str, body: &str, transfer_id: Option<&str>) {
        self.emit("notify", json!({ "title": title, "body": body, "transferId": transfer_id }));
    }

    // ───────────────────────────── Kimlik & ayarlar ─────────────────────────────

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.identity.id.clone(),
            name: self.settings.lock().unwrap().device_name.clone(),
            port: self.port(),
            fingerprint: self.identity.fingerprint.clone(),
            os: std::env::consts::OS.to_string(),
            version: CURRENT_VERSION.to_string(),
            pkg: self.update_pkg.lock().unwrap().as_ref().map(|h| h.version.clone()).unwrap_or_default(),
            avatar: self.my_avatar_hash(),
        }
    }

    // ───────────────────────────── Güncelleme ─────────────────────────────

    fn package_path(&self) -> PathBuf {
        self.update_dir.join("package.ffupdate")
    }

    fn update_key(&self) -> Vec<u8> {
        self.update_key.lock().unwrap().clone()
    }

    /// Önbellekteki paketi yükler; kullanılan sürümden eskiyse siler.
    fn load_cached_package(&self) {
        let path = self.package_path();
        if !path.exists() {
            return;
        }
        match update::open_verified(&path, &self.update_key()) {
            Ok((h, _)) if !update::is_newer(CURRENT_VERSION, &h.version) => {
                *self.update_pkg.lock().unwrap() = Some(h);
            }
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Kurulmayı bekleyen (çalışandan yeni) güncelleme sürümü.
    pub fn pending_update(&self) -> Option<String> {
        self.update_pkg
            .lock()
            .unwrap()
            .as_ref()
            .filter(|h| update::is_newer(&h.version, CURRENT_VERSION))
            .map(|h| h.version.clone())
    }

    /// Dağıtılabilen paket sürümü (kurulu olanla aynı da olabilir).
    pub fn cached_package_version(&self) -> Option<String> {
        self.update_pkg.lock().unwrap().as_ref().map(|h| h.version.clone())
    }

    fn package_ready(&self, h: &PackageHeader) {
        *self.update_pkg.lock().unwrap() = Some(h.clone());
        if update::is_newer(&h.version, CURRENT_VERSION) {
            self.emit("update-ready", json!({ "version": h.version }));
        }
    }

    /// Yayıncının verdiği .ffupdate dosyasını doğrulayıp önbelleğe alır (ve dağıtmaya başlar).
    pub fn import_update(&self, path: &Path) -> Result<PackageHeader, String> {
        let key = self.update_key();
        let (h, _) = update::open_verified(path, &key)?;
        if update::is_newer(CURRENT_VERSION, &h.version) {
            return Err(format!("Bu paket ({}) kullandığınız sürümden ({CURRENT_VERSION}) eski", h.version));
        }
        if let Some(cur) = self.cached_package_version() {
            if !update::is_newer(&h.version, &cur) {
                return if cur == h.version { Ok(h) } else { Err(format!("Zaten daha yeni bir paket var ({cur})")) };
            }
        }
        let tmp = self.update_dir.join("import.tmp");
        std::fs::copy(path, &tmp).map_err(|e| format!("Paket kopyalanamadı: {e}"))?;
        let verified = update::open_verified(&tmp, &key);
        if let Err(e) = verified {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        std::fs::rename(&tmp, self.package_path()).map_err(|e| e.to_string())?;
        self.package_ready(&h);
        Ok(h)
    }

    /// Gelen kutusu: `updates\inbox` klasörüne bırakılan .ffupdate paketlerini içe aktarır.
    /// Yayın betiği (release.ps1) yeni paketi buraya koyar; böylece sürüm çıkarmak tek komuttur
    /// ve güncelleme ağa kendiliğinden yayılır. İmza her zamanki gibi doğrulanır.
    pub fn check_update_inbox(&self) {
        let inbox = self.update_dir.join("inbox");
        let Ok(entries) = std::fs::read_dir(&inbox) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()).map_or(true, |e| !e.eq_ignore_ascii_case("ffupdate")) {
                continue;
            }
            match self.import_update(&path) {
                Ok(h) => eprintln!("[fastfiles] gelen kutusundan güncelleme alındı: {}", h.version),
                Err(e) => eprintln!("[fastfiles] gelen kutusundaki paket kullanılamadı ({}): {e}", path.display()),
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Ağda daha yeni paket dağıtan bir cihaz varsa arka planda indirir.
    pub fn check_for_updates(self: &Arc<Self>) {
        if self.update_fetching.load(Ordering::Relaxed) {
            return;
        }
        let cached = self.cached_package_version();
        let best = self
            .peers()
            .into_iter()
            .filter(|p| !p.blocked && update::is_newer(&p.pkg, CURRENT_VERSION))
            .filter(|p| cached.as_ref().map_or(true, |c| update::is_newer(&p.pkg, c)))
            .filter(|p| {
                let fails = self.update_failures.lock().unwrap();
                fails.get(&format!("{}@{}", p.fingerprint, p.pkg)).map_or(true, |t| t.elapsed() > Duration::from_secs(120))
            })
            .max_by_key(|p| update::parse_version(&p.pkg));
        let Some(peer) = best else { return };
        self.update_fetching.store(true, Ordering::Relaxed);
        let node = self.clone();
        tokio::spawn(async move {
            if let Err(e) = node.fetch_update(&peer).await {
                eprintln!("[fastfiles] güncelleme indirilemedi ({}): {e}", peer.name);
                node.update_failures.lock().unwrap().insert(format!("{}@{}", peer.fingerprint, peer.pkg), Instant::now());
                let _ = tokio::fs::remove_file(node.update_dir.join("download.tmp")).await;
            }
            node.update_fetching.store(false, Ordering::Relaxed);
        });
    }

    async fn fetch_update(&self, peer: &Peer) -> Result<PackageHeader, String> {
        let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut stream, &Message::UpdateRequest).await.map_err(|e| e.to_string())?;
        let reply = timeout(Duration::from_secs(10), read_msg(&mut stream))
            .await
            .map_err(|_| "yanıt yok".to_string())?
            .map_err(|e| e.to_string())?;
        let header = match reply {
            Message::UpdateOffer { header } => header,
            Message::UpdateNone => return Err("karşı tarafta paket yok".into()),
            _ => return Err("beklenmeyen yanıt".into()),
        };
        // İmzayı veri indirmeden önce kontrol et: sahte paket için tek bayt bile yazılmaz.
        let key = self.update_key();
        update::verify_header(&header, &key)?;
        if !update::is_newer(&header.version, CURRENT_VERSION) {
            return Err("paket yeni değil".into());
        }

        let tmp = self.update_dir.join("download.tmp");
        let mut head = Vec::new();
        update::write_header(&mut head, &header).map_err(|e| e.to_string())?;
        let mut w = BufWriter::new(tokio::fs::File::create(&tmp).await.map_err(|e| e.to_string())?);
        w.write_all(&head).await.map_err(|e| e.to_string())?;
        let mut remaining = header.size;
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
        drop(w.into_inner().into_std().await);

        // Tam doğrulama (imza + SHA-256 + boyut), sonra önbelleğe al.
        let (path, key2) = (tmp.clone(), key.clone());
        let (verified, _) = tokio::task::spawn_blocking(move || update::open_verified(&path, &key2))
            .await
            .map_err(|e| e.to_string())??;
        if verified != header {
            return Err("başlık uyuşmuyor".into());
        }
        tokio::fs::rename(&tmp, self.package_path()).await.map_err(|e| e.to_string())?;
        self.package_ready(&header);
        Ok(header)
    }

    async fn serve_update(&self, wr: &mut SrvWrite) -> Result<(), String> {
        let path = self.package_path();
        let key = self.update_key();
        let cached = tokio::task::spawn_blocking(move || update::open_verified(&path, &key).ok())
            .await
            .map_err(|e| e.to_string())?;
        let Some((header, offset)) = cached else {
            return write_msg(wr, &Message::UpdateNone).await.map_err(|e| e.to_string());
        };
        write_msg(wr, &Message::UpdateOffer { header: header.clone() }).await.map_err(|e| e.to_string())?;
        let mut f = tokio::fs::File::open(self.package_path()).await.map_err(|e| e.to_string())?;
        f.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| e.to_string())?;
        let mut limited = f.take(header.size);
        tokio::io::copy(&mut limited, wr).await.map_err(|e| e.to_string())?;
        wr.flush().await.map_err(|e| e.to_string())?;
        let _ = wr.shutdown().await;
        Ok(())
    }

    /// Aktarım veya bekleyen istek varken güncelleme kurulmaz.
    pub fn is_busy(&self) -> bool {
        self.transfers.lock().unwrap().values().any(|t| !t.is_finished()) || !self.pending.lock().unwrap().is_empty()
    }

    /// Kurulum dosyasını geçici klasöre çıkarır (yeniden doğrulayarak) ve yolunu döndürür.
    pub fn prepare_install(&self) -> Result<(PathBuf, String), String> {
        let version = self.pending_update().ok_or("Kurulacak güncelleme yok")?;
        if self.is_busy() {
            return Err("Süren bir aktarım var; bitince güncellenecek".into());
        }
        let out = std::env::temp_dir().join(format!("FastFiles_{version}_setup.exe"));
        update::extract_installer(&self.package_path(), &out, &self.update_key())?;
        Ok((out, version))
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Settings {
        let snapshot = {
            let mut s = self.settings.lock().unwrap();
            f(&mut s);
            s.clone()
        };
        if let Err(e) = snapshot.save(&self.settings_path) {
            eprintln!("[fastfiles] ayarlar kaydedilemedi: {e}");
        }
        snapshot
    }

    pub fn is_trusted(&self, fp: &str) -> bool {
        self.settings.lock().unwrap().trusted.iter().any(|t| t.fingerprint == fp)
    }

    pub fn set_trusted(&self, fp: &str, name: &str, trusted: bool) -> Settings {
        let s = self.update_settings(|s| {
            s.trusted.retain(|t| t.fingerprint != fp);
            if trusted {
                s.blocked.retain(|t| t.fingerprint != fp);
                s.trusted.push(TrustedDevice { fingerprint: fp.to_string(), name: name.to_string() });
            }
        });
        self.emit_peers();
        s
    }

    pub fn is_blocked(&self, fp: &str) -> bool {
        self.settings.lock().unwrap().blocked.iter().any(|t| t.fingerprint == fp)
    }

    /// Engellenen cihaz güvenilirler listesinden de çıkarılır.
    pub fn set_blocked(&self, fp: &str, name: &str, blocked: bool) -> Settings {
        let s = self.update_settings(|s| {
            s.blocked.retain(|t| t.fingerprint != fp);
            if blocked {
                s.trusted.retain(|t| t.fingerprint != fp);
                s.blocked.push(TrustedDevice { fingerprint: fp.to_string(), name: name.to_string() });
            }
        });
        self.emit_peers();
        s
    }

    // ───────────────────────────── Cihaz listesi ─────────────────────────────

    pub fn peers(&self) -> Vec<Peer> {
        let (trusted, blocked): (HashSet<String>, HashSet<String>) = {
            let s = self.settings.lock().unwrap();
            (
                s.trusted.iter().map(|t| t.fingerprint.clone()).collect(),
                s.blocked.iter().map(|t| t.fingerprint.clone()).collect(),
            )
        };
        let mut list: Vec<Peer> = self
            .peers
            .lock()
            .unwrap()
            .values()
            .cloned()
            .map(|mut p| {
                p.trusted = trusted.contains(&p.fingerprint);
                p.blocked = blocked.contains(&p.fingerprint);
                p
            })
            .collect();
        list.sort_by_key(|p| p.name.to_lowercase());
        list
    }

    pub fn peer(&self, id: &str) -> Option<Peer> {
        self.peers.lock().unwrap().get(id).cloned()
    }

    /// Cihaz listesini yalnızca arayüzde görünen bir şey değiştiyse gönderir.
    fn emit_peers(&self) {
        let payload = json!(self.peers());
        let text = payload.to_string();
        {
            let mut last = self.last_peers.lock().unwrap();
            if *last == text {
                return;
            }
            *last = text;
        }
        self.emit("peers", payload);
    }

    pub fn upsert_peer(&self, mut d: DeviceInfo, ip: IpAddr, manual_addr: Option<String>) {
        if d.id == self.identity.id || !valid_message_id(&d.id) {
            return;
        }
        d.name = sanitize_name(&d.name);
        d.os = d.os.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
        if update::parse_version(&d.version).is_none() {
            d.version.clear();
        }
        if update::parse_version(&d.pkg).is_none() {
            d.pkg.clear();
        }
        if !avatar::valid_avatar_hash(&d.avatar) {
            d.avatar.clear();
        }
        let now = Instant::now();
        let changed = {
            let mut peers = self.peers.lock().unwrap();
            match peers.get_mut(&d.id) {
                Some(p) => {
                    match p.addrs.iter_mut().find(|(a, _)| *a == ip) {
                        Some(entry) => entry.1 = now,
                        None => p.addrs.push((ip, now)),
                    }
                    p.addrs.retain(|(_, t)| now.duration_since(*t) < PEER_TTL);
                    // Aynı cihaz birden fazla arayüzden duyulabilir; adres her pakette
                    // zıplamasın diye en iyi skorlu adresi seç, eşitlikte mevcut olanı koru.
                    let best = p
                        .addrs
                        .iter()
                        .map(|(a, _)| *a)
                        .max_by_key(|a| (self.addr_score(a), a.to_string() == p.ip))
                        .unwrap_or(ip)
                        .to_string();
                    let changed = p.name != d.name
                        || p.ip != best
                        || p.port != d.port
                        || p.fingerprint != d.fingerprint
                        || p.version != d.version
                        || p.pkg != d.pkg
                        || p.avatar != d.avatar;
                    p.avatar = d.avatar;
                    p.version = d.version;
                    p.pkg = d.pkg;
                    p.name = d.name;
                    p.ip = best;
                    p.port = d.port;
                    p.fingerprint = d.fingerprint;
                    p.os = d.os;
                    if manual_addr.is_some() {
                        p.manual_addr = manual_addr;
                    }
                    p.last_seen = Some(Instant::now());
                    changed
                }
                None => {
                    peers.insert(
                        d.id.clone(),
                        Peer {
                            id: d.id,
                            name: d.name,
                            fingerprint: d.fingerprint,
                            os: d.os,
                            ip: ip.to_string(),
                            port: d.port,
                            manual_addr,
                            trusted: false,
                            blocked: false,
                            version: d.version,
                            pkg: d.pkg,
                            avatar: d.avatar,
                            last_seen: Some(now),
                            addrs: vec![(ip, now)],
                        },
                    );
                    true
                }
            }
        };
        if changed {
            self.emit_peers();
        }
    }

    pub fn remove_peer(&self, id: &str) {
        if self.peers.lock().unwrap().remove(id).is_some() {
            self.emit_peers();
        }
    }

    pub fn prune_peers(&self) {
        let removed = {
            let mut peers = self.peers.lock().unwrap();
            let before = peers.len();
            peers.retain(|_, p| {
                let keep = p.last_seen.map_or(false, |t| t.elapsed() < PEER_TTL);
                #[cfg(debug_assertions)]
                if !keep {
                    eprintln!("[prune] {} ({})", p.name, p.ip);
                }
                keep
            });
            before != peers.len()
        };
        if removed {
            self.emit_peers();
        }
    }

    // ───────────────────────────── Aktarım listesi ─────────────────────────────

    pub fn transfers(&self) -> Vec<Transfer> {
        let mut list: Vec<Transfer> = self.transfers.lock().unwrap().values().cloned().collect();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        list
    }

    fn insert_transfer(&self, t: Transfer) {
        self.emit("transfer", json!(t));
        self.transfers.lock().unwrap().insert(t.id.clone(), t);
    }

    fn update_transfer(&self, id: &str, f: impl FnOnce(&mut Transfer)) {
        let snapshot = {
            let mut map = self.transfers.lock().unwrap();
            map.get_mut(id).map(|t| {
                f(t);
                t.last_emit = Some(Instant::now());
                t.clone()
            })
        };
        if let Some(t) = snapshot {
            self.emit("transfer", json!(t));
        }
    }

    fn set_current_file(&self, id: &str, name: &str) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(id) {
            t.current_file = Some(name.to_string());
        }
    }

    fn add_progress(&self, id: &str, n: u64) {
        let snapshot = {
            let mut map = self.transfers.lock().unwrap();
            let Some(t) = map.get_mut(id) else { return };
            t.done += n;
            let now = Instant::now();
            match t.sample {
                None => t.sample = Some((now, t.done)),
                Some((at, bytes)) => {
                    let dt = now.duration_since(at);
                    if dt >= Duration::from_millis(300) {
                        let inst = (t.done - bytes) as f64 / dt.as_secs_f64();
                        t.speed = if t.speed == 0.0 { inst } else { t.speed * 0.6 + inst * 0.4 };
                        t.sample = Some((now, t.done));
                    }
                }
            }
            if t.last_emit.map_or(true, |l| now.duration_since(l) >= EMIT_INTERVAL) {
                t.last_emit = Some(now);
                Some(t.clone())
            } else {
                None
            }
        };
        if let Some(t) = snapshot {
            self.emit("transfer", json!(t));
        }
    }

    fn finish_transfer(&self, id: &str, status: Status, error: Option<String>) {
        self.cancels.lock().unwrap().remove(id);
        self.update_transfer(id, |t| {
            t.status = status;
            t.error = error;
            t.speed = 0.0;
            t.current_file = None;
            t.finished_at = Some(now_ms());
        });
    }

    pub fn cancel_transfer(&self, id: &str) {
        if let Some(tok) = self.cancels.lock().unwrap().get(id) {
            tok.cancel();
        }
    }

    pub fn clear_finished(&self) -> Vec<Transfer> {
        self.transfers.lock().unwrap().retain(|_, t| !t.is_finished());
        let list = self.transfers();
        self.emit("transfers", json!(list));
        list
    }

    pub fn respond(&self, request_id: &str, decision: Decision) -> bool {
        match self.pending.lock().unwrap().remove(request_id) {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    // ───────────────────────────── Sunucu (alıcı taraf) ─────────────────────────────

    /// TCP dinleyicisini başlatır. `preferred` doluysa rastgele bir port seçilir.
    pub async fn start_server(self: &Arc<Self>, preferred: u16) -> std::io::Result<u16> {
        let listener = match TcpListener::bind(("0.0.0.0", preferred)).await {
            Ok(l) => l,
            Err(_) => TcpListener::bind(("0.0.0.0", 0)).await?,
        };
        let port = listener.local_addr()?.port();
        self.port.store(port, Ordering::Relaxed);

        let node = self.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    r = listener.accept() => r,
                    _ = node.shutdown.cancelled() => break,
                };
                match accepted {
                    Ok((tcp, addr)) => {
                        let n = node.clone();
                        tokio::spawn(async move {
                            if let Err(e) = n.handle_connection(tcp, addr).await {
                                eprintln!("[fastfiles] {addr}: {e}");
                            }
                        });
                    }
                    Err(_) => sleep(Duration::from_millis(100)).await,
                }
            }
        });
        Ok(port)
    }

    async fn handle_connection(self: Arc<Self>, tcp: TcpStream, addr: SocketAddr) -> Result<(), String> {
        tcp.set_nodelay(true).ok();
        let tls = timeout(Duration::from_secs(10), self.acceptor.accept(tcp))
            .await
            .map_err(|_| "TLS zaman aşımı".to_string())?
            .map_err(|e| format!("TLS hatası: {e}"))?;
        let client_fp = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| fingerprint(c.as_ref()))
            .ok_or("istemci sertifikası yok")?;

        let (mut rd, mut wr) = tokio::io::split(tls);
        let msg = timeout(Duration::from_secs(15), read_msg(&mut rd))
            .await
            .map_err(|_| "ilk mesaj gelmedi".to_string())?
            .map_err(|e| e.to_string())?;

        match msg {
            Message::Hello { device } => {
                if device.fingerprint == client_fp {
                    self.upsert_peer(device, addr.ip(), None);
                }
                write_msg(&mut wr, &Message::Hello { device: self.device_info() })
                    .await
                    .map_err(|e| e.to_string())
            }
            Message::Offer { mut device, files, total, .. } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                if self.is_blocked(&client_fp) {
                    let _ = write_msg(&mut wr, &Message::Decline { reason: "declined".into() }).await;
                    return Ok(());
                }
                device.name = sanitize_name(&device.name);
                self.receive(rd, wr, addr.ip(), device, files, total).await
            }
            Message::UpdateRequest => self.serve_update(&mut wr).await,
            Message::ShareUpdate { device, share } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_share_update(&client_fp, share);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::ShareManifest { device, share_id, entries } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_share_manifest(&client_fp, &share_id, entries);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::ShareFetch { device, share_id, path } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.serve_share_file(&client_fp, &share_id, &path, &mut wr).await
            }
            Message::ShareUnlock { device, share_id, password } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_share_unlock(&client_fp, &share_id, &password);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::ShareLeave { device, share_id } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.accept_share_leave(&client_fp, &share_id);
                write_msg(&mut wr, &Message::GroupAck).await.map_err(|e| e.to_string())
            }
            Message::PollUpdate { device, poll, owner_name } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_poll_update(&client_fp, poll, &owner_name);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::PollVote { device, poll_id, option } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_poll_vote(&client_fp, &poll_id, option);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::AvatarRequest => {
                let reply = self.serve_avatar();
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::GroupUpdate { device, group } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_group_update(&client_fp, group);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::GroupChat { device, group_id, id, text, sent_at } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = match self.accept_group_chat(device, addr.ip(), &group_id, &id, &text, sent_at) {
                    Ok(()) => Message::ChatAck { id },
                    Err(reason) => Message::ChatReject { reason: reason.into() },
                };
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::GroupLeave { device, group_id } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.accept_group_leave(&client_fp, &group_id);
                write_msg(&mut wr, &Message::GroupAck).await.map_err(|e| e.to_string())
            }
            Message::Chat { device, id, text } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = match self.accept_chat(device, addr.ip(), &id, &text) {
                    Ok(()) => Message::ChatAck { id },
                    Err(reason) => Message::ChatReject { reason: reason.into() },
                };
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            _ => Err("beklenmeyen mesaj".into()),
        }
    }

    async fn receive(
        self: &Arc<Self>,
        mut rd: SrvRead,
        mut wr: SrvWrite,
        ip: IpAddr,
        device: DeviceInfo,
        files: Vec<FileEntry>,
        total: u64,
    ) -> Result<(), String> {
        self.upsert_peer(device.clone(), ip, None);

        let mut sanitized: Vec<(Vec<String>, u64)> = Vec::with_capacity(files.len());
        for f in &files {
            let parts = sanitize_rel_path(&f.path).ok_or_else(|| format!("geçersiz yol: {}", f.path))?;
            sanitized.push((parts, f.size));
        }
        if sanitized.is_empty() || sanitized.iter().map(|f| f.1).sum::<u64>() != total {
            let _ = write_msg(&mut wr, &Message::Decline { reason: "invalid".into() }).await;
            return Err("geçersiz teklif".into());
        }

        let mut top_names: Vec<String> = Vec::new();
        for (parts, _) in &sanitized {
            if !top_names.contains(&parts[0]) {
                top_names.push(parts[0].clone());
            }
        }
        let title = if top_names.len() == 1 {
            top_names[0].clone()
        } else {
            format!("{} öğe", top_names.len())
        };

        // ── Kullanıcı onayı ──
        let auto_accepted = self.is_trusted(&device.fingerprint);
        let decision = if auto_accepted {
            Decision { accept: true, trust: false }
        } else {
            let request_id = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = oneshot::channel();
            self.pending.lock().unwrap().insert(request_id.clone(), tx);
            self.emit(
                "incoming",
                json!({
                    "requestId": request_id,
                    "device": device,
                    "ip": ip.to_string(),
                    "title": title,
                    "fileCount": sanitized.len(),
                    "total": total,
                    "files": sanitized.iter().take(8).map(|(p, s)| json!({"path": p.join("/"), "size": s})).collect::<Vec<_>>(),
                }),
            );
            let mut probe = [0u8; 1];
            let result = tokio::select! {
                d = rx => d.ok(),
                // Gönderen vazgeçerse bağlantı kapanır ve okuma sonuçlanır.
                _ = rd.read(&mut probe) => None,
                _ = sleep(DECISION_TIMEOUT) => None,
            };
            self.pending.lock().unwrap().remove(&request_id);
            self.emit("incoming-closed", json!({ "requestId": request_id }));
            result.unwrap_or(Decision { accept: false, trust: false })
        };

        if !decision.accept {
            let _ = write_msg(&mut wr, &Message::Decline { reason: "declined".into() }).await;
            return Ok(());
        }
        if decision.trust {
            self.set_trusted(&device.fingerprint, &device.name, true);
        }

        // ── Hedef klasör ve benzersiz isimler ──
        let dest = PathBuf::from(self.settings().download_dir);
        if let Err(e) = tokio::fs::create_dir_all(&dest).await {
            let _ = write_msg(&mut wr, &Message::Decline { reason: "storage".into() }).await;
            return Err(format!("indirme klasörü oluşturulamadı: {e}"));
        }
        let mut tops: HashMap<String, String> = HashMap::new();
        let mut used: HashSet<String> = HashSet::new();
        for (parts, _) in &sanitized {
            if !tops.contains_key(&parts[0]) {
                let name = unique_name(&dest, &parts[0], parts.len() == 1, &used);
                used.insert(name.to_lowercase());
                tops.insert(parts[0].clone(), name);
            }
        }
        let save_path = if tops.len() == 1 {
            dest.join(tops.values().next().unwrap())
        } else {
            dest.clone()
        };

        write_msg(&mut wr, &Message::Accept).await.map_err(|e| e.to_string())?;

        let id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id.clone(), token.clone());
        let summary = format!("{title} · {}", fmt_bytes(total));
        let mut t = Transfer::new(
            id.clone(),
            Direction::Receive,
            device.id.clone(),
            device.name.clone(),
            title.clone(),
            sanitized.len(),
            total,
            Status::Active,
        );
        t.save_path = Some(save_path.to_string_lossy().into_owned());
        self.insert_transfer(t);
        // Güvenilir cihazdan sormadan kabul edildiyse kullanıcının haberi olsun.
        if auto_accepted {
            self.notify(&format!("{} dosya gönderiyor", device.name), &summary, Some(&id));
        }

        // ── Veri akışı ──
        // Ağdan okuma bu görevde, diske yazma paralel disk işçilerinde yapılır (bkz. receive_files).
        let plan: Vec<FilePlan> = sanitized
            .iter()
            .map(|(parts, size)| {
                let mut target = dest.join(&tops[&parts[0]]);
                for p in &parts[1..] {
                    target.push(p);
                }
                let mut part_name = target.file_name().unwrap_or_default().to_os_string();
                part_name.push(".ffpart");
                FilePlan { part: target.with_file_name(part_name), target, size: *size, display: parts.join("/") }
            })
            .collect();
        let result = receive_files(
            &mut rd,
            plan,
            &token,
            |n| self.add_progress(&id, n),
            |name| self.set_current_file(&id, name),
        )
        .await;

        match result {
            Ok(()) => {
                let _ = write_msg(&mut wr, &Message::Done).await;
                let _ = wr.shutdown().await;
                self.finish_transfer(&id, Status::Done, None);
                self.notify(&format!("Dosya alındı · {}", device.name), &summary, Some(&id));
            }
            Err(RecvError::Cancelled) => {
                let _ = write_msg(&mut wr, &Message::Cancel).await;
                self.finish_transfer(&id, Status::Cancelled, None);
            }
            Err(RecvError::Failed(e)) => {
                self.notify(&format!("Aktarım tamamlanamadı · {}", device.name), &format!("{title}: {e}"), Some(&id));
                self.finish_transfer(&id, Status::Failed, Some(e));
            }
        }
        Ok(())
    }

    // ───────────────────────────── Mesajlaşma ─────────────────────────────

    fn chat_rate_ok(&self, fp: &str) -> bool {
        let mut map = self.chat_rate.lock().unwrap();
        let q = map.entry(fp.to_string()).or_default();
        let now = Instant::now();
        while q.front().map_or(false, |t| now.duration_since(*t) > CHAT_RATE_WINDOW) {
            q.pop_front();
        }
        if q.len() >= CHAT_RATE_MAX {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Gelen mesajı doğrular ve kaydeder. Hata durumunda karşı tarafa gidecek ret kodu döner.
    fn accept_chat(&self, device: DeviceInfo, ip: IpAddr, id: &str, text: &str) -> Result<(), &'static str> {
        let fp = device.fingerprint.clone();
        if self.is_blocked(&fp) {
            return Err("blocked");
        }
        if !valid_message_id(id) {
            return Err("invalid");
        }
        if text.chars().count() > MAX_CHAT_CHARS {
            return Err("too_long");
        }
        if !self.chat_rate_ok(&fp) {
            return Err("rate");
        }
        let text = sanitize_text(text, MAX_CHAT_CHARS);
        if text.is_empty() {
            return Err("empty");
        }
        let name = sanitize_name(&device.name);
        self.upsert_peer(device, ip, None);
        // Aynı mesaj (ör. onay kaybolup tekrar gönderilirse) iki kez eklenmez.
        if self.chats.lock().unwrap().contains(&fp, id) {
            return Ok(());
        }
        let msg = ChatMessage { id: id.to_string(), outgoing: false, text, at: now_ms(), status: MsgStatus::Received, error: None, ..Default::default() };
        self.store_chat(&fp, &name, msg);
        Ok(())
    }

    fn store_chat(&self, fp: &str, name: &str, msg: ChatMessage) {
        let unread = self.chats.lock().unwrap().push(fp, name, msg.clone());
        self.emit(
            "chat",
            json!({ "fingerprint": fp, "name": name, "unread": unread, "incoming": !msg.outgoing, "message": msg }),
        );
    }

    fn set_chat_status(&self, fp: &str, id: &str, status: MsgStatus, error: Option<String>) {
        let (updated, meta) = {
            let mut chats = self.chats.lock().unwrap();
            (chats.update_status(fp, id, status, error), chats.conversation_meta(fp))
        };
        if let (Some(msg), Some((name, unread))) = (updated, meta) {
            self.emit("chat", json!({ "fingerprint": fp, "name": name, "unread": unread, "incoming": false, "message": msg }));
        }
    }

    pub fn chat_summaries(&self) -> Vec<ConversationSummary> {
        self.chats.lock().unwrap().summaries()
    }

    pub fn chat_history(&self, fp: &str) -> Vec<ChatMessage> {
        self.chats.lock().unwrap().history(fp)
    }

    pub fn chat_mark_read(&self, fp: &str) {
        self.chats.lock().unwrap().mark_read(fp);
    }

    /// Geçmişi siler. Gruplarda grup bilgisi korunur (yalnızca mesajlar silinir);
    /// ayrılınmış gruplar tamamen kaldırılır.
    pub fn chat_clear(&self, key: &str) {
        let mut chats = self.chats.lock().unwrap();
        let active_group = chats.conv(key).map_or(false, |c| c.group.is_some() && !c.left);
        if active_group {
            chats.with(key, |c| {
                c.messages.retain(|m| m.outgoing && !m.pending.is_empty());
                c.unread = 0;
            });
        } else {
            chats.clear(key);
        }
    }

    /// Mesajı kaydeder ve arka planda iletir. Durum değişiklikleri "chat" olayıyla bildirilir.
    pub fn chat_send(self: &Arc<Self>, peer_id: &str, text: &str) -> Result<ChatMessage, String> {
        let peer = self.peer(peer_id).ok_or("Bu cihaz çevrimdışı, mesaj gönderilemez")?;
        if self.is_blocked(&peer.fingerprint) {
            return Err("Bu cihazı engellediniz".into());
        }
        if text.chars().count() > MAX_CHAT_CHARS {
            return Err(format!("Mesaj çok uzun (en fazla {MAX_CHAT_CHARS} karakter)"));
        }
        let text = sanitize_text(text, MAX_CHAT_CHARS);
        if text.is_empty() {
            return Err("Mesaj boş".into());
        }
        let msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            outgoing: true,
            text,
            at: now_ms(),
            status: MsgStatus::Sending,
            error: None,
            ..Default::default()
        };
        self.store_chat(&peer.fingerprint, &peer.name, msg.clone());
        self.spawn_chat_delivery(peer, msg.clone());
        Ok(msg)
    }

    pub fn chat_retry(self: &Arc<Self>, fp: &str, id: &str) -> Result<(), String> {
        let msg = self.chats.lock().unwrap().get(fp, id).ok_or("Mesaj bulunamadı")?;
        if !msg.outgoing || msg.status != MsgStatus::Failed {
            return Ok(());
        }
        let peer = self
            .peers()
            .into_iter()
            .find(|p| p.fingerprint == fp)
            .ok_or("Bu cihaz çevrimdışı, mesaj gönderilemez")?;
        self.set_chat_status(fp, id, MsgStatus::Sending, None);
        self.spawn_chat_delivery(peer, msg);
        Ok(())
    }

    fn spawn_chat_delivery(self: &Arc<Self>, peer: Peer, msg: ChatMessage) {
        let node = self.clone();
        tokio::spawn(async move {
            let (status, error) = match node.deliver_chat(&peer, &msg).await {
                Ok(()) => (MsgStatus::Sent, None),
                Err(e) => (MsgStatus::Failed, Some(e)),
            };
            node.set_chat_status(&peer.fingerprint, &msg.id, status, error);
        });
    }

    async fn deliver_chat(&self, peer: &Peer, msg: &ChatMessage) -> Result<(), String> {
        let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut stream, &Message::Chat { device: self.device_info(), id: msg.id.clone(), text: msg.text.clone() })
            .await
            .map_err(|_| "Bağlantı koptu".to_string())?;
        let reply = timeout(Duration::from_secs(8), read_msg(&mut stream))
            .await
            .map_err(|_| "Karşı taraf yanıt vermedi".to_string())?
            .map_err(|_| "Karşı taraf yanıt vermedi".to_string())?;
        let _ = stream.shutdown().await;
        match reply {
            Message::ChatAck { id } if id == msg.id => Ok(()),
            Message::ChatReject { reason } => Err(match reason.as_str() {
                "blocked" => "Karşı taraf mesajları kabul etmiyor".into(),
                "rate" => "Çok hızlı mesaj gönderiyorsunuz, biraz bekleyin".into(),
                "too_long" => "Mesaj çok uzun".into(),
                _ => "Mesaj reddedildi".into(),
            }),
            _ => Err("Beklenmeyen yanıt".into()),
        }
    }

    // ───────────────────────────── İstemci (gönderen taraf) ─────────────────────────────

    /// Seçilen dosya/klasörleri bir veya daha fazla cihaza gönderir; aktarım kimliklerini döndürür.
    /// Dosya listesi bir kez taranır, her alıcı için ayrı bir aktarım (ve ayrı onay) başlar.
    pub async fn send_paths(self: &Arc<Self>, peer_ids: &[String], paths: Vec<String>) -> Result<Vec<String>, String> {
        let peers: Vec<Peer> = peer_ids.iter().filter_map(|id| self.peer(id)).collect();
        if peers.is_empty() {
            return Err("Seçilen cihazlar artık çevrimiçi değil".into());
        }
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let title = match paths.as_slice() {
            [one] => display_name(one),
            many => format!("{} öğe", many.len()),
        };
        let entries = tokio::task::spawn_blocking(move || collect_entries(&paths))
            .await
            .map_err(|e| e.to_string())??;
        if entries.is_empty() {
            return Err("Gönderilecek dosya bulunamadı (klasör boş olabilir)".into());
        }
        let entries = Arc::new(entries);
        Ok(peers.into_iter().map(|peer| self.start_send(peer, entries.clone(), title.clone())).collect())
    }

    fn start_send(self: &Arc<Self>, peer: Peer, entries: Arc<Vec<(PathBuf, FileEntry)>>, title: String) -> String {
        let total: u64 = entries.iter().map(|e| e.1.size).sum();
        let id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id.clone(), token.clone());
        self.insert_transfer(Transfer::new(
            id.clone(),
            Direction::Send,
            peer.id.clone(),
            peer.name.clone(),
            title,
            entries.len(),
            total,
            Status::Waiting,
        ));

        let node = self.clone();
        let tid = id.clone();
        tokio::spawn(async move {
            match node.run_send(&tid, &peer, entries, &token).await {
                Ok(SendOutcome::Done) => node.finish_transfer(&tid, Status::Done, None),
                Ok(SendOutcome::Declined(reason)) => {
                    let msg = match reason.as_str() {
                        "declined" => None,
                        "storage" => Some("Alıcının indirme klasörüne yazılamıyor".to_string()),
                        "invalid" => Some("Alıcı teklifi geçersiz buldu".to_string()),
                        other => Some(other.to_string()),
                    };
                    node.finish_transfer(&tid, Status::Declined, msg)
                }
                Ok(SendOutcome::Cancelled) => node.finish_transfer(&tid, Status::Cancelled, None),
                Ok(SendOutcome::PeerCancelled) => node.finish_transfer(
                    &tid,
                    Status::Cancelled,
                    Some("Alıcı iptal etti".into()),
                ),
                Err(e) => node.finish_transfer(&tid, Status::Failed, Some(e)),
            }
        });
        id
    }

    /// Adresleri sırayla dener; ilk TCP bağlantısı kurulan adres üzerinden TLS el sıkışması yapar.
    async fn connect(&self, addrs: &[SocketAddr], expected_fp: Option<String>) -> Result<tokio_rustls::client::TlsStream<TcpStream>, String> {
        let per_try = if addrs.len() > 1 { Duration::from_secs(3) } else { Duration::from_secs(6) };
        let mut last_err = "Adres yok".to_string();
        let mut tcp = None;
        for addr in addrs {
            match timeout(per_try, TcpStream::connect(addr)).await {
                Ok(Ok(s)) => {
                    tcp = Some(s);
                    break;
                }
                Ok(Err(e)) => last_err = format!("Bağlanılamadı ({addr}): {e}"),
                Err(_) => last_err = format!("Bağlantı zaman aşımına uğradı ({addr})"),
            }
        }
        let tcp = tcp.ok_or(last_err)?;
        tcp.set_nodelay(true).ok();
        let connector = TlsConnector::from(tls::client_config(&self.identity, expected_fp)?);
        timeout(Duration::from_secs(10), connector.connect(tls::server_name(), tcp))
            .await
            .map_err(|_| "Güvenli bağlantı kurulamadı (zaman aşımı)".to_string())?
            .map_err(|e| format!("Güvenli bağlantı kurulamadı: {e}"))
    }

    async fn run_send(
        self: &Arc<Self>,
        id: &str,
        peer: &Peer,
        entries: Arc<Vec<(PathBuf, FileEntry)>>,
        token: &CancellationToken,
    ) -> Result<SendOutcome, String> {
        let addrs = self.candidates(peer);
        let stream = tokio::select! {
            r = self.connect(&addrs, Some(peer.fingerprint.clone())) => r?,
            _ = token.cancelled() => return Ok(SendOutcome::Cancelled),
        };
        let (mut rd, mut wr) = tokio::io::split(stream);

        let files: Vec<FileEntry> = entries.iter().map(|e| e.1.clone()).collect();
        let total = files.iter().map(|f| f.size).sum();
        write_msg(
            &mut wr,
            &Message::Offer { transfer_id: id.to_string(), device: self.device_info(), files, total },
        )
        .await
        .map_err(|e| format!("Teklif gönderilemedi: {e}"))?;

        let reply = tokio::select! {
            r = read_msg(&mut rd) => r.map_err(|_| "Alıcı bağlantıyı kapattı".to_string())?,
            _ = token.cancelled() => return Ok(SendOutcome::Cancelled),
            _ = sleep(DECISION_TIMEOUT + Duration::from_secs(10)) => return Err("Alıcı yanıt vermedi".into()),
        };
        match reply {
            Message::Accept => {}
            Message::Decline { reason } => return Ok(SendOutcome::Declined(reason)),
            _ => return Err("Beklenmeyen yanıt".into()),
        }

        self.update_transfer(id, |t| {
            t.status = Status::Active;
            t.sample = None;
        });

        // Diskten okuma paralel iş parçacıklarında önden yapılır; ağ yazımı diski hiç beklemez.
        let mut readers = spawn_readers(&entries);
        let writer = async {
            let n = readers.len();
            for (idx, (_, entry)) in entries.iter().enumerate() {
                self.set_current_file(id, &entry.path);
                let rx = &mut readers[idx % n];
                let mut remaining = entry.size;
                while remaining > 0 {
                    let (got, chunk) = rx.recv().await.ok_or_else(|| "Dosya okuma durdu".to_string())??;
                    debug_assert_eq!(got, idx);
                    wr.write_all(&chunk).await.map_err(|_| "Bağlantı koptu".to_string())?;
                    remaining -= chunk.len() as u64;
                    self.add_progress(id, chunk.len() as u64);
                }
            }
            wr.flush().await.map_err(|_| "Bağlantı koptu".to_string())?;
            Ok::<(), String>(())
        };
        let reader = read_msg(&mut rd);
        tokio::pin!(writer);
        tokio::pin!(reader);

        tokio::select! {
            w = &mut writer => {
                if let Err(e) = w {
                    // Yazma hatası alıcının iptalinden kaynaklanıyor olabilir.
                    if let Ok(Ok(Message::Cancel)) = timeout(Duration::from_secs(2), &mut reader).await {
                        return Ok(SendOutcome::PeerCancelled);
                    }
                    return Err(e);
                }
                tokio::select! {
                    m = &mut reader => match m {
                        Ok(Message::Done) => Ok(SendOutcome::Done),
                        Ok(Message::Cancel) => Ok(SendOutcome::PeerCancelled),
                        _ => Err("Alıcı onaylamadan bağlantı kapandı".into()),
                    },
                    _ = token.cancelled() => Ok(SendOutcome::Cancelled),
                    _ = sleep(Duration::from_secs(120)) => Err("Alıcıdan onay gelmedi".into()),
                }
            }
            m = &mut reader => match m {
                Ok(Message::Done) => Ok(SendOutcome::Done),
                Ok(Message::Cancel) => Ok(SendOutcome::PeerCancelled),
                _ => Err("Bağlantı koptu".into()),
            },
            _ = token.cancelled() => Ok(SendOutcome::Cancelled),
        }
    }

    /// Bir adrese bağlanıp cihaz bilgisini alır (elle ekleme ve canlılık kontrolü için).
    pub async fn ping(self: &Arc<Self>, addr: SocketAddr) -> Result<DeviceInfo, String> {
        let mut stream = self.connect(&[addr], None).await?;
        let server_fp = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| fingerprint(c.as_ref()))
            .ok_or("Sertifika alınamadı")?;
        write_msg(&mut stream, &Message::Hello { device: self.device_info() })
            .await
            .map_err(|e| e.to_string())?;
        let reply = timeout(Duration::from_secs(5), read_msg(&mut stream))
            .await
            .map_err(|_| "Yanıt gelmedi".to_string())?
            .map_err(|e| e.to_string())?;
        let _ = stream.shutdown().await;
        match reply {
            Message::Hello { device } if device.fingerprint == server_fp => {
                if device.id == self.identity.id {
                    Err("Bu adres bu bilgisayarın kendisi".into())
                } else {
                    Ok(device)
                }
            }
            _ => Err("Bu adreste FastFiles çalışmıyor".into()),
        }
    }

    pub async fn add_manual_peer(self: &Arc<Self>, address: &str) -> Result<Peer, String> {
        let address = address.trim().to_string();
        let addr = resolve(&address).await?;
        let device = self.ping(addr).await?;
        let id = device.id.clone();
        self.upsert_peer(device, addr.ip(), Some(address.clone()));
        self.update_settings(|s| {
            if !s.manual_peers.contains(&address) {
                s.manual_peers.push(address.clone());
            }
        });
        self.peers().into_iter().find(|p| p.id == id).ok_or("Cihaz eklenemedi".into())
    }

    pub fn forget_manual_peer(&self, address: &str) {
        self.update_settings(|s| s.manual_peers.retain(|a| a != address));
        let id = self
            .peers
            .lock()
            .unwrap()
            .values()
            .find(|p| p.manual_addr.as_deref() == Some(address))
            .map(|p| p.id.clone());
        if let Some(id) = id {
            self.remove_peer(&id);
        }
    }

    /// Elle eklenen cihazları periyodik olarak yoklar.
    pub async fn run_manual_pinger(self: Arc<Self>) {
        loop {
            for address in self.settings().manual_peers {
                if let Ok(addr) = resolve(&address).await {
                    if let Ok(device) = self.ping(addr).await {
                        self.upsert_peer(device, addr.ip(), Some(address.clone()));
                    }
                }
            }
            tokio::select! {
                _ = sleep(Duration::from_secs(4)) => {}
                _ = self.shutdown.cancelled() => break,
            }
        }
    }
}

async fn resolve(address: &str) -> Result<SocketAddr, String> {
    if let Ok(a) = address.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = address.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let with_port = if address.contains(':') { address.to_string() } else { format!("{address}:{DEFAULT_PORT}") };
    tokio::net::lookup_host(with_port)
        .await
        .map_err(|_| format!("Adres çözümlenemedi: {address}"))?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| format!("Adres çözümlenemedi: {address}"))
}

// ───────────────────────────── Hızlı aktarım boru hattı ─────────────────────────────

/// Gönderende her okuyucunun önden okuduğu parça sayısı (okuyucu başına en fazla ~8 MB bellek).
const READ_AHEAD_CHUNKS: usize = 8;
/// Gönderende paralel disk okuyucu sayısı (küçük dosyalarda açma/kapama gecikmesini örtüştürür).
const DISK_READERS: usize = 4;

type ChunkRx = tokio::sync::mpsc::Receiver<Result<(usize, Vec<u8>), String>>;

/// Gönderen: dosyaları `DISK_READERS` iş parçacığına dağıtıp önden okutur.
/// i. dosyayı (i % N). okuyucu okur; tüketici dosya sırasıyla ilgili kanaldan alır, sıra korunur.
fn spawn_readers(entries: &Arc<Vec<(PathBuf, FileEntry)>>) -> Vec<ChunkRx> {
    let n = DISK_READERS.min(entries.len().max(1));
    (0..n)
        .map(|k| {
            let (tx, rx) = tokio::sync::mpsc::channel(READ_AHEAD_CHUNKS);
            let mine: Vec<(usize, PathBuf, FileEntry)> = entries
                .iter()
                .enumerate()
                .filter(|(i, _)| i % n == k)
                .map(|(i, (p, e))| (i, p.clone(), e.clone()))
                .collect();
            tokio::task::spawn_blocking(move || read_files_ahead(&mine, tx));
            rx
        })
        .collect()
}
/// Alıcıda paralel disk işçisi sayısı. Küçük dosyalarda dosya açma/kapama/yeniden adlandırma
/// (ve Windows Defender taraması) gecikmesini örtüştürür.
const DISK_WORKERS: usize = 4;

/// Gönderen: dosyaları sırayla okuyup parçalar halinde kanala koyar (engelleyen iş parçacığında).
fn read_files_ahead(
    entries: &[(usize, PathBuf, FileEntry)],
    tx: tokio::sync::mpsc::Sender<Result<(usize, Vec<u8>), String>>,
) {
    use std::io::Read;
    for (idx, src, entry) in entries.iter() {
        let idx = *idx;
        let mut f = match std::fs::File::open(src) {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.blocking_send(Err(format!("{} açılamadı: {e}", entry.path)));
                return;
            }
        };
        let mut remaining = entry.size;
        while remaining > 0 {
            let want = remaining.min(CHUNK as u64) as usize;
            let mut buf = vec![0u8; want];
            let mut filled = 0;
            while filled < want {
                match f.read(&mut buf[filled..]) {
                    Ok(0) => {
                        let _ = tx.blocking_send(Err(format!("{} gönderim sırasında değişti", entry.path)));
                        return;
                    }
                    Ok(n) => filled += n,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(format!("{} okunamadı: {e}", entry.path)));
                        return;
                    }
                }
            }
            remaining -= want as u64;
            if tx.blocking_send(Ok((idx, buf))).is_err() {
                return; // gönderim iptal edildi
            }
        }
    }
}

#[derive(Clone)]
struct FilePlan {
    part: PathBuf,
    target: PathBuf,
    size: u64,
    display: String,
}

enum DiskCmd {
    Start(usize),
    Data(usize, Vec<u8>),
    End(usize),
}

/// Paylaşılan hata durumu: ilk disk hatası kaydedilir, ağ tarafı bunu görünce durur.
#[derive(Default)]
struct DiskFailure {
    failed: AtomicBool,
    message: Mutex<Option<String>>,
}

impl DiskFailure {
    fn set(&self, msg: String) {
        let mut m = self.message.lock().unwrap();
        if m.is_none() {
            *m = Some(msg);
        }
        self.failed.store(true, Ordering::Relaxed);
    }
    fn get(&self) -> Option<String> {
        if self.failed.load(Ordering::Relaxed) {
            self.message.lock().unwrap().clone()
        } else {
            None
        }
    }
}

/// Alıcı disk işçisi: kendisine atanan dosyaları .ffpart olarak yazar, bitince yeniden adlandırır.
/// Kanal kapanınca yarım kalan dosyaları siler.
fn disk_worker(mut rx: tokio::sync::mpsc::Receiver<DiskCmd>, plan: Arc<Vec<FilePlan>>, failure: Arc<DiskFailure>) {
    use std::io::Write;
    let mut open: HashMap<usize, std::io::BufWriter<std::fs::File>> = HashMap::new();
    let mut made_dirs: HashSet<PathBuf> = HashSet::new();
    while let Some(cmd) = rx.blocking_recv() {
        if failure.failed.load(Ordering::Relaxed) {
            continue; // hata sonrası kanalı boşalt
        }
        let r: Result<(), String> = (|| match cmd {
            DiskCmd::Start(i) => {
                let p = &plan[i];
                if let Some(parent) = p.target.parent() {
                    if !made_dirs.contains(parent) {
                        std::fs::create_dir_all(parent).map_err(|e| format!("Klasör oluşturulamadı: {e}"))?;
                        made_dirs.insert(parent.to_path_buf());
                    }
                }
                let f = std::fs::File::create(&p.part)
                    .map_err(|e| format!("Dosya oluşturulamadı ({}): {e}", p.target.display()))?;
                open.insert(i, std::io::BufWriter::with_capacity(512 * 1024, f));
                Ok(())
            }
            DiskCmd::Data(i, bytes) => match open.get_mut(&i) {
                Some(w) => w.write_all(&bytes).map_err(|e| format!("Diske yazılamadı: {e}")),
                None => Ok(()),
            },
            DiskCmd::End(i) => {
                if let Some(w) = open.remove(&i) {
                    // Windows'ta açık dosya yeniden adlandırılamaz: önce kapat.
                    let f = w.into_inner().map_err(|e| format!("Diske yazılamadı: {e}"))?;
                    drop(f);
                    let p = &plan[i];
                    std::fs::rename(&p.part, &p.target).map_err(|e| format!("Dosya kaydedilemedi: {e}"))?;
                }
                Ok(())
            }
        })();
        if let Err(e) = r {
            failure.set(e);
        }
    }
    for (i, w) in open.drain() {
        drop(w);
        for _ in 0..10 {
            match std::fs::remove_file(&plan[i].part) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => std::thread::sleep(Duration::from_millis(100)),
                _ => break,
            }
        }
    }
}

/// Alıcı: dosyaların ham baytlarını okur ve paralel disk işçilerine dağıtır.
/// Bir dosyanın tüm komutları aynı işçiye gider (sıra korunur); farklı dosyalar paralel yazılır.
async fn receive_files<R: tokio::io::AsyncRead + Unpin>(
    rd: &mut R,
    plan: Vec<FilePlan>,
    token: &CancellationToken,
    mut on_bytes: impl FnMut(u64),
    mut on_file: impl FnMut(&str),
) -> Result<(), RecvError> {
    let plan = Arc::new(plan);
    let failure = Arc::new(DiskFailure::default());
    let mut txs = Vec::with_capacity(DISK_WORKERS);
    let mut workers = Vec::with_capacity(DISK_WORKERS);
    for _ in 0..DISK_WORKERS {
        let (tx, rx) = tokio::sync::mpsc::channel::<DiskCmd>(64);
        let (p, f) = (plan.clone(), failure.clone());
        workers.push(tokio::task::spawn_blocking(move || disk_worker(rx, p, f)));
        txs.push(tx);
    }
    let lost = || RecvError::Failed("Gönderen iptal etti veya bağlantı koptu".into());
    let disk_err = |f: &DiskFailure| RecvError::Failed(f.get().unwrap_or_else(|| "Disk hatası".into()));

    let result: Result<(), RecvError> = async {
        let mut buf = vec![0u8; 256 * 1024];
        for (i, p) in plan.iter().enumerate() {
            let worker = &txs[i % DISK_WORKERS];
            on_file(&p.display);
            worker.send(DiskCmd::Start(i)).await.map_err(|_| disk_err(&failure))?;
            let mut remaining = p.size;
            while remaining > 0 {
                let want = remaining.min(buf.len() as u64) as usize;
                let n = tokio::select! {
                    r = rd.read(&mut buf[..want]) => r.map_err(|_| lost())?,
                    _ = token.cancelled() => return Err(RecvError::Cancelled),
                };
                if n == 0 {
                    return Err(lost());
                }
                if failure.failed.load(Ordering::Relaxed) {
                    return Err(disk_err(&failure));
                }
                worker.send(DiskCmd::Data(i, buf[..n].to_vec())).await.map_err(|_| disk_err(&failure))?;
                remaining -= n as u64;
                on_bytes(n as u64);
            }
            worker.send(DiskCmd::End(i)).await.map_err(|_| disk_err(&failure))?;
        }
        Ok(())
    }
    .await;

    // İşçilerin işini bitirmesini bekle (başarıda son dosyalar diske yazılır, hatada yarımlar silinir).
    drop(txs);
    for w in workers {
        let _ = w.await;
    }
    match result {
        Ok(()) => match failure.get() {
            Some(e) => Err(RecvError::Failed(e)),
            None => Ok(()),
        },
        Err(e) => Err(e),
    }
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i > 0 && v < 10.0 {
        format!("{} {}", format!("{v:.1}").replace('.', ","), UNITS[i])
    } else {
        format!("{v:.0} {}", UNITS[i])
    }
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().trim_end_matches(['\\', '/', ':']).to_string())
}

/// Seçilen yolları (dosya veya klasör) gönderilecek dosya listesine çevirir.
fn collect_entries(paths: &[PathBuf]) -> Result<Vec<(PathBuf, FileEntry)>, String> {
    let mut out = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p).map_err(|e| format!("{} okunamadı: {e}", p.display()))?;
        let base = display_name(p);
        if meta.is_file() {
            out.push((p.clone(), FileEntry { path: base, size: meta.len() }));
        } else if meta.is_dir() {
            for entry in walkdir::WalkDir::new(p).follow_links(false).sort_by_file_name() {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }
                let Ok(rel) = entry.path().strip_prefix(p) else { continue };
                let Ok(m) = entry.metadata() else { continue };
                let mut rel_str = base.clone();
                for c in rel.components() {
                    rel_str.push('/');
                    rel_str.push_str(&c.as_os_str().to_string_lossy());
                }
                out.push((entry.path().to_path_buf(), FileEntry { path: rel_str, size: m.len() }));
            }
        }
    }
    Ok(out)
}

fn unique_name(dir: &Path, name: &str, is_file: bool, used: &HashSet<String>) -> String {
    let taken = |n: &str| dir.join(n).exists() || used.contains(&n.to_lowercase());
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if is_file && i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|i| format!("{stem} ({i}){ext}"))
        .find(|c| !taken(c))
        .unwrap()
}


// ───────────────────────────── Testler ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn quiet() -> Emitter {
        Arc::new(|_, _| {})
    }

    async fn make_node(root: &Path, name: &str) -> Arc<Node> {
        let cfg = root.join(name).join("config");
        let dl = root.join(name).join("downloads");
        let node = Node::new(&cfg, &dl, quiet()).unwrap();
        node.update_settings(|s| s.device_name = name.to_string());
        node.start_server(0).await.unwrap();
        node
    }

    async fn wait_finished(node: &Node, id: &str) -> Transfer {
        for _ in 0..400 {
            if let Some(t) = node.transfers().into_iter().find(|t| t.id == id) {
                if t.is_finished() {
                    return t;
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
        panic!("aktarım zamanında bitmedi");
    }

    fn introduce(a: &Node, b: &Node) {
        a.upsert_peer(b.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), None);
    }

    #[tokio::test]
    async fn folder_transfer_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        // Bob, Alice'e güveniyor → otomatik kabul.
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);

        // Gönderilecek klasör: iç içe dosyalar + 5 MB rastgele veri + boş dosya.
        let src = tmp.path().join("Proje");
        std::fs::create_dir_all(src.join("assets/img")).unwrap();
        std::fs::write(src.join("readme.txt"), "merhaba dünya").unwrap();
        std::fs::write(src.join("assets/empty.bin"), b"").unwrap();
        let big: Vec<u8> = (0..5 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        std::fs::write(src.join("assets/img/big.bin"), &big).unwrap();
        let single = tmp.path().join("not.md");
        std::fs::write(&single, "# not").unwrap();

        let id = alice
            .send_one(&bob.identity.id, vec![src.to_string_lossy().into(), single.to_string_lossy().into()])
            .await
            .unwrap();
        let t = wait_finished(&alice, &id).await;
        assert_eq!(t.status, Status::Done, "hata: {:?}", t.error);
        assert_eq!(t.done, t.total);

        let dl = PathBuf::from(bob.settings().download_dir);
        assert_eq!(std::fs::read_to_string(dl.join("Proje/readme.txt")).unwrap(), "merhaba dünya");
        assert_eq!(std::fs::read(dl.join("Proje/assets/img/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(dl.join("Proje/assets/empty.bin")).unwrap().len(), 0);
        assert_eq!(std::fs::read_to_string(dl.join("not.md")).unwrap(), "# not");

        // Aynı şeyi tekrar gönderince üzerine yazmamalı, "(1)" eklemeli.
        let id2 = alice.send_one(&bob.identity.id, vec![single.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id2).await.status, Status::Done);
        assert_eq!(std::fs::read_to_string(dl.join("not (1).md")).unwrap(), "# not");

        // Alıcı tarafında da aktarımlar "done" görünmeli.
        let recv: Vec<_> = bob.transfers().into_iter().filter(|t| t.direction == Direction::Receive).collect();
        assert_eq!(recv.len(), 2);
        assert!(recv.iter().all(|t| t.status == Status::Done));
        // Yarım kalmış .ffpart dosyası kalmamalı.
        assert!(walkdir::WalkDir::new(&dl).into_iter().flatten().all(|e| !e.path().to_string_lossy().ends_with(".ffpart")));
    }

    #[tokio::test]
    async fn decline_and_accept_with_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let r2 = requests.clone();
        let emitter: Emitter = Arc::new(move |ev, payload| {
            if ev == "incoming" {
                r2.lock().unwrap().push(payload["requestId"].as_str().unwrap().to_string());
            }
        });
        let alice = make_node(tmp.path(), "alice").await;
        let bob = Node::new(&tmp.path().join("bob/config"), &tmp.path().join("bob/dl"), emitter).unwrap();
        bob.start_server(0).await.unwrap();
        introduce(&alice, &bob);

        let file = tmp.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();

        // 1) Reddet
        let id = alice.send_one(&bob.identity.id, vec![file.to_string_lossy().into()]).await.unwrap();
        let req = loop {
            if let Some(r) = requests.lock().unwrap().pop() {
                break r;
            }
            sleep(Duration::from_millis(20)).await;
        };
        assert!(bob.respond(&req, Decision { accept: false, trust: false }));
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Declined);

        // 2) Kabul et + güven → sonraki gönderim sormadan gelmeli
        let id = alice.send_one(&bob.identity.id, vec![file.to_string_lossy().into()]).await.unwrap();
        let req = loop {
            if let Some(r) = requests.lock().unwrap().pop() {
                break r;
            }
            sleep(Duration::from_millis(20)).await;
        };
        assert!(bob.respond(&req, Decision { accept: true, trust: true }));
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Done);
        assert!(bob.is_trusted(&alice.identity.fingerprint));

        let id = alice.send_one(&bob.identity.id, vec![file.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Done);
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sender_cancel_cleans_up_receiver() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);

        let big = tmp.path().join("big.bin");
        std::fs::write(&big, vec![7u8; 300 * 1024 * 1024]).unwrap();
        let id = alice.send_one(&bob.identity.id, vec![big.to_string_lossy().into()]).await.unwrap();
        // Biraz veri aksın, sonra iptal et.
        for _ in 0..200 {
            if alice.transfers().iter().any(|t| t.id == id && t.done > 0) {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        alice.cancel_transfer(&id);
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Cancelled);

        // Alıcı tarafı başarısız olarak bitirmeli ve yarım dosyayı silmeli.
        let rid = loop {
            if let Some(t) = bob.transfers().into_iter().next() {
                break t.id;
            }
            sleep(Duration::from_millis(20)).await;
        };
        let rt = wait_finished(&bob, &rid).await;
        assert_ne!(rt.status, Status::Done);
        sleep(Duration::from_millis(300)).await;
        let dl = PathBuf::from(bob.settings().download_dir);
        let leftovers: Vec<_> = std::fs::read_dir(&dl).unwrap().flatten().collect();
        assert!(leftovers.is_empty(), "yarım dosya kaldı: {:?}", leftovers.iter().map(|e| e.path()).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn peer_address_prefers_local_subnet_and_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        // 192.168.1.0/24 yerel ağ; 172.16.0.2 ise /32 VPN arayüzü gibi davranır.
        alice.set_local_subnets(vec![(u32::from(Ipv4Addr::new(192, 168, 1, 0)), u32::from(Ipv4Addr::new(255, 255, 255, 0)))]);
        let dev = DeviceInfo { id: "x".into(), name: "x".into(), port: 1, fingerprint: "f".into(), os: "windows".into(), ..Default::default() };
        let vpn = IpAddr::V4(Ipv4Addr::new(172, 16, 0, 2));
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50));
        alice.upsert_peer(dev.clone(), vpn, None);
        assert_eq!(alice.peer("x").unwrap().ip, "172.16.0.2");
        for _ in 0..3 {
            alice.upsert_peer(dev.clone(), lan, None);
            assert_eq!(alice.peer("x").unwrap().ip, "192.168.1.50");
            alice.upsert_peer(dev.clone(), vpn, None);
            assert_eq!(alice.peer("x").unwrap().ip, "192.168.1.50");
        }
        let c = alice.candidates(&alice.peer("x").unwrap());
        assert_eq!(c[0].ip(), lan);
        assert_eq!(c[1].ip(), vpn);
    }

    impl Node {
        async fn send_one(self: &Arc<Self>, to: &str, paths: Vec<String>) -> Result<String, String> {
            Ok(self.send_paths(&[to.to_string()], paths).await?.remove(0))
        }
    }

    async fn wait_until(mut f: impl FnMut() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            sleep(Duration::from_millis(25)).await;
        }
        panic!("koşul zamanında gerçekleşmedi");
    }

    impl Node {
        fn set_update_key(&self, k: Vec<u8>) {
            *self.update_key.lock().unwrap() = k;
        }
    }

    #[tokio::test]
    async fn update_spreads_over_network_and_rejects_forgeries() {
        let tmp = tempfile::tempdir().unwrap();
        let (sk, pk) = update::generate_keypair().unwrap();
        let (evil_sk, _) = update::generate_keypair().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        for n in [&alice, &bob, &carol] {
            n.set_update_key(pk.clone());
        }
        let installer = tmp.path().join("setup.exe");
        let bytes: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&installer, &bytes).unwrap();

        // Başka bir anahtarla imzalanmış (sahte) paket kabul edilmez.
        let evil = tmp.path().join("evil.ffupdate");
        update::build_package(&installer, "99.0.0", &evil_sk, &evil).unwrap();
        assert!(alice.import_update(&evil).is_err());
        assert_eq!(alice.pending_update(), None);

        // Kullanılandan eski paket kabul edilmez (sürüm düşürme saldırısı).
        let old = tmp.path().join("old.ffupdate");
        update::build_package(&installer, "0.0.1", &sk, &old).unwrap();
        assert!(alice.import_update(&old).is_err());

        // Gerçek paket: Alice yükler ve duyurmaya başlar.
        let pkg = tmp.path().join("real.ffupdate");
        update::build_package(&installer, "99.0.0", &sk, &pkg).unwrap();
        alice.import_update(&pkg).unwrap();
        assert_eq!(alice.pending_update().as_deref(), Some("99.0.0"));
        assert_eq!(alice.device_info().pkg, "99.0.0");

        // Bob, Alice'in duyurusunu görünce paketi ağdan indirir.
        bob.upsert_peer(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        bob.check_for_updates();
        wait_until(|| bob.pending_update().as_deref() == Some("99.0.0")).await;
        assert_eq!(bob.device_info().pkg, "99.0.0", "indiren cihaz da dağıtmaya başlamalı");

        // Carol paketi Alice'ten değil Bob'dan alır: güncelleme ağda yayılır.
        carol.upsert_peer(bob.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        carol.check_for_updates();
        wait_until(|| carol.pending_update().as_deref() == Some("99.0.0")).await;

        // Kurulum dosyası birebir aynı çıkar.
        let (setup, version) = carol.prepare_install().unwrap();
        assert_eq!(version, "99.0.0");
        assert_eq!(std::fs::read(&setup).unwrap(), bytes);
        let _ = std::fs::remove_file(setup);
    }

    #[tokio::test]
    async fn update_inbox_imports_signed_and_drops_forged() {
        let tmp = tempfile::tempdir().unwrap();
        let (sk, pk) = update::generate_keypair().unwrap();
        let (evil, _) = update::generate_keypair().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        alice.set_update_key(pk);
        let inbox = alice.update_dir.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let installer = tmp.path().join("setup.exe");
        std::fs::write(&installer, vec![3u8; 50_000]).unwrap();

        // Sahte imzalı paket içe alınmaz ve gelen kutusundan silinir.
        update::build_package(&installer, "99.0.0", &evil, &inbox.join("sahte.ffupdate")).unwrap();
        alice.check_update_inbox();
        assert_eq!(alice.pending_update(), None);
        assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), 0);

        // Gerçek paket içe alınır ve dağıtılmaya başlanır.
        update::build_package(&installer, "99.0.0", &sk, &inbox.join("FastFiles-99.0.0.ffupdate")).unwrap();
        alice.check_update_inbox();
        assert_eq!(alice.pending_update().as_deref(), Some("99.0.0"));
        assert_eq!(alice.device_info().pkg, "99.0.0");
        assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn corrupted_package_is_never_served() {
        let tmp = tempfile::tempdir().unwrap();
        let (sk, pk) = update::generate_keypair().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        alice.set_update_key(pk.clone());
        bob.set_update_key(pk);
        let installer = tmp.path().join("setup.exe");
        std::fs::write(&installer, vec![1u8; 100_000]).unwrap();
        let pkg = tmp.path().join("p.ffupdate");
        update::build_package(&installer, "99.0.0", &sk, &pkg).unwrap();
        alice.import_update(&pkg).unwrap();

        // Alice'in diskindeki paket sonradan bozulursa (veya kurcalanırsa) dağıtılmaz.
        let cached = alice.package_path();
        let mut data = std::fs::read(&cached).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0xFF;
        std::fs::write(&cached, data).unwrap();

        bob.upsert_peer(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        bob.check_for_updates();
        sleep(Duration::from_millis(800)).await;
        assert_eq!(bob.pending_update(), None);
        assert!(!bob.package_path().exists());
    }

    #[tokio::test]
    async fn busy_node_does_not_install() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        // Bob güvenmiyor → istek bekliyor → Bob "meşgul".
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![file.to_string_lossy().into()]).await.unwrap();
        wait_until(|| bob.is_busy()).await;
        assert!(alice.is_busy());
        alice.cancel_transfer(&id);
        wait_finished(&alice, &id).await;
    }

    /// Hız ölçümü (normal test çalıştırmasında atlanır):
    ///   cargo test --release --lib bench -- --ignored --nocapture
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn bench_transfer_speed() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);

        // 1) Tek büyük dosya (1 GiB)
        let big = tmp.path().join("big.bin");
        {
            use std::io::Write;
            let mut f = std::io::BufWriter::new(std::fs::File::create(&big).unwrap());
            let block: Vec<u8> = (0..1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 7) as u8).collect();
            for _ in 0..1024 {
                f.write_all(&block).unwrap();
            }
        }
        let t = Instant::now();
        let id = alice.send_one(&bob.identity.id, vec![big.to_string_lossy().into()]).await.unwrap();
        let r = wait_finished(&alice, &id).await;
        assert_eq!(r.status, Status::Done, "{:?}", r.error);
        let secs = t.elapsed().as_secs_f64();
        println!("BENCH büyük dosya : 1024 MiB in {secs:.2}s = {:.0} MB/s", 1024.0 / secs);

        // 2) Çok sayıda küçük dosya (3000 × 16 KiB)
        let dir = tmp.path().join("Proje");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..3000 {
            std::fs::write(dir.join(format!("f{i:05}.dat")), vec![(i % 251) as u8; 16 * 1024]).unwrap();
        }
        let t = Instant::now();
        let id = alice.send_one(&bob.identity.id, vec![dir.to_string_lossy().into()]).await.unwrap();
        let r = wait_finished(&alice, &id).await;
        assert_eq!(r.status, Status::Done, "{:?}", r.error);
        let secs = t.elapsed().as_secs_f64();
        println!("BENCH küçük dosyalar: 3000 × 16 KiB in {secs:.2}s = {:.0} dosya/s, {:.1} MB/s", 3000.0 / secs, 46.875 / secs);
    }

    /// Yalnızca disk: 3000 × 16 KiB dosya yazma (iş parçacığı sayısı ve .ffpart→ad değişikliği etkisi).
    #[test]
    #[ignore]
    fn bench_disk_only() {
        for (threads, rename) in [(1, true), (4, true), (8, true), (16, true), (8, false)] {
            let tmp = tempfile::tempdir().unwrap();
            let data = vec![7u8; 16 * 1024];
            let t = Instant::now();
            std::thread::scope(|s| {
                for k in 0..threads {
                    let (dir, data) = (tmp.path(), &data);
                    s.spawn(move || {
                        for i in (k..3000).step_by(threads) {
                            let fin = dir.join(format!("f{i:05}.dat"));
                            let part = dir.join(format!("f{i:05}.dat.ffpart"));
                            let path = if rename { &part } else { &fin };
                            std::fs::write(path, data).unwrap();
                            if rename {
                                std::fs::rename(&part, &fin).unwrap();
                            }
                        }
                    });
                }
            });
            let s = t.elapsed().as_secs_f64();
            println!("BENCH disk {threads:>2} iş parçacığı, rename={rename}: {:.0} dosya/s", 3000.0 / s);
        }
    }

    #[test]
    fn bytes_are_formatted_in_turkish() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1,5 KB");
        assert_eq!(fmt_bytes(66 * 1024 * 1024), "66 MB");
    }

    #[tokio::test]
    async fn send_to_multiple_recipients() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        for r in [&bob, &carol] {
            introduce(&alice, r);
            r.set_trusted(&alice.identity.fingerprint, "alice", true);
        }
        let file = tmp.path().join("rapor.pdf");
        std::fs::write(&file, vec![42u8; 3 * 1024 * 1024]).unwrap();

        let ids = alice
            .send_paths(&[bob.identity.id.clone(), carol.identity.id.clone(), "yok".into()], vec![file.to_string_lossy().into()])
            .await
            .unwrap();
        assert_eq!(ids.len(), 2, "çevrimdışı cihaz atlanmalı, diğer ikisine gitmeli");
        for id in &ids {
            assert_eq!(wait_finished(&alice, id).await.status, Status::Done);
        }
        for r in [&bob, &carol] {
            let got = std::fs::read(PathBuf::from(r.settings().download_dir).join("rapor.pdf")).unwrap();
            assert_eq!(got.len(), 3 * 1024 * 1024);
        }
        assert!(alice.send_paths(&["yok".into()], vec![file.to_string_lossy().into()]).await.is_err());
    }

    fn chat_node_events(store: Arc<Mutex<Vec<Value>>>) -> Emitter {
        Arc::new(move |ev, payload| {
            if ev == "chat" {
                store.lock().unwrap().push(payload);
            }
        })
    }

    async fn chat_pair(tmp: &Path) -> (Arc<Node>, Arc<Node>, Arc<Mutex<Vec<Value>>>) {
        let alice = make_node(tmp, "alice").await;
        let events = Arc::new(Mutex::new(Vec::new()));
        let bob = Node::new(&tmp.join("bob/config"), &tmp.join("bob/dl"), chat_node_events(events.clone())).unwrap();
        bob.update_settings(|s| s.device_name = "bob".into());
        bob.start_server(0).await.unwrap();
        introduce(&alice, &bob);
        (alice, bob, events)
    }

    fn last_status(node: &Node, fp: &str) -> Option<MsgStatus> {
        node.chat_history(fp).last().map(|m| m.status)
    }

    #[tokio::test]
    async fn chat_delivery_history_and_unread() {
        let tmp = tempfile::tempdir().unwrap();
        let (alice, bob, events) = chat_pair(tmp.path()).await;
        let afp = alice.identity.fingerprint.clone();
        let bfp = bob.identity.fingerprint.clone();

        let sent = alice.chat_send(&bob.identity.id, "  Selam Bob!\r\nNasılsın?  ").unwrap();
        assert_eq!(sent.text, "Selam Bob!\nNasılsın?");
        wait_until(|| last_status(&alice, &bfp) == Some(MsgStatus::Sent)).await;

        let hist = bob.chat_history(&afp);
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].text, "Selam Bob!\nNasılsın?");
        assert!(!hist[0].outgoing);
        let summary = bob.chat_summaries();
        assert_eq!(summary[0].unread, 1);
        assert_eq!(summary[0].name, "alice");
        assert!(events.lock().unwrap().iter().any(|e| e["incoming"] == true));

        bob.chat_mark_read(&afp);
        assert_eq!(bob.chat_summaries()[0].unread, 0);

        // Geçmiş diske yazılır ve yeniden açılışta yüklenir.
        drop(bob);
        let bob2 = Node::new(&tmp.path().join("bob/config"), &tmp.path().join("bob/dl"), quiet()).unwrap();
        assert_eq!(bob2.chat_history(&afp).len(), 1);

        // Boş ve çok uzun mesajlar gönderilmez.
        assert!(alice.chat_send(&bob2.identity.id, " \u{200B} ").is_err());
        assert!(alice.chat_send(&bob2.identity.id, &"a".repeat(MAX_CHAT_CHARS + 1)).is_err());
    }

    #[tokio::test]
    async fn chat_rejects_blocked_and_spoofed_text_is_cleaned() {
        let tmp = tempfile::tempdir().unwrap();
        let (alice, bob, _events) = chat_pair(tmp.path()).await;
        let afp = alice.identity.fingerprint.clone();
        let bfp = bob.identity.fingerprint.clone();

        // Ağdan doğrudan gelen tehlikeli karakterler temizlenir.
        assert!(bob
            .accept_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), "m1", "fatura\u{202E}fdp.exe\u{0000}")
            .is_ok());
        assert_eq!(bob.chat_history(&afp)[0].text, "faturafdp.exe");
        // Aynı kimlikli mesaj iki kez eklenmez.
        assert!(bob.accept_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), "m1", "x").is_ok());
        assert_eq!(bob.chat_history(&afp).len(), 1);
        // Geçersiz kimlik reddedilir.
        assert_eq!(bob.accept_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), "../x", "y"), Err("invalid"));

        // Engellenen cihazın mesajı reddedilir, gönderen "başarısız" görür.
        bob.set_blocked(&afp, "alice", true);
        alice.chat_send(&bob.identity.id, "orada mısın?").unwrap();
        wait_until(|| last_status(&alice, &bfp) == Some(MsgStatus::Failed)).await;
        assert_eq!(bob.chat_history(&afp).len(), 1);
        // Engellenen cihazın dosya teklifi de reddedilir.
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![file.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Declined);

        // Engeli kaldırınca "tekrar dene" çalışır.
        bob.set_blocked(&afp, "alice", false);
        let failed_id = alice.chat_history(&bfp).last().unwrap().id.clone();
        alice.chat_retry(&bfp, &failed_id).unwrap();
        wait_until(|| last_status(&alice, &bfp) == Some(MsgStatus::Sent)).await;
        assert_eq!(bob.chat_history(&afp).len(), 2);
    }

    // ───────────── Grup testleri ─────────────

    fn introduce_all(nodes: &[&Arc<Node>]) {
        for a in nodes {
            for b in nodes {
                if !Arc::ptr_eq(a, b) {
                    introduce(a, b);
                }
            }
        }
    }

    /// Keşif döngüsünü taklit eder: koşul sağlanana kadar her düğümün giden kutusunu boşaltır.
    async fn wait_flushing(nodes: &[&Arc<Node>], mut f: impl FnMut() -> bool) {
        for _ in 0..100 {
            if f() {
                return;
            }
            for n in nodes {
                n.flush_outbox();
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("koşul zamanında gerçekleşmedi");
    }

    fn group_texts(node: &Node, key: &str) -> Vec<String> {
        node.chat_history(key).into_iter().filter(|m| !m.system).map(|m| m.text).collect()
    }

    #[tokio::test]
    async fn group_create_send_and_receive() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        introduce_all(&[&alice, &bob, &carol]);

        let g = alice
            .group_create("Proje Ekibi", &[bob.identity.fingerprint.clone(), carol.identity.fingerprint.clone()])
            .unwrap();
        let key = g.fingerprint.clone();
        assert_eq!(g.group.as_ref().unwrap().members.len(), 3);

        // Üyeler grubu öğrenir.
        wait_until(|| bob.chat_summaries().iter().any(|c| c.fingerprint == key)).await;
        wait_until(|| carol.chat_summaries().iter().any(|c| c.fingerprint == key)).await;
        assert_eq!(bob.chat_summaries()[0].name, "Proje Ekibi");

        // Herkes yazar, herkes alır.
        alice.group_send(&key, "Merhaba ekip").unwrap();
        bob.group_send(&key, "Selam!").unwrap();
        let all = [&alice, &bob, &carol];
        wait_flushing(&all, || group_texts(&carol, &key).len() == 2).await;
        wait_flushing(&all, || group_texts(&alice, &key).len() == 2).await;
        let carol_msgs: Vec<_> = carol.chat_history(&key).into_iter().filter(|m| !m.system).collect();
        assert!(carol_msgs.iter().any(|m| m.text == "Selam!" && m.sender_name.as_deref() == Some("bob")));
        assert_eq!(carol.chat_summaries().iter().find(|c| c.fingerprint == key).unwrap().unread, 2);
        // Gönderen tarafta "herkese iletildi".
        wait_flushing(&all, || alice.chat_history(&key).iter().any(|m| m.outgoing && m.status == MsgStatus::Sent && m.pending.is_empty())).await;
    }

    #[tokio::test]
    async fn group_outbox_delivers_when_member_comes_online() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        introduce_all(&[&alice, &bob, &carol]);
        let carol_fp = carol.identity.fingerprint.clone();
        let carol_id = carol.identity.id.clone();

        let key = alice.group_create("Ekip", &[bob.identity.fingerprint.clone(), carol_fp.clone()]).unwrap().fingerprint;
        wait_until(|| bob.chat_summaries().iter().any(|c| c.fingerprint == key)).await;

        // Carol "çevrimdışı": Alice'in listesinden düşsün.
        alice.remove_peer(&carol_id);
        alice.group_send(&key, "Carol yokken yazıldı").unwrap();
        wait_flushing(&[&alice], || group_texts(&bob, &key).len() == 1).await;
        let pending = alice.chat_history(&key).into_iter().find(|m| m.outgoing).unwrap().pending;
        assert_eq!(pending, vec![carol_fp.clone()], "Carol için beklemeli");

        // Carol geri gelince giden kutusu önce grup bilgisini, sonra mesajı teslim eder.
        introduce(&alice, &carol);
        for _ in 0..40 {
            alice.flush_outbox();
            if group_texts(&carol, &key).len() == 1 {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(group_texts(&carol, &key), vec!["Carol yokken yazıldı"]);
        wait_until(|| alice.chat_history(&key).iter().any(|m| m.outgoing && m.pending.is_empty())).await;
    }

    #[tokio::test]
    async fn group_only_admin_can_change_and_non_members_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        let mallory = make_node(tmp.path(), "mallory").await;
        introduce_all(&[&alice, &bob, &carol, &mallory]);
        let key = alice.group_create("Gizli", &[bob.identity.fingerprint.clone()]).unwrap().fingerprint;
        wait_until(|| bob.chat_summaries().iter().any(|c| c.fingerprint == key)).await;
        let group = bob.group_of(&key).unwrap().0;

        // Bob (yönetici değil) düzenleyemez.
        assert!(bob.group_edit(&key, Some("Bob'un grubu".into()), &[], &[]).is_err());
        // Mallory, grubu kendi yöneticiliğinde "güncellemeye" çalışır → reddedilir.
        let mut forged = group.clone();
        forged.admin = mallory.identity.fingerprint.clone();
        forged.rev += 10;
        forged.members.push(crate::chat::GroupMember { fingerprint: mallory.identity.fingerprint.clone(), name: "mallory".into() });
        assert!(matches!(bob.accept_group_update(&mallory.identity.fingerprint, forged.clone()), Message::ChatReject { .. }));
        // Yönetici alanını doğru yazsa da gönderen (sertifika) yönetici değilse reddedilir.
        let mut forged2 = group.clone();
        forged2.rev += 10;
        forged2.name = "Ele geçirildi".into();
        assert!(matches!(bob.accept_group_update(&mallory.identity.fingerprint, forged2), Message::ChatReject { .. }));
        assert_eq!(bob.group_of(&key).unwrap().0.name, "Gizli");
        // Üye olmayan Mallory gruba mesaj yazamaz.
        assert_eq!(
            bob.accept_group_chat(mallory.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), &group.id, "m1", "selam", 0),
            Err("sender_not_member")
        );
        // Bilinmeyen grup.
        assert_eq!(
            carol.accept_group_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), &group.id, "m2", "x", 0),
            Err("unknown_group")
        );

        // Yönetici Carol'ı ekler, Bob'u çıkarır, adı değiştirir.
        alice
            .group_edit(&key, Some("Yeni Ad".into()), &[carol.identity.fingerprint.clone()], &[bob.identity.fingerprint.clone()])
            .unwrap();
        wait_until(|| carol.group_of(&key).map_or(false, |(g, _)| g.name == "Yeni Ad")).await;
        wait_until(|| bob.group_of(&key).map_or(false, |(_, left)| left)).await;
        assert!(bob.group_send(&key, "hâlâ burada mıyım?").is_err());
    }

    #[tokio::test]
    async fn group_leave_and_admin_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        introduce_all(&[&alice, &bob, &carol]);
        let key = alice
            .group_create("Ekip", &[bob.identity.fingerprint.clone(), carol.identity.fingerprint.clone()])
            .unwrap()
            .fingerprint;
        wait_until(|| bob.group_of(&key).is_some() && carol.group_of(&key).is_some()).await;

        // Bob ayrılır → yönetici çıkarır → Carol'a da yansır.
        bob.group_leave(&key).unwrap();
        wait_until(|| !alice.group_of(&key).unwrap().0.has(&bob.identity.fingerprint)).await;
        for _ in 0..40 {
            alice.flush_outbox();
            if !carol.group_of(&key).unwrap().0.has(&bob.identity.fingerprint) {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        assert!(!carol.group_of(&key).unwrap().0.has(&bob.identity.fingerprint));

        // Yönetici grubu siler → Carol için grup kapanır.
        alice.group_leave(&key).unwrap();
        wait_until(|| carol.group_of(&key).map_or(false, |(g, left)| g.deleted && left)).await;
        assert!(carol.group_send(&key, "x").is_err());
    }

    #[tokio::test]
    async fn avatar_is_shared_and_verified() {
        use base64::Engine;
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        // Küçük "JPEG" (yalnızca imza baytları önemli).
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpeg.extend(std::iter::repeat(42u8).take(2000));
        let url = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(&jpeg));
        alice.set_avatar(&url).unwrap();
        assert_eq!(alice.device_info().avatar.len(), 16);
        // SVG/çalıştırılabilir içerik reddedilir.
        assert!(alice.set_avatar("data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+").is_err());

        // Sahte özet duyurusu: resim indirilir ama özet tutmadığı için kaydedilmez.
        let mut forged = alice.device_info();
        forged.avatar = "0123456789abcdef".into();
        bob.upsert_peer(forged, IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        bob.check_avatars();
        sleep(Duration::from_millis(500)).await;
        assert!(!bob.avatars().contains_key(&alice.identity.fingerprint));

        // Gerçek duyuru: Bob resmi Alice'ten alır.
        bob.upsert_peer(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        bob.check_avatars();
        wait_until(|| bob.avatars().get(&alice.identity.fingerprint) == Some(&url)).await;

        // Alice resmini kaldırınca Bob'da da silinir.
        alice.remove_avatar();
        bob.upsert_peer(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        bob.check_avatars();
        assert!(!bob.avatars().contains_key(&alice.identity.fingerprint));
    }

    // ───────────── Ortak klasör testleri ─────────────

    /// Dosyayı yazar ve değişim zamanını geçmişe çeker (taramadaki "yazılıyor" beklemesini atlamak için).
    fn write_old(path: &Path, content: &[u8]) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, content).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(10)).unwrap();
    }

    async fn share_ticks(nodes: &[&Arc<Node>], mut cond: impl FnMut() -> bool) {
        for _ in 0..150 {
            if cond() {
                return;
            }
            for n in nodes {
                n.share_tick().await;
            }
            sleep(Duration::from_millis(80)).await;
        }
        panic!("ortak klasör zamanında eşitlenmedi");
    }

    fn share_dir(node: &Node, id: &str) -> PathBuf {
        PathBuf::from(node.share_list().into_iter().find(|s| s.id == id).unwrap().dir)
    }

    fn share_paths(node: &Node, id: &str) -> Vec<String> {
        let mut v: Vec<String> = node.share_files(id).map(|f| f.into_iter().map(|x| x.path).collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shared_folder_syncs_adds_edits_and_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        introduce_all(&[&alice, &bob, &carol]);
        let all = [&alice, &bob, &carol];

        let s = alice
            .share_create("Tasarım", &[bob.identity.fingerprint.clone(), carol.identity.fingerprint.clone()], None)
            .unwrap();
        share_ticks(&all, || bob.share_list().len() == 1 && carol.share_list().len() == 1).await;

        // Alice dosya ve klasör ekler.
        let src = tmp.path().join("kaynak");
        write_old(&src.join("logo.png"), b"PNGDATA");
        write_old(&src.join("Sunum/slayt1.txt"), "ilk sürüm".as_bytes());
        alice
            .share_add_files(&s.id, vec![src.join("logo.png").to_string_lossy().into(), src.join("Sunum").to_string_lossy().into()])
            .await
            .unwrap();
        let expected = vec!["Sunum/slayt1.txt".to_string(), "logo.png".to_string()];
        share_ticks(&all, || share_paths(&bob, &s.id) == expected && share_paths(&carol, &s.id) == expected).await;
        assert_eq!(std::fs::read(share_dir(&carol, &s.id).join("Sunum/slayt1.txt")).unwrap(), "ilk sürüm".as_bytes());

        // Bob dosyayı düzenler → herkese gider, eski hâli Alice'te çöp klasörüne taşınır.
        write_old(&share_dir(&bob, &s.id).join("Sunum/slayt1.txt"), "ikinci sürüm".as_bytes());
        share_ticks(&all, || {
            std::fs::read(share_dir(&alice, &s.id).join("Sunum/slayt1.txt")).ok().as_deref() == Some("ikinci sürüm".as_bytes())
                && std::fs::read(share_dir(&carol, &s.id).join("Sunum/slayt1.txt")).ok().as_deref() == Some("ikinci sürüm".as_bytes())
        })
        .await;
        let trash = share_dir(&alice, &s.id).join(".fastfiles").join("silinen");
        assert!(std::fs::read_dir(&trash).unwrap().count() >= 1, "üzerine yazılan sürüm saklanmalı");

        // Carol bir dosyayı siler → diğerlerinde de silinir.
        std::fs::remove_file(share_dir(&carol, &s.id).join("logo.png")).unwrap();
        share_ticks(&all, || {
            !share_dir(&alice, &s.id).join("logo.png").exists() && !share_dir(&bob, &s.id).join("logo.png").exists()
        })
        .await;
        assert_eq!(share_paths(&bob, &s.id), vec!["Sunum/slayt1.txt".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shared_folder_password_and_access_control() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let mallory = make_node(tmp.path(), "mallory").await;
        introduce_all(&[&alice, &bob, &mallory]);
        let all = [&alice, &bob, &mallory];

        assert!(alice.share_create("Gizli", &[bob.identity.fingerprint.clone()], Some("123".into())).is_err(), "kısa şifre");
        let s = alice.share_create("Gizli", &[bob.identity.fingerprint.clone()], Some("ofis-2026".into())).unwrap();
        write_old(&share_dir(&alice, &s.id).join("maaslar.xlsx"), b"cok gizli");
        share_ticks(&all, || bob.share_list().len() == 1).await;

        // Bob üye ama kilidi açmadı: liste yok, dosya gelmez.
        let bs = bob.share_list().remove(0);
        assert!(bs.protected && !bs.can_access);
        assert!(bob.share_files(&s.id).is_err());
        for _ in 0..10 {
            for n in &all {
                n.share_tick().await;
            }
        }
        assert!(!share_dir(&bob, &s.id).join("maaslar.xlsx").exists());
        // Alice de kilidi açmamış Bob'un isteklerini reddeder.
        assert!(matches!(alice.accept_share_manifest(&bob.identity.fingerprint, &s.id, vec![]), Message::ChatReject { .. }));

        // Yanlış şifre reddedilir; doğru şifre açar ve dosyalar gelir.
        assert_eq!(bob.share_unlock(&s.id, "yanlis").await.unwrap_err(), "Şifre yanlış");
        bob.share_unlock(&s.id, "ofis-2026").await.unwrap();
        share_ticks(&all, || share_dir(&bob, &s.id).join("maaslar.xlsx").exists()).await;
        assert!(bob.share_list()[0].can_access);

        // Üye olmayan Mallory: listeleyemez, dosya isteyemez, sahte yönetici bilgisi gönderemez.
        assert!(matches!(alice.accept_share_manifest(&mallory.identity.fingerprint, &s.id, vec![]), Message::ChatReject { .. }));
        let mut forged = ShareInfoForTest::from(&alice, &s.id);
        forged.members.push(crate::chat::GroupMember { fingerprint: mallory.identity.fingerprint.clone(), name: "mallory".into() });
        forged.rev += 5;
        assert!(matches!(bob.accept_share_update(&mallory.identity.fingerprint, forged), Message::ChatReject { .. }));
        // Kaba kuvvet sınırı: 5 hatalı denemeden sonra doğru şifre de reddedilir.
        for _ in 0..5 {
            let _ = alice.accept_share_unlock(&mallory.identity.fingerprint, &s.id, "tahmin");
        }
        assert!(matches!(
            alice.accept_share_unlock(&mallory.identity.fingerprint, &s.id, "ofis-2026"),
            Message::ShareUnlockResult { ok: false, .. }
        ));

        // Yönetici Bob'u çıkarınca Bob'un eşitlemesi durur.
        alice.share_edit(&s.id, None, &[], &[bob.identity.fingerprint.clone()]).unwrap();
        share_ticks(&all, || bob.share_list().first().map_or(false, |x| x.left)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shared_folder_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce_all(&[&alice, &bob]);
        let all = [&alice, &bob];
        let s = alice.share_create("Silinecek", &[bob.identity.fingerprint.clone()], None).unwrap();
        write_old(&share_dir(&alice, &s.id).join("a.txt"), b"x");
        share_ticks(&all, || bob.share_list().len() == 1).await;
        share_ticks(&all, || share_dir(&bob, &s.id).join("a.txt").exists()).await;
        let alice_dir = share_dir(&alice, &s.id);
        let bob_dir = share_dir(&bob, &s.id);

        // Sahip siler (dosyalarıyla): listesinden hemen kalkar, Bob'da klasör kapanır.
        alice.share_delete(&s.id, true).unwrap();
        assert!(alice.share_list().is_empty());
        assert!(!alice_dir.exists(), "yerel kopya silinmeli");
        share_ticks(&all, || bob.share_list().first().map_or(false, |x| x.deleted && x.left)).await;
        share_ticks(&all, || alice.shares.lock().unwrap().is_empty()).await;

        // Bob da listesinden kaldırır ama dosyalarını tutar.
        bob.share_delete(&s.id, false).unwrap();
        assert!(bob.share_list().is_empty());
        assert!(bob_dir.join("a.txt").exists());
    }

    // ───────────── Oylama testleri ─────────────

    fn poll_of(node: &Node, id: &str) -> Option<poll::PollView> {
        node.poll_list().into_iter().find(|p| p.poll.id == id)
    }

    async fn poll_ticks(nodes: &[&Arc<Node>], mut cond: impl FnMut() -> bool) {
        for _ in 0..100 {
            if cond() {
                return;
            }
            for n in nodes {
                n.flush_polls();
            }
            sleep(Duration::from_millis(60)).await;
        }
        panic!("oylama zamanında eşitlenmedi");
    }

    #[tokio::test]
    async fn poll_vote_tally_and_close() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        let mallory = make_node(tmp.path(), "mallory").await;
        introduce_all(&[&alice, &bob, &carol, &mallory]);
        let all = [&alice, &bob, &carol];

        assert!(alice.poll_create("Öğle yemeği?", &["Pizza".into()], &[bob.identity.fingerprint.clone()], true, false).is_err());
        let p = alice
            .poll_create(
                "Öğle yemeği?",
                &["Pizza".into(), "Döner".into(), " ".into()],
                &[bob.identity.fingerprint.clone(), carol.identity.fingerprint.clone()],
                true,
                false,
            )
            .unwrap();
        assert_eq!(p.poll.options, vec!["Pizza", "Döner"]);
        let id = p.poll.id.clone();
        poll_ticks(&all, || poll_of(&bob, &id).is_some() && poll_of(&carol, &id).is_some()).await;
        assert!(poll_of(&bob, &id).unwrap().can_vote);
        assert!(poll_of(&mallory, &id).is_none(), "katılımcı olmayan oylamayı görmez");

        alice.poll_vote(&id, 0).unwrap();
        bob.poll_vote(&id, 0).unwrap();
        carol.poll_vote(&id, 1).unwrap();
        poll_ticks(&all, || poll_of(&carol, &id).map_or(false, |v| v.poll.counts == vec![2, 1])).await;
        let bv = poll_of(&bob, &id).unwrap();
        poll_ticks(&all, || poll_of(&bob, &id).map_or(false, |v| v.poll.voted.len() == 3 && !v.vote_pending)).await;
        assert_eq!(bv.my_vote, Some(0));
        // İsimli oylama: kim neye oy verdi görünür.
        assert!(poll_of(&carol, &id).unwrap().poll.ballots.iter().any(|b| b.fingerprint == bob.identity.fingerprint && b.option == 0));

        // Oy değiştirme: Bob Döner'e geçer.
        bob.poll_vote(&id, 1).unwrap();
        poll_ticks(&all, || {
            poll_of(&alice, &id).map_or(false, |v| v.poll.counts == vec![1, 2])
                && poll_of(&carol, &id).map_or(false, |v| v.poll.counts == vec![1, 2])
        })
        .await;

        // Güvenlik: katılımcı olmayan oy veremez, sahip olmayan sonuç gönderemez.
        assert!(matches!(alice.accept_poll_vote(&mallory.identity.fingerprint, &id, 0), Message::ChatReject { .. }));
        assert!(matches!(alice.accept_poll_vote(&bob.identity.fingerprint, &id, 9), Message::ChatReject { .. }));
        let mut forged = poll_of(&carol, &id).unwrap().poll;
        forged.counts = vec![99, 0];
        forged.rev += 10;
        assert!(matches!(carol.accept_poll_update(&bob.identity.fingerprint, forged, "bob"), Message::ChatReject { .. }));
        assert_eq!(poll_of(&carol, &id).unwrap().poll.counts, vec![1, 2]);

        // Sahip bitirir: herkes kapandığını ve sonucu görür, artık oy verilemez.
        alice.poll_close(&id).unwrap();
        poll_ticks(&all, || poll_of(&bob, &id).map_or(false, |v| v.poll.closed) && poll_of(&carol, &id).map_or(false, |v| v.poll.closed)).await;
        assert!(bob.poll_vote(&id, 0).is_err());
        assert!(matches!(alice.accept_poll_vote(&bob.identity.fingerprint, &id, 0), Message::ChatReject { .. }));
        assert_eq!(poll_of(&bob, &id).unwrap().poll.counts, vec![1, 2]);
    }

    #[tokio::test]
    async fn anonymous_poll_hides_ballots() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce_all(&[&alice, &bob]);
        let all = [&alice, &bob];
        let id = alice
            .poll_create("Gizli oy", &["Evet".into(), "Hayır".into()], &[bob.identity.fingerprint.clone()], false, true)
            .unwrap()
            .poll
            .id;
        poll_ticks(&all, || poll_of(&bob, &id).is_some()).await;
        assert!(alice.poll_vote(&id, 0).is_err(), "kendini eklemeyen sahip oy kullanmaz");
        bob.poll_vote(&id, 1).unwrap();
        poll_ticks(&all, || poll_of(&bob, &id).map_or(false, |v| v.poll.counts == vec![0, 1])).await;
        assert!(poll_of(&bob, &id).unwrap().poll.ballots.is_empty());
        assert!(poll_of(&alice, &id).unwrap().poll.ballots.is_empty());

        // Listeden kaldırma: açık oylamayı kaldıran sahip onu önce bitirir.
        alice.poll_remove(&id).unwrap();
        assert!(poll_of(&alice, &id).is_none());
        poll_ticks(&all, || poll_of(&bob, &id).map_or(false, |v| v.poll.closed)).await;
    }

    struct ShareInfoForTest;
    impl ShareInfoForTest {
        fn from(node: &Node, id: &str) -> share::ShareInfo {
            let s = node.share_list().into_iter().find(|s| s.id == id).unwrap();
            share::ShareInfo {
                id: s.id,
                name: s.name,
                admin: s.admin,
                members: s.members,
                rev: 1,
                deleted: false,
                protected: s.protected,
                unlocked: vec![],
            }
        }
    }

    #[tokio::test]
    async fn chat_rate_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let (alice, bob, _events) = chat_pair(tmp.path()).await;
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for i in 0..CHAT_RATE_MAX {
            assert!(bob.accept_chat(alice.device_info(), ip, &format!("m{i}"), "spam").is_ok());
        }
        assert_eq!(bob.accept_chat(alice.device_info(), ip, "fazla", "spam"), Err("rate"));
        assert_eq!(bob.chat_history(&alice.identity.fingerprint).len(), CHAT_RATE_MAX);
    }

    #[tokio::test]
    async fn peers_event_only_on_visible_change() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(Mutex::new(0usize));
        let c2 = count.clone();
        let emitter: Emitter = Arc::new(move |ev, _| {
            if ev == "peers" {
                *c2.lock().unwrap() += 1;
            }
        });
        let node = Node::new(&tmp.path().join("cfg"), &tmp.path().join("dl"), emitter).unwrap();
        let dev = DeviceInfo { id: "x".into(), name: "Taha".into(), port: 53317, fingerprint: "f".into(), os: "windows".into(), ..Default::default() };
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 106));
        for _ in 0..20 {
            node.upsert_peer(dev.clone(), ip, None);
        }
        assert_eq!(*count.lock().unwrap(), 1, "aynı duyuru tekrar tekrar olay üretmemeli");
        node.upsert_peer(DeviceInfo { name: "Taha Y.".into(), ..dev.clone() }, ip, None);
        assert_eq!(*count.lock().unwrap(), 2, "isim değişince olay gelmeli");
    }

    #[tokio::test]
    async fn manual_peer_via_ping() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let peer = alice.add_manual_peer(&format!("127.0.0.1:{}", bob.port())).await.unwrap();
        assert_eq!(peer.id, bob.identity.id);
        assert_eq!(peer.name, "bob");
        // Ping karşı tarafa da bizi tanıtır.
        assert!(bob.peers().iter().any(|p| p.id == alice.identity.id));
        // Kendine bağlanma reddedilir.
        assert!(alice.add_manual_peer(&format!("127.0.0.1:{}", alice.port())).await.is_err());
    }
}
