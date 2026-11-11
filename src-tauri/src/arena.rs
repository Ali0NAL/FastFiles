//! Arena maçının gerçek zamanlı kanalı (`node` modülünün alt modülü).
//!
//! Davet ve oda her oyundaki gibidir (`game.rs`). Oda açılınca **oda sahibinin** bilgisayarı maç
//! sunucusu olur:
//! - Diğer oyuncular sahibe kalıcı, şifreli (TLS) bir bağlantı açar (`Message::ArenaJoin`), sonra
//!   iki yönlü küçük JSON çerçeveler akar: oyuncudan konum/bakış (~30/sn) ve atışlar, sahipten
//!   herkesin durumu (30/sn) ve olaylar (atış, vuruş, öldürme, doğma).
//! - Sahibin kendisi de oyuncudur; onun girdileri ağa çıkmadan doğrudan maça uygulanır.
//!
//! Yetki: hareketi her oyuncu kendi bilgisayarında hesaplar (akıcı olsun diye); sahip yalnızca
//! makul olmayanı reddeder (harita dışı, ışınlanma, imkânsız hız). Vuruşu atan bilgisayar bulur,
//! sahip doğrular: silahın atış hızı, menzil, hedefin sahipteki konumunun atış çizgisine yakınlığı
//! ve arada duvar olup olmadığı (`games::arena::check_hit`). Can, ölüm, yeniden doğma, skor ve
//! maç sonu yalnızca sahiptedir; sonuç odaya yazılır ve herkese her oyundaki gibi iletilir.
//!
//! Dayanıklılık: her bağlantının giden kuyruğu sınırlıdır (dolarsa o tur atlanır, maç beklemez),
//! yazmalar zaman aşımlıdır, 10 sn hiçbir şey göndermeyen oyuncu düşürülür. Herkes ayrılırsa maç
//! bir süre sonra o anki skorla biter.

use super::*;
use crate::games::arena::{self as rules, V3};
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_millis(33);
/// Bağlantı başına bekleyen giden çerçeve sınırı (yaklaşık 1 sn).
const SEND_QUEUE: usize = 32;
const MAX_UP_FRAME: usize = 16 * 1024;
const MAX_DOWN_FRAME: usize = 512 * 1024;
/// Oyuncu bu kadar hiçbir şey göndermezse bağlantısı kopmuş sayılır.
const READ_IDLE: Duration = Duration::from_secs(10);
/// Maç başladıktan sonra kimse bağlı değilse bu kadar sonra o anki skorla biter.
const EMPTY_END: Duration = Duration::from_secs(90);

// ───────────── Paketler ─────────────

/// Oyuncudan sahibe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Up {
    /// Konum (ayak), bakış, seçili silah, çömelme. `sp`: oyuncunun bildiği doğma sayacı; sahip
    /// yeniden doğurduktan sonra eski konumla gelen girdiler yok sayılır.
    Input { p: V3, yaw: f32, pitch: f32, w: u8, c: bool, sp: u32 },
    /// Atış: göz konumu `o`, yön `d`, atanın bulduğu vuruşlar.
    Fire { w: u8, o: V3, d: V3, #[serde(default)] hits: Vec<Hit> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    /// Vurulan koltuk.
    pub s: usize,
    /// Kafaya mı?
    #[serde(default)]
    pub h: bool,
    /// İsabet eden saçma sayısı (tüfek ve keskin nişancıda 1).
    #[serde(default = "one")]
    pub n: u8,
    /// Bu vuruşu yapan saçmanın yönü (pompalıda her saçma biraz sapar); yoksa `d`.
    #[serde(default)]
    pub d: Option<V3>,
}

fn one() -> u8 {
    1
}

