//! Telefondan/telefona dosya (`node` modülünün alt modülü).
//!
//! Telefona uygulama kurmadan: FastFiles yerel ağda küçük bir web sunucusu açar ve adresi QR kod
//! olarak gösterir. Aynı Wi-Fi'daki telefon kodu okutunca açılan sayfadan bilgisayara dosya
//! yükler ya da bilgisayarın "telefona gönder" dediği dosyaları indirir.
//!
//! Güvenlik:
//! - Adres 128 bit rastgele bir anahtar içerir; anahtarı bilmeyen hiçbir şeye erişemez.
//! - Sunucu yalnızca "Telefon" penceresi açıkken çalışır ve 30 dakika kullanılmazsa kapanır.
//! - Telefona yalnızca kullanıcının seçtiği dosyalar sunulur (sıra numarasıyla; yol verilmez).
//! - Yüklenen dosya adları temizlenir, `İndirilenler\FastFiles\Telefon` dışına yazılamaz,
//!   aynı ada sahip dosyanın üzerine yazılmaz.
//! - Bağlantı yerel ağda şifresiz HTTP'dir (telefon tarayıcısı FastFiles sertifikasını tanımaz);
//!   bu yüzden oturum kısa ömürlüdür ve arayüz bunu kullanıcıya söyler.

use super::*;

const IDLE_STOP: Duration = Duration::from_secs(30 * 60);
const MAX_HEADER: usize = 16 * 1024;
const MAX_UPLOAD: u64 = 64 * 1024 * 1024 * 1024;
const MAX_OFFERED: usize = 200;

struct Session {
    url: String,
    stop: CancellationToken,
    offered: Vec<(String, PathBuf, u64)>,
    last_activity: Instant,
}

/// Telefon sunucusunda aynı anda işlenen en fazla HTTP bağlantısı (ağdaki biri soketleri
/// tüketemesin). Telefon sayfası birkaç paralel istek açar; bu sınır rahatça yeter.
const MAX_PHONE_CONNS: usize = 32;

#[derive(Default)]
pub struct State {
    session: Mutex<Option<Session>>,
    /// Süren yükleme/indirme sayısı (otomatik güncelleme bunlar bitene kadar bekler).
    active: std::sync::atomic::AtomicUsize,
    /// Aynı anda açık HTTP bağlantısı sayısı (sel koruması).
    conns: std::sync::atomic::AtomicUsize,
}

impl State {
    pub fn busy(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::Relaxed) > 0
    }
}

/// Süren aktarımı sayar; nasıl biterse bitsin (hata, iptal) sayaç geri düşer.
struct ActiveGuard<'a>(&'a std::sync::atomic::AtomicUsize);
impl<'a> ActiveGuard<'a> {
    fn new(c: &'a std::sync::atomic::AtomicUsize) -> Self {
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(c)
    }
}
impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Dosyayı telefona akıtır. Telefon okumayı bırakırsa (tarayıcı indirmeyi duraklattı, ekran
/// kapandı) 60 sn sonra vazgeçilir; eskiden görev ve dosya sonsuza kadar açık kalıyordu.
async fn copy_with_stall_timeout(file: &mut tokio::fs::File, s: &mut TcpStream) -> Result<(), String> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        timeout(Duration::from_secs(60), s.write_all(&buf[..n]))
            .await
            .map_err(|_| "telefon yanıt vermiyor".to_string())?
            .map_err(|e| e.to_string())?;
    }
}

/// URL'deki %XX kodlarını çözer.
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|c| if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") })
        .collect()
}

/// Telefondan gelen dosya adı: tek parça, güvenli.
fn safe_file_name(raw: &str) -> String {
    let name = sanitize_text(raw, 200);
    let last = name.rsplit(['/', '\\']).next().unwrap_or("").to_string();
    sanitize_rel_path(&last).and_then(|p| p.into_iter().last()).unwrap_or_else(|| "dosya".into())
}

fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap()
}

