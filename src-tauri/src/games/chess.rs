//! Satranç kuralları: hamle üretimi, doğrulama ve oyun sonu tespiti.
//!
//! Hamleler UCI biçimindedir (`e2e4`, rok `e1g1`, terfi `e7e8q`). Kareler `rank * 8 + file`
//! ile numaralanır: a1 = 0, h1 = 7, a8 = 56, h8 = 63.
//!
//! Desteklenenler: rok (şahın geçtiği kareler tehdit altındaysa yasak), geçerken alma, terfi,
//! şah mat, pat, üç kez tekrar, 50 hamle kuralı ve yetersiz malzeme (otomatik beraberlik).

use super::{Eval, Outcome};
use serde_json::json;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Color {
    White,
    Black,
}

impl Color {
    fn other(self) -> Self {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
    /// Koltuk numarası: beyaz 0, siyah 1.
    fn seat(self) -> usize {
        match self {
            Color::White => 0,
            Color::Black => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl Kind {
    fn letter(self) -> char {
        match self {
            Kind::Pawn => 'P',
            Kind::Knight => 'N',
            Kind::Bishop => 'B',
            Kind::Rook => 'R',
            Kind::Queen => 'Q',
            Kind::King => 'K',
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Piece {
    color: Color,
    kind: Kind,
}

impl Piece {
    fn fen_char(self) -> char {
        let c = self.kind.letter();
        if self.color == Color::White {
            c
        } else {
            c.to_ascii_lowercase()
        }
    }
}

const WK: u8 = 1;
const WQ: u8 = 2;
const BK: u8 = 4;
const BQ: u8 = 8;

const KNIGHT: [(i8, i8); 8] = [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)];
const KING: [(i8, i8); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
const ORTHO: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const DIAG: [(i8, i8); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
const PROMOS: [Kind; 4] = [Kind::Queen, Kind::Rook, Kind::Bishop, Kind::Knight];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Move {
    from: u8,
    to: u8,
    promo: Option<Kind>,
}

impl Move {
    fn uci(&self) -> String {
        let mut s = format!("{}{}", sq_name(self.from), sq_name(self.to));
        if let Some(k) = self.promo {
            s.push(k.letter().to_ascii_lowercase());
        }
        s
    }
}

fn sq_name(sq: u8) -> String {
    format!("{}{}", (b'a' + sq % 8) as char, sq / 8 + 1)
}

fn on_board(f: i8, r: i8) -> bool {
    (0..8).contains(&f) && (0..8).contains(&r)
}

fn sq_of(f: i8, r: i8) -> usize {
    (r * 8 + f) as usize
}

#[derive(Clone, Debug)]
struct Position {
    board: [Option<Piece>; 64],
    side: Color,
    castling: u8,
    ep: Option<u8>,
    halfmove: u32,
    fullmove: u32,
}

impl Position {
    fn start() -> Self {
        use Kind::*;
        let back = [Rook, Knight, Bishop, Queen, King, Bishop, Knight, Rook];
        let mut board = [None; 64];
        for f in 0..8 {
            board[f] = Some(Piece { color: Color::White, kind: back[f] });
            board[8 + f] = Some(Piece { color: Color::White, kind: Pawn });
            board[48 + f] = Some(Piece { color: Color::Black, kind: Pawn });
            board[56 + f] = Some(Piece { color: Color::Black, kind: back[f] });
        }
        Self { board, side: Color::White, castling: WK | WQ | BK | BQ, ep: None, halfmove: 0, fullmove: 1 }
    }

    fn get(&self, f: i8, r: i8) -> Option<Piece> {
        if on_board(f, r) {
            self.board[sq_of(f, r)]
        } else {
            None
        }
    }

    fn placement(&self) -> String {
        let mut s = String::new();
        for r in (0..8).rev() {
            let mut empty = 0;
            for f in 0..8 {
                match self.board[r * 8 + f] {
                    Some(p) => {
                        if empty > 0 {
                            s.push_str(&empty.to_string());
                            empty = 0;
                        }
                        s.push(p.fen_char());
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                s.push_str(&empty.to_string());
            }
            if r > 0 {
                s.push('/');
            }
        }
        s
    }

    fn castling_str(&self) -> String {
        let s: String = [(WK, 'K'), (WQ, 'Q'), (BK, 'k'), (BQ, 'q')]
            .iter()
            .filter(|(bit, _)| self.castling & bit != 0)
            .map(|(_, c)| *c)
            .collect();
        if s.is_empty() {
            "-".into()
        } else {
            s
        }
    }

    fn fen(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.placement(),
            if self.side == Color::White { 'w' } else { 'b' },
            self.castling_str(),
            self.ep.map(sq_name).unwrap_or_else(|| "-".into()),
            self.halfmove,
            self.fullmove
        )
    }

    /// Tekrar tespiti için konum anahtarı. Geçerken alma karesi yalnızca gerçekten
    /// alabilecek bir piyon varsa sayılır (yoksa konum aynıdır).
    fn key(&self) -> String {
        let ep = self.ep.filter(|&ep| {
            let (f, r) = ((ep % 8) as i8, (ep / 8) as i8);
            let pr = if self.side == Color::White { r - 1 } else { r + 1 };
            [-1, 1].iter().any(|df| self.get(f + df, pr) == Some(Piece { color: self.side, kind: Kind::Pawn }))
        });
        format!("{} {:?} {} {:?}", self.placement(), self.side, self.castling, ep)
    }

    /// `sq` karesi `by` rengince tehdit ediliyor mu?
    fn attacked(&self, sq: u8, by: Color) -> bool {
        let (f, r) = ((sq % 8) as i8, (sq / 8) as i8);
        let is = |p: Option<Piece>, kinds: &[Kind]| p.is_some_and(|p| p.color == by && kinds.contains(&p.kind));
        // Beyaz piyon bir alt sıradan, siyah piyon bir üst sıradan çapraz tehdit eder.
        let pr = if by == Color::White { r - 1 } else { r + 1 };
        if [-1, 1].iter().any(|df| is(self.get(f + df, pr), &[Kind::Pawn])) {
            return true;
        }
        if KNIGHT.iter().any(|(df, dr)| is(self.get(f + df, r + dr), &[Kind::Knight])) {
            return true;
        }
        if KING.iter().any(|(df, dr)| is(self.get(f + df, r + dr), &[Kind::King])) {
            return true;
        }
        let ray = |dirs: &[(i8, i8)], kinds: &[Kind]| {
            dirs.iter().any(|(df, dr)| {
                let (mut x, mut y) = (f + df, r + dr);
                while on_board(x, y) {
                    if let Some(p) = self.board[sq_of(x, y)] {
                        return p.color == by && kinds.contains(&p.kind);
                    }
                    x += df;
                    y += dr;
                }
                false
            })
        };
        ray(&ORTHO, &[Kind::Rook, Kind::Queen]) || ray(&DIAG, &[Kind::Bishop, Kind::Queen])
    }

    fn king_sq(&self, c: Color) -> Option<u8> {
        (0..64u8).find(|&s| self.board[s as usize] == Some(Piece { color: c, kind: Kind::King }))
    }

    fn in_check(&self) -> bool {
        self.king_sq(self.side).is_some_and(|k| self.attacked(k, self.side.other()))
    }

    /// Şahın tehdit altında kalıp kalmadığına bakmadan üretilen hamleler.
    fn pseudo(&self) -> Vec<Move> {
        let mut out = Vec::with_capacity(48);
        let me = self.side;
        for from in 0..64u8 {
            let Some(p) = self.board[from as usize] else { continue };
            if p.color != me {
                continue;
            }
            let (f, r) = ((from % 8) as i8, (from / 8) as i8);
            let add = |to: usize, out: &mut Vec<Move>| out.push(Move { from, to: to as u8, promo: None });
            match p.kind {
                Kind::Pawn => {
                    let dir = if me == Color::White { 1 } else { -1 };
                    let start = if me == Color::White { 1 } else { 6 };
                    let last = if me == Color::White { 7 } else { 0 };
                    let push = |to: usize, out: &mut Vec<Move>| {
                        if (to / 8) as i8 == last {
                            for k in PROMOS {
                                out.push(Move { from, to: to as u8, promo: Some(k) });
                            }
                        } else {
                            out.push(Move { from, to: to as u8, promo: None });
                        }
                    };
                    if on_board(f, r + dir) && self.get(f, r + dir).is_none() {
                        push(sq_of(f, r + dir), &mut out);
                        if r == start && self.get(f, r + 2 * dir).is_none() {
                            push(sq_of(f, r + 2 * dir), &mut out);
                        }
                    }
                    for df in [-1, 1] {
                        if !on_board(f + df, r + dir) {
                            continue;
                        }
                        let to = sq_of(f + df, r + dir);
                        match self.board[to] {
                            Some(q) if q.color != me => push(to, &mut out),
                            None if self.ep == Some(to as u8) => push(to, &mut out),
                            _ => {}
                        }
                    }
                }
                Kind::Knight | Kind::King => {
                    let steps = if p.kind == Kind::Knight { &KNIGHT } else { &KING };
                    for (df, dr) in steps {
                        let (x, y) = (f + df, r + dr);
                        if on_board(x, y) && self.get(x, y).map_or(true, |q| q.color != me) {
                            add(sq_of(x, y), &mut out);
                        }
                    }
                    if p.kind == Kind::King {
                        self.castles(from, &mut out);
                    }
                }
                Kind::Bishop | Kind::Rook | Kind::Queen => {
                    let dirs: Vec<(i8, i8)> = match p.kind {
                        Kind::Bishop => DIAG.to_vec(),
                        Kind::Rook => ORTHO.to_vec(),
                        _ => ORTHO.iter().chain(DIAG.iter()).copied().collect(),
                    };
                    for (df, dr) in dirs {
                        let (mut x, mut y) = (f + df, r + dr);
                        while on_board(x, y) {
                            match self.get(x, y) {
                                None => add(sq_of(x, y), &mut out),
                                Some(q) => {
                                    if q.color != me {
                                        add(sq_of(x, y), &mut out);
                                    }
                                    break;
                                }
                            }
                            x += df;
                            y += dr;
                        }
                    }
                }
            }
        }
        out
    }

    /// Rok: şah ve kale yerinde, arası boş, şah tehdit altında değil ve geçeceği kareler güvenli.
    fn castles(&self, from: u8, out: &mut Vec<Move>) {
        let me = self.side;
        let (base, kbit, qbit) = if me == Color::White { (0u8, WK, WQ) } else { (56u8, BK, BQ) };
        if from != base + 4 {
            return;
        }
        let rook = Some(Piece { color: me, kind: Kind::Rook });
        let empty = |s: &[u8]| s.iter().all(|&x| self.board[(base + x) as usize].is_none());
        let safe = |s: &[u8]| s.iter().all(|&x| !self.attacked(base + x, me.other()));
        if self.castling & kbit != 0 && self.board[(base + 7) as usize] == rook && empty(&[5, 6]) && safe(&[4, 5, 6]) {
            out.push(Move { from, to: base + 6, promo: None });
        }
        if self.castling & qbit != 0 && self.board[base as usize] == rook && empty(&[1, 2, 3]) && safe(&[4, 3, 2]) {
            out.push(Move { from, to: base + 2, promo: None });
        }
    }

    fn make(&self, m: Move) -> Position {
        let mut p = self.clone();
        let piece = self.board[m.from as usize].expect("hamle boş kareden");
        let mut capture = self.board[m.to as usize].is_some();
        p.board[m.from as usize] = None;
        if piece.kind == Kind::Pawn && Some(m.to) == self.ep && !capture && m.from % 8 != m.to % 8 {
            let victim = if piece.color == Color::White { m.to - 8 } else { m.to + 8 };
            p.board[victim as usize] = None;
            capture = true;
        }
        if piece.kind == Kind::King && (m.from as i8 - m.to as i8).abs() == 2 {
            let (rf, rt) = if m.to > m.from { (m.from + 3, m.from + 1) } else { (m.from - 4, m.from - 1) };
            p.board[rt as usize] = p.board[rf as usize];
            p.board[rf as usize] = None;
        }
        p.board[m.to as usize] = Some(match m.promo {
            Some(k) => Piece { color: piece.color, kind: k },
            None => piece,
        });
        if piece.kind == Kind::King {
            p.castling &= if piece.color == Color::White { !(WK | WQ) } else { !(BK | BQ) };
        }
        for s in [m.from, m.to] {
            p.castling &= match s {
                0 => !WQ,
                7 => !WK,
                56 => !BQ,
                63 => !BK,
                _ => 0xff,
            };
        }
        p.ep = (piece.kind == Kind::Pawn && (m.from as i8 - m.to as i8).abs() == 16).then(|| (m.from + m.to) / 2);
        p.halfmove = if piece.kind == Kind::Pawn || capture { 0 } else { self.halfmove + 1 };
        if self.side == Color::Black {
            p.fullmove += 1;
        }
        p.side = self.side.other();
        p
    }

    fn legal(&self) -> Vec<Move> {
        let me = self.side;
        self.pseudo()
            .into_iter()
            .filter(|&m| {
                let n = self.make(m);
                n.king_sq(me).map_or(true, |k| !n.attacked(k, me.other()))
            })
            .collect()
    }

    /// Mat mümkün değil: yalnızca şahlar; şah + tek hafif taş; ya da yalnızca aynı renk
    /// karelerde filler.
    fn insufficient(&self) -> bool {
        let mut minors = Vec::new();
        for (sq, p) in self.board.iter().enumerate() {
            let Some(p) = p else { continue };
            match p.kind {
                Kind::King => {}
                Kind::Pawn | Kind::Rook | Kind::Queen => return false,
                _ => minors.push((p.kind, (sq % 8 + sq / 8) % 2)),
            }
        }
        match minors.len() {
            0 | 1 => true,
            _ => minors.iter().all(|(k, c)| *k == Kind::Bishop && *c == minors[0].1),
        }
    }

    /// Kısa cebirsel notasyon (şah/mat işareti hariç; onu çağıran ekler).
    fn san(&self, m: Move, legal: &[Move]) -> String {
        let p = self.board[m.from as usize].expect("hamle boş kareden");
        if p.kind == Kind::King && (m.from as i8 - m.to as i8).abs() == 2 {
            return if m.to > m.from { "O-O".into() } else { "O-O-O".into() };
        }
        let capture = self.board[m.to as usize].is_some() || (p.kind == Kind::Pawn && m.from % 8 != m.to % 8);
        let mut s = String::new();
        if p.kind == Kind::Pawn {
            if capture {
                s.push((b'a' + m.from % 8) as char);
                s.push('x');
            }
            s.push_str(&sq_name(m.to));
            if let Some(k) = m.promo {
                s.push('=');
                s.push(k.letter());
            }
            return s;
        }
        s.push(p.kind.letter());
        let rivals: Vec<&Move> = legal
            .iter()
            .filter(|o| o.to == m.to && o.from != m.from && self.board[o.from as usize].map(|q| q.kind) == Some(p.kind))
            .collect();
        if !rivals.is_empty() {
            let file = (b'a' + m.from % 8) as char;
            let rank = (b'1' + m.from / 8) as char;
            if !rivals.iter().any(|o| o.from % 8 == m.from % 8) {
                s.push(file);
            } else if !rivals.iter().any(|o| o.from / 8 == m.from / 8) {
                s.push(rank);
            } else {
                s.push(file);
                s.push(rank);
            }
        }
        if capture {
            s.push('x');
        }
        s.push_str(&sq_name(m.to));
        s
    }
}

/// Hamle listesini baştan oynatır ve durumu hesaplar.
pub fn evaluate(moves: &[String]) -> Result<Eval, String> {
    let mut pos = Position::start();
    let mut legal = pos.legal();
    let mut seen: HashMap<String, u32> = HashMap::new();
    seen.insert(pos.key(), 1);
    let mut sans = Vec::with_capacity(moves.len());
    let mut outcome: Option<Outcome> = None;
    for text in moves {
        if outcome.is_some() {
            return Err("oyun bitmişken hamle".into());
        }
        let m = *legal.iter().find(|m| m.uci() == *text).ok_or_else(|| format!("kurala aykırı hamle: {text}"))?;
        let mut san = pos.san(m, &legal);
        pos = pos.make(m);
        legal = pos.legal();
        if pos.in_check() {
            san.push(if legal.is_empty() { '#' } else { '+' });
        }
        sans.push(san);
        let count = seen.entry(pos.key()).or_insert(0);
        *count += 1;
        outcome = status(&pos, &legal, *count);
    }
    Ok(Eval {
        turn: outcome.is_none().then(|| pos.side.seat()),
        legal: if outcome.is_none() { legal.iter().map(Move::uci).collect() } else { Vec::new() },
        outcome,
        info: json!({
            "fen": pos.fen(),
            "check": pos.in_check(),
            "san": sans,
            "lastMove": moves.last(),
        }),
    })
}

fn status(pos: &Position, legal: &[Move], repeats: u32) -> Option<Outcome> {
    if legal.is_empty() {
        return Some(if pos.in_check() {
            Outcome { winner: Some(pos.side.other().seat()), reason: "checkmate" }
        } else {
            Outcome { winner: None, reason: "stalemate" }
        });
    }
    if pos.insufficient() {
        return Some(Outcome { winner: None, reason: "material" });
    }
    if repeats >= 3 {
        return Some(Outcome { winner: None, reason: "repetition" });
    }
    if pos.halfmove >= 100 {
        return Some(Outcome { winner: None, reason: "fifty" });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_fen(fen: &str) -> Position {
        let parts: Vec<&str> = fen.split_whitespace().collect();
        let mut board = [None; 64];
        for (i, row) in parts[0].split('/').enumerate() {
            let r = 7 - i;
            let mut f = 0;
            for c in row.chars() {
                if let Some(d) = c.to_digit(10) {
                    f += d as usize;
                    continue;
                }
                let kind = match c.to_ascii_lowercase() {
                    'p' => Kind::Pawn,
                    'n' => Kind::Knight,
                    'b' => Kind::Bishop,
                    'r' => Kind::Rook,
                    'q' => Kind::Queen,
                    _ => Kind::King,
                };
                let color = if c.is_ascii_uppercase() { Color::White } else { Color::Black };
                board[r * 8 + f] = Some(Piece { color, kind });
                f += 1;
            }
        }
        let mut castling = 0;
        for c in parts[2].chars() {
            castling |= match c {
                'K' => WK,
                'Q' => WQ,
                'k' => BK,
                'q' => BQ,
                _ => 0,
            };
        }
        let ep = (parts[3] != "-").then(|| {
            let b = parts[3].as_bytes();
            (b[1] - b'1') * 8 + (b[0] - b'a')
        });
        Position {
            board,
            side: if parts[1] == "w" { Color::White } else { Color::Black },
            castling,
            ep,
            halfmove: parts[4].parse().unwrap(),
            fullmove: parts[5].parse().unwrap(),
        }
    }

    fn perft(p: &Position, depth: u32) -> u64 {
        if depth == 0 {
            return 1;
        }
        let moves = p.legal();
        if depth == 1 {
            return moves.len() as u64;
        }
        moves.iter().map(|&m| perft(&p.make(m), depth - 1)).sum()
    }

    fn mv(list: &str) -> Vec<String> {
        list.split_whitespace().map(String::from).collect()
    }

    /// Bilinen perft sayıları: rok, geçerken alma, açmaz ve terfi kurallarını birlikte doğrular.
    #[test]
    fn perft_known_positions() {
        assert_eq!(perft(&Position::start(), 3), 8902);
        let kiwipete = from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1");
        assert_eq!(perft(&kiwipete, 1), 48);
        assert_eq!(perft(&kiwipete, 2), 2039);
        assert_eq!(perft(&kiwipete, 3), 97862);
        let pos3 = from_fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1");
        assert_eq!(perft(&pos3, 4), 43238);
        let pos4 = from_fen("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1");
        assert_eq!(perft(&pos4, 3), 9467);
    }

    #[test]
    fn fen_roundtrip_and_start() {
        let start = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
        assert_eq!(Position::start().fen(), start);
        assert_eq!(from_fen(start).fen(), start);
        let e = evaluate(&mv("e2e4")).unwrap();
        assert_eq!(e.info["fen"], "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1");
        assert_eq!(e.turn, Some(1));
    }

    #[test]
    fn scholars_mate() {
        let e = evaluate(&mv("e2e4 e7e5 d1h5 b8c6 f1c4 g8f6 h5f7")).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: Some(0), reason: "checkmate" }));
        assert_eq!(e.turn, None);
        assert!(e.legal.is_empty());
        assert_eq!(e.info["san"].as_array().unwrap().last().unwrap(), "Qxf7#");
        // Bitmiş oyuna hamle eklenemez.
        assert!(evaluate(&mv("e2e4 e7e5 d1h5 b8c6 f1c4 g8f6 h5f7 e8e7")).is_err());
    }

    #[test]
    fn illegal_moves_rejected() {
        assert!(evaluate(&mv("e2e5")).is_err());
        assert!(evaluate(&mv("e7e5")).is_err(), "sıra beyazda");
        assert!(evaluate(&mv("e1g1")).is_err(), "arada taş varken rok olmaz");
        // Açmazdaki taş oynatılamaz: e5'teki siyah vezir e2'deki atı şaha açmazlamış.
        let e = evaluate(&mv("e2e4 d7d5 e4d5 d8d5 g1e2 d5e5")).unwrap();
        assert!(!e.legal.iter().any(|m| m.starts_with("e2")));
        // Şah çekilirken şahı kurtarmayan hamle yapılamaz.
        assert!(evaluate(&mv("e2e4 e7e5 g1f3 d8h4 f3e5 h4e4 b1c3")).is_err());
    }

    #[test]
    fn castling_en_passant_promotion_and_san() {
        // Kısa rok ve O-O notasyonu.
        let e = evaluate(&mv("e2e4 e7e5 g1f3 b8c6 f1c4 g8f6 e1g1")).unwrap();
        assert_eq!(e.info["san"][6], "O-O");
        assert!(e.info["fen"].as_str().unwrap().starts_with("r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQ1RK1 b kq"));
        // Geçerken alma.
        let e = evaluate(&mv("e2e4 a7a6 e4e5 d7d5 e5d6")).unwrap();
        assert_eq!(e.info["san"][4], "exd6");
        assert!(e.info["fen"].as_str().unwrap().starts_with("rnbqkbnr/1pp1pppp/p2P4/8/8/8/PPPP1PPP/RNBQKBNR b"));
        // Terfi (alarak) ve terfi hamlesinin harfi zorunlu.
        let line = "a2a4 b7b5 a4b5 a7a6 b5a6 c8b7 a6b7 h7h6";
        assert!(evaluate(&mv(&format!("{line} b7a8"))).is_err());
        let e = evaluate(&mv(&format!("{line} b7a8q"))).unwrap();
        assert_eq!(e.info["san"][8], "bxa8=Q");
        // Belirsizlik giderme: c5 ve d4'teki atların ikisi de b3'e gidebiliyor.
        let e = evaluate(&mv("g1f3 a7a6 b1c3 a6a5 f3d4 a5a4 c3e4 h7h6 e4c5 h6h5 c5b3")).unwrap();
        assert_eq!(e.info["san"][10], "Ncb3");
    }

    #[test]
    fn draws() {
        // Üç kez tekrar: atlar gidip geliyor.
        let e = evaluate(&mv("g1f3 g8f6 f3g1 f6g8 g1f3 g8f6 f3g1 f6g8")).unwrap();
        assert_eq!(e.outcome, Some(Outcome { winner: None, reason: "repetition" }));
        // Pat.
        let p = from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1");
        assert!(p.legal().is_empty() && !p.in_check());
        assert_eq!(status(&p, &p.legal(), 1).unwrap().reason, "stalemate");
        // Yetersiz malzeme.
        assert!(from_fen("8/8/4k3/8/8/2B5/4K3/8 w - - 0 1").insufficient());
        assert!(from_fen("8/8/4k3/8/8/2N5/4K3/8 w - - 0 1").insufficient());
        assert!(!from_fen("8/8/4k3/8/8/2NN4/4K3/8 w - - 0 1").insufficient());
        assert!(!from_fen("8/8/4k3/8/8/2R5/4K3/8 w - - 0 1").insufficient());
        assert!(from_fen("8/2b5/4k3/8/8/2B5/4K3/8 w - - 0 1").insufficient(), "aynı renk kare filleri");
        // 50 hamle kuralı.
        let p = from_fen("8/8/4k3/8/8/2R5/4K3/8 w - - 100 80");
        assert_eq!(status(&p, &p.legal(), 1).unwrap().reason, "fifty");
    }
}