/// Sahipten oyunculara.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Down {
    Welcome { seat: usize, map: String },
    Snap {
        /// Maçın bitmesine kalan süre (ms); başlamadıysa tam süre.
        left: u64,
        started: bool,
        players: Vec<PView>,
        ev: Vec<Ev>,
    },
    End { winner: Option<usize>, stats: Vec<[u32; 2]> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PView {
    pub s: usize,
    pub p: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub c: bool,
    pub w: u8,
    pub hp: f32,
    pub alive: bool,
    pub k: u32,
    pub d: u32,
    pub sp: u32,
    /// Bağlı mı?
    pub on: bool,
    /// Ölüyse yeniden doğmaya kalan süre (ms).
    pub respawn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "camelCase")]
pub enum Ev {
    Fire { s: usize, w: u8, o: V3, d: V3 },
    Hit { by: usize, to: usize, dmg: f32, head: bool },
    Kill { by: usize, to: usize, w: u8, head: bool },
    Spawn { s: usize },
    Join { s: usize },
    Leave { s: usize },
    Start,
}

fn frame(v: &impl Serialize) -> Option<Arc<Vec<u8>>> {
    let body = serde_json::to_vec(v).ok()?;
    let mut f = Vec::with_capacity(body.len() + 4);
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(&body);
    Some(Arc::new(f))
}

async fn read_frame<R: AsyncReadExt + Unpin, T: serde::de::DeserializeOwned>(r: &mut R, max: usize) -> Result<T, String> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.map_err(|e| e.to_string())?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err("çerçeve çok büyük".into());
    }
    let mut body = Vec::with_capacity(len.min(64 * 1024));
    let got = (&mut *r).take(len as u64).read_to_end(&mut body).await.map_err(|e| e.to_string())?;
    if got != len {
        return Err("çerçeve yarım".into());
    }
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

// ───────────── Durum ─────────────

#[derive(Default)]
pub struct State {
    /// (Sahip) süren maçlar: oda → maç.
    matches: Mutex<HashMap<String, Arc<Match>>>,
    /// (Oyuncu) bağlı olduğum maç.
    client: Mutex<Option<Client>>,
    conn_seq: std::sync::atomic::AtomicU64,
}

struct Client {
    room_id: String,
    /// Sahipsem `None`: girdiler doğrudan maça uygulanır.
    tx: Option<mpsc::Sender<Up>>,
    stop: CancellationToken,
}

struct Match {
    room_id: String,
    map: &'static rules::MapDef,
    stop: CancellationToken,
    inner: Mutex<MatchInner>,
}

struct MatchInner {
    players: Vec<PState>,
    /// Bağlı oyuncular: koltuk → (bağlantı numarası, giden çerçeve kuyruğu).
    conns: HashMap<usize, (u64, mpsc::Sender<Arc<Vec<u8>>>)>,
    /// Sahip arayüzden bağlıysa koltuğu.
    local: Option<usize>,
    started: Option<Instant>,
    /// Kimsenin bağlı olmadığı andan beri.
    empty_since: Option<Instant>,
    events: Vec<Ev>,
    ended: bool,
    spawn_pick: usize,
}

#[derive(Default)]
struct PState {
    connected: bool,
    quit: bool,
    alive: bool,
    hp: f32,
    pos: V3,
    yaw: f32,
    pitch: f32,
    crouch: bool,
    w: u8,
    kills: u32,
    deaths: u32,
    last_fire: [Option<Instant>; 3],
    last_damage: Option<Instant>,
    respawn_at: Option<Instant>,
    sp: u32,
    last_input: Option<Instant>,
}

impl MatchInner {
    fn spawn(&mut self, map: &rules::MapDef, s: usize) {
        let others: Vec<V3> = self.players.iter().enumerate().filter(|(i, p)| *i != s && p.alive).map(|(_, p)| p.pos).collect();
        let pos = rules::pick_spawn(map, &others, self.spawn_pick);
        self.spawn_pick = self.spawn_pick.wrapping_add(1);
        let p = &mut self.players[s];
        p.pos = pos;
        p.alive = true;
        p.hp = rules::MAX_HP;
        p.respawn_at = None;
        p.last_damage = None;
        p.last_input = None;
        p.sp = p.sp.wrapping_add(1);
        // Doğduğu yerde haritanın ortasına baksın (ileri yön: (-sin yaw, -cos yaw)).
        p.yaw = pos[0].atan2(pos[2]);
        p.pitch = 0.0;
        self.events.push(Ev::Spawn { s });
    }