/// QR kod (SVG).
fn qr_svg(text: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

/// Telefona en uygun yerel adres: özel ağ adresleri önce (192.168, 10, 172.16-31).
fn best_ip() -> String {
    let ips = crate::discovery::local_ips();
    let score = |ip: &str| {
        if ip.starts_with("192.168.") {
            3
        } else if ip.starts_with("10.") {
            2
        } else if ip.starts_with("172.") {
            1
        } else {
            0
        }
    };
    ips.iter().max_by_key(|ip| score(ip)).cloned().unwrap_or_else(|| "127.0.0.1".into())
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    content_length: u64,
}

async fn read_head(s: &mut TcpStream) -> Result<(Request, Vec<u8>), String> {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 4096];
    let end = loop {
        let n = timeout(Duration::from_secs(20), s.read(&mut tmp)).await.map_err(|_| "zaman aşımı")?.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("bağlantı kapandı".into());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEADER {
            return Err("başlık çok büyük".into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let rest = buf[end + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/");
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    let query = q
        .split('&')
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (pct_decode(k), pct_decode(v))))
        .collect();
    let mut content_length = 0;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    Ok((Request { method, path: path.to_string(), query, content_length }, rest))
}

async fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes()).await;
    let _ = s.write_all(body).await;
    let _ = s.flush().await;
}

