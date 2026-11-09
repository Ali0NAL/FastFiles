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

#[derive(Default)]
pub struct State {
    session: Mutex<Option<Session>>,
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
        *self.extras.phone.session.lock().unwrap() = Some(Session {

            url: url.clone(),
            stop: stop.clone(),
            offered: Vec::new(),
            last_activity: Instant::now(),
        });
        let node = self.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    r = listener.accept() => r,
                    _ = stop.cancelled() => break,
                    _ = sleep(Duration::from_secs(60)) => {
                        let idle = node.extras.phone.session.lock().unwrap().as_ref().map_or(true, |s| s.last_activity.elapsed() > IDLE_STOP);
                        if idle {
                            node.phone_stop();
                            break;
                        }
                        continue;
                    }
                };
                let Ok((stream, _)) = accepted else { continue };
                let node = node.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    let _ = node.phone_handle(stream, &token).await;
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
                s.write_all(head.as_bytes()).await.map_err(|e| e.to_string())?;
                tokio::io::copy(&mut file, &mut s).await.map_err(|e| e.to_string())?;
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
                let target = free_name(&dir, &name);
                let part = target.with_extension(format!(
                    "{}ffpart",
                    target.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()
                ));
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
                // Bu arada aynı adla bir dosya geldiyse yine üzerine yazma.
                let target = if target.exists() { free_name(&dir, &name) } else { target };
                tokio::fs::rename(&part, &target).await.map_err(|e| e.to_string())?;
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

/// Telefonda açılan sayfa (tek dosya, dış kaynak yok).
const PAGE: &str = r##"<!doctype html>
<html lang="tr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>FastFiles · {{NAME}}</title>
<style>
:root{--a:#5b55f6;--bg:#f4f5fa;--s:#fff;--t:#171a2b;--m:#6b7088;--b:#e4e6ef;--ok:#16a35f;--bad:#e0424a}
@media (prefers-color-scheme:dark){:root{--bg:#0f1020;--s:#1a1c33;--t:#eceefa;--m:#9196b5;--b:#2a2d48;--a:#7c77ff}}
*{box-sizing:border-box}body{margin:0;font:16px/1.4 system-ui,-apple-system,"Segoe UI",sans-serif;background:var(--bg);color:var(--t)}
header{padding:20px 16px 8px;display:flex;align-items:center;gap:12px}
.logo{width:40px;height:40px;border-radius:12px;background:linear-gradient(135deg,var(--a),#22c5e0);display:grid;place-items:center;color:#fff;font-weight:800}
h1{font-size:18px;margin:0}small{color:var(--m)}
main{padding:8px 16px 40px;display:grid;gap:14px;max-width:560px;margin:0 auto}
.card{background:var(--s);border:1px solid var(--b);border-radius:16px;padding:16px}
.card h2{font-size:15px;margin:0 0 10px}
.pick{display:block;text-align:center;padding:22px 12px;border:2px dashed var(--a);border-radius:14px;color:var(--a);font-weight:700;cursor:pointer}
.pick input{display:none}
.row{display:flex;align-items:center;gap:10px;padding:10px 0;border-top:1px solid var(--b)}
.row:first-child{border-top:0}
.row .n{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.row .sz{color:var(--m);font-size:13px}
.bar{height:6px;background:var(--b);border-radius:9px;overflow:hidden;margin-top:6px}
.bar i{display:block;height:100%;width:0;background:var(--a);transition:width .2s}
.ok{color:var(--ok);font-weight:700}.bad{color:var(--bad);font-weight:700}
a.dl{background:var(--a);color:#fff;text-decoration:none;padding:8px 14px;border-radius:10px;font-weight:700;font-size:14px}
.empty{color:var(--m);font-size:14px}
</style></head><body>
<header><div class="logo">⚡</div><div><h1>{{NAME}}</h1><small>FastFiles · aynı Wi-Fi ağı</small></div></header>
<main>
<section class="card"><h2>Bilgisayara gönder</h2>
<label class="pick">📤 Fotoğraf, video ya da dosya seç<input id="f" type="file" multiple></label>
<div id="up"></div></section>
<section class="card"><h2>Bilgisayardan al</h2><div id="list"><div class="empty">Bilgisayarda "Telefona gönder" ile dosya seçilince burada görünür.</div></div></section>
</main>
<script>
const base=location.pathname.replace(/\/$/,'');
const fmt=n=>{const u=['B','KB','MB','GB'];let i=0;while(n>=1024&&i<3){n/=1024;i++}return (i?n.toFixed(1):n)+' '+u[i]};
const esc=s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
let queue=Promise.resolve();
document.getElementById('f').onchange=e=>{for(const file of e.target.files){const row=document.createElement('div');row.className='row';
row.innerHTML='<div class="n"></div><div class="sz"></div>';row.querySelector('.n').innerHTML=esc(file.name)+'<div class="bar"><i></i></div>';
row.querySelector('.sz').textContent=fmt(file.size);document.getElementById('up').prepend(row);
queue=queue.then(()=>new Promise(done=>{const x=new XMLHttpRequest();x.open('PUT',base+'/up?name='+encodeURIComponent(file.name));
x.upload.onprogress=p=>{if(p.lengthComputable)row.querySelector('i').style.width=(p.loaded/p.total*100)+'%'};
x.onload=()=>{row.querySelector('.sz').innerHTML=x.status==200?'<span class="ok">✓ Gönderildi</span>':'<span class="bad">Hata</span>';done()};
x.onerror=()=>{row.querySelector('.sz').innerHTML='<span class="bad">Bağlantı koptu</span>';done()};x.send(file)}))}
e.target.value=''};
let sig='';async function poll(){try{const r=await fetch(base+'/list',{cache:'no-store'});if(r.ok){const l=await r.json();const s=JSON.stringify(l);
if(s!==sig){sig=s;document.getElementById('list').innerHTML=l.length?l.map(f=>'<div class="row"><div class="n">'+esc(f.name)+'<div class="sz">'+fmt(f.size)+'</div></div><a class="dl" href="'+base+'/f/'+f.i+'" download>İndir</a></div>').join(''):'<div class="empty">Bilgisayarda "Telefona gönder" ile dosya seçilince burada görünür.</div>'}}}catch(e){}setTimeout(poll,2500)}
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
