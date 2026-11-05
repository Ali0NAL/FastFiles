//! Aktarımın disk tarafı: ağ ile disk birbirini beklemesin diye ayrı iş parçacıklarında çalışır.
//!
//! **Gönderen:** Tek bir okuyucu iş parçacığı dosyaları sırayla okur, BLAKE3 özetini hesaplar,
//! gerekirse bloğu zstd ile sıkıştırır ve hazır baytları kanala koyar. Ağ görevi yalnızca yazar.
//!
//! **Alıcı:** Ağ görevi baytları okur. Büyük dosyalar sırayla tek bir yazıcı iş parçacığına,
//! küçük dosyalar (tamamı bellekte) paralel bir yazıcı havuzuna gider. Windows'ta dosya oluşturma/
//! yeniden adlandırma gecikmesi baskın olduğundan küçük dosyalarda paralel yazım birkaç kat hızlıdır.
//!
//! **Çerçeve biçimi** (`FEATURE_V2`): her dosyanın kalan kısmı bloklar halinde gider.
//! Blok = `u32 (big-endian)` başlık + veri. Başlığın en üst biti 1 ise veri zstd ile sıkıştırılmıştır;
//! diğer 31 bit veri uzunluğudur. Bir blok en fazla `CHUNK` bayt (sıkıştırılmamış) içerir.
//! Dosyanın blokları bitince tüm dosyanın 32 baytlık BLAKE3 özeti gelir.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub const CHUNK: usize = 1024 * 1024;
/// Bu boyuta kadar olan dosyalar alıcıda bellekte toplanıp paralel yazılır.
pub const SMALL_FILE: u64 = 256 * 1024;
const COMPRESSED_FLAG: u32 = 1 << 31;
/// Sıkıştırılmış blok, ham halinin en fazla bu oranı kadarsa sıkıştırılmış gönderilir.
const MIN_GAIN: f64 = 0.92;
/// Arka arkaya bu kadar blok sıkışmazsa o dosyada sıkıştırma bırakılır.
const MAX_MISSES: u32 = 2;
/// Küçük dosya yazıcı sayısı. Ölçüm: tek iş parçacığına göre ~1,6× (antivirüs önbellekteyken);
/// antivirüs her yeni dosyayı tarıyorsa sınır tarama hızıdır (~550 dosya/sn), paralellik az fark eder.
const SMALL_WRITERS: usize = 8;

// ───────────────────────────── Gönderen ─────────────────────────────

pub struct ReadJob {
    pub src: PathBuf,
    pub name: String,
    pub size: u64,
    /// Alıcıda zaten bulunan bayt sayısı.
    pub offset: u64,
}

pub enum Piece {
    /// Sıradaki dosyaya geçildi (arayüzde "şu an" için).
    File(usize),
    /// Ağa yazılacak baytlar. `progress`: karşılık gelen ham (sıkıştırılmamış) veri miktarı.
    Bytes { data: Vec<u8>, progress: u64 },
}

#[derive(Clone, Copy)]
pub struct Mode {
    /// Çerçeveli bloklar + dosya sonunda BLAKE3 özeti (yeni protokol).
    pub v2: bool,
    /// Uygun dosyalarda zstd sıkıştırma.
    pub compress: bool,
}

/// Zaten sıkıştırılmış biçimler: bunları sıkıştırmayı denemek boşuna CPU harcar.
fn incompressible_ext(name: &str) -> bool {
    const EXT: &[&str] = &[
        "zip", "7z", "rar", "gz", "tgz", "bz2", "xz", "zst", "lz4", "cab", "jar", "apk", "msi", "iso",
        "jpg", "jpeg", "png", "gif", "webp", "heic", "avif", "jxl", "mp4", "mkv", "mov", "avi", "webm",
        "m4v", "wmv", "mp3", "aac", "m4a", "ogg", "opus", "flac", "wma", "docx", "xlsx", "pptx", "odt",
        "ods", "odp", "epub", "pdf", "psd", "ai", "indd", "woff", "woff2", "ffupdate",
    ];
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    EXT.contains(&ext.as_str())
}

