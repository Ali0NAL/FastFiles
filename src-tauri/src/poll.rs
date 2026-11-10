//! Oylamalar (`node` modülünün alt modülü).
//!
//! Oylamayı açan cihaz (sahip) tek yetkilidir: oyları o toplar, sonuçları o hesaplar ve her
//! değişiklikte güncel oylamayı tüm katılımcılara gönderir. Sunucu yoktur; çevrimdışı olan
//! katılımcıya güncel durum, çevrimiçi olunca otomatik iletilir.
//!
//! Güvenlik
//! - Oylama bilgisi yalnızca sahibinden kabul edilir (TLS sertifika parmak izi), `rev` ile sıralanır.
//! - Oy yalnızca oylamanın sahibine gider ve sahip yalnızca katılımcı listesindekilerden, açık
//!   oylamaya, geçerli bir seçenek için oy kabul eder. Herkesin bir oyu vardır (değiştirilebilir).
//! - Gizli oylamada kimin neye oy verdiği diğer katılımcılara gönderilmez; yalnızca sayılar.

use super::*;
use crate::chat::GroupMember;
use serde::Deserialize;

pub const MAX_POLL_OPTIONS: usize = 10;
const MAX_QUESTION_CHARS: usize = 200;
const MAX_OPTION_CHARS: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Ballot {
    pub fingerprint: String,
    pub option: u32,
}

/// Katılımcılara gönderilen oylama (sonuçlarıyla birlikte).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Poll {
    pub id: String,
    pub owner: String,
    pub question: String,
    pub options: Vec<String>,
    /// Oy kullanabilecek kişiler (sahip de olabilir).
    pub voters: Vec<GroupMember>,
    pub anonymous: bool,
    pub closed: bool,
    pub rev: u64,
    pub created_at: u64,
    /// Seçenek başına oy sayısı
    pub counts: Vec<u32>,
    /// Oy kullanmış olanlar
    pub voted: Vec<String>,
    /// Kim neye oy verdi (gizli oylamada boş)
    #[serde(default)]
    pub ballots: Vec<Ballot>,
}

