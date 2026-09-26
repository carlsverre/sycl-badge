//! Indexed pixel art. Index 0 is transparent; the rest look up a palette at draw
//! time, which is what lets one sprite fade into fog or change paint colour.

use sycl_cart::Color;

/// Row-major palette indices.
pub struct Art {
    pub w: usize,
    pub h: usize,
    pub px: &'static [u8],
}

/// Parse art rows at compile time. With `mirror`, each row holds the left half
/// and the right half is its reflection.
pub const fn parse<const N: usize>(rows: &[&str], w: usize, mirror: bool) -> [u8; N] {
    let mut out = [0u8; N];
    assert!(rows.len() * w == N, "row count does not match the art size");
    let row_len = if mirror { w / 2 } else { w };
    let mut y = 0;
    while y < rows.len() {
        let r = rows[y].as_bytes();
        assert!(r.len() == row_len, "art row has the wrong width");
        let mut x = 0;
        while x < row_len {
            let v = match r[x] {
                b'.' => 0,
                c @ b'1'..=b'9' => c - b'0',
                c @ b'a'..=b'f' => c - b'a' + 10,
                _ => panic!("bad art character"),
            };
            out[y * w + x] = v;
            if mirror {
                out[y * w + (w - 1 - x)] = v;
            }
            x += 1;
        }
        y += 1;
    }
    out
}

macro_rules! art {
    ($w:expr, $mirror:expr, [$($row:expr),* $(,)?]) => {{
        const ROWS: &[&str] = &[$($row),*];
        const PX: [u8; $w * ROWS.len()] = parse::<{ $w * ROWS.len() }>(ROWS, $w, $mirror);
        Art { w: $w, h: ROWS.len(), px: &PX }
    }};
}

pub type Palette = [Color; 16];

const fn pal(entries: &[u32]) -> Palette {
    let mut p = [Color::BLACK; 16];
    let mut i = 0;
    while i < entries.len() {
        p[i + 1] = Color::hex(entries[i]);
        i += 1;
    }
    p
}

// ── The player's car, rear view ─────────────────────────────────────────────

/// Rows above this are the cabin, which leans into corners.
pub const CABIN_ROWS: usize = 9;

pub const PLAYER: Art = art!(44, true, [
    "..............aaaa....",
    ".............aaaaaa...",
    ".............aaaaaa...",
    "......77777777aaaa7777",
    ".....7cc77777777777777",
    "....7cc777777777777777",
    "..33333333333333333333",
    ".344444444444444444444",
    ".333333333333333333333",
    "3322222222222222222222",
    "3211111111111111111111",
    "3215555555161616161616",
    "3215555555616161616161",
    "3211111111111111111111",
    "3322222222222222bbbbbb",
    "3322222222222222b1b1bb",
    "3366666666666666666666",
    ".266666666666666666666",
    "11111111......66......",
    "11111111..............",
    "11111111..............",
    ".111111...............",
    "..dddddddddddddddddddd",
]);

/// Palette index of the tail lights, which brighten when braking and glow
/// through fog at night.
pub const TAIL: usize = 5;

pub const PLAYER_PAL: Palette = pal(&[
    0x101018, // 1 tyres, grille
    0x8c1018, // 2 dark red
    0xe02030, // 3 red
    0xff7a70, // 4 highlight
    0xc02810, // 5 tail lights
    0x8890a0, // 6 bumper, exhaust
    0x283058, // 7 glass
    0xf0b080, // 8 skin
    0xffe060, // 9 blonde
    0x301808, // a dark hair
    0xf0f0f0, // b plate
    0x7888c8, // c glass highlight
    0x181820, // d shadow
]);

pub const BRAKE_LIGHT: Color = Color::hex(0xff4020);

// ── Traffic ─────────────────────────────────────────────────────────────────

pub const CAR: Art = art!(24, true, [
    ".....3333333",
    "....37777777",
    "...377c77777",
    "..3333333333",
    ".34444444444",
    ".33333333333",
    "321111111111",
    "321555111bbb",
    "321111111bbb",
    "332222222222",
    "366666666666",
    "1111........",
    "1111........",
    ".ddddddddddd",
]);

const fn car_pal(dark: u32, body: u32, hi: u32) -> Palette {
    pal(&[
        0x101018, dark, body, hi, 0xd03018, 0x8890a0, 0x283058, 0, 0, 0, 0xf0f0f0, 0x7888c8,
        0x181820,
    ])
}

pub const CAR_PALS: [Palette; 5] = [
    car_pal(0x10306a, 0x2860d0, 0x78a8ff),
    car_pal(0x8a6a00, 0xf0c010, 0xfff080),
    car_pal(0x8890a0, 0xe8e8f0, 0xffffff),
    car_pal(0x0a5a30, 0x20a860, 0x80f0a0),
    car_pal(0x401060, 0x8030c0, 0xc080ff),
];

// ── Scenery ─────────────────────────────────────────────────────────────────

