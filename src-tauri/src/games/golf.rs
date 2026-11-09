//! Mini golf: 2–6 oyuncu, 6 delik, sırayla vuruş.
//!
//! Hamleler:
//! - `a<açı>p<güç>`: vuruş. Açı derecenin onda biri (0–3599; 0 = sağa, 900 = aşağı, ekran
//!   koordinatları), güç yüzde (1–100). Ör. `a1800p65` = sola, %65 güçle.
//! - `q<koltuk>`: oyuncu oyundan çekildi (sıradan bağımsız; yalnızca oda sahibi ekler).
//!
//! Fizik sabit adımlı (1/240 sn) ve yalnızca bu dosyadadır: oda sahibi de diğer oyuncular da
//! aynı vuruşu aynı kodla oynatır, topun yolu arayüze buradan gider (`lastShot.path`).
//!
//! Sıra: her delikte sırayla birer vuruş; deliği bitiren (topu sokan ya da `MAX_STROKES`'a
//! ulaşan) atlanır. Deliğe ilk vuran her delikte bir sonraki oyuncuya kayar. Suya düşen top
//! vuruşun başladığı yere döner ve bir ceza vuruşu yazılır. Bütün delikler bitince en az toplam
//! vuruş kazanır; eşitlikte berabere. Bir kişi dışında herkes çekilirse kalan kazanır.

use super::{Eval, Outcome};
use serde_json::{json, Value};

pub const MAX_STROKES: u32 = 8;
const WIDTH: f64 = 800.0;
const HEIGHT: f64 = 500.0;
const BALL_R: f64 = 7.0;
const CUP_R: f64 = 11.0;
/// %100 güçte topun ilk hızı (birim/sn). Açık çimde ~1090 birim yol alır.
const VMAX: f64 = 1150.0;
const DT: f64 = 1.0 / 240.0;
const MAX_STEPS: usize = 240 * 25;
/// Yavaşlama: sabit (sürtünme) + hızla orantılı (sürüklenme).
const GRASS: (f64, f64) = (230.0, 0.55);
const SAND: (f64, f64) = (900.0, 2.2);
const STOP_SPEED: f64 = 5.0;
/// Bundan hızlı top deliğin üstünden geçer.
const CUP_SPEED: f64 = 420.0;
/// Duvardan sekme katsayısı.
const BOUNCE: f64 = 0.78;
/// Arayüze giden yolda kaç adımda bir nokta alınır (~40 nokta/sn).
const SAMPLE_EVERY: usize = 6;

type P = (f64, f64);
/// (x, y, genişlik, yükseklik)
type Rect = (f64, f64, f64, f64);

struct Hole {
    name: &'static str,
    par: u32,
    /// Sahanın sınırı (çokgen; kenarları duvardır).
    bounds: &'static [P],
    /// İçerideki duvar blokları (çokgen).
    blocks: &'static [&'static [P]],
    /// Yuvarlak direkler (x, y, yarıçap).
    posts: &'static [(f64, f64, f64)],
    sand: &'static [Rect],
    water: &'static [Rect],
    tee: P,
    cup: P,
}

const fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> [P; 4] {
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

const H1: [P; 4] = rect(80.0, 150.0, 720.0, 350.0);
const H2: [P; 4] = rect(60.0, 80.0, 740.0, 420.0);
const H2_WALL: [P; 4] = rect(385.0, 80.0, 415.0, 320.0);
const H3: [P; 6] = [(60.0, 60.0), (360.0, 60.0), (360.0, 290.0), (740.0, 290.0), (740.0, 440.0), (60.0, 440.0)];
const H4: [P; 4] = rect(60.0, 90.0, 740.0, 410.0);
const H5: [P; 4] = rect(60.0, 70.0, 740.0, 430.0);
const H6: [P; 4] = rect(60.0, 60.0, 740.0, 440.0);
const H6_A: [P; 4] = rect(230.0, 60.0, 260.0, 330.0);
const H6_B: [P; 4] = rect(480.0, 170.0, 510.0, 440.0);

const HOLES: &[Hole] = &[
    Hole { name: "Isınma", par: 2, bounds: &H1, blocks: &[], posts: &[], sand: &[], water: &[], tee: (150.0, 250.0), cup: (650.0, 250.0) },
    Hole { name: "Duvar", par: 3, bounds: &H2, blocks: &[&H2_WALL], posts: &[], sand: &[], water: &[], tee: (160.0, 160.0), cup: (640.0, 160.0) },
    Hole {
        name: "Dönemeç",
        par: 3,
        bounds: &H3,
        blocks: &[],
        posts: &[],
        sand: &[(430.0, 380.0, 120.0, 60.0)],
        water: &[],
        tee: (200.0, 130.0),
        cup: (660.0, 365.0),
    },
    Hole {
        name: "Göl",
        par: 3,
        bounds: &H4,
        blocks: &[],
        posts: &[],
        sand: &[(330.0, 350.0, 140.0, 60.0)],
        water: &[(330.0, 90.0, 140.0, 200.0)],
        tee: (130.0, 250.0),
        cup: (670.0, 190.0),
    },
    Hole {
        name: "Direkler",
        par: 3,
        bounds: &H5,
        blocks: &[],
        posts: &[(300.0, 180.0, 22.0), (300.0, 320.0, 22.0), (450.0, 250.0, 28.0), (590.0, 165.0, 20.0), (590.0, 335.0, 20.0)],
        sand: &[],
        water: &[],
        tee: (120.0, 250.0),
        cup: (690.0, 250.0),
    },
    Hole {
        name: "Zikzak",
        par: 4,
        bounds: &H6,
        blocks: &[&H6_A, &H6_B],
        posts: &[],
        sand: &[(560.0, 60.0, 180.0, 60.0)],
        water: &[],
        tee: (140.0, 120.0),
        cup: (650.0, 380.0),
    },
];

pub fn quit_move(seat: usize) -> String {
    format!("q{seat}")
}

/// `a<açı>p<güç>` → (açı, güç). Yalnızca kanonik yazım kabul edilir (baştaki sıfır yok).
pub fn parse_shot(mv: &str) -> Option<(u32, u32)> {
    let rest = mv.strip_prefix('a')?;
    let (a, p) = rest.split_once('p')?;
    let (a, p): (u32, u32) = (a.parse().ok()?, p.parse().ok()?);
    (a < 3600 && (1..=100).contains(&p) && format!("a{a}p{p}") == mv).then_some((a, p))
}

pub fn parse_quit(mv: &str) -> Option<usize> {
    let seat: usize = mv.strip_prefix('q')?.parse().ok()?;
    (quit_move(seat) == mv).then_some(seat)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ShotEnd {
    Stop,
    In,
    Water,
}

struct Shot {
    end: P,
    result: ShotEnd,
    path: Vec<P>,
}

fn inside_rect(r: &Rect, x: f64, y: f64) -> bool {
    x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3
}

fn inside_poly(poly: &[P], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn edges(poly: &[P]) -> impl Iterator<Item = (P, P)> + '_ {
    (0..poly.len()).map(move |i| (poly[i], poly[(i + 1) % poly.len()]))
}

/// Topu (x, y) noktasından en yakın engel noktasına göre dışarı iter ve hızını yansıtır.
fn bounce_from(x: &mut f64, y: &mut f64, vx: &mut f64, vy: &mut f64, px: f64, py: f64, min_d: f64) {
    let (dx, dy) = (*x - px, *y - py);
    let d = dx.hypot(dy);
    if d >= min_d || d < 1e-9 {
        return;
    }
    let (nx, ny) = (dx / d, dy / d);
    let vn = *vx * nx + *vy * ny;
    if vn < 0.0 {
        *vx -= (1.0 + BOUNCE) * vn * nx;
        *vy -= (1.0 + BOUNCE) * vn * ny;
    }
    *x = px + nx * min_d;
    *y = py + ny * min_d;
}

fn simulate(hole: &Hole, start: P, angle: u32, power: u32) -> Shot {
    let segs: Vec<(P, P)> = edges(hole.bounds).chain(hole.blocks.iter().flat_map(|b| edges(b))).collect();
    let a = (angle as f64 / 10.0).to_radians();
    let v0 = VMAX * power as f64 / 100.0;
    let (mut x, mut y) = start;
    let (mut vx, mut vy) = (v0 * a.cos(), v0 * a.sin());
    let mut path = vec![start];
    for step in 1..=MAX_STEPS {
        let (decel, drag) = if hole.sand.iter().any(|r| inside_rect(r, x, y)) { SAND } else { GRASS };
        let sp = vx.hypot(vy);
        if sp > 0.0 {
            let f = (sp - (decel + drag * sp) * DT).max(0.0) / sp;
            vx *= f;
            vy *= f;
        }
        x += vx * DT;
        y += vy * DT;
        for &((ax, ay), (bx, by)) in &segs {
            let (ex, ey) = (bx - ax, by - ay);
            let t = (((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
            bounce_from(&mut x, &mut y, &mut vx, &mut vy, ax + ex * t, ay + ey * t, BALL_R);
        }
        for &(px, py, r) in hole.posts {
            let (dx, dy) = (x - px, y - py);
            let d = dx.hypot(dy);
            if d < r + BALL_R && d > 1e-9 {
                // Direğin yüzeyindeki en yakın nokta.
                bounce_from(&mut x, &mut y, &mut vx, &mut vy, px + dx / d * r, py + dy / d * r, BALL_R);
            }
        }
        if hole.water.iter().any(|r| inside_rect(r, x, y)) {
            path.push((x, y));
            return Shot { end: start, result: ShotEnd::Water, path };
        }
        let sp = vx.hypot(vy);
        if (x - hole.cup.0).hypot(y - hole.cup.1) < CUP_R && sp < CUP_SPEED {
            path.push(hole.cup);
            return Shot { end: hole.cup, result: ShotEnd::In, path };
        }
        if sp < STOP_SPEED {
            break;
        }
        if step % SAMPLE_EVERY == 0 {
            path.push((x, y));
        }
    }
    path.push((x, y));
    // Güvenlik: sayısal bir hatayla saha dışına çıkan top vuruşun başladığı yere döner.
    let outside = !inside_poly(hole.bounds, x, y) || hole.blocks.iter().any(|b| inside_poly(b, x, y));
    Shot { end: if outside { start } else { (x, y) }, result: ShotEnd::Stop, path }
}

#[derive(Debug, Clone, Default)]
struct Ball {
    /// `None`: henüz vurmadı (başlangıç noktasında).
    pos: Option<P>,
    strokes: u32,
    done: bool,
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn hole_json(h: &Hole) -> Value {
    let pts = |p: &[P]| p.iter().map(|&(x, y)| json!([x, y])).collect::<Vec<_>>();
    json!({
        "name": h.name,
        "par": h.par,
        "bounds": pts(h.bounds),
        "blocks": h.blocks.iter().map(|b| pts(b)).collect::<Vec<_>>(),
        "posts": h.posts.iter().map(|&(x, y, r)| json!([x, y, r])).collect::<Vec<_>>(),
        "sand": h.sand.iter().map(|&(x, y, w, hh)| json!([x, y, w, hh])).collect::<Vec<_>>(),
        "water": h.water.iter().map(|&(x, y, w, hh)| json!([x, y, w, hh])).collect::<Vec<_>>(),
        "tee": [h.tee.0, h.tee.1],
        "cup": [h.cup.0, h.cup.1],
    })
}

pub fn evaluate(players: usize, moves: &[String]) -> Result<Eval, String> {
    let n = players;
    if !(2..=6).contains(&n) {
        return Err("geçersiz oyuncu sayısı".into());
    }
    let mut hole = 0usize;
    let mut balls = vec![Ball::default(); n];
    let mut quit = vec![false; n];
    let mut scores: Vec<Vec<Option<u32>>> = vec![Vec::new(); n];
    let mut last: Value = Value::Null;
    let mut outcome: Option<Outcome> = None;
    // Sıradaki oyuncu: `from` dahil, çekilmemiş ve deliği bitirmemiş ilk koltuk.
    let pick = |from: usize, balls: &[Ball], quit: &[bool]| (0..n).map(|k| (from + k) % n).find(|&c| !quit[c] && !balls[c].done);
    let mut turn = pick(0, &balls, &quit);

    for mv in moves {
        if outcome.is_some() {
            return Err("oyun bitmişken hamle".into());
        }
        if let Some(seat) = parse_quit(mv) {
            if seat >= n || quit[seat] {
                return Err("geçersiz çekilme".into());
            }
            quit[seat] = true;
            if turn == Some(seat) {
                turn = pick((seat + 1) % n, &balls, &quit);
            }
            let left: Vec<usize> = (0..n).filter(|&s| !quit[s]).collect();
            if left.len() <= 1 {
                outcome = Some(Outcome { winner: left.first().copied(), reason: "forfeit" });
                turn = None;
                continue;
            }
        } else if let Some((angle, power)) = parse_shot(mv) {
            let s = turn.ok_or("sıra kimsede değil")?;
            let h = &HOLES[hole];
            let start = balls[s].pos.unwrap_or(h.tee);
            let shot = simulate(h, start, angle, power);
            let b = &mut balls[s];
            b.strokes += if shot.result == ShotEnd::Water { 2 } else { 1 };
            b.pos = Some(shot.end);
            if shot.result == ShotEnd::In || b.strokes >= MAX_STROKES {
                b.done = true;
                b.strokes = b.strokes.min(MAX_STROKES);
            }
            last = json!({
                "seat": s,
                "hole": hole,
                "result": match shot.result { ShotEnd::In => "in", ShotEnd::Water => "water", ShotEnd::Stop if b.done => "max", ShotEnd::Stop => "stop" },
                "strokes": b.strokes,
                "path": shot.path.iter().map(|&(x, y)| json!([r1(x), r1(y)])).collect::<Vec<_>>(),
                "end": [r1(shot.end.0), r1(shot.end.1)],
            });
            turn = pick((s + 1) % n, &balls, &quit);
        } else {
            return Err(format!("geçersiz hamle: {mv}"));
        }
        // Delik bitti mi?
        if turn.is_none() {
            for s in 0..n {
                scores[s].push((!quit[s]).then_some(balls[s].strokes));
            }
            if hole + 1 == HOLES.len() {
                let totals: Vec<(usize, u32)> =
                    (0..n).filter(|&s| !quit[s]).map(|s| (s, scores[s].iter().flatten().sum())).collect();
                let best = totals.iter().map(|t| t.1).min().unwrap_or(0);
                let winners: Vec<usize> = totals.iter().filter(|t| t.1 == best).map(|t| t.0).collect();
                outcome = Some(if winners.len() == 1 {
                    Outcome { winner: Some(winners[0]), reason: "score" }
                } else {
                    Outcome { winner: None, reason: "tie" }
                });
            } else {
                hole += 1;
                balls = vec![Ball::default(); n];
                turn = pick(hole % n, &balls, &quit);
            }
        }
    }

    let h = &HOLES[hole];
    Ok(Eval {
        turn: if outcome.is_none() { turn } else { None },
        legal: Vec::new(),
        info: json!({
            "width": WIDTH,
            "height": HEIGHT,
            "ballR": BALL_R,
            "cupR": CUP_R,
            "maxStrokes": MAX_STROKES,
            "holes": HOLES.iter().map(hole_json).collect::<Vec<_>>(),
            "hole": hole,
            "balls": balls.iter().map(|b| {
                let (x, y) = b.pos.unwrap_or(h.tee);
                json!({ "x": r1(x), "y": r1(y), "strokes": b.strokes, "done": b.done })
            }).collect::<Vec<_>>(),
            "quit": quit,
            "scores": scores,
            "totals": scores.iter().map(|s| s.iter().flatten().sum::<u32>()).collect::<Vec<_>>(),
            "lastShot": last,
        }),
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn move_formats() {
        assert_eq!(parse_shot("a1800p65"), Some((1800, 65)));
        assert_eq!(parse_shot("a0p1"), Some((0, 1)));
        for bad in ["a3600p50", "a10p0", "a10p101", "a010p5", "ap5", "a10", "b10p5", "a10p5x"] {
            assert_eq!(parse_shot(bad), None, "{bad}");
        }
        assert_eq!(parse_quit("q3"), Some(3));
        assert_eq!(parse_quit("q03"), None);
        assert!(evaluate(3, &shots(&["q5"])).is_err());
        assert!(evaluate(2, &shots(&["q0", "q0"])).is_err());
        assert!(evaluate(1, &[]).is_err());
        assert!(evaluate(7, &[]).is_err());
    }

    /// Tüm tasarlanan delikler: başlangıç ve delik sahanın içinde, engel ve suyun dışında.
    #[test]
    fn holes_are_sane() {
        for h in HOLES {
            for (x, y) in [h.tee, h.cup] {
                assert!(inside_poly(h.bounds, x, y), "{}", h.name);
                assert!(!h.blocks.iter().any(|b| inside_poly(b, x, y)), "{}", h.name);
                assert!(!h.water.iter().any(|r| inside_rect(r, x, y)), "{}", h.name);
                assert!(!h.posts.iter().any(|&(px, py, r)| (x - px).hypot(y - py) < r + BALL_R), "{}", h.name);
            }
        }
    }

    #[test]
    fn straight_putt_can_hole_and_hard_shot_lips_out() {
        let h = &HOLES[0];
        let holed: Vec<u32> = (30..=90).filter(|&p| simulate(h, h.tee, 0, p).result == ShotEnd::In).collect();
        assert!(!holed.is_empty(), "ilk delik düz vuruşla girebilmeli");
        let hard = simulate(h, h.tee, 0, 100);
        assert_ne!(hard.result, ShotEnd::In, "çok hızlı top delikten geçer");
        // Duvara çarpıp sekmeli ama sahada kalmalı.
        assert!(inside_poly(h.bounds, hard.end.0, hard.end.1));
        assert!(hard.end.0 < 720.0 - BALL_R + 0.01);
    }

    #[test]
    fn walls_posts_and_water() {
        // 2. delik: doğrudan deliğe vurulan top ortadaki duvardan geri seker.
        let h = &HOLES[1];
        let s = simulate(h, h.tee, 0, 80);
        assert!(s.end.0 < 385.0, "duvarın içinden geçmemeli: {:?}", s.end);
        // 5. delik: ortadaki direğe vurulan top geri döner.
        let h = &HOLES[4];
        let s = simulate(h, (380.0, 250.0), 0, 40);
        assert!(s.end.0 < 450.0 - 28.0, "direğin içinden geçmemeli: {:?}", s.end);
        // 4. delik: göle vurulan top başladığı yere döner.
        let h = &HOLES[3];
        let s = simulate(h, h.tee, 3300, 60);
        assert_eq!(s.result, ShotEnd::Water);
        assert_eq!(s.end, h.tee);
        // Kum topu yavaşlatır.
        let grass = simulate(&HOLES[0], (100.0, 250.0), 0, 30);
        let sanded = simulate(&HOLES[2], (380.0, 410.0), 0, 30);
        assert!(sanded.end.0 - 380.0 < grass.end.0 - 100.0);
    }

    #[test]
    fn turns_holes_and_scoring() {
        // Herkes hep en hafif vuruşu yaparsa her delikte 8'e ulaşılır; berabere biter.
        let n = 3;
        let mut moves = Vec::new();
        let mut first_turns = Vec::new();
        for hole in 0..HOLES.len() {
            let e = evaluate(n, &moves).unwrap();
            assert_eq!(e.info["hole"], hole);
            first_turns.push(e.turn.unwrap());
            for _ in 0..(MAX_STROKES as usize * n) {
                moves.push("a0p1".to_string());
            }
        }
        assert_eq!(first_turns, vec![0, 1, 2, 0, 1, 2], "ilk vuruş her delikte sıradakine geçer");
        let e = evaluate(n, &moves).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: None, reason: "tie" }));
        assert_eq!(e.info["totals"], json!([48, 48, 48]));
        assert!(evaluate(n, &[moves.clone(), shots(&["a0p1"])].concat()).is_err(), "bitmiş oyuna vuruş olmaz");
    }

    #[test]
    fn quitting_and_turn_order() {
        let e = evaluate(3, &shots(&["a0p1"])).unwrap();
        assert_eq!(e.turn, Some(1));
        assert_eq!(e.info["balls"][0]["strokes"], 1);
        // Sıradaki oyuncu çekilirse sıra bir sonrakine geçer.
        let e = evaluate(3, &shots(&["a0p1", "q1"])).unwrap();
        assert_eq!(e.turn, Some(2));
        let e = evaluate(3, &shots(&["a0p1", "q1", "a0p1"])).unwrap();
        assert_eq!(e.turn, Some(0));
        // Biri dışında herkes çekilirse kalan kazanır.
        let e = evaluate(3, &shots(&["a0p1", "q1", "q0"])).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: Some(2), reason: "forfeit" }));
        // Deliği sokan oyuncu o delikte atlanır; en az vuruş kazanır.
        let p = (30..=90).find(|&p| simulate(&HOLES[0], HOLES[0].tee, 0, p).result == ShotEnd::In).unwrap();
        let e = evaluate(2, &shots(&[&format!("a0p{p}"), "a0p1"])).unwrap();
        assert_eq!(e.info["balls"][0]["done"], true);
        assert_eq!(e.info["lastShot"]["seat"], 1);
        assert_eq!(e.turn, Some(1));
        // Aynı vuruş listesi her zaman aynı sonucu verir.
        assert_eq!(evaluate(2, &shots(&["a450p70", "a3000p55"])).unwrap().info, evaluate(2, &shots(&["a450p70", "a3000p55"])).unwrap().info);
    }
}
