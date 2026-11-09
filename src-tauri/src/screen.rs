//! Tek tıkla ekran paylaşımı (`node` modülünün alt modülü).
//!
//! Akış:
//! 1. Paylaşan "Ekranımı paylaş" der: bir oturum açılır ve seçilen kişilere `ScreenOffer` gider.
//! 2. İzleyici kabul ederse paylaşana bağlanır (`ScreenWatch`). Paylaşan, izleyicinin bu oturuma
//!    davetli olduğunu TLS parmak iziyle doğrular; değilse reddeder.
//! 3. Ana ekran saniyede ~8 kez yakalanır, JPEG'e sıkıştırılır ve `u32 uzunluk + JPEG` olarak
//!    akıtılır (uzunluk 0 = bitti). Bağlantı diğer her şey gibi karşılıklı TLS ile şifrelidir.
//! 4. Ekran yalnızca en az bir izleyici bağlıyken yakalanır; tek yakalama tüm izleyicilere gider.

use super::*;
use tokio::sync::watch;

/// Hedef kare hızı.
const FPS: u64 = 8;
/// Bundan geniş ekranlar yarıya küçültülür (ağ ve işlemci yükü).
const MAX_WIDTH: u32 = 2200;
const JPEG_QUALITY: u8 = 62;
/// Bir JPEG karesinin kabul edilen en büyük boyutu (izleyici tarafında).
const MAX_FRAME_BYTES: usize = 12 * 1024 * 1024;
/// Kabul edilmeyen davet bu süre sonra unutulur.
const OFFER_TTL: Duration = Duration::from_secs(10 * 60);

pub struct Frame {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

struct Sharing {
    session: String,
    allowed: Vec<String>,
    viewers: Vec<String>,
    stop: CancellationToken,
    frames: watch::Sender<Option<Arc<Frame>>>,
}

#[derive(Default)]
pub struct State {
    sharing: Mutex<Option<Sharing>>,
    /// Bana gelen davetler: oturum → (paylaşanın parmak izi, zaman).
    offers: Mutex<HashMap<String, (String, Instant)>>,
    /// İzlediğim oturumlar: oturum → durdurma jetonu.
    watching: Mutex<HashMap<String, CancellationToken>>,
}

/// Ekranı yakalar ve JPEG'e çevirir (bloklayan iş; ayrı iş parçacığında çalışır).
fn capture(monitor: &xcap::Monitor) -> Result<Frame, String> {
    let img = monitor.capture_image().map_err(|e| e.to_string())?;
    let (mut w, mut h) = (img.width(), img.height());
    let mut rgba = img.into_raw();
    if w > MAX_WIDTH {
        // 2x2 piksel ortalamasıyla yarıya küçült.
        let (nw, nh) = (w / 2, h / 2);
        let mut out = vec![0u8; (nw * nh * 4) as usize];
        let stride = (w * 4) as usize;
        for y in 0..nh as usize {
            for x in 0..nw as usize {
                let i0 = (2 * y) * stride + (2 * x) * 4;
                let i1 = i0 + stride;
                let o = (y * nw as usize + x) * 4;
                for c in 0..3 {
                    let sum = rgba[i0 + c] as u32 + rgba[i0 + 4 + c] as u32 + rgba[i1 + c] as u32 + rgba[i1 + 4 + c] as u32;
                    out[o + c] = (sum / 4) as u8;
                }
                out[o + 3] = 255;
            }
        }
        rgba = out;
        w = nw;
        h = nh;
    }
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err("Ekran çok büyük".into());
    }
    let mut jpeg = Vec::with_capacity((w * h / 6) as usize);
    jpeg_encoder::Encoder::new(&mut jpeg, JPEG_QUALITY)
        .encode(&rgba, w as u16, h as u16, jpeg_encoder::ColorType::Rgba)
        .map_err(|e| e.to_string())?;
    Ok(Frame { jpeg, width: w, height: h })
}

fn primary_monitor() -> Result<xcap::Monitor, String> {
    let all = xcap::Monitor::all().map_err(|e| format!("Ekran bulunamadı: {e}"))?;
    let primary = all.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
    all.into_iter().nth(primary).ok_or_else(|| "Ekran bulunamadı".to_string())
}