/// Okuyucu iş parçacığını başlatır. Kanal kapanırsa (aktarım bitti/iptal) iş parçacığı da durur.
pub fn spawn_reader(jobs: Vec<ReadJob>, mode: Mode) -> mpsc::Receiver<Result<Piece, String>> {
    let (tx, rx) = mpsc::channel(16);
    std::thread::spawn(move || {
        if let Err(e) = read_all(&jobs, mode, &tx) {
            let _ = tx.blocking_send(Err(e));
        }
    });
    rx
}

fn read_all(jobs: &[ReadJob], mode: Mode, tx: &mpsc::Sender<Result<Piece, String>>) -> Result<(), String> {
    // Kanal kapandıysa sessizce dur.
    let send = |p: Piece| tx.blocking_send(Ok(p)).map_err(|_| String::new());
    let mut compressor = zstd::bulk::Compressor::new(1).map_err(|e| e.to_string())?;
    for (i, job) in jobs.iter().enumerate() {
        send(Piece::File(i))?;
        let changed = || format!("{} gönderim sırasında değişti", job.name);
        let mut f = std::fs::File::open(&job.src).map_err(|e| format!("{} açılamadı: {e}", job.name))?;
        let mut hasher = blake3::Hasher::new();
        if job.offset > 0 {
            // Alıcıdaki kısım gönderilmez ama tüm dosyanın özeti için okunur.
            let copied = std::io::copy(&mut (&mut f).take(job.offset), &mut hasher)
                .map_err(|e| format!("{} okunamadı: {e}", job.name))?;
            if copied != job.offset {
                return Err(changed());
            }
        }
        let mut try_compress = mode.v2 && mode.compress && !incompressible_ext(&job.name);
        let mut misses = 0;
        let mut remaining = job.size - job.offset;
        while remaining > 0 {
            let n = remaining.min(CHUNK as u64) as usize;
            let mut buf = vec![0u8; n];
            f.read_exact(&mut buf).map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => changed(),
                _ => format!("{} okunamadı: {e}", job.name),
            })?;
            remaining -= n as u64;
            if !mode.v2 {
                send(Piece::Bytes { data: buf, progress: n as u64 })?;
                continue;
            }
            hasher.update(&buf);
            let mut packed = None;
            if try_compress && n >= 512 {
                if let Ok(c) = compressor.compress(&buf) {
                    if (c.len() as f64) <= n as f64 * MIN_GAIN {
                        packed = Some(c);
                        misses = 0;
                    } else {
                        misses += 1;
                        try_compress = misses < MAX_MISSES;
                    }
                }
            }
            let (flag, body) = match packed {
                Some(c) => (COMPRESSED_FLAG, c),
                None => (0, buf),
            };
            let mut data = Vec::with_capacity(body.len() + 4);
            data.extend_from_slice(&(flag | body.len() as u32).to_be_bytes());
            data.extend_from_slice(&body);
            send(Piece::Bytes { data, progress: n as u64 })?;
        }
        if mode.v2 {
            send(Piece::Bytes { data: hasher.finalize().as_bytes().to_vec(), progress: 0 })?;
        }
    }
    Ok(())
}

// ───────────────────────────── Alıcı ─────────────────────────────

/// Ağdan bir dosyanın kalan kısmını okur: önce `next` ile veri parçaları, sonra `hash`.
pub struct Body {
    left: u64,
    v2: bool,
    wait: std::time::Duration,
    stall: std::time::Duration,
}

impl Body {
    /// `first_wait`: ilk bayt için bekleme (gönderen devam eden dosyanın başını özetliyor olabilir).
    pub fn new(remaining: u64, v2: bool, first_wait: std::time::Duration, stall: std::time::Duration) -> Self {
        Self { left: remaining, v2, wait: first_wait, stall }
    }