impl Node {
    /// Telefon bağlantısını başlatır (açıksa aynısını döndürür): adres ve QR kod.
    pub async fn phone_start(self: &Arc<Self>) -> Result<Value, String> {
        if let Some(s) = self.extras.phone.session.lock().unwrap().as_mut() {
            s.last_activity = Instant::now();
            return Ok(json!({ "url": s.url, "svg": qr_svg(&s.url)?, "offered": offered_json(&s.offered) }));
        }
        let listener = TcpListener::bind("0.0.0.0:0").await.map_err(|e| format!("Sunucu açılamadı: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let url = format!("http://{}:{port}/p/{token}", best_ip());
        let stop = self.shutdown.child_token();
        {
            let mut session = self.extras.phone.session.lock().unwrap();
            // Bu arada (adres alınırken) başka bir çağrı oturumu açtıysa onu kullan: iki sunucu
            // açılıp biri sahipsiz (kapatılamaz) kalmasın.
            if let Some(s) = session.as_mut() {
                s.last_activity = Instant::now();
                return Ok(json!({ "url": s.url, "svg": qr_svg(&s.url)?, "offered": offered_json(&s.offered) }));
            }
            *session = Some(Session { url: url.clone(), stop: stop.clone(), offered: Vec::new(), last_activity: Instant::now() });
        }
        let node = self.clone();
        tokio::spawn(async move {
            // Boşta kalma denetimi sabit aralıkla yapılır (gelen her bağlantıda yeniden kurulan bir
            // zamanlayıcı, ağdaki tarayıcılar yüzünden hiç dolmayabiliyordu).
            let mut idle_check = tokio::time::interval(Duration::from_secs(60));
            idle_check.tick().await;
            loop {
                let accepted = tokio::select! {
                    r = listener.accept() => r,
                    _ = stop.cancelled() => break,
                    _ = idle_check.tick() => {
                        let idle = !node.extras.phone.busy()
                            && node.extras.phone.session.lock().unwrap().as_ref().map_or(true, |s| s.last_activity.elapsed() > IDLE_STOP);
                        if idle {
                            node.phone_stop();
                            break;
                        }
                        continue;
                    }
                };
                let Ok((stream, _)) = accepted else { continue };
                // Eşzamanlı bağlantı sayısı sınırlı: aşıldıysa yeni bağlantı hemen kapatılır.
                use std::sync::atomic::Ordering;
                if node.extras.phone.conns.fetch_add(1, Ordering::Relaxed) >= MAX_PHONE_CONNS {
                    node.extras.phone.conns.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                let node = node.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    let _ = node.phone_handle(stream, &token).await;
                    node.extras.phone.conns.fetch_sub(1, Ordering::Relaxed);
                });
            }
        });
        Ok(json!({ "url": url, "svg": qr_svg(&url)?, "offered": [] }))
    }

    pub fn phone_stop(&self) {
        if let Some(s) = self.extras.phone.session.lock().unwrap().take() {
            s.stop.cancel();
            self.emit("phone", json!({ "kind": "closed" }));
        }
    }

    /// Telefona sunulacak dosyaları ekler (klasörler atlanır).
    pub fn phone_offer(&self, paths: Vec<String>) -> Result<Value, String> {
        let mut guard = self.extras.phone.session.lock().unwrap();
        let s = guard.as_mut().ok_or("Telefon bağlantısı kapalı")?;
        let mut skipped = 0;
        for p in paths {
            let path = PathBuf::from(&p);
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() && s.offered.len() < MAX_OFFERED => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dosya".into());
                    if !s.offered.iter().any(|o| o.1 == path) {
                        s.offered.push((name, path, m.len()));
                    }
                }
                _ => skipped += 1,
            }
        }
        s.last_activity = Instant::now();
        Ok(json!({ "offered": offered_json(&s.offered), "skipped": skipped }))
    }

    pub fn phone_unoffer(&self, index: usize) -> Value {
        let mut guard = self.extras.phone.session.lock().unwrap();
        match guard.as_mut() {
            Some(s) => {
                if index < s.offered.len() {
                    s.offered.remove(index);
                }
                json!({ "offered": offered_json(&s.offered) })
            }
            None => json!({ "offered": [] }),
        }
    }

    async fn phone_handle(self: &Arc<Self>, mut s: TcpStream, token: &str) -> Result<(), String> {
        let (req, rest) = read_head(&mut s).await?;
        let prefix = format!("/p/{token}");
        let Some(sub) = req.path.strip_prefix(&prefix) else {
            respond(&mut s, "404 Not Found", "text/plain; charset=utf-8", b"Bulunamadi").await;
            return Ok(());
        };
        if let Some(sess) = self.extras.phone.session.lock().unwrap().as_mut() {
            sess.last_activity = Instant::now();
        }
        match (req.method.as_str(), sub) {
            ("GET", "" | "/") => {
                let name = self.settings.lock().unwrap().device_name.clone();
                let html = PAGE.replace("{{NAME}}", &html_escape(&name));
                respond(&mut s, "200 OK", "text/html; charset=utf-8", html.as_bytes()).await;
                self.emit("phone", json!({ "kind": "connected" }));
            }
            ("GET", "/list") => {
                let list = self.extras.phone.session.lock().unwrap().as_ref().map(|x| offered_json(&x.offered)).unwrap_or(json!([]));
                respond(&mut s, "200 OK", "application/json", list.to_string().as_bytes()).await;
            }
            ("GET", f) if f.starts_with("/f/") => {
                let idx: usize = f[3..].parse().unwrap_or(usize::MAX);
                let item = self.extras.phone.session.lock().unwrap().as_ref().and_then(|x| x.offered.get(idx).cloned());
                let Some((name, path, _)) = item else {
                    respond(&mut s, "404 Not Found", "text/plain; charset=utf-8", b"Yok").await;
                    return Ok(());
                };
                let mut file = tokio::fs::File::open(&path).await.map_err(|e| e.to_string())?;
                let len = file.metadata().await.map_err(|e| e.to_string())?.len();
                let ascii: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_ ".contains(c) { c } else { '_' }).collect();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nContent-Disposition: attachment; filename=\"{ascii}\"; filename*=UTF-8''{}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    pct_encode(&name)
                );
                let _active = ActiveGuard::new(&self.extras.phone.active);
                s.write_all(head.as_bytes()).await.map_err(|e| e.to_string())?;
                copy_with_stall_timeout(&mut file, &mut s).await?;
                let _ = s.flush().await;
                self.emit("phone", json!({ "kind": "downloaded", "name": name }));
            }
            ("PUT" | "POST", "/up") => {
                let name = safe_file_name(req.query.get("name").map(String::as_str).unwrap_or("dosya"));
                if req.content_length == 0 || req.content_length > MAX_UPLOAD {
                    respond(&mut s, "400 Bad Request", "text/plain; charset=utf-8", b"Gecersiz boyut").await;
                    return Ok(());
                }
                let dir = PathBuf::from(self.settings.lock().unwrap().download_dir.clone()).join("Telefon");
                tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
                let _active = ActiveGuard::new(&self.extras.phone.active);
                let target = free_name(&dir, &name);
                // Yarım dosyanın adı her yüklemeye özeldir: aynı adla iki yükleme (iki telefon ya
                // da iki sekme) aynı anda gelirse birbirinin dosyasını ezmesin.
                let part = dir.join(format!(".{}.{}.ffpart", uuid::Uuid::new_v4().simple(), name));
                let mut out = BufWriter::with_capacity(1 << 20, tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?);
                let mut remaining = req.content_length;
                let first = rest.len().min(remaining as usize);
                out.write_all(&rest[..first]).await.map_err(|e| e.to_string())?;
                remaining -= first as u64;
                let mut buf = vec![0u8; 256 * 1024];
                let mut ok = true;
                while remaining > 0 {
                    let want = buf.len().min(remaining as usize);
                    match timeout(Duration::from_secs(60), s.read(&mut buf[..want])).await {
                        Ok(Ok(n)) if n > 0 => {
                            if out.write_all(&buf[..n]).await.is_err() {
                                ok = false;
                                break;
                            }
                            remaining -= n as u64;
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                let flushed = out.flush().await.is_ok();
                drop(out);
                if !ok || !flushed {
                    let _ = tokio::fs::remove_file(&part).await;
                    respond(&mut s, "400 Bad Request", "text/plain; charset=utf-8", b"Yarida kaldi").await;
                    return Ok(());
                }
                // Bu arada aynı adla bir dosya geldiyse yine üzerine yazma: son ad, yalnızca
                // henüz yoksa oluşturularak (create_new) sahiplenilir, sonra üzerine taşınır.
                let mut target = target;
                let claimed = loop {
                    match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
                        Ok(_) => break true,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => target = free_name(&dir, &name),
                        Err(_) => break false,
                    }
                };
                if !claimed || tokio::fs::rename(&part, &target).await.is_err() {
                    let _ = tokio::fs::remove_file(&part).await;
                    if claimed {
                        let _ = tokio::fs::remove_file(&target).await; // sahiplenmek için açılan boş dosya
                    }
                    respond(&mut s, "500 Internal Server Error", "text/plain; charset=utf-8", b"Kaydedilemedi").await;
                    return Ok(());
                }
                respond(&mut s, "200 OK", "application/json", b"{\"ok\":true}").await;
                self.emit(
                    "phone",
                    json!({ "kind": "received", "name": target.file_name().map(|n| n.to_string_lossy().into_owned()), "size": req.content_length, "path": target.to_string_lossy() }),
                );
                self.notify_to("Telefondan dosya geldi", &name, json!({ "view": "phone" }));
            }
            _ => respond(&mut s, "404 Not Found", "text/plain; charset=utf-8", b"Bulunamadi").await,
        }
        Ok(())
    }
}