    fn connected_count(&self) -> usize {
        self.players.iter().filter(|p| p.connected && !p.quit).count()
    }

    fn join(&mut self, map: &rules::MapDef, s: usize) {
        let p = &mut self.players[s];
        p.connected = true;
        self.empty_since = None;
        self.events.push(Ev::Join { s });
        if !self.players[s].alive && self.players[s].respawn_at.is_none() {
            self.spawn(map, s);
        }
    }

    fn leave(&mut self, s: usize) {
        if let Some(p) = self.players.get_mut(s) {
            if p.connected {
                p.connected = false;
                self.events.push(Ev::Leave { s });
            }
        }
    }

    fn input(&mut self, map: &rules::MapDef, s: usize, p: V3, yaw: f32, pitch: f32, w: u8, c: bool, sp: u32) {
        let now = Instant::now();
        let Some(pl) = self.players.get_mut(s) else { return };
        if !pl.connected || pl.quit || !(yaw.is_finite() && pitch.is_finite()) {
            return;
        }
        pl.yaw = yaw;
        pl.pitch = pitch.clamp(-1.6, 1.6);
        if (w as usize) < rules::WEAPONS.len() {
            pl.w = w;
        }
        pl.crouch = c;
        if !pl.alive || sp != pl.sp || !rules::finite(p) {
            return;
        }
        let p = rules::clamp_to_map(map, p);
        // Işınlanma/hız denetimi: son girdiden bu yana gidilebilecek yoldan fazlası reddedilir.
        let dt = pl.last_input.map_or(0.25, |t| now.duration_since(t).as_secs_f32().clamp(0.0, 1.0));
        let moved = ((p[0] - pl.pos[0]).powi(2) + (p[2] - pl.pos[2]).powi(2)).sqrt();
        if moved <= rules::MAX_SPEED * dt + 1.0 {
            pl.pos = p;
        }
        pl.last_input = Some(now);
    }

    fn fire(&mut self, map: &rules::MapDef, s: usize, w: u8, o: V3, d: V3, hits: Vec<Hit>) {
        let now = Instant::now();
        let Some(weapon) = rules::weapon(w) else { return };
        let Some(d) = rules::normalize(d) else { return };
        let Some(sh) = self.players.get_mut(s) else { return };
        if !sh.connected || sh.quit || !sh.alive || !rules::finite(o) {
            return;
        }
        // Atış hızı (ağ dalgalanması için %30 pay).
        let min_gap = Duration::from_millis(weapon.interval_ms * 7 / 10);
        if sh.last_fire[w as usize].is_some_and(|t| now.duration_since(t) < min_gap) {
            return;
        }
        // Atış, oyuncunun bilinen göz konumundan yapılmalı.
        let eye = [sh.pos[0], sh.pos[1] + if sh.crouch { rules::CROUCH_EYE } else { rules::EYE }, sh.pos[2]];
        if rules::len([o[0] - eye[0], o[1] - eye[1], o[2] - eye[2]]) > 2.5 {
            return;
        }
        sh.last_fire[w as usize] = Some(now);
        self.events.push(Ev::Fire { s, w, o, d });
        let mut pellets_left = weapon.pellets as i32;
        let mut seen = HashSet::new();
        for hit in hits.into_iter().take(8) {
            if hit.s == s || !seen.insert(hit.s) || hit.s >= self.players.len() {
                continue;
            }
            let n = (hit.n.max(1) as i32).min(pellets_left);
            if n <= 0 {
                break;
            }
            pellets_left -= n;
            let hd = match hit.d.and_then(rules::normalize) {
                // Saçmanın yönü ana yönden fazla sapamaz.
                Some(hd) if hd[0] * d[0] + hd[1] * d[1] + hd[2] * d[2] >= (weapon.spread_move * 3.0 + 0.02).cos() => hd,
                _ => d,
            };
            let t = &self.players[hit.s];
            if !t.alive || !t.connected {
                continue;
            }
            let Some((dist, head_zone)) = rules::check_hit(map, weapon, o, hd, t.pos, t.crouch) else { continue };
            let head = hit.h && head_zone;
            let dmg = weapon.damage * n as f32 * if head { weapon.head_mult } else { 1.0 } * rules::falloff(weapon, dist);
            self.damage(s, hit.s, dmg, head, w, now);
        }
    }

