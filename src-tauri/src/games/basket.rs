//! Basket: takımlı atış yarışması (1v1, 2v2, 3v3).
//!
//! Takımlar: çift koltuklar (0, 2, 4) Kırmızı, tek koltuklar (1, 3, 5) Mavi takımdır. Davet
//! eden, oyuncuları bu sırayla dizerek takımları kurar; iki takım eşit sayıdadır.
//!
//! Oyun `BASE_ROUNDS` turdur. Her turun bir atış noktası (uzaklığı ve değeri: 2, 3 ya da yarı
//! sahadan 4 sayı) ve bazen bir özelliği vardır: 🌬️ rüzgâr (top yana sürüklenir), 🙌 savunmacı
//! (önünde blok yapan biri durur, top yüksek atılmalı), 💰 para topu (sayılar iki katı; son tur
//! her zaman para topudur). Noktalar ve özellikler oda kimliğinden (`seed`) türetilir: herkes
//! aynı turları görür, her oyun farklıdır.
//!
//! Sıra: her turda herkes bir kez atar, takımlar dönüşümlü (K1, M1, K2, M2...). Turu başlatan
//! takım her turda değişir. Takımından biri çekilirse onun atışını takım arkadaşı yapar;
//! takımın hepsi çekilirse diğer takım kazanır.
//!
//! Sayı: isabetli atış noktanın değeri kadar. Hiçbir yere değmeden giren atış (temiz) +1;
//! art arda iki isabetten sonra oyuncu "alev alır" 🔥 ve kaçırana kadar her isabeti +1.
//! Para topu turunda toplam iki katı. Turlar bitince sayısı çok olan takım kazanır; eşitse
//! en fazla `MAX_OVERTIME` uzatma turu (serbest atış çizgisi) oynanır, yine eşitse berabere.
//!
//! Fizik sabit adımlı (1/240 sn), yan görünüşte ve yalnızca bu dosyadadır: oda sahibi de diğer
//! oyuncular da aynı atışı aynı kodla oynatır; topun yolu arayüze buradan gider.
//!
//! Hamleler (`common.rs`): `s<koltuk>a<açı>p<güç>` atış (2700 = dik yukarı), `q<koltuk>` çekilme.

use super::common::{fnv, parse_quit, parse_shot, Rng};
use super::{Eval, Outcome};
use serde_json::{json, Value};

pub const BASE_ROUNDS: usize = 5;
pub const MAX_OVERTIME: usize = 3;
const RECENT_SHOTS: usize = 12;

const WIDTH: f64 = 800.0;
const HEIGHT: f64 = 500.0;
/// Zemin (y aşağı doğru artar).
const FLOOR: f64 = 460.0;
const BALL_R: f64 = 15.0;
/// Çember: önü ve arkası iki küçük daire, aralarından geçen top sayıdır.
const RIM_Y: f64 = 200.0;
const RIM_FRONT: f64 = 642.0;
const RIM_BACK: f64 = 698.0;
const RIM_R: f64 = 3.0;
/// Çember ve panonun zıplatması: yüksek olunca kötü atış dışarı seker (oyun kolaylaşmasın).
const RIM_REST: f64 = 0.65;
const BOARD_REST: f64 = 0.85;
/// Pano (dikey, kalınlığı `BOARD_R * 2`) ve onu taşıyan direk.
const BOARD_X: f64 = 712.0;
const BOARD_TOP: f64 = 95.0;
const BOARD_BOTTOM: f64 = 225.0;
const BOARD_R: f64 = 4.0;
const POLE_X: f64 = 752.0;
const POLE_R: f64 = 6.0;
/// Bu çizginin solundan atılan atış 3 sayıdır (yalnızca çizim; değer turun noktasında yazılı).
const THREE_X: f64 = 390.0;
/// Topun elden çıktığı yükseklik ve oyuncunun önündeki uzaklık.
const RELEASE_Y: f64 = 330.0;
const RELEASE_DX: f64 = 18.0;
/// Savunmacı: atıcının önünde, kolları havada.
const DEF_DX: f64 = 85.0;
const DEF_TOP: f64 = 290.0;
const DEF_R: f64 = 12.0;
const G: f64 = 980.0;
/// %1 ve %100 güçteki ilk hız (birim/sn).
const VMIN: f64 = 300.0;
const VMAX: f64 = 1050.0;
const DT: f64 = 1.0 / 240.0;
/// Yol noktaları arası adım (40 nokta/sn).
const SAMPLE: usize = 6;
const MAX_T: f64 = 5.0;