impl Node {
    fn emit_sharing(&self) {
        let info = self.extras.screen.sharing.lock().unwrap().as_ref().map(|s| (s.session.clone(), s.viewers.clone(), s.allowed.len()));
        let payload = match info {
            Some((session, viewers, invited)) => {
                let names: Vec<String> = viewers.iter().map(|fp| self.display_name(fp)).collect();
                json!({ "active": true, "session": session, "viewers": names, "invited": invited })
            }
            None => json!({ "active": false }),
        };
        self.emit("screen-sharing", payload);
    }

    fn display_name(&self, fp: &str) -> String {
        self.peers()
            .into_iter()
            .find(|p| p.fingerprint == fp)
            .map(|p| p.name)
            .or_else(|| self.chats.lock().unwrap().conversation_meta(fp).map(|m| m.0))
            .unwrap_or_else(|| "Biri".into())
    }

    /// Ekran paylaşımını başlatır: `key` birebir sohbetse o kişi, grupsa tüm üyeler davet edilir.
    /// Zaten paylaşılıyorsa yeni kişiler aynı oturuma eklenir.
    pub fn screen_share_start(self: &Arc<Self>, key: &str) -> Result<String, String> {
        let me = self.identity.fingerprint.clone();
        let targets: Vec<String> = match self.group_of(key) {
            Some((g, left)) => {
                if left || g.deleted {
                    return Err("Bu grubun üyesi değilsiniz".into());
                }
                g.members.iter().map(|m| m.fingerprint.clone()).filter(|f| *f != me).collect()
            }
            None if valid_fingerprint(key) => vec![key.to_string()],
            None => return Err("Geçersiz kişi".into()),
        };
        let online: Vec<Peer> = self.peers().into_iter().filter(|p| !p.blocked && targets.contains(&p.fingerprint)).collect();
        if online.is_empty() {
            return Err("Paylaşılacak kişi çevrimiçi değil".into());
        }
        // Ekran yakalanabiliyor mu? (Hata varsa davet göndermeden söyle.)
        primary_monitor()?;
        let (session, fresh) = {
            let mut guard = self.extras.screen.sharing.lock().unwrap();
            match guard.as_mut() {
                Some(s) => {
                    for t in &targets {
                        if !s.allowed.contains(t) {
                            s.allowed.push(t.clone());
                        }
                    }
                    (s.session.clone(), false)
                }
                None => {
                    let (tx, _) = watch::channel(None);
                    let s = Sharing {
                        session: uuid::Uuid::new_v4().to_string(),
                        allowed: targets.clone(),
                        viewers: Vec::new(),
                        stop: self.shutdown.child_token(),
                        frames: tx,
                    };
                    let id = s.session.clone();
                    *guard = Some(s);
                    (id, true)
                }
            }
        };
        if fresh {
            self.spawn_capture();
        }
        for peer in online {
            let node = self.clone();
            let session = session.clone();
            tokio::spawn(async move {
                let msg = Message::ScreenOffer { device: node.device_info(), session };
                if let Ok(mut s) = node.connect(&node.candidates(&peer), Some(peer.fingerprint.clone())).await {
                    if write_msg(&mut s, &msg).await.is_ok() {
                        let _ = timeout(Duration::from_secs(5), read_msg_max(&mut s, 4096)).await;
                    }
                    let _ = s.shutdown().await;
                }
            });
        }
        self.emit_sharing();
        Ok(session)
    }