    fn damage(&mut self, by: usize, to: usize, dmg: f32, head: bool, w: u8, now: Instant) {
        let t = &mut self.players[to];
        t.hp -= dmg;
        t.last_damage = Some(now);
        self.events.push(Ev::Hit { by, to, dmg, head });
        if t.hp <= 0.0 {
            t.hp = 0.0;
            t.alive = false;
            t.deaths += 1;
            t.respawn_at = Some(now + Duration::from_millis(rules::RESPAWN_MS));
            if by != to {
                self.players[by].kills += 1;
            }
            self.events.push(Ev::Kill { by, to, w, head });
        }
    }

    /// Bir tur: doğma, can yenilenmesi, başlama ve bitiş denetimi.
    fn step(&mut self, map: &rules::MapDef, dt: f32) -> Option<(Option<usize>, &'static str)> {
        let now = Instant::now();
        for s in 0..self.players.len() {
            let p = &mut self.players[s];
            if p.quit {
                p.alive = false;
                continue;
            }
            if !p.alive && p.connected && p.respawn_at.is_some_and(|t| now >= t) {
                self.spawn(map, s);
                continue;
            }
            let p = &mut self.players[s];
            if p.alive && p.hp < rules::MAX_HP && p.last_damage.map_or(true, |t| now.duration_since(t).as_millis() as u64 >= rules::REGEN_DELAY_MS) {
                p.hp = (p.hp + rules::REGEN_PER_SEC * dt).min(rules::MAX_HP);
            }
        }
        let connected = self.connected_count();
        if self.started.is_none() && connected >= 2 {
            // İki kişi olunca maç başlar: skorlar sıfırlanır, bağlı herkes yeniden doğar.
            self.started = Some(now);
            self.events.push(Ev::Start);
            for s in 0..self.players.len() {
                let p = &mut self.players[s];
                p.kills = 0;
                p.deaths = 0;
                if p.connected && !p.quit {
                    self.spawn(map, s);
                }
            }
        }
        let started = self.started?;
        if connected == 0 {
            let since = *self.empty_since.get_or_insert(now);
            if now.duration_since(since) >= EMPTY_END {
                return Some(self.winner());
            }
        }
        if now.duration_since(started) >= Duration::from_secs(rules::MATCH_SECS) || self.players.iter().any(|p| p.kills >= rules::KILL_LIMIT) {
            return Some(self.winner());
        }
        None
    }

    /// En çok öldüren kazanır; eşitlikte daha az ölen; yine eşitse berabere.
    fn winner(&self) -> (Option<usize>, &'static str) {
        let key = |p: &PState| (p.kills, std::cmp::Reverse(p.deaths));
        let best = self.players.iter().filter(|p| !p.quit).map(key).max();
        let tops: Vec<usize> = (0..self.players.len()).filter(|&s| !self.players[s].quit && Some(key(&self.players[s])) == best).collect();
        if tops.len() == 1 {
            (Some(tops[0]), "kills")
        } else {
            (None, "kills_tie")
        }
    }

    fn stats(&self) -> Vec<[u32; 2]> {
        self.players.iter().map(|p| [p.kills, p.deaths]).collect()
    }

