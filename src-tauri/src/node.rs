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
#[path = "game.rs"]
pub mod game;
#[path = "extras.rs"]
pub mod extras;
#[path = "screen.rs"]
pub mod screen;
#[path = "phone.rs"]
pub mod phone;

use crate::chat::{ChatMessage, ChatStore, ConversationSummary, FileRef, FileTransferRef, MsgStatus};
use crate::config::{fingerprint, Identity, Settings, TrustedDevice};
use crate::pipeline;
use crate::protocol::*;
use crate::resume::{files_signature, ResumeEntry, ResumeStore};
use crate::tls;
use crate::update::{self, PackageHeader, CURRENT_VERSION};
use serde::{Deserialize, Serialize};
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

const PEER_TTL: Duration = Duration::from_secs(10);
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
/// Disk/ağ tamponu boyutu (ortak klasör indirmeleri vb.).
const CHUNK: usize = 1024 * 1024;
const EMIT_INTERVAL: Duration = Duration::from_millis(150);
/// Bir cihazdan kabul edilen en fazla mesaj sayısı / zaman penceresi (spam koruması).
const CHAT_RATE_MAX: usize = 30;
const CHAT_RATE_WINDOW: Duration = Duration::from_secs(30);
/// Veri akışı bu kadar süre hiç ilerlemezse bağlantı kopmuş sayılır.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Kopan aktarım için otomatik yeniden deneme beklemeleri (saniye). İlerleme olursa sayaç sıfırlanır.
const RETRY_DELAYS: [u64; 6] = [2, 4, 8, 15, 30, 30];
/// Aktarım geçmişinde saklanan en fazla kayıt.
const HISTORY_MAX: usize = 300;

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
    /// Sanal ofiste seçtiği koltuk (0 = seçmedi).
    pub seat: u8,
    /// Durum: "" | "busy" | "focus" | "away".
    pub status: String,
    #[serde(skip)]
    last_seen: Option<Instant>,
    /// Cihazın son görüldüğü tüm adresler (Wi-Fi + ethernet + VPN gibi birden fazla olabilir).
    #[serde(skip)]
    addrs: Vec<(IpAddr, Instant)>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Waiting,
    Active,
    Done,
    Declined,
    Cancelled,
    Failed,
    /// Gönderen duraklattı; devam ettirilince kaldığı yerden sürer.
    Paused,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Send,
    Receive,
}

/// Aktarım kaydı. Bitenler `transfers.json`'a yazılır (aktarım geçmişi).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Gönderimde kullanıcının seçtiği dosya/klasörler ("Tekrar gönder" için).
    #[serde(default)]
    pub sources: Vec<String>,
    /// Kaldığı yerden devam edildi mi (arayüzde bilgi olarak gösterilir).
    #[serde(default)]
    pub resumed: bool,
    /// Karşı taraf kaldığı yerden devamı destekliyor: duraklatılabilir.
    #[serde(default)]
    pub resumable: bool,
    /// Grup sohbetinden gönderildiyse grubun kimliği.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
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
            sources: Vec::new(),
            resumed: false,
            resumable: false,
            group_id: None,
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

/// Gönderim hatası. `retry`: alıcı kaldığı yerden devamı destekliyor ve hata bağlantıyla
/// ilgili; aynı aktarım kimliğiyle yeniden denenebilir.
struct SendError {
    msg: String,
    retry: bool,
}

impl From<String> for SendError {
    fn from(msg: String) -> Self {
        Self { msg, retry: false }
    }
}

enum RecvError {
    Cancelled,
    Failed(String),
    /// Bağlantı koptu; yarım dosyalar saklandı, gönderen yeniden denerse devam edilir.
    Interrupted,
}

