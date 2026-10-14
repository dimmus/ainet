//! Seeded 16x16 pixel-art critters: one face and one name per agent.
//!
//! A port of Den's `internal/critter` and `internal/badge` (MIT, © 2026 Martin
//! Granström), which are themselves ports of the agent-badge mod's
//! `hooks/register.tsx`. Seeds, species, poses and names match bit for bit, so
//! a Claude Code session id gets the same critter here, in Den and in
//! agent-badge.
//!
//! The crate has no dependencies and knows nothing about terminals: it returns
//! pixel grids and cell grids, and the TUI paints them.

mod octants;

use octants::OCTANTS;

/// Sprite size in pixels.
pub const N: usize = 16;

/// An sRGB colour.
pub type Rgb = [u8; 3];

/// One pixel: `None` is transparent.
pub type Px = Option<Rgb>;

/// The full sprite.
pub type Grid = [[Px; N]; N];

/// Body colours used when the caller passes none, in agent-badge's order.
pub const DEFAULT_BODIES: [Rgb; 3] = [[0x66, 0xcb, 0x8e], [0xe8, 0xb8, 0x4a], [0xea, 0x9e, 0x9e]];

/// Parses `#rrggbb` (the `#` is optional).
pub fn parse_hex(hex: &str) -> Option<Rgb> {
    let h = hex.strip_prefix('#').unwrap_or(hex);
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Formats a colour as `#rrggbb`.
pub fn to_hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// agent-badge's seed for a session id: 32-bit FNV-1a over UTF-16 code units.
pub fn hash(text: &str) -> u32 {
    let mut acc: u32 = 0x811c_9dc5;
    for unit in text.encode_utf16() {
        acc ^= u32::from(unit);
        acc = acc.wrapping_mul(0x0100_0193);
    }
    acc
}

/// mulberry32, as agent-badge uses it.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }

    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    fn index(&mut self, len: usize) -> usize {
        (self.next() * len as f64).floor() as usize % len
    }
}

const ADJECTIVES: [&str; 20] = [
    "Mossy", "Fizzy", "Sleepy", "Brave", "Tiny", "Cosmic", "Fuzzy", "Zippy", "Wobbly", "Sunny", "Misty",
    "Plucky", "Snug", "Bouncy", "Quiet", "Spicy", "Velvet", "Pebble", "Comet", "Maple",
];

const CRITTERS: [&str; 20] = [
    "Otter", "Moth", "Newt", "Fox", "Owl", "Axolotl", "Panda", "Gecko", "Quokka", "Hedgehog", "Puffin",
    "Wombat", "Ferret", "Koala", "Lemur", "Capybara", "Badger", "Narwhal", "Sprite", "Tanuki",
];

/// The critter name agent-badge shows for a session id, e.g. "Mossy Lemur".
pub fn name(session_id: &str) -> String {
    let mut r = Rng::new(hash(session_id) ^ 0x9e37_79b9);
    let adjective = ADJECTIVES[r.index(ADJECTIVES.len())];
    let critter = CRITTERS[r.index(CRITTERS.len())];
    format!("{adjective} {critter}")
}

/// One animation frame's parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pose {
    pub breathe: bool,
    pub blink: bool,
    /// -1 looks left, 0 ahead, 1 right.
    pub look: i32,
    pub hop: bool,
}