    /// Sıradaki (açılmış) veri parçası; dosyanın verisi bittiyse `None`.
    pub async fn next<R: AsyncRead + Unpin>(
        &mut self,
        rd: &mut R,
        decompressor: &mut zstd::bulk::Decompressor<'static>,
    ) -> Result<Option<Vec<u8>>, BodyError> {
        if self.left == 0 {
            return Ok(None);
        }
        let chunk = if self.v2 {
            let mut head = [0u8; 4];
            read_exact_timeout(rd, &mut head, self.wait).await?;
            let head = u32::from_be_bytes(head);
            let len = (head & !COMPRESSED_FLAG) as usize;
            if len == 0 || len > CHUNK + 64 * 1024 {
                return Err(BodyError::Protocol("geçersiz blok uzunluğu".into()));
            }
            let mut body = vec![0u8; len];
            read_exact_timeout(rd, &mut body, self.stall).await?;
            if head & COMPRESSED_FLAG != 0 {
                // Kapasite sınırı: kötü niyetli "sıkıştırma bombası" belleği şişiremez.
                let cap = self.left.min(CHUNK as u64) as usize;
                decompressor
                    .decompress(&body, cap)
                    .map_err(|_| BodyError::Protocol("sıkıştırılmış blok çözülemedi".into()))?
            } else {
                body
            }
        } else {
            let mut buf = vec![0u8; self.left.min(CHUNK as u64) as usize];
            read_exact_timeout(rd, &mut buf, self.wait).await?;
            buf
        };
        if chunk.is_empty() || chunk.len() as u64 > self.left {
            return Err(BodyError::Protocol("blok dosya boyutunu aşıyor".into()));
        }
        self.wait = self.stall;
        self.left -= chunk.len() as u64;
        Ok(Some(chunk))
    }

    /// Dosya sonundaki beklenen özet (v2) ya da `None` (eski protokol).
    pub async fn hash<R: AsyncRead + Unpin>(&mut self, rd: &mut R) -> Result<Option<[u8; 32]>, BodyError> {
        if !self.v2 {
            return Ok(None);
        }
        let mut hash = [0u8; 32];
        read_exact_timeout(rd, &mut hash, self.wait).await?;
        Ok(Some(hash))
    }
}

pub enum BodyError {
    /// Bağlantı koptu veya veri akışı durdu.
    Lost,
    Protocol(String),
}

async fn read_exact_timeout<R: AsyncRead + Unpin>(rd: &mut R, buf: &mut [u8], wait: std::time::Duration) -> Result<(), BodyError> {
    match timeout(wait, rd.read_exact(buf)).await {
        Ok(Ok(_)) => Ok(()),
        _ => Err(BodyError::Lost),
    }
}

pub struct Target {
    pub name: String,
    pub part: PathBuf,
    pub target: PathBuf,
    pub offset: u64,
    /// Devam eden dosyanın mevcut kısmının özet durumu.
    pub prefix: Option<blake3::Hasher>,
}

enum BigMsg {
    Open(Target),
    Data(Vec<u8>),
    Finish(Option<[u8; 32]>),
}

struct SmallJob {
    t: Target,
    data: Vec<u8>,
    expected: Option<[u8; 32]>,
}

#[derive(Default)]
struct Shared {
    error: Mutex<Option<String>>,
    /// Yazılmakta olan yarım dosyalar (iptal/hata durumunda silinir).
    in_flight: Mutex<HashSet<PathBuf>>,
    made_dirs: Mutex<HashSet<PathBuf>>,
    /// İptal: kuyruktaki işler atlanır.
    abort: AtomicBool,
    /// Tamamlanan dosya sayısı.
    completed: std::sync::atomic::AtomicUsize,
}

impl Shared {
    fn fail(&self, e: String) {
        self.error.lock().unwrap().get_or_insert(e);
    }

    fn ensure_parent(&self, path: &Path) -> Result<(), String> {
        let Some(parent) = path.parent() else { return Ok(()) };
        if self.made_dirs.lock().unwrap().contains(parent) {
            return Ok(());
        }
        std::fs::create_dir_all(parent).map_err(|e| format!("Klasör oluşturulamadı: {e}"))?;
        self.made_dirs.lock().unwrap().insert(parent.to_path_buf());
        Ok(())
    }