pub const PALM: Art = art!(24, false, [
    "........33......3.......",
    "......332222..3322......",
    "....3322221122222223....",
    "..33222211.12221112223..",
    ".322211...6612....11223.",
    "3221.....6665.1.....1123",
    "321.....33455122.....113",
    "21....33222541222.....12",
    "1....3222115412222.....1",
    "....322211.54..12222....",
    "...32211...54....1222...",
    "..3211.....54.....1122..",
    "..211......54.......12..",
    "..1.......454........1..",
    "..........454...........",
    "..........455...........",
    "..........454...........",
    "..........455...........",
    ".........4554...........",
    ".........454............",
    ".........455............",
    ".........454............",
    ".........455............",
    "........4554............",
    "........454.............",
    "........455.............",
    "........454.............",
    "........455.............",
    "........454.............",
    "........4554............",
    "........4545............",
    ".......44554............",
]);

pub const PALM_PAL: Palette = pal(&[0x0c5020, 0x20a030, 0x70d848, 0x5a3418, 0xa06830, 0x4a2c10]);

pub const BOULDER: Art = art!(20, true, [
    "......2222",
    "....223333",
    "...2233333",
    "..22333322",
    ".223332222",
    ".222222222",
    "2222222211",
    "2222221111",
    "2222111111",
    "2211111111",
    "1111111111",
    ".111111111",
    "..11111111",
    "..dddddddd",
]);

pub const BOULDER_PAL: Palette = {
    let mut p = pal(&[0x4a4038, 0x7a6a58, 0xa89880]);
    p[13] = Color::hex(0x282420);
    p
};

/// A curve warning sign. The arrow points left; flip it for a right-hander.
pub const SIGN: Art = art!(16, false, [
    "1111111111111111",
    "1777777777777771",
    "1777711777117771",
    "1777117771177771",
    "1771177711777771",
    "1711777117777771",
    "1771177711777771",
    "1777117771177771",
    "1777711777117771",
    "1777777777777771",
    "1111111111111111",
    ".......66.......",
    ".......66.......",
    ".......66.......",
    ".......66.......",
    ".......66.......",
]);

pub const SIGN_PAL: Palette = pal(&[0x101010, 0, 0, 0, 0, 0x707880, 0xffc818]);

// ── Runtime-rendered text art: billboards and gantry banners ────────────────

pub const BOARD_W: usize = 52;
pub const BOARD_H: usize = 28;
pub const BOARD_TEXT: [(&str, &str); 4] = [
    ("SYCL", "2026"),
    ("ZIG", "+RUST"),
    ("GO", "FAST!"),
    ("SUNSET", "RUN"),
];
pub const BOARD_PALS: [Palette; 4] = [
    pal(&[0x0a0a14, 0x14206a, 0xffe040, 0x606870]),
    pal(&[0x0a0a14, 0x8a1020, 0xffffff, 0x606870]),
    pal(&[0x0a0a14, 0x0a6a6a, 0xfff0a0, 0x606870]),
    pal(&[0x0a0a14, 0x5a1a7a, 0xff9040, 0x606870]),
];

pub const BANNER_W: usize = 84;
pub const BANNER_H: usize = 12;
pub const BANNER_PAL: Palette = pal(&[0x0a0a14, 0x1830a0, 0xfff040, 0x606870, 0xf0f0f0]);

/// Draw 8x8 text into an index buffer, centred on `cy` (the top of the glyphs).
pub fn text_into(buf: &mut [u8], w: usize, text: &str, cy: usize, fg: u8) {
    let tw = text.len() * 8;
    let x0 = (w - tw.min(w)) / 2;
    for (i, ch) in text.bytes().enumerate() {
        let glyph = &sycl_cart::font::GLYPHS[(ch - sycl_cart::font::FIRST) as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits & (0x80 >> col) != 0 {
                    let x = x0 + i * 8 + col;
                    if x < w {
                        buf[(cy + row) * w + x] = fg;
                    }
                }
            }
        }
    }
}

/// A billboard: dark frame, coloured board, two lines of text, two legs.
pub fn build_board(buf: &mut [u8; BOARD_W * BOARD_H], lines: (&str, &str)) {
    let (w, board_h) = (BOARD_W, 21);
    for y in 0..BOARD_H {
        for x in 0..w {
            buf[y * w + x] = if y < board_h {
                if x == 0 || x == w - 1 || y == 0 || y == board_h - 1 {
                    1
                } else {
                    2
                }
            } else if (6..9).contains(&x) || (w - 9..w - 6).contains(&x) {
                4
            } else {
                0
            };
        }
    }
    text_into(buf, w, lines.0, 2, 3);
    text_into(buf, w, lines.1, 11, 3);
}

/// A gantry banner with a chequered edge.
pub fn build_banner(buf: &mut [u8; BANNER_W * BANNER_H], text: &str) {
    for y in 0..BANNER_H {
        for x in 0..BANNER_W {
            buf[y * BANNER_W + x] = if y == 0 || y == BANNER_H - 1 {
                if (x / 2) % 2 == 0 {
                    1
                } else {
                    5
                }
            } else {
                2
            };
        }
    }
    text_into(buf, BANNER_W, text, 2, 3);
}