impl Pose {
    /// agent-badge's animation at `tick`: a slow breath at rest with the odd
    /// blink; while working, no breathing but glances around and little hops.
    pub fn at(seed: u32, tick: u64, working: bool) -> Self {
        let period = if working { 3 } else { 6 };
        let breathe = (tick / period) % 2 == 1;
        let blink = (u64::from(seed) + tick * 7919).is_multiple_of(37);
        if !working {
            return Pose {
                breathe,
                blink,
                ..Pose::default()
            };
        }
        let look = match (tick / 8) % 4 {
            1 => -1,
            3 => 1,
            _ => 0,
        };
        Pose {
            breathe: false,
            blink,
            look,
            hop: tick % 6 < 2,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ears {
    Cat,
    Bunny,
    Antenna,
    Horns,
    Round,
    Tuft,
}

struct Species {
    ears: Ears,
    half_width: f64,
    top: i32,
    belly: bool,
    spots: bool,
    mouth: bool,
}

impl Species {
    fn new(seed: u32) -> Self {
        let mut r = Rng::new(seed ^ 0x51ed_270b);
        // Field order is draw order: each field consumes the generator in turn.
        let ears = [
            Ears::Cat,
            Ears::Bunny,
            Ears::Antenna,
            Ears::Horns,
            Ears::Round,
            Ears::Tuft,
        ][r.index(6)];
        let half_width = if r.next() < 0.5 { 6.2 } else { 5.5 };
        let top = if r.next() < 0.5 { 6 } else { 5 };
        let belly = r.next() < 0.7;
        let spots = r.next() < 0.35;
        let mouth = r.next() < 0.6;
        Species {
            ears,
            half_width,
            top,
            belly,
            spots,
            mouth,
        }
    }
}

/// The palette a sprite is drawn with.
#[derive(Clone, Debug)]
pub struct Colors {
    /// Candidate body colours; the seed picks one. Empty means [`DEFAULT_BODIES`].
    pub bodies: Vec<Rgb>,
    /// The background the critter sits on; eyes and outline lean towards it.
    pub dark: Rgb,
    /// Cheeks.
    pub blush: Rgb,
}

impl Default for Colors {
    fn default() -> Self {
        Colors {
            bodies: DEFAULT_BODIES.to_vec(),
            dark: [0, 0, 0],
            blush: [0xf4, 0xc4, 0xc4],
        }
    }
}

impl Colors {
    /// A palette with a single body colour, for an agent whose colour was
    /// already assigned.
    pub fn with_body(body: Rgb) -> Self {
        Colors {
            bodies: vec![body],
            ..Colors::default()
        }
    }

    fn body(&self, seed: u32) -> Rgb {
        if self.bodies.is_empty() {
            DEFAULT_BODIES[seed as usize % DEFAULT_BODIES.len()]
        } else {
            self.bodies[seed as usize % self.bodies.len()]
        }
    }

    /// The colour the sprite draws eyes and mouth with.
    pub fn ink(&self, seed: u32) -> Rgb {
        round(mix(f(self.dark), f(self.body(seed)), 0.15))
    }
}

type F3 = [f64; 3];

fn f(c: Rgb) -> F3 {
    [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]
}

fn round(c: F3) -> Rgb {
    c.map(|v| v.round().clamp(0.0, 255.0) as u8)
}

fn mix(a: F3, b: F3, t: f64) -> F3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

struct Canvas([[Option<F3>; N]; N]);

impl Canvas {
    fn at(&self, x: i32, y: i32) -> Option<F3> {
        if (0..N as i32).contains(&x) && (0..N as i32).contains(&y) {
            self.0[y as usize][x as usize]
        } else {
            None
        }
    }

    fn put(&mut self, x: i32, y: i32, c: F3) {
        if (0..N as i32).contains(&x) && (0..N as i32).contains(&y) {
            self.0[y as usize][x as usize] = Some(c);
        }
    }

    fn mirror(&mut self, x: i32, y: i32, c: F3) {
        self.put(x, y, c);
        self.put(N as i32 - 1 - x, y, c);
    }
}

/// Draws the sprite for `seed` in `pose`.
pub fn pixels(seed: u32, colors: &Colors, pose: Pose) -> Grid {
    let s = Species::new(seed);
    let body = f(colors.body(seed));
    let white = [255.0; 3];
    let ink = mix(f(colors.dark), body, 0.15);
    let outline = mix(body, ink, 0.6);
    let light = mix(body, white, 0.35);
    let shade = mix(body, ink, 0.25);
    let belly_color = mix(body, white, 0.5);
    let blush = f(colors.blush);
    let mut spots_rng = Rng::new(seed ^ 0x2545_f491);

    let mut g = Canvas([[None; N]; N]);
    let half = (N / 2) as f64;

    let lift = i32::from(pose.hop);
    let top = s.top - lift + i32::from(pose.breathe);
    let bottom = 13 - lift;
    let cy = f64::from(top + bottom) / 2.0 + 0.5;
    let ry = f64::from(bottom - top + 1) / 2.0;
    let rx = s.half_width + if pose.breathe { 0.3 } else { 0.0 };

    for y in 0..N as i32 {
        for x in 0..N as i32 {
            let dx = (f64::from(x) + 0.5 - half) / rx;
            let dy = (f64::from(y) + 0.5 - cy) / ry;
            if dx * dx + dy * dy > 1.0 {
                continue;
            }
            let c = if dy > 0.45 {
                shade
            } else if dx < -0.25 && dy < -0.2 {
                light
            } else {
                body
            };
            g.put(x, y, c);
        }
    }

    let ex = (half - s.half_width + 1.5).round() as i32;
    match s.ears {
        Ears::Cat => {
            for k in 0..3 {
                for i in 0..=2 - k {
                    g.mirror(ex + i, top - k, body);
                }
            }
            g.mirror(ex + 1, top, light);
        }
        Ears::Bunny => {
            for k in 1..=4 {
                g.mirror(ex + 1, top - k, body);
                g.mirror(ex + 2, top - k, if k > 1 && k < 4 { belly_color } else { body });
            }
        }
        Ears::Antenna => {
            for k in 1..=2 {
                g.mirror(ex + 2, top - k, outline);
            }
            g.mirror(ex + 1, top - 3, light);
            g.mirror(ex + 2, top - 3, light);
        }
        Ears::Horns => {
            g.mirror(ex + 1, top - 1, belly_color);
            g.mirror(ex, top - 2, belly_color);
        }
        Ears::Round => {
            for (dx, dy) in [(0, 0), (1, 0), (0, -1), (1, -1), (-1, 0)] {
                g.mirror(ex + dx, top + dy, body);
            }
            g.mirror(ex, top - 1, light);
        }
        Ears::Tuft => {
            g.put(7, top - 1, body);
            g.put(8, top - 2, body);
            g.put(8, top - 1, light);
        }
    }

    if s.belly {
        for y in 0..N as i32 {
            for x in 0..N as i32 {
                let dx = (f64::from(x) + 0.5 - half) / (rx - 2.6);
                let dy = (f64::from(y) + 0.5 - (cy + ry * 0.45)) / (ry * 0.45);
                if dx * dx + dy * dy <= 1.0 && g.at(x, y).is_some() {
                    g.put(x, y, belly_color);
                }
            }
        }
    }

    if s.spots {
        for _ in 0..3 {
            let x = 3 + (spots_rng.next() * 3.0).floor() as i32;
            let y = (cy - 1.0 + spots_rng.next() * 3.0).round() as i32;
            if g.at(x, y).is_some() {
                g.mirror(x, y, shade);
            }
        }
    }

    // Feet.
    g.mirror(5, bottom + 1, outline);
    g.mirror(4, bottom + 1, outline);

    // Outline: every empty pixel touching the body.
    let filled = g.0.map(|row| row.map(|p| p.is_some()));
    for y in 0..N as i32 {
        for x in 0..N as i32 {
            if filled[y as usize][x as usize] {
                continue;
            }
            let near = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                let (nx, ny) = (x + dx, y + dy);
                (0..N as i32).contains(&nx) && (0..N as i32).contains(&ny) && filled[ny as usize][nx as usize]
            });
            if near {
                g.put(x, y, outline);
            }
        }
    }

    let eye_y = (cy - ry * 0.2).round() as i32;
    for side in [-1, 1] {
        let ex0 = if side < 0 { 5 } else { 9 } + pose.look;
        if pose.blink {
            g.put(ex0, eye_y, ink);
            g.put(ex0 + 1, eye_y, ink);
        } else {
            g.put(ex0, eye_y - 1, ink);
            g.put(ex0 + 1, eye_y - 1, ink);
            g.put(ex0, eye_y, ink);
            g.put(ex0 + 1, eye_y, ink);
            let sparkle = if pose.look > 0 { ex0 + 1 } else { ex0 };
            g.put(sparkle, eye_y - 1, white);
        }
    }
    let under = |g: &Canvas, x: i32| g.at(x, eye_y + 1).unwrap_or(body);
    let cheek_l = mix(under(&g, 4), blush, 0.7);
    let cheek_r = mix(under(&g, 11), blush, 0.7);
    g.put(4, eye_y + 1, cheek_l);
    g.put(11, eye_y + 1, cheek_r);
    if s.mouth {
        let m7 = mix(under(&g, 7), ink, 0.45);
        let m8 = mix(under(&g, 8), ink, 0.45);
        g.put(7, eye_y + 1, m7);
        g.put(8, eye_y + 1, m8);
    }

    g.0.map(|row| row.map(|p| p.map(round)))
}

/// One terminal cell of a rendered sprite. `fg` colours the glyph, `bg` the
/// cell behind it; `None` leaves the terminal's own colour showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Px,
    pub bg: Px,
}