    fn verify_and_rename(&self, t: &Target, hasher: Option<blake3::Hasher>, expected: Option<[u8; 32]>) -> Result<(), String> {
        if let (Some(h), Some(exp)) = (hasher, expected) {
            if h.finalize().as_bytes() != &exp {
                let _ = std::fs::remove_file(&t.part);
                self.in_flight.lock().unwrap().remove(&t.part);
                return Err(format!("{} bozuk geldi (doğrulama başarısız)", t.name));
            }
        }
        rename_with_retry(&t.part, &t.target).map_err(|e| format!("Dosya kaydedilemedi: {e}"))?;
        self.in_flight.lock().unwrap().remove(&t.part);
        self.completed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn write_small(&self, job: SmallJob) -> Result<(), String> {
        let SmallJob { t, data, expected } = job;
        self.ensure_parent(&t.target)?;
        self.in_flight.lock().unwrap().insert(t.part.clone());
        std::fs::write(&t.part, &data).map_err(|e| format!("Diske yazılamadı ({}): {e}", t.name))?;
        let hasher = expected.map(|_| {
            let mut h = blake3::Hasher::new();
            h.update(&data);
            h
        });
        self.verify_and_rename(&t, hasher, expected)
    }
}

/// Windows'ta virüs tarayıcısı yeni dosyayı kısa süre açık tutabilir; birkaç kez denenir.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for i in 0..10 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(20 * (i + 1)));
            }
        }
    }
    Err(last.unwrap())
}

/// Alıcının disk tarafı.
pub struct DiskSink {
    shared: Arc<Shared>,
    big_tx: Option<mpsc::Sender<BigMsg>>,
    small_tx: Option<mpsc::Sender<SmallJob>>,
    handles: Vec<JoinHandle<()>>,
}

/// Aktarım bittikten sonra yarım dosyaları temizlemek için.
#[derive(Clone)]
pub struct SinkHandle(Arc<Shared>);

impl SinkHandle {
    /// Yazımı yarıda kalan dosyalar.
    pub fn in_flight(&self) -> Vec<PathBuf> {
        self.0.in_flight.lock().unwrap().iter().cloned().collect()
    }
    /// Kuyruktaki yazımları iptal eder.
    pub fn abort(&self) {
        self.0.abort.store(true, Ordering::Relaxed);
    }
}

impl DiskSink {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        let mut handles = Vec::new();

        let (big_tx, mut big_rx) = mpsc::channel::<BigMsg>(8);
        let s = shared.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            let mut cur: Option<(Target, std::io::BufWriter<std::fs::File>, Option<blake3::Hasher>)> = None;
            while let Some(msg) = big_rx.blocking_recv() {
                if s.abort.load(Ordering::Relaxed) || s.error.lock().unwrap().is_some() {
                    continue; // kuyruğu boşalt
                }
                let r: Result<(), String> = (|| match msg {
                    BigMsg::Open(mut t) => {
                        s.ensure_parent(&t.target)?;
                        s.in_flight.lock().unwrap().insert(t.part.clone());
                        let file = open_part(&t.part, t.offset).map_err(|e| format!("Dosya oluşturulamadı ({}): {e}", t.name))?;
                        let hasher = t.prefix.take();
                        cur = Some((t, std::io::BufWriter::with_capacity(CHUNK, file), hasher));
                        Ok(())
                    }
                    BigMsg::Data(d) => {
                        let (_, w, h) = cur.as_mut().ok_or("beklenmeyen veri")?;
                        if let Some(h) = h {
                            h.update(&d);
                        }
                        w.write_all(&d).map_err(|e| format!("Diske yazılamadı: {e}"))
                    }
                    BigMsg::Finish(expected) => {
                        let (t, w, h) = cur.take().ok_or("beklenmeyen dosya sonu")?;
                        let file = w.into_inner().map_err(|e| format!("Diske yazılamadı: {e}"))?;
                        drop(file); // Windows: açık dosya yeniden adlandırılamaz
                        s.verify_and_rename(&t, h, expected)
                    }
                })();
                if let Err(e) = r {
                    s.fail(e);
                }
            }
            // Kanal kapandı: yarım dosya diske yazılıp kapatılır (kaldığı yerden devam için).
            if let Some((_, mut w, _)) = cur.take() {
                let _ = w.flush();
            }
        }));

        let (small_tx, small_rx) = mpsc::channel::<SmallJob>(64);
        let small_rx = Arc::new(Mutex::new(small_rx));
        for _ in 0..SMALL_WRITERS {
            let (s, rx) = (shared.clone(), small_rx.clone());
            handles.push(tokio::task::spawn_blocking(move || loop {
                let job = rx.lock().unwrap().blocking_recv();
                let Some(job) = job else { break };
                if s.abort.load(Ordering::Relaxed) || s.error.lock().unwrap().is_some() {
                    continue;
                }
                if let Err(e) = s.write_small(job) {
                    s.fail(e);
                }
            }));
        }
        Self { shared, big_tx: Some(big_tx), small_tx: Some(small_tx), handles }
    }

    pub fn handle(&self) -> SinkHandle {
        SinkHandle(self.shared.clone())
    }

    /// Disk tarafında hata oluştuysa.
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }

    fn closed(&self) -> String {
        self.error().unwrap_or_else(|| "Disk yazıcısı durdu".into())
    }

    pub async fn open_big(&self, t: Target) -> Result<(), String> {
        self.big_tx.as_ref().unwrap().send(BigMsg::Open(t)).await.map_err(|_| self.closed())
    }

    pub async fn big_data(&self, d: Vec<u8>) -> Result<(), String> {
        self.big_tx.as_ref().unwrap().send(BigMsg::Data(d)).await.map_err(|_| self.closed())
    }

    pub async fn finish_big(&self, expected: Option<[u8; 32]>) -> Result<(), String> {
        self.big_tx.as_ref().unwrap().send(BigMsg::Finish(expected)).await.map_err(|_| self.closed())
    }

    pub async fn small(&self, t: Target, data: Vec<u8>, expected: Option<[u8; 32]>) -> Result<(), String> {
        self.small_tx.as_ref().unwrap().send(SmallJob { t, data, expected }).await.map_err(|_| self.closed())
    }

    /// Tüm yazımların bitmesini bekler; ilk hatayı döndürür.
    pub async fn finish(mut self) -> Result<usize, String> {
        self.big_tx = None;
        self.small_tx = None;
        for h in self.handles.drain(..) {
            let _ = h.await;
        }
        match self.error() {
            Some(e) => Err(e),
            None => Ok(self.shared.completed.load(Ordering::Relaxed)),
        }
    }

    /// İptal/kopma sonrası: kanalları kapatır ve iş parçacıklarının durmasını bekler.
    pub async fn close(mut self) {
        self.big_tx = None;
        self.small_tx = None;
        for h in self.handles.drain(..) {
            let _ = h.await;
        }
    }
}