fn offered_json(list: &[(String, PathBuf, u64)]) -> Value {
    Value::Array(list.iter().enumerate().map(|(i, (name, _, size))| json!({ "i": i, "name": name, "size": size })).collect())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Telefonda açılan sayfa (tek dosya, dış kaynak yok; logo ve renkler uygulamayla aynı).
const PAGE: &str = r##"<!doctype html>
<html lang="tr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="color-scheme" content="light dark"><meta name="robots" content="noindex,nofollow">
<title>FastFiles · {{NAME}}</title>
<style>
:root{--a:#5b4fd1;--a2:#7a6fe0;--ink:#fff;--bg:#f4f4f2;--s:#fff;--s2:#f6f6f4;--t:#191918;--m:#6c6b66;--b:#e4e3df;--bs:#d3d2cc;--ok:#14804a;--bad:#d23434;--tile:#f4f2ee;--tb:#d3d2cc}
@media (prefers-color-scheme:dark){:root{--a:#8b80f0;--a2:#a49bf5;--ink:#0d0e10;--bg:#0d0e10;--s:#151619;--s2:#1b1c20;--t:#ececee;--m:#92939b;--b:#26282d;--bs:#34363d;--ok:#3ecf7f;--bad:#ff6b6b}}
*{box-sizing:border-box}html{-webkit-text-size-adjust:100%}
body{margin:0;font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif;background:var(--bg);color:var(--t);-webkit-font-smoothing:antialiased;overflow-x:hidden}
header{padding:calc(18px + env(safe-area-inset-top)) 18px 10px;display:flex;align-items:center;gap:12px;max-width:620px;margin:0 auto}
.logo{width:44px;height:44px;border-radius:13px;background:var(--tile);border:1px solid var(--tb);box-shadow:inset 0 1px 0 rgba(255,255,255,.5),0 1px 2px rgba(0,0,0,.12);display:grid;place-items:center;flex-shrink:0}
.logo svg{width:30px;height:30px;display:block}
h1{font-size:18px;font-weight:600;margin:0;letter-spacing:-.01em;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
small{color:var(--m);font-size:13px}
main{padding:10px 18px calc(44px + env(safe-area-inset-bottom));display:grid;gap:14px;max-width:620px;margin:0 auto}
.card{background:var(--s);border:1px solid var(--b);border-radius:16px;padding:16px;box-shadow:0 1px 2px rgba(0,0,0,.03);min-width:0}
.card h2{font-size:12px;font-weight:600;text-transform:uppercase;letter-spacing:.04em;color:var(--m);margin:0 0 12px}
.pick{display:flex;flex-direction:column;align-items:center;gap:6px;text-align:center;padding:26px 14px;border:1.5px dashed var(--bs);border-radius:14px;color:var(--a);font-weight:600;cursor:pointer;transition:border-color .15s,background .15s}
.pick:active,.pick.drag{border-color:var(--a);background:color-mix(in srgb,var(--a) 8%,transparent)}
.pick svg{width:30px;height:30px;stroke:var(--a);fill:none;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}
.pick .sub{font-size:12.5px;font-weight:400;color:var(--m)}
.pick input{display:none}
.row{display:flex;align-items:center;gap:12px;padding:12px 0;border-top:1px solid var(--b);min-width:0}
.row:first-child{border-top:0}
.ic{width:34px;height:34px;flex-shrink:0;border-radius:9px;background:var(--s2);display:grid;place-items:center;color:var(--a)}
.ic svg{width:17px;height:17px;stroke:currentColor;fill:none;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}
.mid{flex:1;min-width:0}
.n{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:500}
.sz{color:var(--m);font-size:12.5px;margin-top:1px}
.bar{height:5px;background:var(--b);border-radius:9px;overflow:hidden;margin-top:7px}
.bar i{display:block;height:100%;width:0;background:var(--a);border-radius:9px;transition:width .2s}
.ok{color:var(--ok);font-weight:600}.bad{color:var(--bad);font-weight:600}
a.dl{display:inline-flex;align-items:center;gap:6px;background:var(--a);color:var(--ink);text-decoration:none;padding:9px 15px;border-radius:10px;font-weight:600;font-size:14px;flex-shrink:0;box-shadow:inset 0 1px 0 rgba(255,255,255,.16)}
a.dl:active{filter:brightness(1.06)}
a.dl svg{width:15px;height:15px;stroke:currentColor;fill:none;stroke-width:2;stroke-linecap:round;stroke-linejoin:round}
.empty{color:var(--m);font-size:13.5px;padding:4px 0}
.note{display:flex;gap:8px;align-items:flex-start;color:var(--m);font-size:12.5px;line-height:1.5;padding:2px 2px 0;max-width:620px;margin:0 auto}
.note svg{width:15px;height:15px;flex-shrink:0;margin-top:1px;stroke:currentColor;fill:none;stroke-width:1.8}
</style></head><body>
<header>
<div class="logo" aria-hidden="true"><svg viewBox="7 7.5 46 46"><rect x="21" y="14" width="8" height="36" rx="2" fill="#5b4fd1"/><rect x="21" y="14" width="21" height="8" rx="2" fill="#5b4fd1"/><polygon points="41,11 51,18 41,25" fill="#5b4fd1"/><rect x="21" y="29" width="16" height="7" rx="2" fill="#5b4fd1"/><rect x="9" y="16.5" width="8" height="3" rx="1.5" fill="#a49bf5"/><rect x="12" y="31" width="5" height="3" rx="1.5" fill="#a49bf5"/></svg></div>
<div style="min-width:0"><h1>{{NAME}}</h1><small>FastFiles · aynı Wi-Fi ağı</small></div>
</header>
<main>
<section class="card"><h2>Bilgisayara gönder</h2>
<label class="pick" id="pick">
<svg viewBox="0 0 24 24"><path d="M12 15V4m0 0-4 4m4-4 4 4M5 15v3a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-3"/></svg>
<span>Fotoğraf, video ya da dosya seç</span>
<span class="sub">Birden fazla seçebilirsiniz · uygulama gerekmez</span>
<input id="f" type="file" multiple></label>
<div id="up"></div></section>
<section class="card"><h2>Bilgisayardan al</h2><div id="list"><div class="empty">Bilgisayarda "Telefona gönder" ile dosya seçilince burada görünür.</div></div></section>
<div class="note"><svg viewBox="0 0 24 24"><path d="M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6z"/></svg><span>Bağlantı yalnızca yerel ağda kalır, internete çıkmaz. Pencere kapanınca bu adres geçersiz olur.</span></div>
</main>
<script>
const base=location.pathname.replace(/\/$/,'');
const $=s=>document.querySelector(s);
const fmt=n=>{const u=['B','KB','MB','GB'];let i=0;while(n>=1024&&i<3){n/=1024;i++}return (i?n.toFixed(1):n)+' '+u[i]};
const esc=s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const UP='<svg viewBox="0 0 24 24"><path d="M12 19V5m0 0-6 6m6-6 6 6"/></svg>';
const DL='<svg viewBox="0 0 24 24"><path d="M12 5v10m0 0-4-4m4 4 4-4M5 19h14"/></svg>';
let queue=Promise.resolve();
function send(file){const row=document.createElement('div');row.className='row';
row.innerHTML='<div class="ic">'+UP+'</div><div class="mid"><div class="n"></div><div class="bar"><i></i></div></div><div class="sz"></div>';
row.querySelector('.n').textContent=file.name;row.querySelector('.sz').textContent=fmt(file.size);$('#up').prepend(row);
queue=queue.then(()=>new Promise(done=>{const x=new XMLHttpRequest();x.open('PUT',base+'/up?name='+encodeURIComponent(file.name));
x.upload.onprogress=p=>{if(p.lengthComputable)row.querySelector('i').style.width=(p.loaded/p.total*100)+'%'};
x.onload=()=>{row.querySelector('.bar').style.display='none';row.querySelector('.sz').innerHTML=x.status==200?'<span class="ok">✓ Gönderildi</span>':'<span class="bad">Hata</span>';done()};
x.onerror=()=>{row.querySelector('.bar').style.display='none';row.querySelector('.sz').innerHTML='<span class="bad">Bağlantı koptu</span>';done()};x.send(file)}))}
$('#f').onchange=e=>{for(const file of e.target.files)send(file);e.target.value=''};
const pick=$('#pick');
['dragenter','dragover'].forEach(ev=>pick.addEventListener(ev,e=>{e.preventDefault();pick.classList.add('drag')}));
['dragleave','drop'].forEach(ev=>pick.addEventListener(ev,e=>{e.preventDefault();pick.classList.remove('drag')}));
pick.addEventListener('drop',e=>{for(const file of e.dataTransfer.files)send(file)});
let sig='';async function poll(){try{const r=await fetch(base+'/list',{cache:'no-store'});if(r.ok){const l=await r.json();const s=JSON.stringify(l);
if(s!==sig){sig=s;$('#list').innerHTML=l.length?l.map(f=>'<div class="row"><div class="ic">'+DL+'</div><div class="mid"><div class="n">'+esc(f.name)+'</div><div class="sz">'+fmt(f.size)+'</div></div><a class="dl" href="'+base+'/f/'+f.i+'" download>'+DL+'İndir</a></div>').join(''):'<div class="empty">Bilgisayarda "Telefona gönder" ile dosya seçilince burada görünür.</div>'}}}catch(e){}setTimeout(poll,2500)}
poll();
</script></body></html>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(safe_file_name("../../evil.exe"), "evil.exe");
        assert_eq!(safe_file_name("C:\\Windows\\x.dll"), "x.dll");
        assert_eq!(safe_file_name("foto 1.jpg"), "foto 1.jpg");
        assert_eq!(safe_file_name(""), "dosya");
        assert_eq!(safe_file_name("a\u{202E}gpj.exe"), "agpj.exe");
        assert_eq!(pct_decode("ek%C3%A7i%20dosya.jpg"), "ekçi dosya.jpg");
        assert_eq!(pct_encode("ç a.jpg"), "%C3%A7%20a.jpg");
    }

    /// QR SVG'si keskin kenarlı ve ölçeklenebilir olmalı. `FF_QR_OUT` verilirse dosyaya da yazılır
    /// (dışarıdan bir QR çözücüyle doğrulamak için).
    #[test]
    fn qr_svg_is_crisp_and_scalable() {
        let url = "http://192.168.1.107:57055/p/0e29734f40c84cd492ae0447aabbccdd";
        let svg = qr_svg(url).unwrap();
        assert!(svg.contains("viewBox=") && svg.contains("crispEdges"));
        assert!(svg.contains(r##"fill="#000000""##));
        if let Ok(out) = std::env::var("FF_QR_OUT") {
            std::fs::write(out, &svg).unwrap();
        }
    }
}