    fn snapshot(&mut self) -> Down {
        let now = Instant::now();
        let total = Duration::from_secs(rules::MATCH_SECS);
        let left = self.started.map_or(total, |t| total.saturating_sub(now.duration_since(t)));
        Down::Snap {
            left: left.as_millis() as u64,
            started: self.started.is_some(),
            players: self
                .players
                .iter()
                .enumerate()
                .map(|(s, p)| PView {
                    s,
                    p: p.pos,
                    yaw: p.yaw,
                    pitch: p.pitch,
                    c: p.crouch,
                    w: p.w,
                    hp: p.hp,
                    alive: p.alive,
                    k: p.kills,
                    d: p.deaths,
                    sp: p.sp,
                    on: p.connected && !p.quit,
                    respawn: p.respawn_at.map_or(0, |t| t.saturating_duration_since(now).as_millis() as u64),
                })
                .collect(),
            ev: std::mem::take(&mut self.events),
        }
    }
}

impl Node {
    /// Arayüze giden statik veri: haritalar, silahlar, kurallar.
    pub fn arena_info(&self) -> Value {
        rules::info_json()
    }

    /// (Sahip) odanın maçı; yoksa başlatır. Oda sürüyor olmalı ve sahibi ben olmalıyım.
    fn arena_match(self: &Arc<Self>, room_id: &str) -> Result<Arc<Match>, String> {
        if let Some(m) = self.extras.arena.matches.lock().unwrap().get(room_id) {
            return Ok(m.clone());
        }
        let room = self.game_room(room_id).ok_or("Oyun bulunamadı")?;
        if room.game != "arena" {
            return Err("Bu bir arena oyunu değil".into());
        }
        if room.host != self.identity.fingerprint {
            return Err("Maçı oda sahibi yönetir".into());
        }
        if room.status != game::RoomStatus::Active {
            return Err("Maç şu an oynanmıyor".into());
        }
        let map = rules::map(&room.variant).or_else(|| rules::maps().first()).ok_or("Harita yok")?;
        let quit: HashSet<usize> = room.moves.iter().filter_map(|m| m.strip_prefix('q').and_then(|s| s.parse().ok())).collect();
        let m = Arc::new(Match {
            room_id: room_id.to_string(),
            map,
            stop: self.shutdown.child_token(),
            inner: Mutex::new(MatchInner {
                players: (0..room.players.len()).map(|s| PState { quit: quit.contains(&s), ..Default::default() }).collect(),
                conns: HashMap::new(),
                local: None,
                started: None,
                empty_since: None,
                events: Vec::new(),
                ended: false,
                spawn_pick: room.id.bytes().map(|b| b as usize).sum(),
            }),
        });
        {
            let mut all = self.extras.arena.matches.lock().unwrap();
            if let Some(existing) = all.get(room_id) {
                return Ok(existing.clone());
            }
            all.insert(room_id.to_string(), m.clone());
        }
        if tokio::runtime::Handle::try_current().is_ok() {
            let node = self.clone();
            let mm = m.clone();
            tokio::spawn(async move { node.arena_loop(mm).await });
        }
        Ok(m)
    }