pub struct Node {
    pub identity: Identity,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    port: AtomicU16,
    peers: Mutex<HashMap<String, Peer>>,
    transfers: Mutex<HashMap<String, Transfer>>,
    history_path: PathBuf,
    history_write: Mutex<()>,
    cancels: Mutex<HashMap<String, CancellationToken>>,
    /// Alıcıda süren aktarımı, aynı aktarımın yeni bağlantısı gelince kesmek için.
    interrupts: Mutex<HashMap<String, CancellationToken>>,
    resume: Mutex<ResumeStore>,
    clip_dir: PathBuf,
    /// Duraklatılması istenen gönderimler.
    pauses: Mutex<HashSet<String>>,
    /// "Yazıyor…" sinyali: sohbet başına son gönderim / cihaz başına son alım zamanı.
    typing_sent: Mutex<HashMap<String, Instant>>,
    typing_seen: Mutex<HashMap<String, Instant>>,
    /// Gönderim hız sınırı için jeton kovası: (son güncelleme, jeton).
    bucket: Mutex<(Instant, f64)>,
    pending: Mutex<HashMap<String, oneshot::Sender<Decision>>>,
    emit: Emitter,
    acceptor: TlsAcceptor,
    /// Yerel alt ağlar (ağ adresi, maske) — en uygun karşı adresi seçmek için.
    subnets: Mutex<Vec<(u32, u32)>>,
    last_peers: Mutex<String>,
    chats: Mutex<ChatStore>,
    chat_rate: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// Sormadan kabul edilen resimlerin cihaz başına sayacı (diski doldurma girişimine karşı).
    image_rate: Mutex<HashMap<String, VecDeque<Instant>>>,
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
    games: Mutex<HashMap<String, game::RoomState>>,
    games_path: PathBuf,
    /// Cihaz başına (parmak izi) son başarılı bağlantı adresi: sonraki bağlantılar önce onu dener.
    good_addr: Mutex<HashMap<String, SocketAddr>>,
    /// Durum, sohbet olayları, ekran paylaşımı ve telefon bağlantısı.
    pub extras: extras::State,
    pub shutdown: CancellationToken,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Node {
    pub fn new(config_dir: &Path, default_download: &Path, emit: Emitter) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
        let mut identity = Identity::load_or_create(config_dir)?;
        let settings_path = config_dir.join("settings.json");
        let settings = Settings::load(&settings_path, default_download);
        // Kimlik dosyaları bozuksa (TLS kurulamıyorsa) her açılışta çökmek yerine yeni kimlik
        // üretilir. Diğer cihazlar bu bilgisayarı yeni bir cihaz olarak görür.
        let server_cfg = match tls::server_config(&identity) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[fastfiles] kimlik kullanılamıyor ({e}); yeni kimlik oluşturuluyor");
                for f in ["device-id", "cert.der", "key.der"] {
                    let _ = std::fs::remove_file(config_dir.join(f));
                }
                identity = Identity::load_or_create(config_dir)?;
                tls::server_config(&identity)?
            }
        };
        let acceptor = TlsAcceptor::from(server_cfg);
        let chats = ChatStore::load(&config_dir.join("chats.json"));
        let update_dir = config_dir.join("updates");
        std::fs::create_dir_all(&update_dir).map_err(|e| e.to_string())?;
        let history_path = config_dir.join("transfers.json");
        let history = load_history(&history_path);
        let resume = ResumeStore::load(&config_dir.join("resume.json"), now_ms());
        let node = Arc::new(Self {
            identity,
            settings: Mutex::new(settings),
            settings_path,
            port: AtomicU16::new(0),
            peers: Mutex::new(HashMap::new()),
            transfers: Mutex::new(history),
            history_path,
            history_write: Mutex::new(()),
            cancels: Mutex::new(HashMap::new()),
            interrupts: Mutex::new(HashMap::new()),
            resume: Mutex::new(resume),
            clip_dir: config_dir.join("clipboard"),
            bucket: Mutex::new((Instant::now(), 0.0)),
            pauses: Mutex::new(HashSet::new()),
            typing_sent: Mutex::new(HashMap::new()),
            typing_seen: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            emit,
            acceptor,
            subnets: Mutex::new(Vec::new()),
            last_peers: Mutex::new(String::new()),
            chats: Mutex::new(chats),
            chat_rate: Mutex::new(HashMap::new()),
            image_rate: Mutex::new(HashMap::new()),
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
            games: Mutex::new(HashMap::new()),
            games_path: config_dir.join("games.json"),
            good_addr: Mutex::new(HashMap::new()),
            extras: extras::State::default(),
            shutdown: CancellationToken::new(),
        });
        node.load_cached_package();
        let _ = std::fs::create_dir_all(&node.avatar_dir);
        node.load_avatars();
        let _ = std::fs::create_dir_all(&node.shares_dir);
        node.load_shares();
        node.load_polls();
        node.load_games();
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
        // En son başarıyla bağlanılan adres en öne: ulaşılamayan bir adres (ör. VPN ya da sanal
        // ağ kartı) her bağlantıda yeniden denenip bekletmesin.
        let good = self.good_addr.lock().unwrap().get(&peer.fingerprint).map(|a| a.ip());
        ips.sort_by_key(|a| std::cmp::Reverse((Some(*a) == good, self.addr_score(a), a.to_string() == current)));
        ips.into_iter().map(|ip| SocketAddr::new(ip, peer.port)).collect()
    }

    fn emit(&self, event: &str, payload: Value) {
        (self.emit)(event, payload)
    }

    /// Bildirim; tıklanınca arayüzün açacağı yer `target` ile verilir (ör. {"view":"polls"}).
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
        // Ayarlar tek seferde okunur: yapı ifadesindeki kilit geçicileri ifade bitene kadar
        // yaşar, aynı kilidi ikinci kez almak kilitlenmeye yol açar.
        let (name, seat) = {
            let s = self.settings.lock().unwrap();
            (s.device_name.clone(), s.seat)
        };
        DeviceInfo {
            id: self.identity.id.clone(),
            name,
            port: self.port(),
            fingerprint: self.identity.fingerprint.clone(),
            os: std::env::consts::OS.to_string(),
            version: CURRENT_VERSION.to_string(),
            pkg: self.update_pkg.lock().unwrap().as_ref().map(|h| h.version.clone()).unwrap_or_default(),
            avatar: self.my_avatar_hash(),
            seat,
            status: self.extras.status_now(),
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
        // Kaydetme kilit altında: eşzamanlı iki değişiklik aynı geçici dosyaya yazıp birbirini
        // geri almasın (ör. kaldırılan bir engel geri gelmesin).
        let mut s = self.settings.lock().unwrap();
        f(&mut s);
        if let Err(e) = s.save(&self.settings_path) {
            eprintln!("[fastfiles] ayarlar kaydedilemedi: {e}");
        }
        s.clone()
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
        if d.seat > crate::protocol::MAX_SEAT {
            d.seat = 0;
        }
        if !valid_status(&d.status) {
            d.status.clear();
        }
        let now = Instant::now();
        let changed = {
            let mut peers = self.peers.lock().unwrap();
            match peers.get_mut(&d.id) {
                // Aynı cihaz kimliğini başka bir sertifika kullanıyorsa ve asıl cihaz hâlâ
                // görünüyorsa duyuruyu yok say: başkası "Bob" kartını ele geçiremesin.
                Some(p) if p.fingerprint != d.fingerprint && p.last_seen.is_some_and(|t| now.duration_since(t) < PEER_TTL) => false,
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
                        || p.avatar != d.avatar
                        || p.seat != d.seat
                        || p.status != d.status;
                    p.avatar = d.avatar;
                    p.seat = d.seat;
                    p.status = d.status;
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
                            seat: d.seat,
                            status: d.status,
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
        self.save_history();
    }

    /// Biten aktarımları diske yazar (en yeni `HISTORY_MAX` kayıt). Aynı anda biten iki aktarım
    /// aynı geçici dosyaya yazıp geçmişi bozmasın diye yazımlar sıraya alınır.
    fn save_history(&self) {
        let _guard = self.history_write.lock().unwrap();
        let mut done: Vec<Transfer> = self.transfers.lock().unwrap().values().filter(|t| t.is_finished()).cloned().collect();
        done.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if done.len() > HISTORY_MAX {
            let drop: HashSet<String> = done.split_off(HISTORY_MAX).into_iter().map(|t| t.id).collect();
            self.transfers.lock().unwrap().retain(|id, _| !drop.contains(id));
        }
        let write = || -> std::io::Result<()> {
            let tmp = self.history_path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(&done)?)?;
            std::fs::rename(&tmp, &self.history_path)
        };
        if let Err(e) = write() {
            eprintln!("[fastfiles] aktarım geçmişi yazılamadı: {e}");
        }
    }

    pub fn cancel_transfer(self: &Arc<Self>, id: &str) {
        self.pauses.lock().unwrap().remove(id);
        if let Some(tok) = self.cancels.lock().unwrap().get(id) {
            tok.cancel();
            return;
        }
        // Duraklatılmış / yarıda kalmış gönderim: alıcıdaki yarım dosyalar silinsin.
        let t = self.transfers.lock().unwrap().get(id).cloned();
        let Some(t) = t.filter(|t| t.direction == Direction::Send && t.resumable && matches!(t.status, Status::Paused | Status::Failed)) else {
            return;
        };
        self.update_transfer(id, |t| {
            t.status = Status::Cancelled;
            t.error = None;
        });
        self.save_history();
        if let Some(peer) = self.peer(&t.peer_id) {
            let node = self.clone();
            let tid = id.to_string();
            tokio::spawn(async move {
                node.send_notice(&peer, Message::Abort { device: node.device_info(), transfer_id: tid }).await;
            });
        }
    }

    /// Süresi dolan yarım alımları (ve `.ffpart` dosyalarını) temizler; uygulama uzun süre
    /// açık kalsa da birikmesin.
    pub fn prune_resume(&self) {
        self.resume.lock().unwrap().prune(now_ms());
    }

    /// Gönderimi duraklatır: bağlantı kapanır, alıcıdaki yarım dosyalar korunur.
    /// Devam ettirmek için `retry_transfer`.
    pub fn pause_transfer(&self, id: &str) -> Result<(), String> {
        let t = self.transfers.lock().unwrap().get(id).cloned().ok_or("Aktarım bulunamadı")?;
        if t.direction != Direction::Send || t.is_finished() {
            return Err("Bu aktarım duraklatılamaz".into());
        }
        if !t.resumable {
            return Err("Alıcının FastFiles sürümü duraklatmayı desteklemiyor".into());
        }
        self.pauses.lock().unwrap().insert(id.to_string());
        if let Some(tok) = self.cancels.lock().unwrap().get(id) {
            tok.cancel();
        }
        Ok(())
    }

    pub fn clear_finished(&self) -> Vec<Transfer> {
        self.transfers.lock().unwrap().retain(|_, t| !t.is_finished());
        self.save_history();
        let list = self.transfers();
        self.emit("transfers", json!(list));
        list
    }

    /// Geçmişten tek bir kaydı siler.
    pub fn remove_transfer(&self, id: &str) -> Vec<Transfer> {
        self.transfers.lock().unwrap().retain(|tid, t| tid != id || !t.is_finished());
        self.save_history();
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
        set_keepalive(&tcp);
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
            Message::Offer { mut device, files, total, transfer_id, features, group_id } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                if self.is_blocked(&client_fp) {
                    let _ = write_msg(&mut wr, &Message::Decline { reason: "declined".into() }).await;
                    return Ok(());
                }
                device.name = sanitize_name(&device.name);
                let offer = IncomingOffer {
                    transfer_id,
                    resume: features.iter().any(|f| f == FEATURE_V2),
                    files,
                    total,
                    group_id,
                };
                self.receive(rd, wr, addr.ip(), device, offer).await
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
            Message::GameUpdate { device, room, host_name } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_game_update(&client_fp, room, &host_name);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::GameAction { device, room_id, action } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_game_action(&client_fp, &room_id, action);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::AvatarRequest => {
                let reply = self.serve_avatar();
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::Typing { device, group_id } => {
                if device.fingerprint == client_fp {
                    self.accept_typing(&device, group_id.as_deref());
                }
                Ok(())
            }
            Message::PauseNotice { device, transfer_id } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.pause_receive(&client_fp, &transfer_id).await;
                write_msg(&mut wr, &Message::AbortAck).await.map_err(|e| e.to_string())
            }
            Message::Abort { device, transfer_id } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.abort_receive(&client_fp, &transfer_id).await;
                write_msg(&mut wr, &Message::AbortAck).await.map_err(|e| e.to_string())
            }
            Message::GroupUpdate { device, group } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = self.accept_group_update(&client_fp, group);
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::GroupChat { device, group_id, id, text, sent_at, reply: quoted } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = match self.accept_group_chat(device, addr.ip(), &group_id, &id, &text, sent_at, quoted) {
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
            Message::Chat { device, id, text, reply: quoted } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let reply = match self.accept_chat(device, addr.ip(), &id, &text, quoted) {
                    Ok(()) => Message::ChatAck { id },
                    Err(reason) => Message::ChatReject { reason: reason.into() },
                };
                write_msg(&mut wr, &reply).await.map_err(|e| e.to_string())
            }
            Message::ChatEvent { device, group_id, event } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.accept_chat_event(&device, addr.ip(), group_id.as_deref(), event);
                write_msg(&mut wr, &Message::GroupAck).await.map_err(|e| e.to_string())
            }
            Message::ScreenOffer { device, session } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.accept_screen_offer(&device, &session);
                write_msg(&mut wr, &Message::GroupAck).await.map_err(|e| e.to_string())
            }
            Message::ScreenEnd { device, session } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                self.accept_screen_end(&device.fingerprint, &session);
                write_msg(&mut wr, &Message::GroupAck).await.map_err(|e| e.to_string())
            }
            Message::ScreenWatch { device, session } => {
                if device.fingerprint != client_fp {
                    return Err("cihaz kimliği sertifikayla eşleşmiyor".into());
                }
                let (_rd, wr) = (rd, wr);
                self.serve_screen(device.fingerprint.clone(), session, wr).await
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
        offer: IncomingOffer,
    ) -> Result<(), String> {
        self.upsert_peer(device.clone(), ip, None);
        let IncomingOffer { transfer_id, resume, files, total, group_id } = offer;
        // Grup bağlamı yalnızca iki taraf da o grubun üyesiyse geçerlidir.
        let group = group_id.as_deref().and_then(|gid| self.shared_group(gid, &device.fingerprint));
        let fp = device.fingerprint.clone();

        let mut sanitized: Vec<(Vec<String>, u64)> = Vec::with_capacity(files.len());
        for f in &files {
            let parts = sanitize_rel_path(&f.path).ok_or_else(|| format!("geçersiz yol: {}", f.path))?;
            sanitized.push((parts, f.size));
        }
        if sanitized.is_empty() || sanitized.iter().map(|f| f.1).sum::<u64>() != total {
            let _ = write_msg(&mut wr, &Message::Decline { reason: "invalid".into() }).await;
            return Err("geçersiz teklif".into());
        }

        // Kaldığı yerden devam: yalnızca geçerli bir aktarım kimliğiyle.
        let resume = resume && valid_message_id(&transfer_id);
        let signature = files_signature(&files);
        let prior = if resume { self.resume.lock().unwrap().find(&fp, &transfer_id, &signature) } else { None };

        // HashSet: çok sayıda farklı üst düzey adla gelen bir teklif, onay sorulmadan önce
        // karesel tarama yüzünden uygulamayı kilitleyemesin.
        let mut top_names: Vec<String> = Vec::new();
        let mut top_seen: HashSet<&str> = HashSet::new();
        for (parts, _) in &sanitized {
            if top_seen.insert(parts[0].as_str()) {
                top_names.push(parts[0].clone());
            }
        }
        let title = if top_names.len() == 1 {
            top_names[0].clone()
        } else {
            format!("{} öğe", top_names.len())
        };

        // ── Kullanıcı onayı (yarıda kalan aktarımın devamı zaten onaylanmıştı) ──
        // Tek bir resim dosyası sohbet mesajı gibi sormadan alınır; klasörler, birden fazla
        // dosya ve diğer türler onay ister. Kısa sürede çok fazla resim gelirse yine sorulur.
        let trusted = self.is_trusted(&fp);
        let image = prior.is_none() && is_auto_image(&sanitized);
        let auto_image = image && !trusted && rate_ok(&self.image_rate, &fp, IMAGE_RATE_MAX, IMAGE_RATE_WINDOW);
        let decision = if prior.is_some() || trusted || auto_image {
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
                    "group": group.as_ref().map(|(_, name)| name.clone()),
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
            self.set_trusted(&fp, &device.name, true);
        }

        // ── Hedef klasör ve benzersiz isimler (devamda öncekiler aynen kullanılır) ──
        let dest = match &prior {
            Some(p) => PathBuf::from(&p.dest),
            None => PathBuf::from(self.settings().download_dir),
        };
        if let Err(e) = tokio::fs::create_dir_all(&dest).await {
            let _ = write_msg(&mut wr, &Message::Decline { reason: "storage".into() }).await;
            return Err(format!("indirme klasörü oluşturulamadı: {e}"));
        }
        let tops: HashMap<String, String> = match &prior {
            Some(p) => p.tops.clone(),
            None => {
                // Yarım aktarımların ayırdığı adlar da dolu sayılır.
                let mut used: HashSet<String> =
                    self.resume.lock().unwrap().reserved_names(&dest.to_string_lossy()).into_iter().collect();
                let mut tops = HashMap::new();
                for (parts, _) in &sanitized {
                    if !tops.contains_key(&parts[0]) {
                        let name = unique_name(&dest, &parts[0], parts.len() == 1, &used);
                        used.insert(name.to_lowercase());
                        tops.insert(parts[0].clone(), name);
                    }
                }
                tops
            }
        };
        let Some(targets) = sanitized
            .iter()
            .map(|(parts, _)| {
                let mut t = dest.join(tops.get(&parts[0])?);
                for p in &parts[1..] {
                    t.push(p);
                }
                Some(t)
            })
            .collect::<Option<Vec<PathBuf>>>()
        else {
            let _ = write_msg(&mut wr, &Message::Decline { reason: "invalid".into() }).await;
            return Err("devam kaydı dosya listesiyle uyuşmuyor".into());
        };
        let save_path = if tops.len() == 1 {
            dest.join(tops.values().next().unwrap())
        } else {
            dest.clone()
        };

        // Aynı aktarımın eski bağlantısı hâlâ açık görünüyorsa (ör. karşı tarafın Wi-Fi'ı koptu)
        // önce onu kapat: aynı yarım dosyaya iki yazıcı olmasın.
        let local_id = prior.as_ref().map(|p| p.local_id.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if prior.is_some() {
            self.interrupt_receive(&local_id).await;
        }
        // Kesme anahtarı hemen kaydedilir: yarım dosyaların özetlenmesi uzun sürebilir ve bu
        // sırada gelen yeni bağlantı bu işi durdurabilmeli (aynı dosyaya iki yazıcı olmasın).
        let interrupt = CancellationToken::new();
        self.interrupts.lock().unwrap().insert(local_id.clone(), interrupt.clone());

        // ── Devam noktaları ve yarım dosyaların mevcut kısmının özeti ──
        let mut offsets = vec![0u64; sanitized.len()];
        let mut prefix_hashers: HashMap<usize, blake3::Hasher> = HashMap::new();
        let prepare = async {
            if prior.is_none() {
                return;
            }
            for (i, ((_, size), target)) in sanitized.iter().zip(&targets).enumerate() {
                let part = part_path(target);
                offsets[i] = match tokio::fs::metadata(&part).await {
                    // Son bayt hiç işaretlenmez: özet her zaman karşı tarafla doğrulansın.
                    Ok(m) if m.is_file() => m.len().min(size.saturating_sub(1)),
                    _ => match tokio::fs::metadata(target).await {
                        Ok(m) if *size > 0 && m.is_file() && m.len() == *size => *size,
                        _ => 0,
                    },
                };
                if offsets[i] > 0 && offsets[i] < *size {
                    match hash_prefix(part, offsets[i]).await {
                        Ok(h) => {
                            prefix_hashers.insert(i, h);
                        }
                        Err(_) => offsets[i] = 0,
                    }
                }
            }
        };
        let interrupted = tokio::select! {
            _ = prepare => false,
            _ = interrupt.cancelled() => true,
        };
        if interrupted {
            self.interrupts.lock().unwrap().remove(&local_id);
            return Ok(());
        }
        if resume && prior.is_none() {
            self.resume.lock().unwrap().insert(ResumeEntry {
                transfer_id: transfer_id.clone(),
                sender: fp.clone(),
                local_id: local_id.clone(),
                signature,
                dest: dest.to_string_lossy().into_owned(),
                tops: tops.clone(),
                created_at: now_ms(),
            });
        }

        let accept = if resume {
            Message::Accept { features: vec![FEATURE_V2.into()], offsets: offsets.clone() }
        } else {
            Message::Accept { features: vec![], offsets: vec![] }
        };
        if let Err(e) = write_msg(&mut wr, &accept).await {
            // Kabul iletilemedi: yeni açılan devam kaydı adları boşuna ayırmasın.
            if resume && prior.is_none() {
                self.resume.lock().unwrap().remove(&fp, &transfer_id);
            }
            self.interrupts.lock().unwrap().remove(&local_id);
            return Err(e.to_string());
        }

        let id = local_id;
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id.clone(), token.clone());
        let summary = format!("{title} · {}", fmt_bytes(total));
        let already: u64 = offsets.iter().sum();
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
        t.done = already;
        t.resumed = prior.is_some();
        t.group_id = group.as_ref().and(group_id.clone());
        self.insert_transfer(t);
        // Sohbette dosya balonu: grup bağlamı doğrulandıysa grup sohbetinde, değilse birebir.
        let file = FileRef {
            title: title.clone(),
            count: sanitized.len() as u32,
            total,
            transfers: vec![FileTransferRef { to: self.identity.fingerprint.clone(), id: id.clone() }],
            sources: Vec::new(),
        };
        // Resimler sohbet mesajı gibi gelir (okunmamış + mesaj bildirimi).
        let loud = image;
        match &group {
            Some((key, name)) => self.store_file_bubble(key, name, &id, false, file, Some((&device.fingerprint, &device.name)), loud),
            None => self.store_file_bubble(&device.fingerprint, &device.name, &id, false, file, None, loud),
        }
        // Güvenilir cihazdan sormadan kabul edildiyse kullanıcının haberi olsun.
        if trusted && prior.is_none() && !image {
            self.notify(&format!("{} dosya gönderiyor", device.name), &summary, Some(&id));
        }

        // ── Veri akışı: ağ bu görevde okunur, disk yazımı ayrı iş parçacıklarında yapılır ──
        const LOST: &str = "Gönderen iptal etti veya bağlantı koptu";
        let sink = pipeline::DiskSink::new();
        let sink_handle = sink.handle();
        let mut rd = tokio::io::BufReader::with_capacity(256 * 1024, rd);
        let result = {
            let work = async {
                let mut dec = zstd::bulk::Decompressor::new().map_err(|e| e.to_string())?;
                for (i, ((parts, size), target)) in sanitized.iter().zip(&targets).enumerate() {
                    let offset = offsets[i];
                    if resume && *size > 0 && offset >= *size {
                        continue; // önceki denemede tamamlanmış
                    }
                    if let Some(e) = sink.error() {
                        return Err(e);
                    }
                    let name = parts.join("/");
                    self.set_current_file(&id, &name);
                    let remaining = *size - offset;
                    let t = pipeline::Target {
                        name,
                        part: part_path(target),
                        target: target.clone(),
                        offset,
                        prefix: resume.then(|| prefix_hashers.remove(&i).unwrap_or_default()),
                    };
                    // Gönderen, devam eden dosyanın baştaki kısmını önce okuyup özetler;
                    // ilk baytı beklerken bu süre de hesaba katılır (≥ 40 MB/sn varsayımı).
                    let first_wait = STALL_TIMEOUT + Duration::from_secs(offset / (40 * 1024 * 1024));
                    let body_err = |e: pipeline::BodyError| match e {
                        pipeline::BodyError::Lost => LOST.to_string(),
                        pipeline::BodyError::Protocol(e) => e,
                    };
                    let mut body = pipeline::Body::new(remaining, resume, first_wait, STALL_TIMEOUT);
                    if offset == 0 && remaining <= pipeline::SMALL_FILE {
                        // Küçük dosya: bellekte toplanır, paralel yazıcılardan birine verilir.
                        let mut data = Vec::with_capacity(remaining as usize);
                        while let Some(c) = body.next(&mut rd, &mut dec).await.map_err(body_err)? {
                            self.add_progress(&id, c.len() as u64);
                            data.extend_from_slice(&c);
                        }
                        let expected = body.hash(&mut rd).await.map_err(body_err)?;
                        sink.small(t, data, expected).await?;
                    } else {
                        sink.open_big(t).await?;
                        while let Some(c) = body.next(&mut rd, &mut dec).await.map_err(body_err)? {
                            self.add_progress(&id, c.len() as u64);
                            sink.big_data(c).await?;
                        }
                        let expected = body.hash(&mut rd).await.map_err(body_err)?;
                        sink.finish_big(expected).await?;
                    }
                }
                Ok::<(), String>(())
            };
            tokio::select! {
                r = work => r.map_err(|e| if resume && e == LOST { RecvError::Interrupted } else { RecvError::Failed(e) }),
                _ = token.cancelled() => Err(RecvError::Cancelled),
                _ = interrupt.cancelled() => Err(RecvError::Interrupted),
            }
        };
        // Diske yazımların bitmesini bekle (başarıda doğrulama hataları da burada çıkar).
        let result = match result {
            Ok(()) => sink.finish().await.map(|_| ()).map_err(RecvError::Failed),
            Err(e) => {
                if !matches!(e, RecvError::Interrupted) {
                    sink_handle.abort();
                }
                sink.close().await;
                Err(e)
            }
        };

        // Kopan bağlantıda yarım dosyalar saklanır (devam için); diğer hatalarda silinir.
        if matches!(result, Err(RecvError::Cancelled) | Err(RecvError::Failed(_))) {
            for p in sink_handle.in_flight() {
                remove_with_retry(&p).await;
            }
        }
        if resume && matches!(result, Ok(()) | Err(RecvError::Cancelled)) {
            self.resume.lock().unwrap().remove(&fp, &transfer_id);
        }

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
            Err(RecvError::Interrupted) => {
                // Gönderen birazdan yeniden bağlanır; bildirim gösterilmez.
                self.finish_transfer(
                    &id,
                    Status::Failed,
                    Some("Bağlantı koptu · gönderen yeniden denediğinde kaldığı yerden devam edecek".into()),
                );
            }
            Err(RecvError::Failed(e)) => {
                // Yeni gönderenler nedeni gösterir ve yeniden dener; eskiler "bağlantı koptu" der.
                let _ = write_msg(&mut wr, &Message::Decline { reason: e.clone() }).await;
                self.notify(&format!("Aktarım tamamlanamadı · {}", device.name), &format!("{title}: {e}"), Some(&id));
                self.finish_transfer(&id, Status::Failed, Some(e));
            }
        }
        // En sonda: yeni bağlantı bu kaydın kalkmasını bekleyip aynı kimlikle devam eder.
        self.interrupts.lock().unwrap().remove(&id);
        Ok(())
    }

    /// Alıcıda süren aynı aktarımı keser ve bitmesini bekler.
    async fn interrupt_receive(&self, local_id: &str) {
        let tok = self.interrupts.lock().unwrap().get(local_id).cloned();
        let Some(tok) = tok else { return };
        tok.cancel();
        for _ in 0..100 {
            if !self.interrupts.lock().unwrap().contains_key(local_id) {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        // Kapanan dosya tutamacının arka plandaki son yazması tamamlansın.
        sleep(Duration::from_millis(200)).await;
    }

    /// Gönderen, yarıda kalan aktarımı iptal etti: yarım dosyalar ve devam kaydı silinir.
    /// Gönderen duraklattı: yarım dosyalar ve devam kaydı korunur, kayıt "duraklatıldı" olur.
    async fn pause_receive(&self, fp: &str, transfer_id: &str) {
        let entry = self.resume.lock().unwrap().find_by_id(fp, transfer_id);
        let Some(entry) = entry else { return };
        self.interrupt_receive(&entry.local_id).await;
        self.update_transfer(&entry.local_id, |t| {
            if t.status == Status::Failed {
                t.status = Status::Paused;
                t.error = Some("Gönderen duraklattı · devam ettirdiğinde kaldığı yerden sürecek".into());
            }
        });
        self.save_history();
    }

    async fn abort_receive(&self, fp: &str, transfer_id: &str) {
        let entry = self.resume.lock().unwrap().find_by_id(fp, transfer_id);
        let Some(entry) = entry else { return };
        self.interrupt_receive(&entry.local_id).await;
        self.resume.lock().unwrap().remove(fp, transfer_id);
        for p in entry.part_files() {
            remove_with_retry(&p).await;
        }
        self.update_transfer(&entry.local_id, |t| {
            if matches!(t.status, Status::Failed | Status::Paused) {
                t.status = Status::Cancelled;
                t.error = Some("Gönderen iptal etti".into());
            }
        });
        self.save_history();
    }

    // ───────────────────────────── Mesajlaşma ─────────────────────────────

    fn chat_rate_ok(&self, fp: &str) -> bool {
        rate_ok(&self.chat_rate, fp, CHAT_RATE_MAX, CHAT_RATE_WINDOW)
    }

    /// Gelen mesajı doğrular ve kaydeder. Hata durumunda karşı tarafa gidecek ret kodu döner.
    fn accept_chat(&self, device: DeviceInfo, ip: IpAddr, id: &str, text: &str, reply: Option<crate::chat::ReplyRef>) -> Result<(), &'static str> {
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
        // Zaten alınmış mesajın yeniden gönderimi (onay kaybolduysa) hız sınırından düşmez.
        if self.chats.lock().unwrap().contains(&fp, id) {
            return Ok(());
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
        let msg = ChatMessage {
            id: id.to_string(),
            outgoing: false,
            text,
            at: now_ms(),
            status: MsgStatus::Received,
            error: None,
            reply: reply.map(extras::clean_reply),
            ..Default::default()
        };
        self.store_chat(&fp, &name, msg);
        Ok(())
    }

    /// Aktarımı sohbette dosya balonu olarak gösterir (aynı aktarım için bir kez).
    /// Okunmamış sayısını artırmaz ve bildirim tetiklemez: aktarımın kendi bildirimi var.
    /// `loud`: okunmamış sayılır ve gelen mesaj gibi bildirilir (sormadan alınan resimler).
    #[allow(clippy::too_many_arguments)]
    fn store_file_bubble(
        &self,
        key: &str,
        name: &str,
        transfer_id: &str,
        outgoing: bool,
        file: FileRef,
        sender: Option<(&str, &str)>,
        loud: bool,
    ) {
        let msg_id = format!("f-{transfer_id}");
        let msg = {
            let mut chats = self.chats.lock().unwrap();
            if chats.contains(key, &msg_id) {
                return;
            }
            let msg = ChatMessage {
                id: msg_id,
                outgoing,
                text: format!("📎 {}", file.title),
                at: now_ms(),
                status: if outgoing { MsgStatus::Sent } else { MsgStatus::Received },
                sender: sender.map(|s| s.0.to_string()),
                sender_name: sender.map(|s| s.1.to_string()),
                file: Some(file),
                ..Default::default()
            };
            if loud {
                chats.push(key, name, msg.clone());
            } else {
                chats.push_quiet(key, name, msg.clone());
            }
            msg
        };
        let unread = self.chats.lock().unwrap().conversation_meta(key).map_or(0, |m| m.1);
        self.emit("chat", json!({ "fingerprint": key, "name": name, "unread": unread, "incoming": loud, "message": msg }));
    }

    /// `group_id` grubunda hem bu cihaz hem `fp` üyeyse (grup sohbet anahtarı, grup adı).
    fn shared_group(&self, group_id: &str, fp: &str) -> Option<(String, String)> {
        if !valid_message_id(group_id) {
            return None;
        }
        let key = crate::chat::group_key(group_id);
        let (g, left) = self.group_of(&key)?;
        (!left && !g.deleted && g.has(&self.identity.fingerprint) && g.has(fp)).then(|| (key, g.name))
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

    /// "Yazıyor…" sinyali gönderir (sohbet başına en fazla 2,5 sn'de bir). Grupta çevrimiçi
    /// üyelere, birebir sohbette karşı cihaza gider; yanıt beklenmez.
    pub fn send_typing(self: &Arc<Self>, key: &str) {
        // Tokio çalışma ortamı dışından çağrılırsa (ör. senkron bir komut) çökmek yerine atla:
        // sürüm derlemesinde panik uygulamayı kapatır. "Yazıyor…" kaybı önemsizdir.
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            eprintln!("[fastfiles] send_typing çalışma ortamı dışında çağrıldı; atlandı");
            return;
        };
        let now = Instant::now();
        {
            let mut sent = self.typing_sent.lock().unwrap();
            if sent.get(key).is_some_and(|t| now.duration_since(*t) < Duration::from_millis(2500)) {
                return;
            }
            sent.insert(key.to_string(), now);
        }
        let me = self.identity.fingerprint.clone();
        let (targets, group_id): (Vec<String>, Option<String>) = match self.group_of(key) {
            Some((g, left)) => {
                if left || g.deleted {
                    return;
                }
                (g.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect(), Some(g.id))
            }
            None => (vec![key.to_string()], None),
        };
        for peer in self.peers().into_iter().filter(|p| !p.blocked && targets.contains(&p.fingerprint)) {
            let node = self.clone();
            let group_id = group_id.clone();
            rt.spawn(async move {
                if let Ok(mut s) = node.connect(&node.candidates(&peer), Some(peer.fingerprint.clone())).await {
                    let _ = write_msg(&mut s, &Message::Typing { device: node.device_info(), group_id }).await;
                    let _ = s.shutdown().await;
                }
            });
        }
    }

    fn accept_typing(&self, device: &DeviceInfo, group_id: Option<&str>) {
        let fp = device.fingerprint.clone();
        if self.is_blocked(&fp) {
            return;
        }
        // Grup sinyali yalnızca iki taraf da üyeyse geçerli; değilse yok sayılır.
        let key = match group_id {
            Some(gid) => match self.shared_group(gid, &fp) {
                Some((key, _)) => key,
                None => return,
            },
            None => fp.clone(),
        };
        {
            // Aynı sohbette aynı cihazdan saniyede birden sık sinyal yok sayılır.
            let mut seen = self.typing_seen.lock().unwrap();
            let now = Instant::now();
            let slot = format!("{fp}|{key}");
            if seen.get(&slot).is_some_and(|t| now.duration_since(*t) < Duration::from_millis(900)) {
                return;
            }
            seen.insert(slot, now);
        }
        self.emit("typing", json!({ "key": key, "fingerprint": fp, "name": sanitize_name(&device.name) }));
    }

    /// PIN unutulunca: mesajlar ve aktarım geçmişi silinir.
    pub fn wipe_private_data(&self) {
        self.chats.lock().unwrap().wipe_messages();
        self.transfers.lock().unwrap().retain(|_, t| !t.is_finished());
        self.save_history();
    }

    pub fn chat_summaries(&self) -> Vec<ConversationSummary> {
        self.chats.lock().unwrap().summaries()
    }

    pub fn chat_history(&self, fp: &str) -> Vec<ChatMessage> {
        self.chats.lock().unwrap().history(fp)
    }

    /// Sohbetteki tek dosyalık resim balonunun yerel dosya yolu: gönderende seçilen kaynak,
    /// alıcıda (aktarım bittiyse) kaydedilen dosya. Yol arayüzden alınmaz; yalnızca mesaj
    /// kaydından çözülür, böylece bu komutla rastgele dosya okunamaz.
    pub fn chat_image_path(&self, key: &str, id: &str) -> Result<PathBuf, String> {
        let msg = self.chats.lock().unwrap().get(key, id).ok_or("Mesaj bulunamadı")?;
        let file = msg.file.ok_or("Bu mesaj bir dosya değil")?;
        if file.count != 1 {
            return Err("Birden fazla dosya".into());
        }
        let transfers: Vec<Transfer> = {
            let map = self.transfers.lock().unwrap();
            file.transfers.iter().filter_map(|r| map.get(&r.id).cloned()).collect()
        };
        let path = if msg.outgoing {
            file.sources.first().cloned().or_else(|| transfers.iter().find_map(|t| t.sources.first().cloned()))
        } else {
            transfers.iter().find(|t| t.status == Status::Done).and_then(|t| t.save_path.clone())
        };
        let path = PathBuf::from(path.ok_or("Dosya henüz hazır değil")?);
        if !is_image_path(&path) {
            return Err("Resim dosyası değil".into());
        }
        Ok(path)
    }

    /// Resim balonunun içeriği (sohbette önizleme ve resim gösterici için).
    pub async fn chat_image(&self, key: &str, id: &str) -> Result<Vec<u8>, String> {
        let path = self.chat_image_path(key, id)?;
        let meta = tokio::fs::metadata(&path).await.map_err(|_| "Dosya artık yok".to_string())?;
        if !meta.is_file() {
            return Err("Dosya artık yok".into());
        }
        if meta.len() > IMAGE_PREVIEW_MAX {
            return Err("Resim önizleme için çok büyük".into());
        }
        tokio::fs::read(&path).await.map_err(|e| e.to_string())
    }

    pub fn chat_mark_read(&self, fp: &str) {
        self.chats.lock().unwrap().mark_read(fp);
    }

    /// Geçmişi siler. Gruplarda grup bilgisi korunur (yalnızca mesajlar silinir);
    /// ayrılınmış gruplar tamamen kaldırılır.
    pub fn chat_clear(&self, key: &str) {
        let mut chats = self.chats.lock().unwrap();
        let (active_group, notices_pending) = chats.conv(key).map_or((false, false), |c| {
            (c.group.is_some() && !c.left, c.leave_pending || !c.sync_pending.is_empty())
        });
        if active_group {
            chats.with(key, |c| {
                c.messages.retain(|m| m.outgoing && !m.pending.is_empty());
                c.unread = 0;
            });
        } else if notices_pending {
            // Ayrılık/silme bildirimi henüz gitmedi: kaydı silersek bildirim kaybolur ve
            // yönetici beni gruba geri katar. Gizle; bildirim gidince kaldırılır.
            chats.with(key, |c| {
                c.messages.clear();
                c.unread = 0;
                c.hidden = true;
            });
        } else {
            chats.clear(key);
        }
    }

    /// Mesajı kaydeder ve arka planda iletir. Durum değişiklikleri "chat" olayıyla bildirilir.
    pub fn chat_send(self: &Arc<Self>, peer_id: &str, text: &str, reply_to: Option<&str>) -> Result<ChatMessage, String> {
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
        let reply = reply_to.and_then(|id| self.reply_ref(&peer.fingerprint, id));
        let msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            outgoing: true,
            text,
            at: now_ms(),
            status: MsgStatus::Sending,
            error: None,
            reply,
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
        write_msg(&mut stream, &Message::Chat { device: self.device_info(), id: msg.id.clone(), text: msg.text.clone(), reply: msg.reply.clone() })
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
        let (entries, title) = scan_sources(&paths).await?;
        Ok(peers
            .into_iter()
            .map(|peer| self.start_send(peer, entries.clone(), title.clone(), paths.clone(), None, None))
            .collect())
    }

    /// Geçmişteki bir gönderimi tekrarlar. Bitmemiş (başarısız/iptal/reddedilmiş) gönderim aynı
    /// kimlikle yeniden başlar; alıcıda yarım dosyalar varsa kaldığı yerden devam eder.
    pub async fn retry_transfer(self: &Arc<Self>, id: &str) -> Result<String, String> {
        let t = self.transfers.lock().unwrap().get(id).cloned().ok_or("Aktarım bulunamadı")?;
        if t.direction != Direction::Send || !t.is_finished() || t.sources.is_empty() {
            return Err("Bu aktarım tekrar gönderilemez".into());
        }
        let peer = self.peer(&t.peer_id).ok_or_else(|| format!("{} şu an çevrimdışı", t.peer_name))?;
        let (entries, title) = scan_sources(&t.sources).await?;
        let reuse = (t.status != Status::Done).then(|| t.id.clone());
        Ok(self.start_send(peer, entries, title, t.sources.clone(), reuse, t.group_id.clone()))
    }

    /// Panodakini gönderir: kopyalanan dosyalar ve ekran görüntüsü dosya olarak, kısa metin
    /// sohbet mesajı olarak, mesaja sığmayan metin .txt dosyası olarak gider.
    pub async fn send_clipboard(self: &Arc<Self>, peer_ids: &[String], stamp: &str) -> Result<Value, String> {
        use crate::clipboard::{self, ClipContent};
        let (dir, st) = (self.clip_dir.clone(), stamp.to_string());
        let content = tokio::task::spawn_blocking(move || clipboard::read(&dir, &st))
            .await
            .map_err(|e| e.to_string())??;
        let (kind, paths) = match content {
            ClipContent::Files(paths) => ("files", paths),
            ClipContent::Image(p) => ("image", vec![p.to_string_lossy().into_owned()]),
            ClipContent::Text(text) if text.chars().count() <= MAX_CHAT_CHARS => {
                let mut sent = 0;
                let mut last_err = None;
                for id in peer_ids {
                    match self.chat_send(id, &text, None) {
                        Ok(_) => sent += 1,
                        Err(e) => last_err = Some(e),
                    }
                }
                return match (sent, last_err) {
                    (0, Some(e)) => Err(e),
                    _ => Ok(json!({ "kind": "text", "count": sent })),
                };
            }
            ClipContent::Text(text) => {
                let p = clipboard::save_text(&self.clip_dir, stamp, &text)?;
                ("textfile", vec![p.to_string_lossy().into_owned()])
            }
        };
        let ids = self.send_paths(peer_ids, paths).await?;
        Ok(json!({ "kind": kind, "count": ids.len() }))
    }

    fn transfer_done(&self, id: &str) -> u64 {
        self.transfers.lock().unwrap().get(id).map_or(0, |t| t.done)
    }

    fn start_send(
        self: &Arc<Self>,
        peer: Peer,
        entries: Arc<Vec<(PathBuf, FileEntry)>>,
        title: String,
        sources: Vec<String>,
        reuse_id: Option<String>,
        group_id: Option<String>,
    ) -> String {
        let total: u64 = entries.iter().map(|e| e.1.size).sum();
        let was_resumable = reuse_id
            .as_ref()
            .and_then(|r| self.transfers.lock().unwrap().get(r).map(|t| t.resumable))
            .unwrap_or(false);
        let id = reuse_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Birebir gönderim o kişiyle sohbette dosya balonu olarak görünür
        // (grup gönderiminde balon grup sohbetindedir).
        if group_id.is_none() {
            let file = FileRef {
                title: title.clone(),
                count: entries.len() as u32,
                total,
                transfers: vec![FileTransferRef { to: peer.fingerprint.clone(), id: id.clone() }],
                sources: Vec::new(),
            };
            self.store_file_bubble(&peer.fingerprint, &peer.name, &id, true, file, None, false);
        }
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id.clone(), token.clone());
        let mut t = Transfer::new(
            id.clone(),
            Direction::Send,
            peer.id.clone(),
            peer.name.clone(),
            title,
            entries.len(),
            total,
            Status::Waiting,
        );
        t.sources = sources;
        t.group_id = group_id;
        t.resumable = was_resumable;
        self.insert_transfer(t);

        let node = self.clone();
        let tid = id.clone();
        tokio::spawn(async move {
            let mut peer = peer;
            // Alıcı kaldığı yerden devamı destekliyorsa kopan bağlantıda otomatik yeniden denenir.
            // Daha önce devam desteğiyle başlamış bir aktarımın yeniden başlatılması (duraklat →
            // devam, Tekrar dene): alıcı önce yarım dosyaları özetler, bu uzun sürebilir.
            let mut resumable = was_resumable;
            let mut failures = 0usize;
            let outcome = loop {
                let before = node.transfer_done(&tid);
                match node.run_send(&tid, &peer, entries.clone(), &token, &mut resumable, failures > 0 || was_resumable).await {
                    Err(e) if e.retry && resumable && !token.is_cancelled() => {
                        if node.transfer_done(&tid) > before {
                            failures = 0; // ilerleme oldu: deneme hakkı yenilenir
                        }
                        let Some(&wait) = RETRY_DELAYS.get(failures) else { break Err(e.msg) };
                        failures += 1;
                        node.update_transfer(&tid, |t| {
                            t.status = Status::Waiting;
                            t.speed = 0.0;
                            t.current_file = None;
                            t.error = Some(format!(
                                "{} · yeniden bağlanılıyor ({failures}/{})",
                                e.msg,
                                RETRY_DELAYS.len()
                            ));
                        });
                        tokio::select! {
                            _ = sleep(Duration::from_secs(wait)) => {}
                            _ = token.cancelled() => break Ok(SendOutcome::Cancelled),
                        }
                        if let Some(p) = node.peer(&peer.id) {
                            peer = p; // adres değişmiş olabilir
                        }
                    }
                    Err(e) => break Err(e.msg),
                    Ok(o) => break Ok(o),
                }
            };
            match outcome {
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
                // Bildirim, kayıt "bitti" olmadan önce gönderilir: yoksa hemen "devam" denince
                // geç gelen bildirim yeni başlayan aktarımı keserdi.
                Ok(SendOutcome::Cancelled) if node.pauses.lock().unwrap().remove(&tid) => {
                    node.send_notice(&peer, Message::PauseNotice { device: node.device_info(), transfer_id: tid.clone() }).await;
                    node.finish_transfer(&tid, Status::Paused, None);
                }
                Ok(SendOutcome::Cancelled) => {
                    // Alıcıda bekleyen yarım dosyalar boşuna yer kaplamasın.
                    if resumable {
                        node.send_notice(&peer, Message::Abort { device: node.device_info(), transfer_id: tid.clone() }).await;
                    }
                    node.finish_transfer(&tid, Status::Cancelled, None);
                }
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

    /// Ayarlardaki hız sınırını tüm gönderimler için ortak uygular.
    async fn throttle(&self, n: usize) {
        let limit = self.settings.lock().unwrap().speed_limit_mb as f64 * 1024.0 * 1024.0;
        if limit <= 0.0 {
            return;
        }
        let wait = {
            let mut b = self.bucket.lock().unwrap();
            let now = Instant::now();
            // En fazla çeyrek saniyelik birikim: duraklamadan sonra ani patlama olmasın.
            let tokens = (b.1 + now.duration_since(b.0).as_secs_f64() * limit).min(limit / 4.0) - n as f64;
            *b = (now, tokens);
            if tokens < 0.0 { Duration::from_secs_f64(-tokens / limit) } else { Duration::ZERO }
        };
        if !wait.is_zero() {
            sleep(wait).await;
        }
    }

    /// Alıcıya kısa bir bildirim (iptal/duraklatma) gönderir.
    async fn send_notice(&self, peer: &Peer, msg: Message) {
        let Ok(mut s) = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await else { return };
        if write_msg(&mut s, &msg).await.is_ok() {
            let _ = timeout(Duration::from_secs(10), read_msg(&mut s)).await;
        }
        let _ = s.shutdown().await;
    }

    /// Adresleri sırayla dener; ilk TCP bağlantısı kurulan adres üzerinden TLS el sıkışması yapar.
    async fn connect(&self, addrs: &[SocketAddr], expected_fp: Option<String>) -> Result<tokio_rustls::client::TlsStream<TcpStream>, String> {
        // Adresler sırayla ama beklemeden denenir: ilki 250 ms'de bağlanamazsa sıradaki de
        // başlar, ilk başaran kazanır. Eskiden ulaşılamayan her adres 3 sn bekletiyordu.
        let per_try = Duration::from_secs(6);
        let mut last_err = "Adres yok".to_string();
        let mut tries = tokio::task::JoinSet::new();
        for (i, addr) in addrs.iter().copied().enumerate() {
            tries.spawn(async move {
                sleep(Duration::from_millis(250 * i as u64)).await;
                match timeout(per_try, TcpStream::connect(addr)).await {
                    Ok(Ok(s)) => Ok((addr, s)),
                    Ok(Err(e)) => Err(format!("Bağlanılamadı ({addr}): {e}")),
                    Err(_) => Err(format!("Bağlantı zaman aşımına uğradı ({addr})")),
                }
            });
        }
        let mut tcp = None;
        while let Some(res) = tries.join_next().await {
            match res {
                Ok(Ok((addr, s))) => {
                    if let Some(fp) = &expected_fp {
                        self.good_addr.lock().unwrap().insert(fp.clone(), addr);
                    }
                    tcp = Some(s);
                    break;
                }
                Ok(Err(e)) => last_err = e,
                Err(_) => {}
            }
        }
        tries.abort_all();
        let tcp = tcp.ok_or(last_err)?;
        tcp.set_nodelay(true).ok();
        set_keepalive(&tcp);
        let connector = TlsConnector::from(tls::client_config(&self.identity, expected_fp)?);
        timeout(Duration::from_secs(10), connector.connect(tls::server_name(), tcp))
            .await
            .map_err(|_| "Güvenli bağlantı kurulamadı (zaman aşımı)".to_string())?
            .map_err(|e| format!("Güvenli bağlantı kurulamadı: {e}"))
    }

    /// Tek bir gönderim denemesi. `resumable`: alıcı kaldığı yerden devamı kabul etti.
    async fn run_send(
        self: &Arc<Self>,
        id: &str,
        peer: &Peer,
        entries: Arc<Vec<(PathBuf, FileEntry)>>,
        token: &CancellationToken,
        resumable: &mut bool,
        retrying: bool,
    ) -> Result<SendOutcome, SendError> {
        // Bağlantıyla ilgili, yeniden denenebilir hata.
        let lost = |msg: String| SendError { msg, retry: true };
        let addrs = self.candidates(peer);
        let stream = tokio::select! {
            r = self.connect(&addrs, Some(peer.fingerprint.clone())) => r.map_err(lost)?,
            _ = token.cancelled() => return Ok(SendOutcome::Cancelled),
        };
        let (mut rd, mut wr) = tokio::io::split(stream);

        let files: Vec<FileEntry> = entries.iter().map(|e| e.1.clone()).collect();
        let total = files.iter().map(|f| f.size).sum();
        let group_id = self.transfers.lock().unwrap().get(id).and_then(|t| t.group_id.clone());
        let offer = Message::Offer {
            transfer_id: id.to_string(),
            device: self.device_info(),
            files,
            total,
            features: vec![FEATURE_V2.into()],
            group_id,
        };
        write_msg(&mut wr, &offer).await.map_err(|e| lost(format!("Teklif gönderilemedi: {e}")))?;

        // Yeniden denemede alıcı onay sormaz ama yarım dosyaları özetlemesi sürebilir.
        let accept_wait = DECISION_TIMEOUT + Duration::from_secs(if retrying { 300 } else { 10 });
        let asked = Instant::now();
        let reply = tokio::select! {
            r = read_msg(&mut rd) => r.map_err(|_| lost("Alıcı bağlantıyı kapattı".into()))?,
            _ = token.cancelled() => return Ok(SendOutcome::Cancelled),
            _ = sleep(accept_wait) => return Err("Alıcı yanıt vermedi".to_string().into()),
        };
        let (features, offsets) = match reply {
            Message::Accept { features, offsets } => {
                // Karşı taraf elle onayladıysa (anında kabul = güvenilir cihaz) gönderene haber ver.
                if !retrying && asked.elapsed() > Duration::from_secs(2) {
                    let title = self.transfers.lock().unwrap().get(id).map(|t| t.title.clone()).unwrap_or_default();
                    self.notify(&format!("{} kabul etti", peer.name), &format!("{title} gönderiliyor"), Some(id));
                    self.emit("accepted", json!({ "transferId": id, "peerName": peer.name, "title": title }));
                }
                (features, offsets)
            }
            Message::Decline { reason } => return Ok(SendOutcome::Declined(reason)),
            _ => return Err("Beklenmeyen yanıt".to_string().into()),
        };
        let resume = features.iter().any(|f| f == FEATURE_V2);
        let offsets = if resume {
            if offsets.len() != entries.len() || offsets.iter().zip(entries.iter()).any(|(o, e)| *o > e.1.size) {
                return Err("Alıcı geçersiz devam noktası bildirdi".to_string().into());
            }
            *resumable = true;
            offsets
        } else {
            vec![0; entries.len()]
        };
        let already: u64 = offsets.iter().sum();

        self.update_transfer(id, |t| {
            t.status = Status::Active;
            t.sample = None;
            t.done = already;
            t.error = None;
            t.resumed |= already > 0;
            t.resumable = resume;
        });

        // Diskten okuma, özet ve sıkıştırma ayrı iş parçacığında; bu görev yalnızca ağa yazar.
        let jobs: Vec<pipeline::ReadJob> = entries
            .iter()
            .zip(&offsets)
            .filter(|((_, e), o)| !(resume && e.size > 0 && **o >= e.size)) // alıcıda zaten tamam
            .map(|((src, e), o)| pipeline::ReadJob { src: src.clone(), name: e.path.clone(), size: e.size, offset: *o })
            .collect();
        let names: Vec<String> = jobs.iter().map(|j| j.name.clone()).collect();
        let mode = pipeline::Mode { v2: resume, compress: self.settings().compress };
        let mut pieces = pipeline::spawn_reader(jobs, mode);

        let writer = async {
            // Küçük dosyaların baytları tek tek paket olmasın: tampon dolunca ya da okuyucu
            // beklerken (kanal boşken) ağa boşaltılır.
            let mut out = tokio::io::BufWriter::with_capacity(512 * 1024, &mut wr);
            let stalled = || lost("Bağlantı koptu".into());
            loop {
                let piece = match pieces.try_recv() {
                    Ok(p) => p,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        timeout(STALL_TIMEOUT, out.flush()).await.map_err(|_| stalled())?.map_err(|_| stalled())?;
                        match pieces.recv().await {
                            Some(p) => p,
                            None => break,
                        }
                    }
                };
                match piece.map_err(SendError::from)? {
                    pipeline::Piece::File(i) => self.set_current_file(id, &names[i]),
                    pipeline::Piece::Bytes { data, progress } => {
                        timeout(STALL_TIMEOUT, out.write_all(&data)).await.map_err(|_| stalled())?.map_err(|_| stalled())?;
                        if progress > 0 {
                            self.add_progress(id, progress);
                        }
                        self.throttle(data.len()).await;
                    }
                }
            }
            timeout(STALL_TIMEOUT, out.flush()).await.map_err(|_| stalled())?.map_err(|_| stalled())?;
            Ok::<(), SendError>(())
        };
        let reader = read_msg(&mut rd);
        tokio::pin!(writer);
        tokio::pin!(reader);

        // Alıcının veri sırasında gönderdiği yanıt.
        let reply_outcome = |m: std::io::Result<Message>, fallback: &str| match m {
            Ok(Message::Done) => Ok(SendOutcome::Done),
            Ok(Message::Cancel) => Ok(SendOutcome::PeerCancelled),
            // Alıcıda hata (ör. doğrulama başarısız): nedeni göster, yeniden dene.
            Ok(Message::Decline { reason }) => Err(lost(format!("Alıcı: {}", sanitize_text(&reason, 200)))),
            _ => Err(lost(fallback.to_string())),
        };

        tokio::select! {
            w = &mut writer => {
                if let Err(e) = w {
                    // Yazma hatası alıcının iptalinden ya da hatasından kaynaklanıyor olabilir.
                    return match timeout(Duration::from_secs(2), &mut reader).await {
                        Ok(m @ Ok(Message::Cancel | Message::Decline { .. })) => reply_outcome(m, ""),
                        _ => Err(e),
                    };
                }
                tokio::select! {
                    m = &mut reader => reply_outcome(m, "Alıcı onaylamadan bağlantı kapandı"),
                    _ = token.cancelled() => Ok(SendOutcome::Cancelled),
                    _ = sleep(Duration::from_secs(120)) => Err(lost("Alıcıdan onay gelmedi".into())),
                }
            }
            m = &mut reader => reply_outcome(m, "Bağlantı koptu"),
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

/// Gelen gönderim teklifi.
struct IncomingOffer {
    transfer_id: String,
    /// Gönderen kaldığı yerden devamı ve BLAKE3 doğrulamasını destekliyor.
    resume: bool,
    files: Vec<FileEntry>,
    total: u64,
    group_id: Option<String>,
}

/// `dosya.txt` → `dosya.txt.ffpart`
fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".ffpart");
    target.with_file_name(name)
}

/// Yarım dosyanın ilk `len` baytının BLAKE3 özet durumunu hesaplar.
async fn hash_prefix(path: PathBuf, len: u64) -> std::io::Result<blake3::Hasher> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?.take(len);
        let mut h = blake3::Hasher::new();
        let copied = std::io::copy(&mut f, &mut h)?;
        if copied != len {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "yarım dosya kısaldı"));
        }
        Ok(h)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Karşı taraf habersiz kaybolursa (kablo çekildi, uyku) bağlantının kopukluğu fark edilsin.
fn set_keepalive(tcp: &TcpStream) {
    let ka = socket2::TcpKeepalive::new().with_time(Duration::from_secs(15)).with_interval(Duration::from_secs(5));
    let _ = socket2::SockRef::from(tcp).set_tcp_keepalive(&ka);
}

/// Aktarım geçmişini yükler. Uygulama kapanırken süren aktarımlar "yarıda kaldı" olur.
fn load_history(path: &Path) -> HashMap<String, Transfer> {
    let list: Vec<Transfer> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    list.into_iter()
        .map(|mut t| {
            if !t.is_finished() {
                t.status = Status::Failed;
                t.error = Some("Uygulama kapandığı için yarıda kaldı".into());
                t.speed = 0.0;
                t.current_file = None;
            }
            (t.id.clone(), t)
        })
        .collect()
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

async fn remove_with_retry(p: &Path) {
    for _ in 0..10 {
        match tokio::fs::remove_file(p).await {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => sleep(Duration::from_millis(100)).await,
        }
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

/// Kullanıcının seçtiği yolları tarar: (dosya listesi, başlık).
/// Sohbette önizlenen resimlerin en büyük boyutu (daha büyükleri normal dosya balonu olarak kalır).
/// Bu boyuta kadar tek bir resim dosyası alıcıya sorulmadan kabul edilir.
const IMAGE_PREVIEW_MAX: u64 = 40 * 1024 * 1024;
/// Bir cihazdan dakikada sormadan kabul edilecek en fazla resim.
const IMAGE_RATE_MAX: usize = 20;
const IMAGE_RATE_WINDOW: Duration = Duration::from_secs(60);

/// Teklif sormadan alınabilecek tek bir resim mi (klasör içinde değil, boş değil, boyut sınırında)?
fn is_auto_image(files: &[(Vec<String>, u64)]) -> bool {
    matches!(files, [(parts, size)] if parts.len() == 1 && *size > 0 && *size <= IMAGE_PREVIEW_MAX && is_image_path(Path::new(&parts[0])))
}

/// Kayan pencereli sayaç: son pencerede `max`'tan az olay varsa yenisini sayar ve true döner.
fn rate_ok(map: &Mutex<HashMap<String, VecDeque<Instant>>>, fp: &str, max: usize, window: Duration) -> bool {
    let mut map = map.lock().unwrap();
    let q = map.entry(fp.to_string()).or_default();
    let now = Instant::now();
    while q.front().is_some_and(|t| now.duration_since(*t) > window) {
        q.pop_front();
    }
    if q.len() >= max {
        return false;
    }
    q.push_back(now);
    true
}

/// Sohbette resim olarak gösterilecek uzantılar (WebView'in güvenle çözebildikleri; SVG hariç).
fn is_image_path(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "jfif" | "gif" | "webp" | "bmp" | "avif" | "ico")
}

async fn scan_sources(paths: &[String]) -> Result<(Arc<Vec<(PathBuf, FileEntry)>>, String), String> {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
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
    Ok((Arc::new(entries), title))
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
            out.push((p.clone(), FileEntry { path: base, size: meta.len(), mtime: mtime_ms(&meta) }));
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
                out.push((entry.path().to_path_buf(), FileEntry { path: rel_str, size: m.len(), mtime: mtime_ms(&m) }));
            }
        }
    }
    Ok(out)
}

fn mtime_ms(m: &std::fs::Metadata) -> Option<u64> {
    m.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

fn unique_name(dir: &Path, name: &str, is_file: bool, used: &HashSet<String>) -> String {
    // Yarım (.ffpart) dosyası olan ad da dolu sayılır: o dosya kaldığı yerden tamamlanabilir.
    let taken = |n: &str| {
        dir.join(n).exists() || dir.join(format!("{n}.ffpart")).exists() || used.contains(&n.to_lowercase())
    };
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

    /// Tek bir resim, güvenilmeyen cihazdan bile sormadan alınır ve sohbet mesajı gibi bildirilir;
    /// resim olmayan dosya ve resim içeren klasör yine onay ister.
    #[tokio::test]
    async fn single_image_arrives_without_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let ev2 = events.clone();
        let emitter: Emitter = Arc::new(move |ev, payload| {
            if ev == "incoming" || ev == "chat" {
                ev2.lock().unwrap().push((ev.to_string(), payload.clone()));
            }
        });
        let alice = make_node(tmp.path(), "alice").await;
        let bob = Node::new(&tmp.path().join("bob/config"), &tmp.path().join("bob/dl"), emitter).unwrap();
        bob.start_server(0).await.unwrap();
        introduce(&alice, &bob);
        let asked = || events.lock().unwrap().iter().filter(|e| e.0 == "incoming").count();

        let img = tmp.path().join("Foto.JPG");
        std::fs::write(&img, b"\xff\xd8\xff\xe0 resim").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![img.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Done);
        assert_eq!(asked(), 0, "resim için onay sorulmamalı");
        let dl = PathBuf::from(bob.settings().download_dir);
        assert!(dl.join("Foto.JPG").exists());
        // Gelen mesaj gibi: okunmamış sayılır ve "incoming" olarak bildirilir.
        let fp = alice.identity.fingerprint.clone();
        assert!(events.lock().unwrap().iter().any(|e| e.0 == "chat" && e.1["incoming"] == true));
        assert_eq!(bob.chats.lock().unwrap().conversation_meta(&fp).map(|m| m.1), Some(1));

        // Resim olmayan dosya: onay sorulur (yanıt verilmez, gönderen vazgeçer).
        let doc = tmp.path().join("rapor.pdf");
        std::fs::write(&doc, "x").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![doc.to_string_lossy().into()]).await.unwrap();
        for _ in 0..100 {
            if asked() > 0 {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(asked(), 1, "resim olmayan dosya onay istemeli");
        alice.cancel_transfer(&id);

        // Klasör içindeki resim de onay ister.
        let dir = tmp.path().join("Albüm");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"\x89PNG").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![dir.to_string_lossy().into()]).await.unwrap();
        for _ in 0..100 {
            if asked() > 1 {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(asked(), 2, "klasör onay istemeli");
        alice.cancel_transfer(&id);
    }

    #[test]
    fn auto_image_rules_and_rate_limit() {
        let one = |name: &str, size: u64| vec![(vec![name.to_string()], size)];
        assert!(is_auto_image(&one("a.png", 10)));
        assert!(is_auto_image(&one("B.WEBP", IMAGE_PREVIEW_MAX)));
        assert!(!is_auto_image(&one("a.png", IMAGE_PREVIEW_MAX + 1)));
        assert!(!is_auto_image(&one("a.png", 0)));
        assert!(!is_auto_image(&one("a.svg", 10)));
        assert!(!is_auto_image(&one("a.png.exe", 10)));
        assert!(!is_auto_image(&[(vec!["k".into(), "a.png".into()], 10)]));
        assert!(!is_auto_image(&[(vec!["a.png".into()], 10), (vec!["b.png".into()], 10)]));

        let map = Mutex::new(HashMap::new());
        for _ in 0..IMAGE_RATE_MAX {
            assert!(rate_ok(&map, "x", IMAGE_RATE_MAX, IMAGE_RATE_WINDOW));
        }
        assert!(!rate_ok(&map, "x", IMAGE_RATE_MAX, IMAGE_RATE_WINDOW), "sınır aşılınca yine sorulmalı");
        assert!(rate_ok(&map, "y", IMAGE_RATE_MAX, IMAGE_RATE_WINDOW), "sayaç cihaz başına");
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

        // Alıcı tarafı başarısız olarak bitirmeli; gönderenin iptal bildirimiyle yarım dosya silinmeli.
        let rid = loop {
            if let Some(t) = bob.transfers().into_iter().next() {
                break t.id;
            }
            sleep(Duration::from_millis(20)).await;
        };
        let rt = wait_finished(&bob, &rid).await;
        assert_ne!(rt.status, Status::Done);
        let dl = PathBuf::from(bob.settings().download_dir);
        let leftovers = || std::fs::read_dir(&dl).unwrap().flatten().count();
        wait_until(|| leftovers() == 0).await;
        wait_until(|| bob.transfers().iter().any(|t| t.id == rid && t.status == Status::Cancelled)).await;
        assert!(bob.resume.lock().unwrap().find_by_id(&alice.identity.fingerprint, &id).is_none());
    }

    /// Büyük bir dosyanın gönderimi başlar, `min` bayt alınınca alıcıdaki bağlantı kesilir.
    async fn start_and_interrupt(alice: &Arc<Node>, bob: &Arc<Node>, src: &Path, min: u64) -> (String, String) {
        let id = alice.send_one(&bob.identity.id, vec![src.to_string_lossy().into()]).await.unwrap();
        let rid = loop {
            if let Some(t) = bob.transfers().into_iter().find(|t| t.direction == Direction::Receive && t.done >= min) {
                break t.id;
            }
            sleep(Duration::from_millis(2)).await;
        };
        bob.interrupt_receive(&rid).await;
        (id, rid)
    }

    fn pattern(len: usize, seed: u32) -> Vec<u8> {
        (0..len as u32).map(|i| (i.wrapping_add(seed).wrapping_mul(2654435761) >> 11) as u8).collect()
    }

    #[tokio::test]
    async fn interrupted_transfer_resumes_where_it_left_off() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);

        let src = tmp.path().join("video.bin");
        let data = pattern(160 * 1024 * 1024, 7);
        std::fs::write(&src, &data).unwrap();

        let (id, rid) = start_and_interrupt(&alice, &bob, &src, 8 * 1024 * 1024).await;
        // Gönderen kopukluğu görür, kısa süre sonra kendiliğinden yeniden bağlanır; alıcı onay sormaz.
        let t = wait_finished(&alice, &id).await;
        assert_eq!(t.status, Status::Done, "hata: {:?}", t.error);
        assert!(t.resumed, "baştan değil, kaldığı yerden devam etmeliydi");

        let dl = PathBuf::from(bob.settings().download_dir);
        assert_eq!(std::fs::read(dl.join("video.bin")).unwrap(), data);
        assert!(!dl.join("video.bin.ffpart").exists());
        // Alıcıda tek satır: aynı kayıt güncellendi.
        let recv: Vec<_> = bob.transfers().into_iter().filter(|t| t.direction == Direction::Receive).collect();
        assert_eq!(recv.len(), 1);
        assert_eq!(recv[0].id, rid);
        assert_eq!(recv[0].status, Status::Done);
        assert!(recv[0].resumed);
        // Başarıdan sonra devam kaydı kalmaz.
        assert!(bob.resume.lock().unwrap().find_by_id(&alice.identity.fingerprint, &id).is_none());
    }

    #[tokio::test]
    async fn pause_and_resume_continues_where_it_left_off() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);
        // Hız sınırı: aktarım duraklatılmadan bitmesin.
        alice.update_settings(|s| s.speed_limit_mb = 40);
        let src = tmp.path().join("yedek.bin");
        let data = pattern(24 * 1024 * 1024, 11);
        std::fs::write(&src, &data).unwrap();

        let id = alice.send_one(&bob.identity.id, vec![src.to_string_lossy().into()]).await.unwrap();
        wait_until(|| alice.transfers().iter().any(|t| t.id == id && t.resumable && t.done > 2 * 1024 * 1024)).await;
        alice.pause_transfer(&id).unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Paused);
        wait_until(|| bob.transfers().iter().any(|t| t.status == Status::Paused)).await;
        let dl = PathBuf::from(bob.settings().download_dir);
        assert!(dl.join("yedek.bin.ffpart").exists(), "duraklatınca yarım dosya korunmalı");
        assert!(alice.pause_transfer(&id).is_err(), "biten aktarım tekrar duraklatılamaz");

        alice.update_settings(|s| s.speed_limit_mb = 0);
        assert_eq!(alice.retry_transfer(&id).await.unwrap(), id, "devam aynı kayıtla sürmeli");
        let t = wait_finished(&alice, &id).await;
        assert_eq!(t.status, Status::Done, "{:?}", t.error);
        assert!(t.resumed);
        assert!(std::fs::read(dl.join("yedek.bin")).unwrap() == data);
    }

    #[tokio::test]
    async fn speed_limit_is_respected() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);
        alice.update_settings(|s| s.speed_limit_mb = 8);
        let src = tmp.path().join("x.bin");
        // Sıkıştırılamayan veri: sınır ağdan geçen bayta uygulanır.
        std::fs::write(&src, noise(12 * 1024 * 1024, 5)).unwrap();
        let start = Instant::now();
        let id = alice.send_one(&bob.identity.id, vec![src.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Done);
        // 12 MB / 8 MB/sn ≈ 1,5 sn (çeyrek saniyelik ilk birikim düşülür).
        let secs = start.elapsed().as_secs_f64();
        assert!(secs > 1.1, "hız sınırı uygulanmadı: {secs:.2} sn");
    }

    #[tokio::test]
    async fn corrupted_partial_file_is_detected_and_resent() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let e2 = errors.clone();
        let emitter: Emitter = Arc::new(move |ev, payload| {
            if let (true, Some(e)) = (ev == "transfer", payload["error"].as_str()) {
                e2.lock().unwrap().push(e.to_string());
            }
        });
        let bob = Node::new(&tmp.path().join("bob/config"), &tmp.path().join("bob/dl"), emitter).unwrap();
        bob.start_server(0).await.unwrap();
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);

        let src = tmp.path().join("arsiv.zip");
        let data = pattern(96 * 1024 * 1024, 3);
        std::fs::write(&src, &data).unwrap();

        let (id, rid) = start_and_interrupt(&alice, &bob, &src, 4 * 1024 * 1024).await;
        wait_finished(&bob, &rid).await;
        // Yeniden denemeden önce alıcıdaki yarım dosya bozulur (disk hatası benzetimi).
        let part = PathBuf::from(bob.settings().download_dir).join("arsiv.zip.ffpart");
        let mut bytes = std::fs::read(&part).unwrap();
        bytes[100] ^= 0xFF;
        std::fs::write(&part, bytes).unwrap();

        // Özet tutmaz → o dosya silinir → gönderen yeniden dener → baştan, doğru şekilde gelir.
        let t = wait_finished(&alice, &id).await;
        assert_eq!(t.status, Status::Done, "hata: {:?}", t.error);
        let got = std::fs::read(PathBuf::from(bob.settings().download_dir).join("arsiv.zip")).unwrap();
        assert!(got == data, "bozuk dosya doğrulamadan geçti");
        assert!(errors.lock().unwrap().iter().any(|e| e.contains("doğrulama başarısız")), "bozukluk fark edilmedi");
    }

    /// Eski sürüm (özellik bildirmeyen) gönderici: alıcı eski protokolle, özetsiz kabul eder.
    #[tokio::test]
    async fn legacy_sender_without_features_still_works() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bob.port());
        let mut s = alice.connect(&[addr], Some(bob.identity.fingerprint.clone())).await.unwrap();
        let files = vec![FileEntry { path: "eski.txt".into(), size: 5, mtime: None }];
        let offer = Message::Offer { transfer_id: "t-eski".into(), device: alice.device_info(), files, total: 5, features: vec![], group_id: None };
        write_msg(&mut s, &offer).await.unwrap();
        match read_msg(&mut s).await.unwrap() {
            Message::Accept { features, offsets } => assert!(features.is_empty() && offsets.is_empty()),
            other => panic!("kabul beklenirdi: {other:?}"),
        }
        s.write_all(b"merha").await.unwrap();
        s.flush().await.unwrap();
        assert!(matches!(read_msg(&mut s).await.unwrap(), Message::Done));
        let dl = PathBuf::from(bob.settings().download_dir);
        assert_eq!(std::fs::read(dl.join("eski.txt")).unwrap(), b"merha");
        assert!(bob.resume.lock().unwrap().find_by_id(&alice.identity.fingerprint, "t-eski").is_none());
    }

    #[tokio::test]
    async fn history_survives_restart_and_can_be_resent() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);
        let file = tmp.path().join("teklif.pdf");
        std::fs::write(&file, b"pdf").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![file.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Done);

        // Aynı ayar klasörüyle yeniden açılınca geçmiş yerinde.
        let cfg = tmp.path().join("alice/config");
        let reopened = Node::new(&cfg, &tmp.path().join("alice/downloads"), quiet()).unwrap();
        let hist = reopened.transfers();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].status, Status::Done);
        assert_eq!(hist[0].sources, vec![file.to_string_lossy().to_string()]);

        // "Tekrar gönder": biten gönderim yeni bir kayıtla tekrar gider.
        let again = alice.retry_transfer(&id).await.unwrap();
        assert_ne!(again, id);
        assert_eq!(wait_finished(&alice, &again).await.status, Status::Done);
        assert!(PathBuf::from(bob.settings().download_dir).join("teklif (1).pdf").exists());

        // Tek kayıt silinebilir; alıcı kaydı tekrar gönderilemez.
        let bob_rid = bob.transfers()[0].id.clone();
        assert!(bob.retry_transfer(&bob_rid).await.is_err());
        assert_eq!(alice.remove_transfer(&id).len(), 1);
    }

    /// Hız ölçümü (localhost: ağ sınırı yok, CPU/disk/dosya başına ek yükü gösterir).
    /// Çalıştırmak için: `cargo test --release bench_ -- --ignored --nocapture --test-threads=1`
    async fn bench_run(name: &str, prepare: impl FnOnce(&Path) -> Vec<String>) {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce(&alice, &bob);
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let paths = prepare(&src);
        // Yeni oluşturulan kaynak dosyaları virüs tarayıcısı arka planda tarar; ölçümü bozmasın.
        if let Ok(w) = std::env::var("FF_BENCH_SETTLE") {
            sleep(Duration::from_secs(w.parse().unwrap_or(0))).await;
        }
        let start = Instant::now();
        let id = alice.send_paths(&[bob.identity.id.clone()], paths).await.unwrap().remove(0);
        let mut t;
        loop {
            t = alice.transfers().into_iter().find(|t| t.id == id).unwrap();
            if t.is_finished() {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        let secs = start.elapsed().as_secs_f64();
        assert_eq!(t.status, Status::Done, "{:?}", t.error);
        println!(
            "[bench] {name}: {} dosya, {} → {:.2} sn, {:.0} MB/sn, {:.0} dosya/sn",
            t.file_count,
            fmt_bytes(t.total),
            secs,
            t.total as f64 / secs / 1e6,
            t.file_count as f64 / secs
        );
    }

    fn noise(len: usize, mut x: u64) -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        while v.len() < len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.truncate(len);
        v
    }

    /// Diskin dosya oluşturma tavanı: tek iş parçacığı ve paralel.
    #[test]
    #[ignore]
    fn bench_disk_create_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let direct = std::env::var("FF_DIRECT").is_ok();
        for threads in [1usize, 8] {
            let dir = tmp.path().join(format!("t{threads}"));
            for d in 0..200 {
                std::fs::create_dir_all(dir.join(format!("d{d}"))).unwrap();
            }
            let start = Instant::now();
            std::thread::scope(|s| {
                for t in 0..threads {
                    let dir = &dir;
                    s.spawn(move || {
                        for i in (t..20_000).step_by(threads) {
                            let target = dir.join(format!("d{}", i % 200)).join(format!("f{i}.txt"));
                            let data = noise(4096, i as u64 + 1); // benzersiz içerik
                            if direct {
                                std::fs::write(&target, &data).unwrap();
                            } else {
                                let part = target.with_extension("txt.ffpart");
                                std::fs::write(&part, &data).unwrap();
                                std::fs::rename(&part, &target).unwrap();
                            }
                        }
                    });
                }
            });
            println!("[bench] disk tavanı, {threads} iş parçacığı: {:.0} dosya/sn", 20_000.0 / start.elapsed().as_secs_f64());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn bench_big_file() {
        bench_run("tek büyük dosya", |dir| {
            let p = dir.join("buyuk.bin");
            std::fs::write(&p, noise(1024 * 1024 * 1024, 42)).unwrap();
            vec![p.to_string_lossy().into()]
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn bench_small_files() {
        bench_run("çok küçük dosya", |dir| {
            let root = dir.join("proje");
            for i in 0..20_000u64 {
                let d = root.join(format!("d{}", i % 200));
                std::fs::create_dir_all(&d).unwrap();
                // FF_BENCH_SAME: aynı içerik (virüs tarayıcısı önbellekten geçer → saf yazılım hızı).
                let seed = if std::env::var("FF_BENCH_SAME").is_ok() { 1 } else { i + 1 };
                std::fs::write(d.join(format!("f{i}.txt")), noise(4096, seed)).unwrap();
            }
            vec![root.to_string_lossy().into()]
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn bench_text_file() {
        bench_run("sıkıştırılabilir metin", |dir| {
            let p = dir.join("log.txt");
            let mut s = String::new();
            let mut i = 0u64;
            while s.len() < 512 * 1024 * 1024 {
                s.push_str(&format!("2026-11-05 14:{:02}:{:02} INFO istek={} kullanici=ahmet durum=200 sure={}ms\n", i % 60, i % 59, i, i % 977));
                i += 1;
            }
            std::fs::write(&p, s).unwrap();
            vec![p.to_string_lossy().into()]
        })
        .await;
    }

    #[test]
    fn unfinished_history_entries_become_failed_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("transfers.json");
        let mut t = Transfer::new("x".into(), Direction::Receive, "p".into(), "P".into(), "a".into(), 1, 10, Status::Active);
        t.done = 5;
        std::fs::write(&path, serde_json::to_vec(&vec![t]).unwrap()).unwrap();
        let loaded = load_history(&path);
        assert_eq!(loaded["x"].status, Status::Failed);
        assert!(loaded["x"].error.is_some());
        assert!(load_history(&tmp.path().join("yok.json")).is_empty());
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

        let sent = alice.chat_send(&bob.identity.id, "  Selam Bob!\r\nNasılsın?  ", None).unwrap();
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
        assert!(alice.chat_send(&bob2.identity.id, " \u{200B} ", None).is_err());
        assert!(alice.chat_send(&bob2.identity.id, &"a".repeat(MAX_CHAT_CHARS + 1), None).is_err());
    }

    #[tokio::test]
    async fn chat_rejects_blocked_and_spoofed_text_is_cleaned() {
        let tmp = tempfile::tempdir().unwrap();
        let (alice, bob, _events) = chat_pair(tmp.path()).await;
        let afp = alice.identity.fingerprint.clone();
        let bfp = bob.identity.fingerprint.clone();

        // Ağdan doğrudan gelen tehlikeli karakterler temizlenir.
        assert!(bob
            .accept_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), "m1", "fatura\u{202E}fdp.exe\u{0000}", None)
            .is_ok());
        assert_eq!(bob.chat_history(&afp)[0].text, "faturafdp.exe");
        // Aynı kimlikli mesaj iki kez eklenmez.
        assert!(bob.accept_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), "m1", "x", None).is_ok());
        assert_eq!(bob.chat_history(&afp).len(), 1);
        // Geçersiz kimlik reddedilir.
        assert_eq!(bob.accept_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), "../x", "y", None), Err("invalid"));

        // Engellenen cihazın mesajı reddedilir, gönderen "başarısız" görür.
        bob.set_blocked(&afp, "alice", true);
        alice.chat_send(&bob.identity.id, "orada mısın?", None).unwrap();
        wait_until(|| last_status(&alice, &bfp) == Some(MsgStatus::Failed)).await;
        assert_eq!(bob.chat_history(&afp).len(), 1);
        // Engellenen cihazın dosya teklifi de reddedilir.
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![file.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Declined);

        // Engeli kaldırınca "tekrar dene" çalışır.
        bob.set_blocked(&afp, "alice", false);
        // (Reddedilen dosya da sohbette balon olarak durur; yeniden denenecek olan metin mesajıdır.)
        let failed_id = alice.chat_history(&bfp).into_iter().rev().find(|m| m.file.is_none()).unwrap().id;
        alice.chat_retry(&bfp, &failed_id).unwrap();
        wait_until(|| alice.chat_history(&bfp).iter().any(|m| m.id == failed_id && m.status == MsgStatus::Sent)).await;
        assert_eq!(bob.chat_history(&afp).iter().filter(|m| m.file.is_none()).count(), 2);
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
        alice.group_send(&key, "Merhaba ekip", None).unwrap();
        bob.group_send(&key, "Selam!", None).unwrap();
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
        alice.group_send(&key, "Carol yokken yazıldı", None).unwrap();
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

    /// Regresyon: "yazıyor…" senkron bağlamdan (tokio çalışma ortamı yok) çağrılınca
    /// panik → sürüm derlemesinde uygulama kapanıyordu.
    #[test]
    fn send_typing_outside_runtime_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let node = Node::new(&tmp.path().join("cfg"), &tmp.path().join("dl"), quiet()).unwrap();
        let fp = "ab".repeat(32);
        let dev = DeviceInfo { id: "p1".into(), name: "Ahmet".into(), port: 1, fingerprint: fp.clone(), os: "windows".into(), ..Default::default() };
        node.upsert_peer(dev, IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        node.send_typing(&fp);
    }

    /// Başka bir sertifika, görünür durumdaki bir cihazın kimliğini (id) ele geçiremez.
    #[test]
    fn peer_id_cannot_be_hijacked_by_another_certificate() {
        let tmp = tempfile::tempdir().unwrap();
        let node = Node::new(&tmp.path().join("cfg"), &tmp.path().join("dl"), quiet()).unwrap();
        let bob = DeviceInfo { id: "bob-id".into(), name: "Bob".into(), port: 1, fingerprint: "b".repeat(64), os: "windows".into(), ..Default::default() };
        node.upsert_peer(bob.clone(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)), None);
        let fake = DeviceInfo { fingerprint: "e".repeat(64), ..bob.clone() };
        node.upsert_peer(fake, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 66)), None);
        let p = node.peer("bob-id").unwrap();
        assert_eq!(p.fingerprint, "b".repeat(64));
        assert_eq!(p.ip, "192.168.1.20");
        assert!(node.candidates(&p).iter().all(|a| a.ip() != IpAddr::V4(Ipv4Addr::new(192, 168, 1, 66))));
    }

    /// Sohbetteki resim yolu yalnızca mesaj kaydından çözülür: resim olmayan dosya, bitmemiş
    /// alım ve çok dosyalı balon önizlenmez.
    #[tokio::test]
    async fn chat_image_resolves_only_finished_single_images() {
        let tmp = tempfile::tempdir().unwrap();
        let node = Node::new(&tmp.path().join("cfg"), &tmp.path().join("dl"), quiet()).unwrap();
        let fp = "b".repeat(64);
        let png = tmp.path().join("foto.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nveri").unwrap();
        let txt = tmp.path().join("not.txt");
        std::fs::write(&txt, "x").unwrap();
        let bubble = |id: &str, outgoing: bool, title: &str, count: u32, sources: Vec<String>, tid: &str| ChatMessage {
            id: id.into(),
            outgoing,
            file: Some(FileRef {
                title: title.into(),
                count,
                total: 12,
                transfers: vec![FileTransferRef { to: fp.clone(), id: tid.into() }],
                sources,
            }),
            ..Default::default()
        };
        {
            let mut chats = node.chats.lock().unwrap();
            chats.push_quiet(&fp, "Bob", bubble("o1", true, "foto.png", 1, vec![png.to_string_lossy().into()], "x1"));
            chats.push_quiet(&fp, "Bob", bubble("o2", true, "not.txt", 1, vec![txt.to_string_lossy().into()], "x2"));
            chats.push_quiet(&fp, "Bob", bubble("o3", true, "2 dosya", 2, vec![png.to_string_lossy().into()], "x3"));
            chats.push_quiet(&fp, "Bob", bubble("i1", false, "foto.png", 1, vec![], "r1"));
        }
        assert_eq!(node.chat_image(&fp, "o1").await.unwrap(), std::fs::read(&png).unwrap());
        assert!(node.chat_image(&fp, "o2").await.is_err(), "resim olmayan dosya okunmamalı");
        assert!(node.chat_image(&fp, "o3").await.is_err());
        assert!(node.chat_image(&fp, "yok").await.is_err());

        // Alım bitmeden önizleme yok; bitince kaydedilen dosya okunur.
        let mut t = Transfer::new("r1".into(), Direction::Receive, "bob".into(), "Bob".into(), "foto.png".into(), 1, 12, Status::Active);
        t.save_path = Some(png.to_string_lossy().into());
        node.transfers.lock().unwrap().insert("r1".into(), t);
        assert!(node.chat_image(&fp, "i1").await.is_err());
        node.transfers.lock().unwrap().get_mut("r1").unwrap().status = Status::Done;
        assert!(node.chat_image(&fp, "i1").await.is_ok());
        std::fs::remove_file(&png).unwrap();
        assert!(node.chat_image(&fp, "i1").await.is_err(), "silinen dosya");
    }

    /// Yönetici çevrimdışı bir üyeyi çıkardıktan sonra grubu yeniden düzenlerse, çıkarılan
    /// üyeye gidecek bildirim kaybolmamalı (yoksa üye gruptan çıkarıldığını hiç öğrenmez).
    #[test]
    fn removed_member_stays_pending_after_further_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let node = Node::new(&tmp.path().join("cfg"), &tmp.path().join("dl"), quiet()).unwrap();
        let (y, z) = ("c".repeat(64), "d".repeat(64));
        let key = node.group_create_offline("Ekip", &[y.clone(), z.clone()]);
        node.group_edit(&key, None, &[], &[y.clone()]).unwrap();
        node.group_edit(&key, Some("Yeni ad".into()), &[], &[]).unwrap();
        let pending = node.chats.lock().unwrap().conv(&key).unwrap().sync_pending.clone();
        assert!(pending.contains(&y), "çıkarılan üye bildirim listesinden düşmemeli");
        assert!(pending.contains(&z));
    }

    impl Node {
        /// Test: giden kutusu tetiklenmeden grup oluşturur (tokio gerektirmez).
        fn group_create_offline(&self, name: &str, members: &[String]) -> String {
            let me = self.identity.fingerprint.clone();
            let id = uuid::Uuid::new_v4().to_string();
            let key = crate::chat::group_key(&id);
            let mut all = vec![crate::chat::GroupMember { fingerprint: me.clone(), name: "ben".into() }];
            all.extend(members.iter().map(|f| crate::chat::GroupMember { fingerprint: f.clone(), name: f[..4].to_string() }));
            let group = crate::chat::GroupInfo { id, name: name.into(), admin: me, members: all, rev: 1, deleted: false };
            self.chats.lock().unwrap().insert(crate::chat::Conversation { fingerprint: key.clone(), name: name.into(), group: Some(group), ..Default::default() });
            key
        }
    }

    #[tokio::test]
    async fn typing_signal_reaches_chat_and_group_and_wipe_keeps_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let emitter: Emitter = Arc::new(move |name, payload| {
            if name == "typing" {
                ev.lock().unwrap().push(payload);
            }
        });
        let bob = Node::new(&tmp.path().join("bob/config"), &tmp.path().join("bob/dl"), emitter).unwrap();
        bob.update_settings(|s| s.device_name = "bob".into());
        bob.start_server(0).await.unwrap();
        introduce_all(&[&alice, &bob]);
        let (afp, bfp) = (alice.identity.fingerprint.clone(), bob.identity.fingerprint.clone());
        let seen = |key: &str| events.lock().unwrap().iter().any(|e| e["key"] == key && e["fingerprint"] == afp.as_str());

        alice.send_typing(&bfp);
        wait_until(|| seen(&afp)).await;
        assert_eq!(events.lock().unwrap()[0]["name"], "alice");

        let key = alice.group_create("Ekip", &[bfp.clone()]).unwrap().fingerprint;
        wait_until(|| bob.chat_summaries().iter().any(|c| c.fingerprint == key)).await;
        alice.send_typing(&key);
        wait_until(|| seen(&key)).await;

        // Engellenen cihazın sinyali gösterilmez.
        bob.set_blocked(&afp, "alice", true);
        events.lock().unwrap().clear();
        sleep(Duration::from_millis(1000)).await;
        alice.typing_sent.lock().unwrap().clear();
        alice.send_typing(&bfp);
        sleep(Duration::from_millis(600)).await;
        assert!(events.lock().unwrap().is_empty());

        // PIN unutulunca: mesajlar gider, grup üyeliği kalır.
        bob.set_blocked(&afp, "alice", false);
        alice.chat_send(&bob.identity.id, "gizli", None).unwrap();
        wait_until(|| !bob.chat_history(&afp).is_empty()).await;
        bob.wipe_private_data();
        assert!(bob.chat_history(&afp).is_empty());
        assert!(bob.group_of(&key).is_some(), "grup üyeliği korunmalı");
    }

    fn file_bubbles(node: &Node, key: &str) -> Vec<ChatMessage> {
        node.chat_history(key).into_iter().filter(|m| m.file.is_some()).collect()
    }

    #[tokio::test]
    async fn files_appear_in_chat_and_reach_offline_group_members() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        let mallory = make_node(tmp.path(), "mallory").await;
        introduce_all(&[&alice, &bob, &carol, &mallory]);
        for r in [&bob, &carol] {
            r.set_trusted(&alice.identity.fingerprint, "alice", true);
        }
        bob.set_trusted(&mallory.identity.fingerprint, "mallory", true);
        let (afp, bfp, cfp) = (alice.identity.fingerprint.clone(), bob.identity.fingerprint.clone(), carol.identity.fingerprint.clone());

        // Birebir: gönderim iki tarafta da sohbette dosya balonu; okunmamış sayısı artmaz.
        let a = tmp.path().join("a.txt");
        std::fs::write(&a, "a").unwrap();
        let id = alice.send_one(&bob.identity.id, vec![a.to_string_lossy().into()]).await.unwrap();
        assert_eq!(wait_finished(&alice, &id).await.status, Status::Done);
        let out = file_bubbles(&alice, &bfp);
        assert_eq!(out.len(), 1);
        assert!(out[0].outgoing);
        assert_eq!(out[0].file.as_ref().unwrap().transfers[0].id, id);
        wait_until(|| file_bubbles(&bob, &afp).len() == 1).await;
        let inc = &file_bubbles(&bob, &afp)[0];
        assert!(!inc.outgoing && inc.file.as_ref().unwrap().title == "a.txt");
        assert_eq!(bob.chat_summaries().iter().find(|c| c.fingerprint == afp).unwrap().unread, 0);

        // Grup: Carol çevrimdışıyken gönderilen dosya, Carol geri gelince ona da gider.
        let key = alice.group_create("Ekip", &[bfp.clone(), cfp.clone()]).unwrap().fingerprint;
        wait_until(|| bob.chat_summaries().iter().any(|c| c.fingerprint == key)).await;
        wait_until(|| carol.chat_summaries().iter().any(|c| c.fingerprint == key)).await;
        alice.remove_peer(&carol.identity.id);
        let rapor = tmp.path().join("rapor.pdf");
        std::fs::write(&rapor, vec![9u8; 300_000]).unwrap();
        let msg = alice.group_send_files(&key, vec![rapor.to_string_lossy().into()]).await.unwrap();
        assert_eq!(msg.recipients, 2);
        let got = |n: &Node| PathBuf::from(n.settings().download_dir).join("rapor.pdf").exists();
        wait_until(|| got(&bob)).await;
        wait_until(|| file_bubbles(&bob, &key).iter().any(|m| m.sender.as_deref() == Some(afp.as_str()))).await;
        let pending = || alice.chat_history(&key).into_iter().find(|m| m.id == msg.id).unwrap().pending;
        assert_eq!(pending(), vec![cfp.clone()], "Carol için beklemeli");

        introduce(&alice, &carol);
        for _ in 0..60 {
            alice.flush_outbox();
            if got(&carol) {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        assert!(got(&carol), "Carol geri gelince dosyayı almalıydı");
        assert!(pending().is_empty());
        let sent = alice.chat_history(&key).into_iter().find(|m| m.id == msg.id).unwrap();
        assert_eq!(sent.file.unwrap().transfers.len(), 2);
        wait_until(|| file_bubbles(&carol, &key).len() == 1).await;

        // Üye olmayan Mallory grup kimliğini taklit etse de dosyası grupta görünmez.
        let gid = bob.group_of(&key).unwrap().0.id;
        let m = tmp.path().join("sahte.txt");
        std::fs::write(&m, "x").unwrap();
        let (entries, title) = scan_sources(&[m.to_string_lossy().into()]).await.unwrap();
        let peer = mallory.peer(&bob.identity.id).unwrap();
        let mid = mallory.start_send(peer, entries, title, vec![], None, Some(gid));
        assert_eq!(wait_finished(&mallory, &mid).await.status, Status::Done);
        wait_until(|| file_bubbles(&bob, &mallory.identity.fingerprint).len() == 1).await;
        assert!(file_bubbles(&bob, &key).iter().all(|m| m.file.as_ref().unwrap().title != "sahte.txt"));
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
            bob.accept_group_chat(mallory.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), &group.id, "m1", "selam", 0, None),
            Err("sender_not_member")
        );
        // Bilinmeyen grup.
        assert_eq!(
            carol.accept_group_chat(alice.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), &group.id, "m2", "x", 0, None),
            Err("unknown_group")
        );

        // Yönetici Carol'ı ekler, Bob'u çıkarır, adı değiştirir.
        alice
            .group_edit(&key, Some("Yeni Ad".into()), &[carol.identity.fingerprint.clone()], &[bob.identity.fingerprint.clone()])
            .unwrap();
        wait_until(|| carol.group_of(&key).map_or(false, |(g, _)| g.name == "Yeni Ad")).await;
        wait_until(|| bob.group_of(&key).map_or(false, |(_, left)| left)).await;
        assert!(bob.group_send(&key, "hâlâ burada mıyım?", None).is_err());
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
        assert!(carol.group_send(&key, "x", None).is_err());
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

    // ───────────── Oyun testleri ─────────────

    fn game_of(node: &Node, id: &str) -> Option<game::GameView> {
        node.game_list().into_iter().find(|g| g.room.id == id)
    }

    async fn game_ticks(nodes: &[&Arc<Node>], mut cond: impl FnMut() -> bool) {
        for _ in 0..100 {
            if cond() {
                return;
            }
            for n in nodes {
                n.flush_games();
            }
            sleep(Duration::from_millis(60)).await;
        }
        panic!("oyun zamanında eşitlenmedi");
    }

    #[tokio::test]
    async fn chess_invite_play_and_checkmate() {
        use game::{GameAction, RoomStatus};
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let mallory = make_node(tmp.path(), "mallory").await;
        introduce_all(&[&alice, &bob, &mallory]);
        let all = [&alice, &bob, &mallory];
        let (afp, bfp, mfp) = (alice.identity.fingerprint.clone(), bob.identity.fingerprint.clone(), mallory.identity.fingerprint.clone());
        let mv = |m: &str| GameAction::Move { mv: m.into() };

        assert!(alice.game_invite("chess", &[], None).is_err(), "rakip gerekli");
        assert!(alice.game_invite("chess", &[bfp.clone(), mfp.clone()], None).is_err(), "satranç iki kişilik");
        // Alice beyaz.
        let g = alice.game_invite("chess", &[bfp.clone()], Some(0)).unwrap();
        let id = g.room.id.clone();
        game_ticks(&all, || game_of(&bob, &id).is_some_and(|v| v.needs_me && v.room.status == RoomStatus::Invited)).await;
        assert!(game_of(&mallory, &id).is_none(), "davet edilmeyen odayı görmez");
        assert!(bob.game_act(&id, mv("e7e5")).is_err(), "kabul etmeden hamle olmaz");

        bob.game_act(&id, GameAction::Join).unwrap();
        game_ticks(&all, || game_of(&alice, &id).is_some_and(|v| v.room.status == RoomStatus::Active && v.my_turn)).await;
        assert!(alice.game_act(&id, mv("e2e5")).is_err(), "kurala aykırı hamle");
        assert!(!game_of(&bob, &id).unwrap().my_turn);

        let line = [(&alice, "e2e4"), (&bob, "e7e5"), (&alice, "d1h5"), (&bob, "b8c6"), (&alice, "f1c4"), (&bob, "g8f6")];
        for (i, (who, m)) in line.iter().enumerate() {
            game_ticks(&all, || game_of(who, &id).is_some_and(|v| v.my_turn && v.legal.iter().any(|l| l == m))).await;
            who.game_act(&id, mv(m)).unwrap();
            game_ticks(&all, || {
                game_of(&alice, &id).is_some_and(|v| v.room.moves.len() == i + 1) && game_of(&bob, &id).is_some_and(|v| v.room.moves.len() == i + 1)
            })
            .await;
        }

        // Saldırılar: oyuncu olmayan hamle yapamaz; sahip, Bob adına hamle uyduramaz ve geçmişi değiştiremez;
        // başkası sahipmiş gibi oda gönderemez.
        assert!(matches!(alice.accept_game_action(&mfp, &id, mv("h5f7")), Message::GameReply { ok: false, .. }));
        let current = game_of(&bob, &id).unwrap().room;
        let mut forged = current.clone();
        forged.moves.extend(["a2a3".to_string(), "a7a6".to_string()]);
        forged.rev += 1;
        assert!(matches!(bob.accept_game_update(&afp, forged, "alice"), Message::GameReply { ok: false, .. }));
        let mut rewritten = current.clone();
        rewritten.moves[1] = "d7d5".into();
        rewritten.rev += 1;
        assert!(matches!(bob.accept_game_update(&afp, rewritten, "alice"), Message::GameReply { ok: false, .. }));
        let mut resigned = current.clone();
        resigned.status = RoomStatus::Finished;
        resigned.result = Some(game::GameResult { winner: Some(afp.clone()), reason: "resign".into(), by: Some(bfp.clone()) });
        resigned.rev += 1;
        assert!(matches!(bob.accept_game_update(&afp, resigned, "alice"), Message::GameReply { ok: false, .. }));
        let mut stolen = current.clone();
        stolen.rev += 5;
        assert!(matches!(bob.accept_game_update(&mfp, stolen, "mallory"), Message::GameReply { ok: false, .. }));
        assert_eq!(game_of(&bob, &id).unwrap().room.moves, current.moves);

        // Alice mat eder.
        alice.game_act(&id, mv("h5f7")).unwrap();
        game_ticks(&all, || game_of(&bob, &id).is_some_and(|v| v.room.status == RoomStatus::Finished)).await;
        let v = game_of(&bob, &id).unwrap();
        let r = v.room.result.unwrap();
        assert_eq!((r.winner.as_deref(), r.reason.as_str()), (Some(afp.as_str()), "checkmate"));
        assert_eq!(v.info["san"].as_array().unwrap().last().unwrap(), "Qxf7#");
        assert!(bob.game_act(&id, mv("e8e7")).is_err());

        // Listeden kaldırma yalnızca yerel.
        bob.game_remove(&id).unwrap();
        assert!(game_of(&bob, &id).is_none());
        assert!(game_of(&alice, &id).is_some());
    }

    /// Oyuncu pes eder / beraberlik teklif eder; sahip çevrimdışıyken hamle bekler, gelince iletilir.
    #[tokio::test]
    async fn chess_draw_offer_resign_and_decline() {
        use game::{GameAction, RoomStatus};
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce_all(&[&alice, &bob]);
        let all = [&alice, &bob];
        let bfp = bob.identity.fingerprint.clone();

        // Reddedilen davet.
        let declined = alice.game_invite("chess", &[bfp.clone()], None).unwrap().room.id;
        game_ticks(&all, || game_of(&bob, &declined).is_some()).await;
        bob.game_act(&declined, GameAction::Decline).unwrap();
        game_ticks(&all, || game_of(&alice, &declined).is_some_and(|v| v.room.status == RoomStatus::Declined)).await;

        // Alice siyah: Bob (beyaz) başlar.
        let id = alice.game_invite("chess", &[bfp.clone()], Some(1)).unwrap().room.id;
        game_ticks(&all, || game_of(&bob, &id).is_some()).await;
        bob.game_act(&id, GameAction::Join).unwrap();
        game_ticks(&all, || game_of(&bob, &id).is_some_and(|v| v.my_turn)).await;
        assert_eq!(game_of(&bob, &id).unwrap().my_seat, Some(0));
        bob.game_act(&id, GameAction::Move { mv: "d2d4".into() }).unwrap();
        bob.game_act(&id, GameAction::OfferDraw).unwrap();
        game_ticks(&all, || game_of(&alice, &id).is_some_and(|v| v.room.draw_offer.as_deref() == Some(bfp.as_str()) && v.needs_me)).await;
        // Alice hamle yaparak teklifi reddeder, sonra Bob pes eder.
        alice.game_act(&id, GameAction::Move { mv: "d7d5".into() }).unwrap();
        game_ticks(&all, || game_of(&bob, &id).is_some_and(|v| v.my_turn && v.room.draw_offer.is_none())).await;
        bob.game_act(&id, GameAction::Resign).unwrap();
        game_ticks(&all, || game_of(&alice, &id).is_some_and(|v| v.room.status == RoomStatus::Finished)).await;
        let r = game_of(&alice, &id).unwrap().room.result.unwrap();
        assert_eq!((r.reason.as_str(), r.by.as_deref()), ("resign", Some(bfp.as_str())));
        game_ticks(&all, || game_of(&bob, &id).is_some_and(|v| v.room.status == RoomStatus::Finished && !v.pending)).await;
    }

    /// Hamle gecikmesi: keşif döngüsü (flush_games çağrısı) olmadan, hamle karşıya hemen gitmeli.
    /// Rakibin hamlesi sahibe, sahibin hamlesi rakibe bir bağlantı süresinde ulaşır.
    #[tokio::test]
    async fn game_moves_arrive_without_waiting_for_discovery_tick() {
        use game::{GameAction, RoomStatus};
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        introduce_all(&[&alice, &bob]);
        let all = [&alice, &bob];
        let id = alice.game_invite("chess", &[bob.identity.fingerprint.clone()], Some(0)).unwrap().room.id;
        game_ticks(&all, || game_of(&bob, &id).is_some()).await;
        bob.game_act(&id, GameAction::Join).unwrap();
        game_ticks(&all, || game_of(&alice, &id).is_some_and(|v| v.room.status == RoomStatus::Active)).await;

        // Artık flush_games'i elle çağırmıyoruz: yalnızca hamlenin kendisi iletimi başlatmalı.
        async fn wait(cond: impl Fn() -> bool) -> Duration {
            let t = Instant::now();
            while !cond() {
                assert!(t.elapsed() < Duration::from_secs(2), "hamle 2 sn içinde ulaşmadı");
                sleep(Duration::from_millis(2)).await;
            }
            t.elapsed()
        }
        let mut worst = Duration::ZERO;
        for (i, (who, other, m)) in [(&alice, &bob, "e2e4"), (&bob, &alice, "e7e5"), (&alice, &bob, "g1f3"), (&bob, &alice, "b8c6")].into_iter().enumerate() {
            who.game_act(&id, GameAction::Move { mv: m.into() }).unwrap();
            let d = wait(|| game_of(other, &id).is_some_and(|v| v.room.moves.len() == i + 1) && game_of(who, &id).is_some_and(|v| v.room.moves.len() == i + 1 && !v.pending)).await;
            eprintln!("hamle {m}: {d:?}");
            worst = worst.max(d);
        }
        assert!(worst < Duration::from_millis(1500), "en yavaş hamle {worst:?}");
    }

    /// Cihazın bilinen adreslerinden biri cevap vermiyorsa (VPN, sanal ağ kartı) bağlantı onu
    /// beklemez; çalışan adres hatırlanır ve sonraki bağlantılar doğrudan ona gider.
    #[tokio::test]
    async fn unreachable_address_does_not_delay_connections() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        // Önce sahte (yanıt vermeyen) özel ağ adresi, sonra gerçek adres.
        alice.upsert_peer(bob.device_info(), IpAddr::V4(Ipv4Addr::new(10, 255, 255, 1)), None);
        alice.upsert_peer(bob.device_info(), IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        let peer = alice.peers().into_iter().find(|p| p.fingerprint == bob.identity.fingerprint).unwrap();
        let addrs = alice.candidates(&peer);
        assert_eq!(addrs[0].ip(), IpAddr::V4(Ipv4Addr::new(10, 255, 255, 1)), "test: sahte adres önce denenmeli");
        let t = Instant::now();
        alice.connect(&addrs, Some(bob.identity.fingerprint.clone())).await.unwrap();
        let first = t.elapsed();
        assert!(first < Duration::from_millis(1500), "ilk bağlantı {first:?}");
        assert_eq!(alice.candidates(&peer)[0].ip(), IpAddr::V4(Ipv4Addr::LOCALHOST), "çalışan adres hatırlanmalı");
        let t = Instant::now();
        alice.connect(&alice.candidates(&peer), Some(bob.identity.fingerprint.clone())).await.unwrap();
        assert!(t.elapsed() < Duration::from_millis(300), "ikinci bağlantı {:?}", t.elapsed());
    }

    /// Çok oyunculu golf: biri reddeder (oyun diğerleriyle başlar), sırayla vuruş, çekilme,
    /// sahibin oyuncu adına vuruş uyduramaması.
    #[tokio::test]
    async fn golf_multiplayer_decline_turns_and_quit() {
        use game::{GameAction, RoomStatus};
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        let carol = make_node(tmp.path(), "carol").await;
        let dave = make_node(tmp.path(), "dave").await;
        introduce_all(&[&alice, &bob, &carol, &dave]);
        let all = [&alice, &bob, &carol, &dave];
        let (afp, bfp, cfp, dfp) = (
            alice.identity.fingerprint.clone(),
            bob.identity.fingerprint.clone(),
            carol.identity.fingerprint.clone(),
            dave.identity.fingerprint.clone(),
        );
        let shot = |m: &str| GameAction::Move { mv: m.into() };

        let id = alice.game_invite("golf", &[bfp.clone(), cfp.clone(), dfp.clone()], Some(0)).unwrap().room.id;
        game_ticks(&all, || [&bob, &carol, &dave].iter().all(|n| game_of(n, &id).is_some())).await;
        assert!(alice.game_act(&id, GameAction::OfferDraw).is_err(), "golfte beraberlik teklifi yok");
        bob.game_act(&id, GameAction::Join).unwrap();
        carol.game_act(&id, GameAction::Join).unwrap();
        dave.game_act(&id, GameAction::Decline).unwrap();
        game_ticks(&all, || game_of(&alice, &id).is_some_and(|v| v.room.status == RoomStatus::Active)).await;
        assert_eq!(game_of(&alice, &id).unwrap().room.players.len(), 3);
        game_ticks(&all, || game_of(&dave, &id).is_some_and(|v| v.room.status == RoomStatus::Declined && !v.pending)).await;

        // Sıra: Alice (0) → Bob (1) → Carol (2).
        let order = [(&alice, "a0p30"), (&bob, "a100p40"), (&carol, "a3500p35")];
        for (i, (who, m)) in order.iter().enumerate() {
            game_ticks(&all, || game_of(who, &id).is_some_and(|v| v.my_turn)).await;
            who.game_act(&id, shot(m)).unwrap();
            game_ticks(&all, || [&alice, &bob, &carol].iter().all(|n| game_of(n, &id).is_some_and(|v| v.room.moves.len() == i + 1))).await;
        }
        let v = game_of(&carol, &id).unwrap();
        assert_eq!(v.info["balls"][1]["strokes"], 1);
        assert_eq!(v.info["lastShot"]["seat"], 2);
        assert!(!v.info["lastShot"]["path"].as_array().unwrap().is_empty());

        // Sahip, Bob adına vuruş uyduramaz.
        let mut forged = game_of(&bob, &id).unwrap().room;
        forged.moves.extend(["a0p50".to_string(), "a0p50".to_string()]);
        forged.rev += 1;
        assert!(matches!(bob.accept_game_update(&afp, forged, "alice"), Message::GameReply { ok: false, .. }));

        // Carol çekilir: oyun Alice ile Bob arasında sürer; sırası gelmeden de çekilebilir.
        carol.game_act(&id, GameAction::Resign).unwrap();
        game_ticks(&all, || game_of(&alice, &id).is_some_and(|v| v.info["quit"][2] == true)).await;
        assert_eq!(game_of(&alice, &id).unwrap().room.status, RoomStatus::Active);
        game_ticks(&all, || game_of(&bob, &id).is_some_and(|v| v.info["quit"][2] == true)).await;
        // Bob da çekilince Alice kazanır.
        bob.game_act(&id, GameAction::Resign).unwrap();
        game_ticks(&all, || game_of(&bob, &id).is_some_and(|v| v.room.status == RoomStatus::Finished && !v.pending)).await;
        game_ticks(&all, || game_of(&carol, &id).is_some_and(|v| v.room.status == RoomStatus::Finished)).await;
        let res = game_of(&alice, &id).unwrap().room.result.unwrap();
        assert_eq!((res.winner.as_deref(), res.reason.as_str()), (Some(afp.as_str()), "forfeit"));
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
        // Açıkken kimin oy verdiği görünür ama sayılar gönderilmez (oy çıkarımı yapılamasın).
        poll_ticks(&all, || poll_of(&bob, &id).map_or(false, |v| v.poll.voted.contains(&bob.identity.fingerprint))).await;
        assert_eq!(poll_of(&bob, &id).unwrap().poll.counts, vec![0, 0]);
        assert!(poll_of(&bob, &id).unwrap().poll.ballots.is_empty());
        assert!(poll_of(&alice, &id).unwrap().poll.ballots.is_empty());

        // Listeden kaldırma: açık oylamayı kaldıran sahip onu önce bitirir; sonuç o an açıklanır.
        alice.poll_remove(&id).unwrap();
        assert!(poll_of(&alice, &id).is_none());
        poll_ticks(&all, || poll_of(&bob, &id).map_or(false, |v| v.poll.closed)).await;
        assert_eq!(poll_of(&bob, &id).unwrap().poll.counts, vec![0, 1]);
        assert!(poll_of(&bob, &id).unwrap().poll.ballots.is_empty());
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
            assert!(bob.accept_chat(alice.device_info(), ip, &format!("m{i}"), "spam", None).is_ok());
        }
        assert_eq!(bob.accept_chat(alice.device_info(), ip, "fazla", "spam", None), Err("rate"));
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

        // Sanal ofis koltuğu: değişince olay gelir, geçersiz numara "seçilmedi" sayılır.
        node.upsert_peer(DeviceInfo { name: "Taha Y.".into(), seat: 4, ..dev.clone() }, ip, None);
        assert_eq!(*count.lock().unwrap(), 3, "koltuk değişince olay gelmeli");
        assert_eq!(node.peers()[0].seat, 4);
        node.upsert_peer(DeviceInfo { name: "Taha Y.".into(), seat: 200, ..dev.clone() }, ip, None);
        assert_eq!(node.peers()[0].seat, 0);

        // Kendi koltuğumuz duyuruya girer.
        node.update_settings(|s| s.seat = 7);
        assert_eq!(node.device_info().seat, 7);
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

    // ───────────── Durum, sohbet olayları, çevrimiçi olunca gönder, telefon ─────────────

    #[tokio::test]
    async fn reactions_edits_deletes_replies_and_nudges() {
        use crate::chat::ChatEventKind;
        let tmp = tempfile::tempdir().unwrap();
        let (alice, bob, _events) = chat_pair(tmp.path()).await;
        let afp = alice.identity.fingerprint.clone();
        let bfp = bob.identity.fingerprint.clone();

        let sent = alice.chat_send(&bob.identity.id, "selam", None).unwrap();
        wait_until(|| last_status(&alice, &bfp) == Some(MsgStatus::Sent)).await;
        let id = sent.id.clone();
        let msg_of = |n: &Node, key: &str| n.chat_history(key).into_iter().find(|m| m.id == id).unwrap();

        // Tepki: Bob'un tepkisi Alice'te Bob adına görünür; listede olmayan emoji reddedilir.
        bob.chat_react(&afp, &id, "👍").unwrap();
        wait_until(|| msg_of(&alice, &bfp).reactions.iter().any(|r| r.fp == bfp && r.emoji == "👍")).await;
        assert!(bob.chat_react(&afp, &id, "<script>").is_err());
        bob.chat_react(&afp, &id, "").unwrap();
        wait_until(|| msg_of(&alice, &bfp).reactions.is_empty()).await;

        // Düzenleme: yalnızca yazan düzenler.
        alice.chat_edit(&bfp, &id, "selam, nasılsın?").unwrap();
        wait_until(|| msg_of(&bob, &afp).text == "selam, nasılsın?").await;
        assert!(msg_of(&bob, &afp).edited);
        assert!(bob.chat_edit(&afp, &id, "taklit").is_err(), "başkasının mesajı düzenlenemez");
        // Bob ağdan doğrudan "düzenleme" gönderse bile Alice'teki (Alice'in yazdığı) mesaj değişmez.
        bob.broadcast_event(&afp, ChatEventKind::Edit { id: id.clone(), text: "ele geçirildi".into() }).unwrap();
        sleep(Duration::from_millis(400)).await;
        assert_eq!(msg_of(&alice, &bfp).text, "selam, nasılsın?");

        // Yanıt: alıntı karşı tarafa da gider.
        let reply = bob.chat_send(&alice.identity.id, "iyiyim", Some(&id)).unwrap();
        assert_eq!(reply.reply.as_ref().unwrap().id, id);
        wait_until(|| alice.chat_history(&bfp).iter().any(|m| m.text == "iyiyim" && m.reply.as_ref().is_some_and(|r| r.id == id && r.name == "alice"))).await;

        // Silme: herkesten silinir.
        alice.chat_delete(&bfp, &id).unwrap();
        wait_until(|| msg_of(&bob, &afp).deleted).await;
        assert!(msg_of(&bob, &afp).text.is_empty());

        // Dürtme: karşıda not düşer, art arda dürtme engellenir.
        alice.chat_nudge(&bfp).unwrap();
        wait_until(|| bob.chat_history(&afp).iter().any(|m| m.system && m.text.contains("dürttü"))).await;
        assert!(alice.chat_nudge(&bfp).is_err());
    }

    #[tokio::test]
    async fn queued_file_is_sent_when_peer_comes_online() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        bob.set_trusted(&alice.identity.fingerprint, "alice", true);
        let bfp = bob.identity.fingerprint.clone();
        let src = tmp.path().join("rapor.txt");
        std::fs::write(&src, b"bekleyen dosya").unwrap();

        // Bob çevrimdışı: dosya sohbette bekler.
        alice.chat_queue_files(&bfp, "bob", vec![src.to_string_lossy().into_owned()]).await.unwrap();
        let queued = alice.chat_history(&bfp);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].pending, vec![bfp.clone()]);
        alice.flush_outbox();
        sleep(Duration::from_millis(200)).await;
        assert!(alice.transfers().is_empty());

        // Bob çevrimiçi oldu: dosya kendiliğinden gider, bekleyen balon aktarım balonuna dönüşür.
        introduce(&alice, &bob);
        alice.flush_outbox();
        let dest = PathBuf::from(bob.settings().download_dir).join("rapor.txt");
        for _ in 0..200 {
            if dest.exists() && std::fs::read(&dest).map_or(false, |b| b == b"bekleyen dosya") {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"bekleyen dosya");
        let hist = alice.chat_history(&bfp);
        assert!(hist.iter().all(|m| m.pending.is_empty()));
        assert!(hist.iter().any(|m| m.id.starts_with("f-")));
    }

    #[tokio::test]
    async fn status_is_announced_and_validated() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let bob = make_node(tmp.path(), "bob").await;
        assert!(alice.set_status("tatil").is_err());
        alice.set_status("focus").unwrap();
        assert!(alice.do_not_disturb());
        assert_eq!(alice.device_info().status, "focus");
        introduce(&bob, &alice);
        assert_eq!(bob.peers()[0].status, "focus");
        // Bilinmeyen durum değeri "müsait" sayılır.
        bob.upsert_peer(DeviceInfo { status: "kötü".into(), ..alice.device_info() }, IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        assert_eq!(bob.peers()[0].status, "");
        alice.set_status("").unwrap();
        assert!(!alice.do_not_disturb());
    }

    async fn http(port: u16, req: &[u8]) -> (String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(req).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&out[..split]).into_owned(), out[split + 4..].to_vec())
    }

    #[tokio::test]
    async fn phone_upload_and_download() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = make_node(tmp.path(), "alice").await;
        let info = alice.phone_start().await.unwrap();
        let url = info["url"].as_str().unwrap().to_string();
        assert!(info["svg"].as_str().unwrap().contains("<svg"));
        let (host, path) = url.trim_start_matches("http://").split_once('/').unwrap();
        let port: u16 = host.rsplit_once(':').unwrap().1.parse().unwrap();
        let path = format!("/{path}");

        // Yanlış anahtar: hiçbir şey yok.
        let (head, _) = http(port, b"GET /p/yanlis HTTP/1.1\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 404"));
        // Sayfa açılır.
        let (head, body) = http(port, format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes()).await;
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(String::from_utf8_lossy(&body).contains("alice"));

        // Yükleme: zararlı ad temizlenir, Telefon klasörüne yazılır.
        let data = b"telefondan gelen veri";
        let req = format!("PUT {path}/up?name=..%2F..%2Ffoto%20%C3%A7.jpg HTTP/1.1\r\nContent-Length: {}\r\n\r\n", data.len());
        let mut bytes = req.into_bytes();
        bytes.extend_from_slice(data);
        let (head, _) = http(port, &bytes).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let saved = PathBuf::from(alice.settings().download_dir).join("Telefon").join("foto ç.jpg");
        assert_eq!(std::fs::read(&saved).unwrap(), data);

        // İndirme: yalnızca sunulan dosyalar, sıra numarasıyla.
        let (head, _) = http(port, format!("GET {path}/f/0 HTTP/1.1\r\n\r\n").as_bytes()).await;
        assert!(head.starts_with("HTTP/1.1 404"));
        let src = tmp.path().join("sunum.pdf");
        std::fs::write(&src, b"PDF!").unwrap();
        alice.phone_offer(vec![src.to_string_lossy().into_owned(), tmp.path().to_string_lossy().into_owned()]).unwrap();
        let (head, body) = http(port, format!("GET {path}/f/0 HTTP/1.1\r\n\r\n").as_bytes()).await;
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(head.contains("sunum.pdf"));
        assert_eq!(body, b"PDF!");
        let (_, list) = http(port, format!("GET {path}/list HTTP/1.1\r\n\r\n").as_bytes()).await;
        let list: Value = serde_json::from_slice(&list).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1, "klasör sunulmaz");

        // Kapatılınca erişim biter.
        alice.phone_stop();
        sleep(Duration::from_millis(100)).await;
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err() || {
            let (h, _) = http(port, format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes()).await;
            h.starts_with("HTTP/1.1 404")
        });
    }
}