impl Poll {
    fn is_voter(&self, fp: &str) -> bool {
        self.voters.iter().any(|v| v.fingerprint == fp)
    }
    /// Sahip + katılımcılar: sonucu görebilecek herkes.
    fn audience(&self) -> Vec<String> {
        let mut all: Vec<String> = self.voters.iter().map(|v| v.fingerprint.clone()).collect();
        if !all.contains(&self.owner) {
            all.push(self.owner.clone());
        }
        all
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct PollState {
    poll: Poll,
    owner_name: String,
    /// (Sahip) oy kayıtları: parmak izi → seçenek
    #[serde(default)]
    votes: HashMap<String, u32>,
    /// (Sahip) güncel oylamayı henüz almamış katılımcılar
    #[serde(default)]
    sync_pending: Vec<String>,
    #[serde(default)]
    my_vote: Option<u32>,
    /// (Katılımcı) oyum sahibine henüz ulaşmadı
    #[serde(default)]
    vote_pending: bool,
    #[serde(default)]
    seen: bool,
    /// Listeden kaldırıldı; bekleyen bildirimler bitince tamamen silinir.
    #[serde(default)]
    hidden: bool,
}

impl PollState {
    /// Listeden kaldırılmış ve işi bitmiş oylama tamamen silinebilir. Başkasının hâlâ açık olan
    /// oylaması silinmez (gizli kalır): silinirse sahibinden gelen bir sonraki güncelleme onu
    /// "yeni oylama" bildirimiyle geri getiriyordu. Kapanmış oylamaya güncelleme gelmez.
    fn removable(&self, me: &str) -> bool {
        self.hidden && self.sync_pending.is_empty() && !self.vote_pending && (self.poll.closed || self.poll.owner == me)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollView {
    #[serde(flatten)]
    pub poll: Poll,
    pub owner_name: String,
    pub my_vote: Option<u32>,
    pub vote_pending: bool,
    pub can_vote: bool,
    pub is_owner: bool,
    pub seen: bool,
}

fn one_line(s: &str, max: usize) -> String {
    sanitize_text(s, max).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Ağdan gelen oylamayı doğrular ve temizler.
fn clean_poll(mut p: Poll) -> Option<Poll> {
    if !valid_message_id(&p.id) || !valid_fingerprint(&p.owner) {
        return None;
    }
    p.question = one_line(&p.question, MAX_QUESTION_CHARS);
    p.options = p.options.iter().map(|o| one_line(o, MAX_OPTION_CHARS)).collect();
    if p.question.is_empty() || p.options.len() < 2 || p.options.len() > MAX_POLL_OPTIONS || p.options.iter().any(|o| o.is_empty()) {
        return None;
    }
    let mut seen = HashSet::new();
    p.voters = p
        .voters
        .into_iter()
        .filter(|v| valid_fingerprint(&v.fingerprint) && seen.insert(v.fingerprint.clone()))
        .map(|v| GroupMember { name: sanitize_name(&v.name), fingerprint: v.fingerprint })
        .take(MAX_GROUP_MEMBERS)
        .collect();
    p.counts.resize(p.options.len(), 0);
    p.voted.retain(|f| valid_fingerprint(f));
    p.ballots.retain(|b| valid_fingerprint(&b.fingerprint) && (b.option as usize) < p.options.len());
    if p.anonymous {
        p.ballots.clear();
    }
    Some(p)
}

impl Node {
    pub(super) fn load_polls(&self) {
        let list: Vec<PollState> = crate::store::read_json(&self.polls_path).unwrap_or_default();
        let mut map = self.polls.lock().unwrap();
        for st in list {
            map.insert(st.poll.id.clone(), st);
        }
    }

    fn save_polls(&self, map: &HashMap<String, PollState>) {
        let list: Vec<&PollState> = map.values().collect();
        if let Ok(json) = serde_json::to_vec(&list) {
            let _ = crate::store::write_atomic(&self.polls_path, &json);
        }
    }

    fn view_of(&self, st: &PollState) -> PollView {
        let me = &self.identity.fingerprint;
        PollView {
            poll: st.poll.clone(),
            owner_name: st.owner_name.clone(),
            my_vote: st.my_vote,
            vote_pending: st.vote_pending,
            can_vote: !st.poll.closed && st.poll.is_voter(me),
            is_owner: st.poll.owner == *me,
            seen: st.seen,
        }
    }

    pub fn poll_list(&self) -> Vec<PollView> {
        let map = self.polls.lock().unwrap();
        let mut list: Vec<PollView> = map.values().filter(|s| !s.hidden).map(|s| self.view_of(s)).collect();
        list.sort_by(|a, b| b.poll.created_at.cmp(&a.poll.created_at));
        list
    }

    fn emit_polls(&self) {
        self.emit("polls", json!(self.poll_list()));
    }

    /// (Sahip) oyları sayar, sonucu günceller ve herkese gönderilmek üzere işaretler.
    fn retally(&self, st: &mut PollState) {
        let me = self.identity.fingerprint.clone();
        let p = &mut st.poll;
        p.counts = vec![0; p.options.len()];
        let mut voted: Vec<String> = Vec::new();
        let mut ballots = Vec::new();
        for v in &p.voters {
            if let Some(&opt) = st.votes.get(&v.fingerprint) {
                if (opt as usize) < p.counts.len() {
                    p.counts[opt as usize] += 1;
                    voted.push(v.fingerprint.clone());
                    ballots.push(Ballot { fingerprint: v.fingerprint.clone(), option: opt });
                }
            }
        }
        p.voted = voted;
        p.ballots = if p.anonymous { Vec::new() } else { ballots };
        // Gizli oylamada sonuçlar kapanana kadar gönderilmez: ardışık iki güncellemede
        // "X oy verdi" ile "k seçeneği bir arttı" karşılaştırılıp X'in oyu çıkarılamasın.
        if p.anonymous && !p.closed {
            p.counts = vec![0; p.options.len()];
        }
        p.rev += 1;
        st.sync_pending = p.audience().into_iter().filter(|f| *f != me).collect();
    }

    // ───────────── Kullanıcı işlemleri ─────────────

    pub fn poll_create(
        &self,
        question: &str,
        options: &[String],
        voter_fps: &[String],
        include_me: bool,
        anonymous: bool,
    ) -> Result<PollView, String> {
        let question = one_line(question, MAX_QUESTION_CHARS);
        if question.is_empty() {
            return Err("Soru boş olamaz".into());
        }
        let options: Vec<String> = options.iter().map(|o| one_line(o, MAX_OPTION_CHARS)).filter(|o| !o.is_empty()).collect();
        if options.len() < 2 {
            return Err("En az iki seçenek girin".into());
        }
        if options.len() > MAX_POLL_OPTIONS {
            return Err(format!("En fazla {MAX_POLL_OPTIONS} seçenek olabilir"));
        }
        let me = self.identity.fingerprint.clone();
        let my_name = self.device_info().name;
        let mut voters = Vec::new();
        if include_me {
            voters.push(GroupMember { fingerprint: me.clone(), name: my_name.clone() });
        }
        for fp in voter_fps {
            if *fp == me || !valid_fingerprint(fp) || voters.iter().any(|v: &GroupMember| v.fingerprint == *fp) {
                continue;
            }
            if let Some(n) = self.name_for(fp) {
                voters.push(GroupMember { fingerprint: fp.clone(), name: n });
            }
        }
        if voters.iter().all(|v| v.fingerprint == me) {
            return Err("Oylamaya en az bir kişi ekleyin".into());
        }
        if voters.len() > MAX_GROUP_MEMBERS {
            return Err(format!("Bir oylamada en fazla {MAX_GROUP_MEMBERS} kişi olabilir"));
        }
        let poll = Poll {
            id: uuid::Uuid::new_v4().to_string(),
            owner: me.clone(),
            question,
            counts: vec![0; options.len()],
            options,
            voters,
            anonymous,
            closed: false,
            rev: 1,
            created_at: now_ms(),
            voted: Vec::new(),
            ballots: Vec::new(),
        };
        let st = PollState {
            sync_pending: poll.audience().into_iter().filter(|f| *f != me).collect(),
            poll,
            owner_name: my_name,
            seen: true,
            ..Default::default()
        };
        let view = self.view_of(&st);
        {
            let mut map = self.polls.lock().unwrap();
            map.insert(st.poll.id.clone(), st);
            self.save_polls(&map);
        }
        self.emit_polls();
        Ok(view)
    }

    pub fn poll_vote(&self, id: &str, option: u32) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        {
            let mut map = self.polls.lock().unwrap();
            let st = map.get_mut(id).ok_or("Oylama bulunamadı")?;
            if st.poll.closed {
                return Err("Bu oylama kapandı".into());
            }
            if !st.poll.is_voter(&me) {
                return Err("Bu oylamada oy kullanamazsınız".into());
            }
            if option as usize >= st.poll.options.len() {
                return Err("Geçersiz seçenek".into());
            }
            st.my_vote = Some(option);
            st.seen = true;
            if st.poll.owner == me {
                st.votes.insert(me.clone(), option);
                self.retally(st);
            } else {
                st.vote_pending = true;
            }
            self.save_polls(&map);
        }
        self.emit_polls();
        Ok(())
    }

    /// (Yalnızca sahip) oylamayı bitirir; sonuç herkese gider.
    pub fn poll_close(&self, id: &str) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        {
            let mut map = self.polls.lock().unwrap();
            let st = map.get_mut(id).ok_or("Oylama bulunamadı")?;
            if st.poll.owner != me {
                return Err("Oylamayı yalnızca açan kişi bitirebilir".into());
            }
            if st.poll.closed {
                return Ok(());
            }
            st.poll.closed = true;
            self.retally(st);
            self.save_polls(&map);
        }
        self.emit_polls();
        Ok(())
    }

    pub fn poll_mark_seen(&self, id: &str) {
        let mut map = self.polls.lock().unwrap();
        if let Some(st) = map.get_mut(id) {
            if !st.seen {
                st.seen = true;
                self.save_polls(&map);
            }
        }
    }

    /// Oylamayı listeden kaldırır. Sahip açık bir oylamayı kaldırırsa oylama önce bitirilir.
    pub fn poll_remove(&self, id: &str) -> Result<(), String> {
        let me = self.identity.fingerprint.clone();
        {
            let mut map = self.polls.lock().unwrap();
            let st = map.get_mut(id).ok_or("Oylama bulunamadı")?;
            if st.poll.owner == me && !st.poll.closed {
                st.poll.closed = true;
                self.retally(st);
            }
            st.hidden = true;
            if st.removable(&me) {
                map.remove(id);
            }
            self.save_polls(&map);
        }
        self.emit_polls();
        Ok(())
    }

    // ───────────── Teslimat ─────────────

    /// Bekleyen oylama bildirimlerini ve oyları çevrimiçi olanlara gönderir (keşif döngüsünden).
    pub fn flush_polls(self: &Arc<Self>) {
        let me = self.identity.fingerprint.clone();
        let online: HashMap<String, Peer> =
            self.peers().into_iter().filter(|p| !p.blocked).map(|p| (p.fingerprint.clone(), p)).collect();
        let mut jobs: Vec<(Peer, String, Message, &'static str)> = Vec::new();
        {
            let mut map = self.polls.lock().unwrap();
            // Kaldırılmış ve işi bitmiş oylamaları temizle.
            let before = map.len();
            map.retain(|_, s| !s.removable(&me));
            if map.len() != before {
                self.save_polls(&map);
            }
            for st in map.values() {
                if st.poll.owner == me {
                    for fp in &st.sync_pending {
                        if let Some(p) = online.get(fp) {
                            let msg = Message::PollUpdate { device: self.device_info(), poll: st.poll.clone(), owner_name: st.owner_name.clone() };
                            jobs.push((p.clone(), format!("poll:{}:sync:{fp}:{}", st.poll.id, st.poll.rev), msg, "sync"));
                        }
                    }
                } else if st.vote_pending {
                    if let (Some(p), Some(opt)) = (online.get(&st.poll.owner), st.my_vote) {
                        let msg = Message::PollVote { device: self.device_info(), poll_id: st.poll.id.clone(), option: opt };
                        jobs.push((p.clone(), format!("poll:{}:vote:{opt}", st.poll.id), msg, "vote"));
                    }
                }
            }
        }
        for (peer, flight, msg, kind) in jobs {
            if self.outbox_backoff.lock().unwrap().get(&flight).map_or(false, |until| Instant::now() < *until) {
                continue;
            }
            if !self.outbox_in_flight.lock().unwrap().insert(flight.clone()) {
                continue;
            }
            let node = self.clone();
            tokio::spawn(async move {
                let reply = node.poll_request(&peer, &msg).await;
                let ok = matches!(reply, Ok(Message::PollAck) | Ok(Message::ChatReject { .. }));
                if ok {
                    let mut map = node.polls.lock().unwrap();
                    let id = match &msg {
                        Message::PollUpdate { poll, .. } => poll.id.clone(),
                        Message::PollVote { poll_id, .. } => poll_id.clone(),
                        _ => String::new(),
                    };
                    if let Some(st) = map.get_mut(&id) {
                        match (kind, &msg) {
                            ("sync", Message::PollUpdate { poll, .. }) if poll.rev == st.poll.rev => {
                                st.sync_pending.retain(|f| *f != peer.fingerprint);
                            }
                            ("vote", Message::PollVote { option, .. }) if st.my_vote == Some(*option) => st.vote_pending = false,
                            _ => {}
                        }
                    }
                    node.save_polls(&map);
                    drop(map);
                    node.emit_polls();
                    node.outbox_backoff.lock().unwrap().remove(&flight);
                } else {
                    node.outbox_backoff.lock().unwrap().insert(flight.clone(), Instant::now() + Duration::from_secs(15));
                }
                node.outbox_in_flight.lock().unwrap().remove(&flight);
            });
        }
    }

    async fn poll_request(&self, peer: &Peer, msg: &Message) -> Result<Message, String> {
        let mut stream = self.connect(&self.candidates(peer), Some(peer.fingerprint.clone())).await?;
        write_msg(&mut stream, msg).await.map_err(|e| e.to_string())?;
        let reply = timeout(Duration::from_secs(8), read_msg(&mut stream))
            .await
            .map_err(|_| "yanıt yok".to_string())?
            .map_err(|e| e.to_string())?;
        let _ = stream.shutdown().await;
        Ok(reply)
    }

    // ───────────── Ağdan gelenler ─────────────

    pub(super) fn accept_poll_update(&self, sender: &str, poll: Poll, owner_name: &str) -> Message {
        if self.is_blocked(sender) {
            return Message::ChatReject { reason: "blocked".into() };
        }
        let Some(poll) = clean_poll(poll) else { return Message::ChatReject { reason: "invalid".into() } };
        if poll.owner != sender {
            return Message::ChatReject { reason: "not_owner".into() };
        }
        let me = self.identity.fingerprint.clone();
        if !poll.is_voter(&me) {
            return Message::PollAck;
        }
        let owner_name = sanitize_name(owner_name);
        let poll_id = poll.id.clone();
        let mut notify: Option<(String, String)> = None;
        let mut is_new = false;
        {
            let mut map = self.polls.lock().unwrap();
            match map.get_mut(&poll.id) {
                Some(st) => {
                    if st.poll.owner != poll.owner {
                        return Message::ChatReject { reason: "not_owner".into() };
                    }
                    if poll.rev <= st.poll.rev {
                        return Message::PollAck;
                    }
                    if poll.closed && !st.poll.closed && !st.hidden {
                        notify = Some((format!("Oylama sonuçlandı · {owner_name}"), result_line(&poll)));
                    }
                    st.poll = poll;
                    st.owner_name = owner_name;
                }
                None => {
                    if !poll.closed {
                        notify = Some((format!("Yeni oylama · {owner_name}"), poll.question.clone()));
                        is_new = true;
                    }
                    map.insert(poll.id.clone(), PollState { poll, owner_name, ..Default::default() });
                }
            }
            self.save_polls(&map);
        }
        if let Some((title, body)) = notify {
            self.notify_to(&title, &body, json!({ "view": "polls", "id": poll_id }));
        }
        self.emit_polls();
        // Yeni oylama: kabuk pencereyi öne getirir.
        if is_new {
            self.emit("poll-new", json!({ "id": poll_id }));
        }
        Message::PollAck
    }

    pub(super) fn accept_poll_vote(&self, sender: &str, id: &str, option: u32) -> Message {
        let reject = |r: &str| Message::ChatReject { reason: r.to_string() };
        if self.is_blocked(sender) {
            return reject("blocked");
        }
        let me = self.identity.fingerprint.clone();
        {
            let mut map = self.polls.lock().unwrap();
            let Some(st) = map.get_mut(id) else { return reject("unknown_poll") };
            if st.poll.owner != me {
                return reject("not_owner");
            }
            if st.poll.closed {
                return reject("closed");
            }
            if !st.poll.is_voter(sender) {
                return reject("not_voter");
            }
            if option as usize >= st.poll.options.len() {
                return reject("invalid");
            }
            if st.votes.get(sender) != Some(&option) {
                st.votes.insert(sender.to_string(), option);
                self.retally(st);
                self.save_polls(&map);
            }
        }
        self.emit_polls();
        Message::PollAck
    }
}

/// "Kazanan: Pizza (4 oy)" gibi kısa sonuç satırı.
fn result_line(p: &Poll) -> String {
    let total: u32 = p.counts.iter().sum();
    if total == 0 {
        return format!("{} — oy kullanılmadı", p.question);
    }
    let max = *p.counts.iter().max().unwrap_or(&0);
    let winners: Vec<&str> = p.options.iter().zip(&p.counts).filter(|(_, c)| **c == max).map(|(o, _)| o.as_str()).collect();
    if winners.len() == 1 {
        format!("{} — Kazanan: {} ({max} oy)", p.question, winners[0])
    } else {
        format!("{} — Berabere: {}", p.question, winners.join(", "))
    }
}