/// Atış noktaları: ad, x, değer.
const SPOTS: &[(&str, f64, u32)] = &[
    ("Turnike", 585.0, 2),
    ("Dirsek", 525.0, 2),
    ("Serbest atış", 470.0, 2),
    ("Orta mesafe", 430.0, 2),
    ("Köşe üçlük", 365.0, 3),
    ("Tepe üçlük", 330.0, 3),
    ("Uzak üçlük", 270.0, 3),
    ("Logo", 190.0, 3),
    ("Yarı saha", 110.0, 4),
];
const FREE_THROW: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Wind,
    Defender,
    Money,
    Overtime,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Kind::Plain => "plain",
            Kind::Wind => "wind",
            Kind::Defender => "defender",
            Kind::Money => "money",
            Kind::Overtime => "overtime",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Round {
    spot: usize,
    kind: Kind,
    /// Yatay ivme (birim/sn², + sağa). Yalnızca rüzgârlı turda sıfırdan farklı.
    wind: f64,
}

impl Round {
    fn x(&self) -> f64 {
        SPOTS[self.spot].1
    }
    fn defender(&self) -> Option<f64> {
        (self.kind == Kind::Defender).then(|| self.x() + DEF_DX)
    }
    fn json(&self, no: usize) -> Value {
        let (name, x, pts) = SPOTS[self.spot];
        json!({
            "no": no + 1,
            "name": name,
            "x": x,
            "pts": pts,
            "kind": self.kind.id(),
            "wind": r1(self.wind),
            "defender": self.defender(),
        })
    }
}

/// Oyunun turları (uzatmalar hariç): oda kimliğinden, herkeste aynı.
fn rounds(seed: &str) -> Vec<Round> {
    let mut rng = Rng(fnv(seed.as_bytes()) ^ 0xb45c_e7ba_11ed_0001);
    // İlk turlar karışık; son tur her zaman para topu.
    let mut kinds = vec![Kind::Plain, Kind::Plain, Kind::Wind, Kind::Defender];
    for i in (1..kinds.len()).rev() {
        kinds.swap(i, rng.below(i + 1));
    }
    kinds.truncate(BASE_ROUNDS - 1);
    kinds.push(Kind::Money);
    let mut spots: Vec<usize> = (0..SPOTS.len()).collect();
    for i in (1..spots.len()).rev() {
        spots.swap(i, rng.below(i + 1));
    }
    kinds
        .into_iter()
        .map(|kind| {
            // Savunmacı potanın altına düşmesin: yalnızca uzak noktalarda.
            let at = spots.iter().position(|&s| kind != Kind::Defender || SPOTS[s].1 <= 450.0).unwrap_or(0);
            let spot = spots.remove(at);
            let wind = if kind == Kind::Wind {
                let mag = 90.0 + 130.0 * rng.unit();
                if rng.next() & 1 == 0 {
                    mag
                } else {
                    -mag
                }
            } else {
                0.0
            };
            Round { spot, kind, wind }
        })
        .collect()
}

fn round_at(base: &[Round], i: usize) -> Round {
    base.get(i).copied().unwrap_or(Round { spot: FREE_THROW, kind: Kind::Overtime, wind: 0.0 })
}

// ───────────── Fizik ─────────────

#[derive(Debug, Clone)]
struct Throw {
    made: bool,
    /// Çembere ya da panoya değmeden girdi.
    swish: bool,
    path: Vec<(f64, f64)>,
}