impl Cell {
    pub const EMPTY: Cell = Cell {
        ch: ' ',
        fg: None,
        bg: None,
    };

    fn half(top: Px, bottom: Px) -> Cell {
        match (top, bottom) {
            (None, None) => Cell::EMPTY,
            (Some(t), None) => Cell {
                ch: '▀',
                fg: Some(t),
                bg: None,
            },
            (None, Some(b)) => Cell {
                ch: '▄',
                fg: Some(b),
                bg: None,
            },
            (Some(t), Some(b)) => Cell {
                ch: '▀',
                fg: Some(t),
                bg: Some(b),
            },
        }
    }
}

/// Renders the sprite with half blocks, two pixels per cell: 16 columns by 8
/// rows. Works in any truecolour terminal.
pub fn half_blocks(px: &Grid) -> [[Cell; N]; N / 2] {
    let mut rows = [[Cell::EMPTY; N]; N / 2];
    for (r, row) in rows.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = Cell::half(px[2 * r][x], px[2 * r + 1][x]);
        }
    }
    rows
}

/// Renders the sprite at half size, 8 columns by 4 rows: each 2x2 block takes
/// the colour most of its pixels share, so the silhouette survives.
pub fn small(px: &Grid) -> [[Cell; N / 2]; N / 4] {
    let mut shrunk = [[None; N / 2]; N / 2];
    for (y, row) in shrunk.iter_mut().enumerate() {
        for (x, out) in row.iter_mut().enumerate() {
            let block = [
                px[2 * y][2 * x],
                px[2 * y][2 * x + 1],
                px[2 * y + 1][2 * x],
                px[2 * y + 1][2 * x + 1],
            ];
            let mut counts: Vec<(Rgb, usize)> = Vec::with_capacity(4);
            let mut best: Option<usize> = None;
            for c in block.into_iter().flatten() {
                let i = match counts.iter().position(|(k, _)| *k == c) {
                    Some(i) => i,
                    None => {
                        counts.push((c, 0));
                        counts.len() - 1
                    }
                };
                counts[i].1 += 1;
                if best.is_none_or(|b| counts[i].1 > counts[b].1) {
                    best = Some(i);
                }
            }
            // A lone pixel in a mostly empty block is noise.
            *out = best.filter(|&b| counts[b].1 >= 2).map(|b| counts[b].0);
        }
    }
    let mut rows = [[Cell::EMPTY; N / 2]; N / 4];
    for (r, row) in rows.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = Cell::half(shrunk[2 * r][x], shrunk[2 * r + 1][x]);
        }
    }
    rows
}