    async fn arena_loop(self: Arc<Self>, m: Arc<Match>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last = Instant::now();
        let mut n: u64 = 0;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = m.stop.cancelled() => break,
            }
            n += 1;
            // Saniyede bir oda denetlenir: oyundan çekilenler ve oda kapanmışsa maç da biter.
            if n % 30 == 0 {
                match self.game_room(&m.room_id) {
                    Some(room) if room.status == game::RoomStatus::Active => {
                        let mut inner = m.inner.lock().unwrap();
                        for mv in &room.moves {
                            if let Some(s) = mv.strip_prefix('q').and_then(|s| s.parse::<usize>().ok()) {
                                if let Some(p) = inner.players.get_mut(s) {
                                    p.quit = true;
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f32().min(0.25);
            last = now;
            let (snap, conns, local, end) = {
                let mut inner = m.inner.lock().unwrap();
                if inner.ended {
                    break;
                }
                let end = inner.step(m.map, dt);
                let snap = inner.snapshot();
                let conns: Vec<_> = inner.conns.values().map(|(_, tx)| tx.clone()).collect();
                (snap, conns, inner.local.is_some(), end.map(|e| (e, inner.stats())))
            };
            if let Some(f) = frame(&snap) {
                for tx in &conns {
                    let _ = tx.try_send(f.clone());
                }
            }
            if local {
                self.emit("arena", json!(snap));
            }
            if let Some(((winner, reason), stats)) = end {
                m.inner.lock().unwrap().ended = true;
                let end = Down::End { winner, stats: stats.clone() };
                if let Some(f) = frame(&end) {
                    for tx in &conns {
                        let _ = tx.try_send(f.clone());
                    }
                }
                if local {
                    self.emit("arena", json!(end));
                }
                self.game_finish_realtime(&m.room_id, winner, reason, stats);
                // Son çerçevelerin gitmesine fırsat ver.
                sleep(Duration::from_secs(2)).await;
                break;
            }
        }
        m.stop.cancel();
        let mut all = self.extras.arena.matches.lock().unwrap();
        if all.get(&m.room_id).is_some_and(|x| Arc::ptr_eq(x, &m)) {
            all.remove(&m.room_id);
        }
    }

    /// (Sahip) bir oyuncunun maç bağlantısı (`Message::ArenaJoin` ile gelir).
    pub(super) async fn serve_arena(self: Arc<Self>, client_fp: String, room_id: String, mut rd: SrvRead, mut wr: SrvWrite) -> Result<(), String> {
        let refuse = |reason: &str| Message::ChatReject { reason: reason.into() };
        if self.is_blocked(&client_fp) {
            let _ = write_msg(&mut wr, &refuse("blocked")).await;
            return Ok(());
        }
        let m = match self.arena_match(&room_id) {
            Ok(m) => m,
            Err(e) => {
                let _ = write_msg(&mut wr, &refuse(&e)).await;
                return Ok(());
            }
        };
        let seat = self.game_room(&room_id).and_then(|r| r.players.iter().position(|p| p.fingerprint == client_fp));
        let Some(seat) = seat.filter(|&s| m.inner.lock().unwrap().players.get(s).is_some_and(|p| !p.quit)) else {
            let _ = write_msg(&mut wr, &refuse("not_player")).await;
            return Ok(());
        };
        let welcome = frame(&Down::Welcome { seat, map: m.map.id.to_string() }).ok_or("çerçeve")?;
        wr.write_all(&welcome).await.map_err(|e| e.to_string())?;
        let (tx, mut rx) = mpsc::channel::<Arc<Vec<u8>>>(SEND_QUEUE);
        let id = self.extras.arena.conn_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut inner = m.inner.lock().unwrap();
            // Aynı oyuncunun eski bağlantısı varsa yerini yenisi alır (eski kuyruk kapanır).
            inner.conns.insert(seat, (id, tx));
            inner.join(m.map, seat);
        }
        let stop = m.stop.clone();
        let writer = tokio::spawn(async move {
            while let Some(f) = rx.recv().await {
                // Donan oyuncu maçı bekletmesin.
                match timeout(Duration::from_secs(5), wr.write_all(&f)).await {
                    Ok(Ok(())) => {}
                    _ => break,
                }
            }
            let _ = wr.shutdown().await;
        });
        loop {
            let up: Up = tokio::select! {
                r = timeout(READ_IDLE, read_frame(&mut rd, MAX_UP_FRAME)) => match r {
                    Ok(Ok(up)) => up,
                    _ => break,
                },
                _ = stop.cancelled() => break,
            };
            let mut inner = m.inner.lock().unwrap();
            // Yerimi yeni bir bağlantı aldıysa bu bağlantı biter.
            if inner.conns.get(&seat).map(|c| c.0) != Some(id) {
                break;
            }
            match up {
                Up::Input { p, yaw, pitch, w, c, sp } => inner.input(m.map, seat, p, yaw, pitch, w, c, sp),
                Up::Fire { w, o, d, hits } => inner.fire(m.map, seat, w, o, d, hits),
            }
        }
        {
            let mut inner = m.inner.lock().unwrap();
            if inner.conns.get(&seat).map(|c| c.0) == Some(id) {
                inner.conns.remove(&seat);
                inner.leave(seat);
            }
        }
        writer.abort();
        Ok(())
    }

    /// (Oyuncu) maça bağlanır. Sahipsem maç burada çalışır, değilsem sahibe bağlanılır.
    /// Dönen: `{ seat, map }`. Sonraki durumlar `arena` olayıyla gelir.
    pub async fn arena_connect(self: &Arc<Self>, room_id: &str) -> Result<Value, String> {
        self.arena_leave();
        let room = self.game_room(room_id).ok_or("Oyun bulunamadı")?;
        let me = self.identity.fingerprint.clone();
        let seat = room.players.iter().position(|p| p.fingerprint == me).ok_or("Bu oyunda oyuncu değilsiniz")?;
        if room.status != game::RoomStatus::Active {
            return Err("Maç şu an oynanmıyor".into());
        }
        if room.host == me {
            let m = self.arena_match(room_id)?;
            {
                let mut inner = m.inner.lock().unwrap();
                if inner.players.get(seat).is_some_and(|p| p.quit) {
                    return Err("Bu maçtan çekildiniz".into());
                }
                inner.local = Some(seat);
                inner.join(m.map, seat);
            }
            *self.extras.arena.client.lock().unwrap() =
                Some(Client { room_id: room_id.to_string(), tx: None, stop: m.stop.child_token() });
            return Ok(json!({ "seat": seat, "map": m.map.id }));
        }
        let host = self.peers().into_iter().find(|p| p.fingerprint == room.host).ok_or("Oda sahibi şu an çevrimdışı")?;
        let stream = self.connect(&self.candidates(&host), Some(host.fingerprint.clone())).await?;
        let (mut rd, mut wr) = tokio::io::split(stream);
        write_msg(&mut wr, &Message::ArenaJoin { device: self.device_info(), room_id: room_id.to_string() })
            .await
            .map_err(|e| e.to_string())?;
        // İlk çerçeve: kabul (Welcome) ya da ret (ChatReject, normal mesaj biçiminde).
        let mut len = [0u8; 4];
        timeout(Duration::from_secs(8), rd.read_exact(&mut len)).await.map_err(|_| "Oda sahibi yanıt vermedi".to_string())?.map_err(|e| e.to_string())?;
        let n = u32::from_be_bytes(len) as usize;
        if n > 64 * 1024 {
            return Err("geçersiz yanıt".into());
        }
        let mut body = vec![0u8; n];
        rd.read_exact(&mut body).await.map_err(|e| e.to_string())?;
        let (seat, map) = match serde_json::from_slice::<Down>(&body) {
            Ok(Down::Welcome { seat, map }) => (seat, map),
            _ => {
                let reason = serde_json::from_slice::<Message>(&body).ok().and_then(|m| match m {
                    Message::ChatReject { reason } => Some(reason),
                    _ => None,
                });
                return Err(match reason.as_deref() {
                    Some("not_player") => "Bu maçta oyuncu değilsiniz".into(),
                    Some(r) if !r.is_empty() => r.to_string(),
                    _ => "Maça girilemedi".into(),
                });
            }
        };
        let stop = self.shutdown.child_token();
        let (tx, mut rx) = mpsc::channel::<Up>(64);
        *self.extras.arena.client.lock().unwrap() = Some(Client { room_id: room_id.to_string(), tx: Some(tx), stop: stop.clone() });
        // Giden: girdiler ve atışlar.
        let wstop = stop.clone();
        tokio::spawn(async move {
            loop {
                let up = tokio::select! {
                    u = rx.recv() => match u { Some(u) => u, None => break },
                    _ = wstop.cancelled() => break,
                };
                let Some(f) = frame(&up) else { continue };
                match timeout(Duration::from_secs(5), wr.write_all(&f)).await {
                    Ok(Ok(())) => {}
                    _ => break,
                }
            }
            wstop.cancel();
            let _ = wr.shutdown().await;
        });
        // Gelen: durumlar arayüze aktarılır.
        let node = self.clone();
        let rid = room_id.to_string();
        tokio::spawn(async move {
            let reason = loop {
                let down: Down = tokio::select! {
                    r = timeout(READ_IDLE, read_frame(&mut rd, MAX_DOWN_FRAME)) => match r {
                        Ok(Ok(d)) => d,
                        Ok(Err(_)) => break "closed",
                        Err(_) => break "timeout",
                    },
                    _ = stop.cancelled() => break "",
                };
                let end = matches!(down, Down::End { .. });
                node.emit("arena", json!(down));
                if end {
                    break "";
                }
            };
            stop.cancel();
            if !reason.is_empty() {
                node.emit("arena", json!({ "t": "closed", "room": rid, "reason": reason }));
            }
        });
        Ok(json!({ "seat": seat, "map": map }))
    }

    /// (Oyuncu) girdi ya da atış gönderir. Bağlı değilsem yok sayılır.
    pub fn arena_send(self: &Arc<Self>, up: Up) {
        let (room_id, tx) = {
            let c = self.extras.arena.client.lock().unwrap();
            match c.as_ref() {
                Some(c) if !c.stop.is_cancelled() => (c.room_id.clone(), c.tx.clone()),
                _ => return,
            }
        };
        match tx {
            // Kuyruk doluysa bu girdi atlanır (bir sonraki zaten daha güncel).
            Some(tx) => {
                let _ = tx.try_send(up);
            }
            None => {
                let Some(m) = self.extras.arena.matches.lock().unwrap().get(&room_id).cloned() else { return };
                let mut inner = m.inner.lock().unwrap();
                let Some(seat) = inner.local else { return };
                match up {
                    Up::Input { p, yaw, pitch, w, c, sp } => inner.input(m.map, seat, p, yaw, pitch, w, c, sp),
                    Up::Fire { w, o, d, hits } => inner.fire(m.map, seat, w, o, d, hits),
                }
            }
        }
    }

    /// (Oyuncu) maç ekranından çıkış: bağlantı kapanır; sahipsem maç diğerleri için sürer.
    pub fn arena_leave(&self) {
        let Some(c) = self.extras.arena.client.lock().unwrap().take() else { return };
        c.stop.cancel();
        if c.tx.is_none() {
            if let Some(m) = self.extras.arena.matches.lock().unwrap().get(&c.room_id).cloned() {
                let mut inner = m.inner.lock().unwrap();
                if let Some(seat) = inner.local.take() {
                    inner.leave(seat);
                }
            }
        }
    }
}

#[cfg(test)]
impl Node {
    /// Test: sahipteki maç durumunu (koltuk başına can, canlı mı, öldürme, ölüm, konum, doğma sayacı) okur.
    pub(super) fn arena_debug(&self, room_id: &str) -> Option<(bool, Vec<(f32, bool, u32, u32, V3, u32, bool)>)> {
        let m = self.extras.arena.matches.lock().unwrap().get(room_id).cloned()?;
        let inner = m.inner.lock().unwrap();
        Some((inner.started.is_some(), inner.players.iter().map(|p| (p.hp, p.alive, p.kills, p.deaths, p.pos, p.sp, p.connected)).collect()))
    }

    /// Test: oyuncuyu bir noktaya taşır.
    pub(super) fn arena_place(&self, room_id: &str, seat: usize, pos: V3) {
        if let Some(m) = self.extras.arena.matches.lock().unwrap().get(room_id).cloned() {
            let mut inner = m.inner.lock().unwrap();
            inner.players[seat].pos = pos;
            inner.players[seat].last_input = Some(Instant::now());
        }
    }
}