/// Topu (`x`,`y`) noktasının, `a`–`b` doğru parçasından `rad` + top yarıçapı uzakta tutar ve
/// hızı yansıtır (`rest`: zıplama katsayısı). Çarpıştıysa `true`.
fn collide(p: &mut (f64, f64), v: &mut (f64, f64), a: (f64, f64), b: (f64, f64), rad: f64, rest: f64) -> bool {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    let t = if len2 > 0.0 { (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (cx, cy) = (a.0 + abx * t, a.1 + aby * t);
    let (dx, dy) = (p.0 - cx, p.1 - cy);
    let d = dx.hypot(dy);
    let min = BALL_R + rad;
    if d >= min || d < 1e-9 {
        return false;
    }
    let (nx, ny) = (dx / d, dy / d);
    p.0 = cx + nx * min;
    p.1 = cy + ny * min;
    let vn = v.0 * nx + v.1 * ny;
    if vn < 0.0 {
        v.0 -= (1.0 + rest) * vn * nx;
        v.1 -= (1.0 + rest) * vn * ny;
        // Sürtünme: teğet hız biraz azalır.
        let vt = (v.0 - (v.0 * nx + v.1 * ny) * nx, v.1 - (v.0 * nx + v.1 * ny) * ny);
        v.0 -= vt.0 * 0.08;
        v.1 -= vt.1 * 0.08;
    }
    true
}

fn speed(power: u32) -> f64 {
    VMIN + (VMAX - VMIN) * (power.clamp(1, 100) as f64 - 1.0) / 99.0
}

fn throw(round: &Round, angle_deg: f64, power: u32) -> Throw {
    let a = angle_deg.to_radians();
    let v0 = speed(power);
    let mut p = (round.x() + RELEASE_DX, RELEASE_Y);
    let mut v = (a.cos() * v0, a.sin() * v0);
    let defender = round.defender();
    let mut path = vec![p];
    let (mut made, mut touched) = (false, false);
    let mut floor_at: Option<f64> = None;
    let mut t = 0.0;
    let mut step = 0usize;
    while t < MAX_T {
        t += DT;
        step += 1;
        v.1 += G * DT;
        v.0 += round.wind * DT;
        let prev_y = p.1;
        p.0 += v.0 * DT;
        p.1 += v.1 * DT;
        let mut hit = false;
        hit |= collide(&mut p, &mut v, (RIM_FRONT, RIM_Y), (RIM_FRONT, RIM_Y), RIM_R, RIM_REST);
        hit |= collide(&mut p, &mut v, (RIM_BACK, RIM_Y), (RIM_BACK, RIM_Y), RIM_R, RIM_REST);
        hit |= collide(&mut p, &mut v, (BOARD_X, BOARD_TOP), (BOARD_X, BOARD_BOTTOM), BOARD_R, BOARD_REST);
        touched |= hit;
        collide(&mut p, &mut v, (POLE_X, BOARD_BOTTOM), (POLE_X, FLOOR), POLE_R, 0.5);
        if let Some(dx) = defender {
            collide(&mut p, &mut v, (dx, DEF_TOP), (dx, FLOOR), DEF_R, 0.45);
        }
        if !made
            && floor_at.is_none()
            && prev_y < RIM_Y
            && p.1 >= RIM_Y
            && v.1 > 0.0
            && p.0 > RIM_FRONT + RIM_R
            && p.0 < RIM_BACK - RIM_R
        {
            made = true;
        }
        if p.1 > FLOOR - BALL_R {
            p.1 = FLOOR - BALL_R;
            v.1 = -v.1 * 0.6;
            v.0 *= 0.85;
            floor_at.get_or_insert(t);
        }
        if p.0 < BALL_R {
            p.0 = BALL_R;
            v.0 = v.0.abs() * 0.6;
        } else if p.0 > WIDTH - BALL_R {
            p.0 = WIDTH - BALL_R;
            v.0 = -v.0.abs() * 0.6;
        }
        if step % SAMPLE == 0 {
            path.push(p);
        }
        // Yere düşünce kısa bir süre daha zıplar, sonra atış biter. Çemberde/panoda takılan
        // top da süre dolunca biter.
        if floor_at.is_some_and(|f| t - f > 0.7) || (!made && t > 4.0) {
            break;
        }
    }
    if path.last() != Some(&p) {
        path.push(p);
    }
    Throw { made, swish: made && !touched, path }
}

// ───────────── Oyun ─────────────

const TEAMS: [&str; 2] = ["Kırmızı", "Mavi"];

#[derive(Debug, Clone, Default)]
struct Stat {
    made: u32,
    att: u32,
    pts: u32,
    /// Art arda isabet.
    streak: u32,
}

impl Stat {
    fn on_fire(&self) -> bool {
        self.streak >= 2
    }
}

fn team_of(seat: usize) -> usize {
    seat % 2
}

/// Turdaki atış sırası (koltuklar). Takımlar dönüşümlü; çekilen oyuncunun yerine takım
/// arkadaşı atar. Takımın hepsi çekildiyse o takımın atışları yoktur.
fn order(round: usize, n: usize, quit: &[bool]) -> Vec<usize> {
    let size = n / 2;
    let first = round % 2;
    let mut out = Vec::new();
    for k in 0..size {
        for team in [first, 1 - first] {
            let members: Vec<usize> = (0..size).map(|i| i * 2 + team).collect();
            if let Some(s) = (0..size).map(|j| members[(k + j) % size]).find(|&s| !quit[s]) {
                out.push(s);
            }
        }
    }
    out
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub fn evaluate(seed: &str, players: usize, moves: &[String]) -> Result<Eval, String> {
    let n = players;
    if !(2..=6).contains(&n) || n % 2 != 0 {
        return Err("takımlar eşit olmalı".into());
    }
    let base = rounds(seed);
    let mut round = 0usize;
    let mut slot = 0usize;
    let mut quit = vec![false; n];
    let mut stats = vec![Stat::default(); n];
    let mut score = [0u32; 2];
    let mut round_pts: [Vec<u32>; 2] = [vec![0], vec![0]];
    let mut recent: Vec<Value> = Vec::new();
    let mut outcome: Option<Outcome> = None;
    let team_alive = |quit: &[bool], t: usize| (0..n).any(|s| team_of(s) == t && !quit[s]);

    for (ply, mv) in moves.iter().enumerate() {
        if outcome.is_some() {
            return Err("oyun bitmişken hamle".into());
        }
        if let Some(seat) = parse_quit(mv) {
            if seat >= n || quit[seat] {
                return Err("geçersiz çekilme".into());
            }
            quit[seat] = true;
            for t in 0..2 {
                if !team_alive(&quit, t) {
                    outcome = Some(Outcome { winner: Some(1 - t), reason: "team_forfeit" });
                }
            }
            continue;
        }
        let Some((s, angle, power)) = parse_shot(mv) else {
            return Err(format!("geçersiz hamle: {mv}"));
        };
        let seq = order(round, n, &quit);
        if seq.get(slot) != Some(&s) {
            return Err("sıra bu oyuncuda değil".into());
        }
        let r = round_at(&base, round);
        let th = throw(&r, angle as f64 / 10.0, power);
        let st = &mut stats[s];
        let fire = st.on_fire();
        st.att += 1;
        let mut pts = 0;
        if th.made {
            pts = SPOTS[r.spot].2 + u32::from(th.swish) + u32::from(fire);
            if r.kind == Kind::Money {
                pts *= 2;
            }
            st.made += 1;
            st.streak += 1;
            st.pts += pts;
        } else {
            st.streak = 0;
        }
        let team = team_of(s);
        score[team] += pts;
        *round_pts[team].last_mut().unwrap() += pts;
        recent.push(json!({
            "ply": ply,
            "seat": s,
            "team": team,
            "round": round,
            "made": th.made,
            "swish": th.swish,
            "fire": fire,
            "nowFire": stats[s].on_fire() && !fire,
            "points": pts,
            "path": th.path.iter().map(|&(x, y)| json!([r1(x), r1(y)])).collect::<Vec<_>>(),
        }));
        if recent.len() > RECENT_SHOTS {
            recent.remove(0);
        }
        slot += 1;
        if slot >= order(round, n, &quit).len() {
            // Tur bitti.
            round += 1;
            slot = 0;
            if round >= BASE_ROUNDS {
                if score[0] != score[1] {
                    let w = usize::from(score[1] > score[0]);
                    outcome = Some(Outcome { winner: Some(w), reason: "points" });
                } else if round >= BASE_ROUNDS + MAX_OVERTIME {
                    outcome = Some(Outcome { winner: None, reason: "tie_points" });
                }
            }
            if outcome.is_none() {
                round_pts[0].push(0);
                round_pts[1].push(0);
            }
        }
    }

    let seq = order(round, n, &quit);
    let shooter = if outcome.is_none() { seq.get(slot).copied() } else { None };
    let shown = round.min(round_pts[0].len() - 1);
    let played: Vec<Value> = (0..round_pts[0].len().max(BASE_ROUNDS)).map(|i| round_at(&base, i).json(i)).collect();
    Ok(Eval {
        turn: shooter,
        movers: shooter.into_iter().collect(),
        legal: Vec::new(),
        info: json!({
            "width": WIDTH,
            "height": HEIGHT,
            "floor": FLOOR,
            "ballR": BALL_R,
            "rim": { "y": RIM_Y, "front": RIM_FRONT, "back": RIM_BACK, "r": RIM_R },
            "board": { "x": BOARD_X, "top": BOARD_TOP, "bottom": BOARD_BOTTOM, "r": BOARD_R },
            "poleX": POLE_X,
            "threeX": THREE_X,
            "release": [RELEASE_DX, RELEASE_Y],
            "defender": { "dx": DEF_DX, "top": DEF_TOP, "r": DEF_R },
            "g": G,
            "vmin": VMIN,
            "vmax": VMAX,
            "baseRounds": BASE_ROUNDS,
            "maxOvertime": MAX_OVERTIME,
            "teamNames": TEAMS,
            "teams": [(0..n).filter(|&s| team_of(s) == 0).collect::<Vec<_>>(), (0..n).filter(|&s| team_of(s) == 1).collect::<Vec<_>>()],
            "rounds": played,
            "round": shown,
            "slot": slot,
            "order": seq,
            "shooter": shooter,
            "score": score,
            "roundPts": round_pts,
            "stats": stats.iter().map(|s| json!({ "made": s.made, "att": s.att, "pts": s.pts, "streak": s.streak, "fire": s.on_fire() })).collect::<Vec<_>>(),
            "quit": quit,
            "shots": recent,
            "lastShot": recent.last().cloned().unwrap_or(Value::Null),
        }),
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bu turda isabet eden bir atış (açı, güç) arar.
    fn find_make(r: &Round) -> Option<(u32, u32)> {
        for a in (2700..=3500).step_by(5) {
            for p in 1..=100 {
                if throw(r, a as f64 / 10.0, p).made {
                    return Some((a, p));
                }
            }
        }
        None
    }

    fn miss() -> String {
        // Dik aşağı: hemen yere düşer.
        "a900p1".into()
    }

    #[test]
    fn every_spot_and_kind_can_score() {
        for spot in 0..SPOTS.len() {
            for (kind, wind) in [(Kind::Plain, 0.0), (Kind::Wind, 200.0), (Kind::Wind, -200.0), (Kind::Defender, 0.0)] {
                if kind == Kind::Defender && SPOTS[spot].1 > 450.0 {
                    continue;
                }
                let r = Round { spot, kind, wind };
                assert!(find_make(&r).is_some(), "{} {:?} {wind}", SPOTS[spot].0, kind);
            }
        }
    }

    #[test]
    fn physics_basics() {
        let r = Round { spot: FREE_THROW, kind: Kind::Plain, wind: 0.0 };
        // Yere atılan top girmez; çok sert atılan top panodan seker ama sahada kalır.
        assert!(!throw(&r, 90.0, 1).made);
        let hard = throw(&r, 300.0, 100);
        assert!(hard.path.iter().all(|&(x, y)| (0.0..=WIDTH).contains(&x) && y <= FLOOR));
        // Aynı atış her zaman aynı yolu çizer.
        assert_eq!(throw(&r, 300.0, 60).path, throw(&r, 300.0, 60).path);
        // Rüzgâr topu sürükler.
        let calm = throw(&r, 300.0, 55).path;
        let windy = throw(&Round { wind: 200.0, kind: Kind::Wind, ..r }, 300.0, 55).path;
        assert!(windy[20].0 > calm[20].0 + 5.0);
        // Savunmacı alçak atışı bloklar.
        let far = Round { spot: 5, kind: Kind::Plain, wind: 0.0 };
        let blocked = Round { kind: Kind::Defender, ..far };
        let flat = throw(&far, 330.0, 60).path;
        let flat_b = throw(&blocked, 330.0, 60).path;
        assert!(flat.iter().map(|p| p.0).fold(0.0, f64::max) > flat_b.iter().map(|p| p.0).fold(0.0, f64::max) + 50.0);
        // Serbest atıştan temiz sayı bulunabilir; panodan sekerek giren temiz sayılmaz.
        let swishes = (2700..=3400).step_by(5).flat_map(|a| (1..=100).map(move |p| (a, p)));
        let outcomes: Vec<Throw> = swishes.map(|(a, p)| throw(&r, a as f64 / 10.0, p)).filter(|t| t.made).collect();
        assert!(outcomes.iter().any(|t| t.swish));
        assert!(outcomes.iter().any(|t| !t.swish));
    }

    #[test]
    fn rounds_are_shared_and_varied() {
        let a = rounds("oda-1");
        assert_eq!(a.len(), BASE_ROUNDS);
        assert_eq!(a.last().unwrap().kind, Kind::Money, "son tur para topu");
        assert!(a.iter().any(|r| r.kind == Kind::Wind) && a.iter().any(|r| r.kind == Kind::Defender));
        let mut spots: Vec<usize> = a.iter().map(|r| r.spot).collect();
        spots.sort();
        spots.dedup();
        assert_eq!(spots.len(), BASE_ROUNDS, "aynı nokta iki kez çıkmaz");
        for r in &a {
            if r.kind == Kind::Wind {
                assert!((90.0..=220.0).contains(&r.wind.abs()));
            }
            if r.kind == Kind::Defender {
                assert!(r.x() <= 450.0);
            }
        }
        let b = rounds("oda-1");
        assert!(a.iter().zip(&b).all(|(x, y)| x.spot == y.spot && x.kind == y.kind && x.wind == y.wind));
        let differ = (0..200).filter(|i| rounds(&format!("{i}")).iter().map(|r| r.spot).collect::<Vec<_>>() != spots).count();
        assert!(differ > 190);
    }

    #[test]
    fn team_order_and_substitutes() {
        // 2v2: K = 0, 2; M = 1, 3. Turu başlatan takım her turda değişir.
        assert_eq!(order(0, 4, &[false; 4]), vec![0, 1, 2, 3]);
        assert_eq!(order(1, 4, &[false; 4]), vec![1, 0, 3, 2]);
        assert_eq!(order(0, 2, &[false; 2]), vec![0, 1]);
        assert_eq!(order(0, 6, &[false; 6]), vec![0, 1, 2, 3, 4, 5]);
        // 2 çekildi: onun atışını takım arkadaşı 0 yapar.
        assert_eq!(order(0, 4, &[false, false, true, false]), vec![0, 1, 0, 3]);
        assert!(evaluate("r", 3, &[]).is_err(), "takımlar eşit olmalı");
        let e = evaluate("r", 4, &[]).unwrap();
        assert_eq!(e.turn, Some(0));
        assert_eq!(e.movers, vec![0]);
        assert!(evaluate("r", 4, &["s1a900p1".into()]).is_err(), "sırası gelmeyen atamaz");
        let e = evaluate("r", 4, &["s0a900p1".into()]).unwrap();
        assert_eq!(e.turn, Some(1));
        // Takımın hepsi çekilince diğer takım kazanır.
        let e = evaluate("r", 4, &["q1".into(), "q3".into()]).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: Some(0), reason: "team_forfeit" }));
    }

    #[test]
    fn scoring_fire_money_and_overtime() {
        let seed = "skor";
        let base = rounds(seed);
        let n = 2;
        let mut moves: Vec<String> = Vec::new();
        // Kırmızı (0) her turda isabet eder, Mavi (1) hep kaçırır.
        let mut expected = 0;
        let mut streak = 0;
        for (i, r) in base.iter().enumerate() {
            let (a, p) = find_make(r).unwrap();
            let t = throw(r, a as f64 / 10.0, p);
            let mut pts = SPOTS[r.spot].2 + u32::from(t.swish) + u32::from(streak >= 2);
            if r.kind == Kind::Money {
                pts *= 2;
            }
            expected += pts;
            streak += 1;
            let mine = format!("s0a{a}p{p}");
            // Turu başlatan takım değişir.
            if i % 2 == 0 {
                moves.push(mine);
                moves.push(format!("s1{}", miss()));
            } else {
                moves.push(format!("s1{}", miss()));
                moves.push(mine);
            }
            let e = evaluate(seed, n, &moves).unwrap();
            assert_eq!(e.info["score"][0], expected, "{i}. tur");
        }
        let e = evaluate(seed, n, &moves).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: Some(0), reason: "points" }));
        assert_eq!(e.info["stats"][0]["made"], BASE_ROUNDS as u32);
        assert_eq!(e.info["stats"][0]["fire"], true);
        assert_eq!(e.info["stats"][1]["pts"], 0);

        // Herkes kaçırırsa uzatmalar oynanır, sonra berabere.
        let mut moves = Vec::new();
        for i in 0..(BASE_ROUNDS + MAX_OVERTIME) {
            let e = evaluate(seed, n, &moves).unwrap();
            assert!(e.outcome.is_none());
            assert_eq!(e.info["round"], i);
            if i >= BASE_ROUNDS {
                assert_eq!(e.info["rounds"][i]["kind"], "overtime");
            }
            let first = e.turn.unwrap();
            moves.push(format!("s{first}{}", miss()));
            moves.push(format!("s{}{}", 1 - first, miss()));
        }
        let e = evaluate(seed, n, &moves).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: None, reason: "tie_points" }));
        assert!(evaluate(seed, n, &[moves, vec!["s0a900p1".into()]].concat()).is_err());
    }
}