/// Renders the sprite with octant characters, 2x4 pixels per cell: the whole
/// creature at full resolution in 8 columns by 4 rows. Needs a font with
/// Unicode 16 octants (recent Ghostty, kitty, WezTerm, foot).
///
/// A cell carries two colours: the most common is the glyph, and the one that
/// stands out most against it (count times distance) is the background, so
/// the dark outline and the eyes are not voted away. Edge cells with
/// transparent pixels can only paint one colour.
pub fn octants(px: &Grid) -> [[Cell; N / 2]; N / 4] {
    let mut rows = [[Cell::EMPTY; N / 2]; N / 4];
    for (r, row) in rows.iter_mut().enumerate() {
        for (c, out) in row.iter_mut().enumerate() {
            let mut cell = [None; 8];
            for (i, p) in cell.iter_mut().enumerate() {
                *p = px[4 * r + i / 2][2 * c + i % 2];
            }
            let mut counts: Vec<(Rgb, usize)> = Vec::with_capacity(8);
            for p in cell.iter().flatten() {
                match counts.iter_mut().find(|(k, _)| k == p) {
                    Some(e) => e.1 += 1,
                    None => counts.push((*p, 1)),
                }
            }
            let filled: usize = counts.iter().map(|e| e.1).sum();
            if filled == 0 {
                continue;
            }
            let mut fg = most_common(&counts);
            let mut bg = if filled == 8 && counts.len() > 1 {
                most_contrasting(&counts, fg)
            } else {
                None
            };
            let mut bits = 0usize;
            for (i, v) in cell.iter().enumerate() {
                let Some(v) = v else { continue };
                if bg.is_none_or(|b| nearer(*v, fg, b)) {
                    bits |= 1 << i;
                }
            }
            if bits == 0 {
                // Everything sits nearer the background: swap so the glyph
                // still has something to draw.
                if let Some(b) = bg {
                    bg = Some(fg);
                    fg = b;
                }
                bits = 0xff;
            }
            let ch = char::from_u32(OCTANTS[bits]).unwrap_or('█');
            *out = Cell { ch, fg: Some(fg), bg };
        }
    }
    rows
}