    /// Yakalama döngüsü: izleyici varken saniyede ~8 kare üretir.
    fn spawn_capture(self: &Arc<Self>) {
        let Some((stop, tx)) = self.extras.screen.sharing.lock().unwrap().as_ref().map(|s| (s.stop.clone(), s.frames.clone())) else {
            return;
        };
        std::thread::spawn(move || {
            let monitor = match primary_monitor() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[fastfiles] ekran paylaşımı: {e}");
                    return;
                }
            };
            let interval = Duration::from_millis(1000 / FPS);
            while !stop.is_cancelled() {
                if tx.receiver_count() == 0 {
                    std::thread::sleep(Duration::from_millis(150));
                    continue;
                }
                let t0 = Instant::now();
                match capture(&monitor) {
                    Ok(f) => {
                        tx.send_replace(Some(Arc::new(f)));
                    }
                    Err(e) => {
                        eprintln!("[fastfiles] ekran yakalanamadı: {e}");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
                if let Some(rest) = interval.checked_sub(t0.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        });
    }

    pub fn screen_share_stop(self: &Arc<Self>) {
        let Some(s) = self.extras.screen.sharing.lock().unwrap().take() else { return };
        s.stop.cancel();
        let targets = s.allowed.clone();
        for peer in self.peers().into_iter().filter(|p| targets.contains(&p.fingerprint)) {
            let node = self.clone();
            let session = s.session.clone();
            tokio::spawn(async move {
                let msg = Message::ScreenEnd { device: node.device_info(), session };
                if let Ok(mut st) = node.connect(&node.candidates(&peer), Some(peer.fingerprint.clone())).await {
                    if write_msg(&mut st, &msg).await.is_ok() {
                        let _ = timeout(Duration::from_secs(5), read_msg_max(&mut st, 4096)).await;
                    }
                    let _ = st.shutdown().await;
                }
            });
        }
        self.emit_sharing();
    }

    /// İzleyiciye kareleri akıtır (paylaşan tarafta).
    pub(super) async fn serve_screen(&self, viewer: String, session: String, mut wr: SrvWrite) -> Result<(), String> {
        let found = {
            let mut guard = self.extras.screen.sharing.lock().unwrap();
            match guard.as_mut() {
                Some(s) if s.session == session && s.allowed.contains(&viewer) && !self.is_blocked(&viewer) => {
                    s.viewers.push(viewer.clone());
                    Some((s.frames.subscribe(), s.stop.clone()))
                }
                _ => None,
            }
        };
        let Some((mut rx, stop)) = found else {
            let _ = write_msg(&mut wr, &Message::ChatReject { reason: "denied".into() }).await;
            return Ok(());
        };
        self.emit_sharing();
        let result = async {
            // İlk kareyi bekle (yakalama döngüsü izleyici gelince başlar).
            let first = loop {
                if let Some(f) = rx.borrow_and_update().clone() {
                    break f;
                }
                tokio::select! {
                    r = rx.changed() => if r.is_err() { return Ok(()) },
                    _ = stop.cancelled() => return Ok(()),
                    _ = sleep(Duration::from_secs(10)) => return Err("Ekran yakalanamadı".to_string()),
                }
            };
            write_msg(&mut wr, &Message::ScreenStart { width: first.width, height: first.height }).await.map_err(|e| e.to_string())?;
            let mut frame = first;
            loop {
                wr.write_all(&(frame.jpeg.len() as u32).to_be_bytes()).await.map_err(|e| e.to_string())?;
                wr.write_all(&frame.jpeg).await.map_err(|e| e.to_string())?;
                wr.flush().await.map_err(|e| e.to_string())?;
                tokio::select! {
                    r = rx.changed() => if r.is_err() { break },
                    _ = stop.cancelled() => break,
                }
                match rx.borrow_and_update().clone() {
                    Some(f) => frame = f,
                    None => break,
                }
            }
            let _ = wr.write_all(&0u32.to_be_bytes()).await;
            let _ = wr.flush().await;
            Ok(())
        }
        .await;
        if let Some(s) = self.extras.screen.sharing.lock().unwrap().as_mut() {
            if let Some(i) = s.viewers.iter().position(|v| *v == viewer) {
                s.viewers.remove(i);
            }
        }
        self.emit_sharing();
        result
    }

    // ───────────── İzleyici tarafı ─────────────

    pub(super) fn accept_screen_offer(&self, device: &DeviceInfo, session: &str) {
        let fp = device.fingerprint.clone();
        if self.is_blocked(&fp) || !valid_message_id(session) {
            return;
        }
        {
            let mut offers = self.extras.screen.offers.lock().unwrap();
            offers.retain(|_, (_, t)| t.elapsed() < OFFER_TTL);
            // Aynı kişiden aynı anda tek davet.
            offers.retain(|_, (from, _)| *from != fp);
            offers.insert(session.to_string(), (fp.clone(), Instant::now()));
        }
        self.emit("screen-offer", json!({ "session": session, "fingerprint": fp, "name": sanitize_name(&device.name) }));
    }

    pub(super) fn accept_screen_end(&self, fp: &str, session: &str) {
        let removed = {
            let mut offers = self.extras.screen.offers.lock().unwrap();
            match offers.get(session) {
                Some((from, _)) if from == fp => offers.remove(session).is_some(),
                _ => false,
            }
        };
        if removed {
            if let Some(t) = self.extras.screen.watching.lock().unwrap().remove(session) {
                t.cancel();
            }
            self.emit("screen-end", json!({ "session": session }));
        }
    }

    /// Paylaşıma bağlanır ve her kareyi `on_frame`'e verir; paylaşım bitince döner.
    /// `on_frame` false döndürürse (pencere kapandı) izleme biter.
    pub async fn screen_watch(
        self: &Arc<Self>,
        session: &str,
        mut on_start: impl FnMut(u32, u32) + Send,
        mut on_frame: impl FnMut(Vec<u8>) -> bool + Send,
    ) -> Result<(), String> {
        let sharer = self.extras.screen.offers.lock().unwrap().get(session).map(|o| o.0.clone()).ok_or("Bu paylaşım artık yok")?;
        let peer = self.peers().into_iter().find(|p| p.fingerprint == sharer).ok_or("Paylaşan kişi çevrimdışı")?;
        let stop = self.shutdown.child_token();
        if let Some(old) = self.extras.screen.watching.lock().unwrap().insert(session.to_string(), stop.clone()) {
            old.cancel();
        }
        let result = async {
            let mut s = self.connect(&self.candidates(&peer), Some(peer.fingerprint.clone())).await?;
            write_msg(&mut s, &Message::ScreenWatch { device: self.device_info(), session: session.to_string() })
                .await
                .map_err(|e| e.to_string())?;
            let start = tokio::select! {
                r = timeout(Duration::from_secs(15), read_msg_max(&mut s, 4096)) => r.map_err(|_| "Paylaşan yanıt vermedi".to_string())?.map_err(|e| e.to_string())?,
                _ = stop.cancelled() => return Ok(()),
            };
            match start {
                Message::ScreenStart { width, height } => on_start(width, height),
                Message::ChatReject { .. } => return Err("Paylaşan izin vermedi".into()),
                _ => return Err("Beklenmeyen yanıt".into()),
            }
            loop {
                let mut len = [0u8; 4];
                tokio::select! {
                    r = s.read_exact(&mut len) => { r.map_err(|_| "Bağlantı koptu".to_string())?; }
                    _ = stop.cancelled() => break,
                }
                let len = u32::from_be_bytes(len) as usize;
                if len == 0 {
                    break;
                }
                if len > MAX_FRAME_BYTES {
                    return Err("Geçersiz kare".into());
                }
                let mut buf = vec![0u8; len];
                tokio::select! {
                    r = s.read_exact(&mut buf) => { r.map_err(|_| "Bağlantı koptu".to_string())?; }
                    _ = stop.cancelled() => break,
                }
                if !on_frame(buf) {
                    break;
                }
            }
            let _ = s.shutdown().await;
            Ok(())
        }
        .await;
        self.extras.screen.watching.lock().unwrap().remove(session);
        result
    }

    pub fn screen_watch_stop(&self, session: &str) {
        if let Some(t) = self.extras.screen.watching.lock().unwrap().remove(session) {
            t.cancel();
        }
    }

    /// Daveti reddet / kapat.
    pub fn screen_dismiss(&self, session: &str) {
        self.extras.screen.offers.lock().unwrap().remove(session);
        self.screen_watch_stop(session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gerçek ekranı yakalar (yalnızca masaüstü oturumunda anlamlı): `cargo test -- --ignored capture`
    #[test]
    #[ignore]
    fn capture_primary_screen() {
        let m = primary_monitor().unwrap();
        let t = Instant::now();
        let mut last = None;
        for _ in 0..5 {
            last = Some(capture(&m).unwrap());
        }
        let f = last.unwrap();
        let per = t.elapsed() / 5;
        eprintln!("kare: {}x{} {} KB, {:?}/kare", f.width, f.height, f.jpeg.len() / 1024, per);
        assert!(f.jpeg.starts_with(&[0xFF, 0xD8]));
        assert!(f.width <= MAX_WIDTH);
    }
}