fn open_part(path: &Path, offset: u64) -> std::io::Result<std::fs::File> {
    if offset == 0 {
        return std::fs::File::create(path);
    }
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.set_len(offset)?;
    f.seek(SeekFrom::Start(offset))?;
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gönderen okuyucusunun tek başına hızı (ağ yok).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn bench_reader_small_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut jobs = Vec::new();
        for i in 0..20_000u64 {
            let d = tmp.path().join(format!("d{}", i % 200));
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join(format!("f{i}.txt"));
            std::fs::write(&p, vec![(i % 251) as u8; 4096]).unwrap();
            jobs.push(ReadJob { src: p, name: format!("f{i}"), size: 4096, offset: 0 });
        }
        let start = std::time::Instant::now();
        let mut rx = spawn_reader(jobs, Mode { v2: true, compress: true });
        let mut n = 0;
        while let Some(p) = rx.recv().await {
            if let Piece::Bytes { .. } = p.unwrap() {
                n += 1;
            }
        }
        println!("[bench] okuyucu: {} parça, {:.0} dosya/sn", n, 20_000.0 / start.elapsed().as_secs_f64());
    }

    /// Alıcı yazıcı havuzunun tek başına hızı (ağ yok).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn bench_sink_small_files() {
        let tmp = tempfile::tempdir().unwrap();
        let unique = std::env::var("FF_BENCH_UNIQUE").is_ok();
        let start = std::time::Instant::now();
        let sink = DiskSink::new();
        let mut wait = std::time::Duration::ZERO;
        for i in 0..20_000u64 {
            // Benzersiz içerik: virüs tarayıcısı aynı içeriği önbellekten geçebilir.
            let mut data = vec![7u8; 4096];
            if unique {
                data[..8].copy_from_slice(&i.to_le_bytes());
                data[4000..4008].copy_from_slice(&(i * 2654435761).to_le_bytes());
            }
            let hash = *blake3::hash(&data).as_bytes();
            let target = tmp.path().join(format!("d{}", i % 200)).join(format!("f{i}.txt"));
            let t = Target { name: format!("f{i}"), part: target.with_extension("txt.ffpart"), target, offset: 0, prefix: None };
            let w = std::time::Instant::now();
            sink.small(t, data, Some(hash)).await.unwrap();
            wait += w.elapsed();
        }
        let n = sink.finish().await.unwrap();
        println!(
            "[bench] yazıcı havuzu: {n} dosya, {:.0} dosya/sn, kuyrukta bekleme {:.1} sn",
            20_000.0 / start.elapsed().as_secs_f64(),
            wait.as_secs_f64()
        );
    }

    #[test]
    fn known_compressed_formats_are_skipped() {
        assert!(incompressible_ext("video.MP4"));
        assert!(incompressible_ext("a/b/rapor.xlsx"));
        assert!(!incompressible_ext("log.txt"));
        assert!(!incompressible_ext("README"));
    }

    /// Okuyucu → (çerçeveli bayt akışı) → read_file_body: veri ve özet birebir geri gelir,
    /// sıkıştırılabilir dosya gerçekten küçülür, sıkıştırılamayan dosya ham gider.
    #[tokio::test]
    async fn framed_roundtrip_with_and_without_compression() {
        let tmp = tempfile::tempdir().unwrap();
        let text: Vec<u8> = "satır satır aynı log kaydı 200 OK\n".repeat(80_000).into_bytes();
        let mut x = 7u64;
        let noise: Vec<u8> = (0..3 * CHUNK + 123)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        std::fs::write(tmp.path().join("a.log"), &text).unwrap();
        std::fs::write(tmp.path().join("b.bin"), &noise).unwrap();
        let jobs = vec![
            ReadJob { src: tmp.path().join("a.log"), name: "a.log".into(), size: text.len() as u64, offset: 0 },
            ReadJob { src: tmp.path().join("b.bin"), name: "b.bin".into(), size: noise.len() as u64, offset: 100 },
        ];
        let mut rx = spawn_reader(jobs, Mode { v2: true, compress: true });
        let mut wire = Vec::new();
        while let Some(p) = rx.recv().await {
            if let Piece::Bytes { data, .. } = p.unwrap() {
                wire.extend_from_slice(&data);
            }
        }
        assert!(wire.len() < text.len() / 5 + noise.len(), "metin sıkışmalıydı: {}", wire.len());

        let mut rd = std::io::Cursor::new(wire);
        let mut dec = zstd::bulk::Decompressor::new().unwrap();
        let wait = std::time::Duration::from_secs(1);
        for (data, offset) in [(&text, 0usize), (&noise, 100)] {
            let mut got = Vec::new();
            let mut body = Body::new((data.len() - offset) as u64, true, wait, wait);
            while let Some(c) = body.next(&mut rd, &mut dec).await.ok().unwrap() {
                got.extend_from_slice(&c);
            }
            let hash = body.hash(&mut rd).await.ok().unwrap();
            assert!(got == data[offset..]);
            assert_eq!(hash.unwrap(), *blake3::hash(data).as_bytes(), "özet tüm dosyayı kapsamalı");
        }
    }

    #[tokio::test]
    async fn oversized_or_bomb_blocks_are_rejected() {
        let mut dec = zstd::bulk::Decompressor::new().unwrap();
        let wait = std::time::Duration::from_secs(1);
        // 10 baytlık dosya için 1 MB'a açılan sıkıştırılmış blok.
        let bomb = zstd::bulk::compress(&vec![0u8; CHUNK], 1).unwrap();
        let mut wire = (COMPRESSED_FLAG | bomb.len() as u32).to_be_bytes().to_vec();
        wire.extend_from_slice(&bomb);
        let r = Body::new(10, true, wait, wait).next(&mut std::io::Cursor::new(wire), &mut dec).await;
        assert!(matches!(r, Err(BodyError::Protocol(_))));
        // Dev uzunluk başlığı.
        let wire = (u32::MAX & !COMPRESSED_FLAG).to_be_bytes().to_vec();
        let r = Body::new(10, true, wait, wait).next(&mut std::io::Cursor::new(wire), &mut dec).await;
        assert!(matches!(r, Err(BodyError::Protocol(_))));
    }
}