/// The colour with the highest count; ties go to the lower colour, as Den
/// breaks them by hex string.
fn most_common(counts: &[(Rgb, usize)]) -> Rgb {
    counts
        .iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|e| e.0)
        .unwrap_or([0; 3])
}

fn most_contrasting(counts: &[(Rgb, usize)], fg: Rgb) -> Option<Rgb> {
    counts
        .iter()
        .filter(|(c, _)| *c != fg)
        .map(|(c, n)| (*c, *n as f64 * dist(*c, fg)))
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|e| e.0)
}

fn nearer(v: Rgb, a: Rgb, b: Rgb) -> bool {
    if v == a {
        return true;
    }
    if v == b {
        return false;
    }
    dist(v, a) <= dist(v, b)
}

fn dist(a: Rgb, b: Rgb) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (f64::from(*x) - f64::from(y)).powi(2))
        .sum()
}

/// Prints the sprite to a truecolour terminal, for `ainet critter` and tests.
pub fn ansi(rows: &[impl AsRef<[Cell]>]) -> String {
    let mut out = String::new();
    for row in rows {
        for cell in row.as_ref() {
            if let Some([r, g, b]) = cell.fg {
                out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
            }
            if let Some([r, g, b]) = cell.bg {
                out.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
            }
            out.push(cell.ch);
            out.push_str("\x1b[0m");
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: [&str; 4] = [
        "11111111-2222-4333-8444-555555555501",
        "11111111-2222-4333-8444-555555555502",
        "11111111-2222-4333-8444-555555555503",
        "11111111-2222-4333-8444-555555555504",
    ];

    #[test]
    fn hash_is_fnv1a() {
        assert_eq!(hash(""), 0x811c_9dc5);
        assert_eq!(hash("a"), 0xe40c_292c);
    }

    #[test]
    fn sprite_is_deterministic_and_bounded() {
        let colors = Colors::default();
        for id in IDS {
            let seed = hash(id);
            let a = pixels(seed, &colors, Pose::default());
            assert_eq!(
                a,
                pixels(seed, &colors, Pose::default()),
                "{id} not deterministic"
            );
            let filled = a.iter().flatten().filter(|p| p.is_some()).count();
            assert!((60..=220).contains(&filled), "{id}: {filled} filled pixels");
        }
    }

    #[test]
    fn poses_follow_agent_badge() {
        let rest = Pose::at(1, 0, false);
        assert!(!rest.hop && rest.look == 0);
        let working = Pose::at(1, 8, true);
        assert_eq!(working.look, -1);
        assert!(!working.breathe);
    }

    #[test]
    fn octants_keep_the_ink() {
        let seed = hash("octant-test");
        let colors = Colors::default();
        let px = pixels(seed, &colors, Pose::default());
        let ink = colors.ink(seed);
        assert!(
            px.iter().flatten().any(|p| *p == Some(ink)),
            "ink missing from sprite"
        );
        let rows = octants(&px);
        let painted = rows.iter().flatten().filter(|c| c.ch != ' ').count();
        let inked = rows
            .iter()
            .flatten()
            .filter(|c| c.bg == Some(ink) || c.fg == Some(ink))
            .count();
        assert!(painted >= 8 && inked > 0, "painted {painted}, inked {inked}");
        assert_eq!(OCTANTS[0b0000_0001], 0x1CEA8);
        assert_eq!(OCTANTS[0b1111_1111], '█' as u32);
        assert_eq!(OCTANTS[0b0101_0101], '▌' as u32);
    }

    #[test]
    fn names_are_two_words() {
        for id in IDS {
            let n = name(id);
            assert_eq!(n.split(' ').count(), 2, "{n}");
            assert_eq!(n, name(id));
        }
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse_hex("#66cb8e"), Some([0x66, 0xcb, 0x8e]));
        assert_eq!(to_hex([0x66, 0xcb, 0x8e]), "#66cb8e");
        assert_eq!(parse_hex("nope"), None);
    }
}