#[cfg(test)]
/// Zorluk ayarı için: her noktada en iyi nişanla isabet oranı.
/// `cargo test --release --lib tune::difficulty -- --ignored --nocapture`
mod tune {
    use super::*;
    #[test]
    #[ignore]
    fn difficulty() {
        for spot in 0..SPOTS.len() {
            let r = Round { spot, kind: Kind::Plain, wind: 0.0 };
            let grid: Vec<Vec<bool>> = (2700..3600).map(|a| (1..=100).map(|p| throw(&r, a as f64 / 10.0, p).made).collect()).collect();
            // İyi bir nişancı: en iyi noktada (±2°, ±3 güç) ve (±3°, ±5 güç) sapmayla isabet oranı.
            let rate = |wa: i32, wp: i32| {
                let mut best = 0.0f64;
                for ca in wa..(900 - wa) {
                    for cp in wp..(100 - wp) {
                        let mut hit = 0;
                        for da in -wa..=wa {
                            for dp in -wp..=wp {
                                hit += grid[(ca + da) as usize][(cp + dp) as usize] as i32;
                            }
                        }
                        best = best.max(hit as f64 / ((2 * wa + 1) * (2 * wp + 1)) as f64);
                    }
                }
                best
            };
            let best = rate(20, 3);
            let loose = rate(30, 5);
            println!("{:14} {:.0}% / {:.0}%", SPOTS[spot].0, best * 100.0, loose * 100.0);
        }
    }
}
