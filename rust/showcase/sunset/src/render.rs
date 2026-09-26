//! The pseudo-3D renderer.
//!
//! The road is drawn near to far, one segment at a time. Each segment projects
//! to a trapezoid; the renderer fills its screen rows and then lowers `maxy` to
//! the segment's far edge, so a hill crest hides everything behind it. Sprites
//! then go far to near, each clipped at the `maxy` its segment saw.
//!
//! The backbuffer is column-major, but SRAM on the RP2350 has no cache, so a
//! strided row write costs the same as a contiguous one.

use crate::art::{self, Palette};
use crate::track::{self, Track, SEG_LEN};
use sycl_cart::gfx::PIXELS;
use sycl_cart::{math, Color, HEIGHT, WIDTH};

pub const ROAD_W: f32 = 2000.0;
pub const CAM_H: f32 = 1000.0;
/// 1 / tan(50°): a 100° horizontal field of view.
pub const DEPTH: f32 = 0.8391;
/// How far ahead of the camera the player's car sits.
pub const PLAYER_Z: f32 = CAM_H * DEPTH;
pub const DRAW_DIST: usize = 150;
pub const HORIZON: f32 = 50.0;
const XS: f32 = WIDTH as f32 / 2.0;
const YS: f32 = 72.0;
/// Screen row of the bottom of the player's car.
pub const PLAYER_Y: i32 = (HORIZON + YS) as i32;
/// World units per art pixel. Chosen so an art pixel on the player's car is
/// one screen pixel.
pub const PX_WORLD: f32 = CAM_H / XS;

const H: usize = HEIGHT;
const FOG_LEVELS: usize = 16;

/// Width of a sprite in road half-widths, which is what collisions compare.
pub fn world_w(art_w: usize, mag: f32) -> f32 {
    art_w as f32 * mag * PX_WORLD / ROAD_W
}

pub const CAR_MAG: f32 = 1.8;
pub const PALM_MAG: f32 = 4.0;
pub const BOULDER_MAG: f32 = 3.0;
pub const SIGN_MAG: f32 = 2.5;
pub const BOARD_MAG: f32 = 3.5;

/// Where a piece of scenery is solid, as (centre, width) in road half-widths.
/// Scenery is drawn outward from its offset, so its centre lies further out.
pub fn scenery_hitbox(kind: u8, off: f32) -> (f32, f32) {
    let side = if off < 0.0 { -1.0 } else { 1.0 };
    let (full, centre, solid) = match kind {
        // Only the trunk is solid, 60 % of the way out from the inner edge.
        track::PALM => (world_w(art::PALM.w, PALM_MAG), 0.6, 0.2),
        track::BOULDER => (world_w(art::BOULDER.w, BOULDER_MAG), 0.5, 0.9),
        track::SIGN_L | track::SIGN_R => (world_w(art::SIGN.w, SIGN_MAG), 0.5, 0.25),
        _ => (world_w(art::BOARD_W, BOARD_MAG), 0.5, 0.9),
    };
    (off + side * full * centre, full * solid)
}

// ── Palettes ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
struct Key {
    sky_top: Color,
    sky_bot: Color,
    sun_top: Color,
    sun_bot: Color,
    far: Color,
    near: Color,
    fog: Color,
    road: [Color; 7],
}

const fn key(
    sky: (u32, u32),
    sun: (u32, u32),
    far: u32,
    near: u32,
    fog: u32,
    road: [u32; 7],
) -> Key {
    let mut r = [Color::BLACK; 7];
    let mut i = 0;
    while i < 7 {
        r[i] = Color::hex(road[i]);
        i += 1;
    }
    Key {
        sky_top: Color::hex(sky.0),
        sky_bot: Color::hex(sky.1),
        sun_top: Color::hex(sun.0),
        sun_bot: Color::hex(sun.1),
        far: Color::hex(far),
        near: Color::hex(near),
        fog: Color::hex(fog),
        road: r,
    }
}

// Road colours: grass light/dark, rumble light/dark, asphalt light/dark, lane.
const DAY: Key = key(
    (0x2a68d0, 0xa8d8f8),
    (0xfffce0, 0xfff0a0),
    0x7890b8,
    0x3a7a48,
    0xb8dcf0,
    [0x48b840, 0x38a434, 0xf0f0f0, 0xd02020, 0x747478, 0x6c6c70, 0xf0f0f0],
);
const DUSK: Key = key(
    (0x1c1048, 0xff8a50),
    (0xfff060, 0xff2a80),
    0x6a2c70,
    0x2e1a48,
    0xf07a60,
    [0x6a9a30, 0x5a8828, 0xf0e8e0, 0xd02838, 0x6c5a6c, 0x645266, 0xf8e8d8],
);
const NIGHT: Key = key(
    (0x000008, 0x0c1438),
    (0xf8f8e8, 0xc8c8d0),
    0x121830,
    0x080c18,
    0x060a18,
    [0x184818, 0x123c12, 0xb0b0b0, 0x902020, 0x383840, 0x303038, 0xd0d0d0],
);

fn mix_key(a: &Key, b: &Key, t: f32) -> Key {
    let mut road = [Color::BLACK; 7];
    for (i, r) in road.iter_mut().enumerate() {
        *r = a.road[i].mix(b.road[i], t);
    }
    Key {
        sky_top: a.sky_top.mix(b.sky_top, t),
        sky_bot: a.sky_bot.mix(b.sky_bot, t),
        sun_top: a.sun_top.mix(b.sun_top, t),
        sun_bot: a.sun_bot.mix(b.sun_bot, t),
        far: a.far.mix(b.far, t),
        near: a.near.mix(b.near, t),
        fog: a.fog.mix(b.fog, t),
        road,
    }
}

/// Everything derived from the time of day, recomputed only when it changes.
struct Look {
    /// Quantized time of day this was built for; `u8::MAX` means never built.
    built_for: u8,
    sky: [u16; H],
    sun: [u16; 40],
    far: u16,
    near: u16,
    ground: u16,
    road: [[u16; 7]; FOG_LEVELS],
    fog: Color,
    night: f32,
    dusk: f32,
}

// ── Scene inputs ────────────────────────────────────────────────────────────

/// One car on the road.
#[derive(Copy, Clone)]
pub struct Car {
    pub z: f32,
    /// Lateral position in road half-widths.
    pub offset: f32,
    pub speed: f32,
    pub paint: u8,
}

impl Car {
    pub const ZERO: Car = Car {
        z: 0.0,
        offset: 0.0,
        speed: 0.0,
        paint: 0,
    };
}

/// Where the camera is and how the background has scrolled.
pub struct View {
    pub position: f32,
    pub player_x: f32,
    /// World height of the road under the player.
    pub player_y: f32,
    pub sky_off: f32,
    pub far_off: f32,
    pub near_off: f32,
    /// Vertical screen shake in pixels.
    pub shake: f32,
    /// 0 = day, 1 = dusk, 2 = night.
    pub time_of_day: f32,
    pub frame: u32,
}

/// The player's car, as the renderer needs it.
pub struct PlayerPose {
    /// Screen pixels the cabin shifts: negative leans left.
    pub lean: i32,
    pub bounce: i32,
    pub braking: bool,
}

#[derive(Copy, Clone)]
struct SegScreen {
    /// False when the near edge is behind the camera.
    ok: bool,
    x1: f32,
    y1: f32,
    s1: f32,
    x2: f32,
    y2: f32,
    s2: f32,
    clip: f32,
}

impl SegScreen {
    const ZERO: SegScreen = SegScreen {
        ok: false,
        x1: 0.0,
        y1: 0.0,
        s1: 0.0,
        x2: 0.0,
        y2: 0.0,
        s2: 0.0,
        clip: 0.0,
    };
}

pub struct Renderer {
    look: Look,
    hm_far: [u8; 256],
    hm_near: [u8; 256],
    fog: [u8; DRAW_DIST],
    screens: [SegScreen; DRAW_DIST],
    boards: [[u8; art::BOARD_W * art::BOARD_H]; 4],
    banners: [[u8; art::BANNER_W * art::BANNER_H]; 2],
}

impl Renderer {
    pub const INIT: Renderer = Renderer {
        look: Look {
            built_for: 0,
            sky: [0; H],
            sun: [0; 40],
            far: 0,
            near: 0,
            ground: 0,
            road: [[0; 7]; FOG_LEVELS],
            fog: Color::BLACK,
            night: 0.0,
            dusk: 0.0,
        },
        hm_far: [0; 256],
        hm_near: [0; 256],
        fog: [0; DRAW_DIST],
        screens: [SegScreen::ZERO; DRAW_DIST],
        boards: [[0; art::BOARD_W * art::BOARD_H]; 4],
        banners: [[0; art::BANNER_W * art::BANNER_H]; 2],
    };

    pub fn init(&mut self) {
        self.look.built_for = u8::MAX;
        let tau = core::f32::consts::TAU;
        for x in 0..256 {
            let p = x as f32 / 256.0 * tau;
            let far = 17.0 + 8.0 * math::sin(p * 3.0) + 5.0 * math::sin(p * 7.0 + 1.0)
                + 2.5 * math::sin(p * 17.0 + 2.0);
            let near = 7.0 + 4.0 * math::sin(p * 2.0 + 0.5) + 2.5 * math::sin(p * 5.0 + 2.0)
                + 1.0 * math::sin(p * 11.0);
            self.hm_far[x] = far.clamp(1.0, 40.0) as u8;
            self.hm_near[x] = near.clamp(1.0, 20.0) as u8;
        }
        for n in 0..DRAW_DIST {
            // 1 - e^(-5 d²), with e^-x approximated as (1 - x/16)^16.
            let d = n as f32 / DRAW_DIST as f32;
            let base = (1.0 - 5.0 * d * d / 16.0).max(0.0);
            let mut e = 1.0;
            for _ in 0..16 {
                e *= base;
            }
            self.fog[n] = (((1.0 - e) * (FOG_LEVELS - 1) as f32) + 0.5) as u8;
        }
        for (i, b) in self.boards.iter_mut().enumerate() {
            art::build_board(b, art::BOARD_TEXT[i]);
        }
        art::build_banner(&mut self.banners[0], "START");
        art::build_banner(&mut self.banners[1], "CHECKPOINT");
    }

    fn update_look(&mut self, t: f32) {
        let q = (t.clamp(0.0, 2.0) * 16.0 + 0.5) as u8;
        if q == self.look.built_for {
            return;
        }
        let t = q as f32 / 16.0;
        let k = if t <= 1.0 {
            mix_key(&DAY, &DUSK, t)
        } else {
            mix_key(&DUSK, &NIGHT, t - 1.0)
        };
        let l = &mut self.look;
        l.built_for = q;
        l.night = (t - 1.0).clamp(0.0, 1.0);
        l.dusk = 1.0 - (t - 1.0).abs().min(1.0);
        l.fog = k.fog;
        let hz = HORIZON as usize;
        for y in 0..H {
            let c = if y < hz {
                let p = y as f32 / hz as f32;
                k.sky_top.mix(k.sky_bot, p * p)
            } else {
                k.fog
            };
            l.sky[y] = c.0;
        }
        for (i, s) in l.sun.iter_mut().enumerate() {
            *s = k.sun_top.mix(k.sun_bot, i as f32 / 39.0).0;
        }
        l.far = k.far.mix(k.fog, 0.35).0;
        l.near = k.near.mix(k.fog, 0.1).0;
        l.ground = k.road[1].mix(k.fog, 1.0).0;
        for (f, row) in l.road.iter_mut().enumerate() {
            let amount = f as f32 / (FOG_LEVELS - 1) as f32;
            for (i, c) in row.iter_mut().enumerate() {
                *c = k.road[i].mix(k.fog, amount).0;
            }
        }
    }

    pub fn draw(
        &mut self,
        buf: &mut [u16; PIXELS],
        trk: &Track,
        v: &View,
        cars: &[Car],
        pose: &PlayerPose,
    ) {
        self.update_look(v.time_of_day);
        let hz = HORIZON + v.shake;
        self.background(buf, v, hz);
        self.road(buf, trk, v, hz);
        self.sprites(buf, trk, v, cars);
        self.player(buf, pose, v.shake as i32);
    }

    // ── Background ──────────────────────────────────────────────────────────

    fn background(&self, buf: &mut [u16; PIXELS], v: &View, hz: f32) {
        let l = &self.look;
        let hzi = (hz as usize).min(H);
        let shift = hz as i32 - HORIZON as i32;
        for x in 0..WIDTH {
            let col = &mut buf[x * H..(x + 1) * H];
            if shift == 0 {
                col.copy_from_slice(&l.sky);
            } else {
                for (y, p) in col.iter_mut().enumerate() {
                    let sy = (y as i32 - shift).clamp(0, H as i32 - 1) as usize;
                    *p = l.sky[sy];
                }
            }
            col[hzi..].fill(l.ground);
        }

        // Stars, twinkling, scrolled with the sky.
        if l.night > 0.2 {
            let star = Color::WHITE.mix(Color::BLACK, 1.0 - l.night).0;
            for i in 0..48u32 {
                let h = hash32(i * 7 + 1);
                let vx = (h % 512) as f32;
                let y = ((h >> 9) % (HORIZON as u32 - 6)) as usize;
                if (h >> 20) % 8 == (v.frame / 8 + i) % 8 && i % 3 == 0 {
                    continue; // twinkle
                }
                let x = wrap_px(vx - v.sky_off, 512.0);
                if x < WIDTH {
                    buf[x * H + y] = star;
                }
            }
        }

        // The sun, or at night the moon. At dusk it sits low with synthwave
        // stripes cut through its lower half.
        let r = (17.0 - 6.0 * l.night) as i32;
        let sun_y = (HORIZON - 30.0 + 22.0 * l.dusk) as i32 + shift;
        let sun_x = wrap_px(80.0 - v.sky_off, 512.0) as i32;
        let striped = l.dusk > 0.5;
        for dx in -r..=r {
            let x = sun_x + dx;
            if !(0..WIDTH as i32).contains(&x) {
                continue;
            }
            let half = math::sqrt((r * r - dx * dx) as f32) as i32;
            for y in (sun_y - half).max(0)..(sun_y + half + 1).min(hzi as i32) {
                let below = y - sun_y;
                if striped && below >= 0 && below % 5 < 1 + below / 6 {
                    continue;
                }
                let i = ((y - (sun_y - r)) * 39 / (2 * r).max(1)).clamp(0, 39) as usize;
                buf[x as usize * H + y as usize] = l.sun[i];
            }
        }

        // Two layers of hills, the nearer scrolling faster.
        for x in 0..WIDTH {
            let base = x * H;
            let hf = self.hm_far[(x as i32 + v.far_off as i32) as usize & 255] as usize;
            let hn = self.hm_near[(x as i32 + v.near_off as i32) as usize & 255] as usize;
            let top_f = hzi.saturating_sub(hf);
            let top_n = hzi.saturating_sub(hn);
            buf[base + top_f..base + hzi].fill(l.far);
            buf[base + top_n..base + hzi].fill(l.near);
        }
    }

    // ── Road ────────────────────────────────────────────────────────────────

    fn road(&mut self, buf: &mut [u16; PIXELS], trk: &Track, v: &View, hz: f32) {
        let len = trk.length();
        let base = trk.index_at(v.position);
        let base_pct = (v.position - base as f32 * SEG_LEN) / SEG_LEN;
        let mut dx = -(trk.seg(base).curve * base_pct);
        let mut x = 0.0;
        let mut maxy = H as f32;
        let cam_x = v.player_x * ROAD_W;
        let cam_y = v.player_y + CAM_H;
        let start = trk.gates[0];

        for n in 0..DRAW_DIST {
            let idx = base + n;
            let seg = *trk.seg(idx);
            let cam_z = if idx >= trk.len {
                v.position - len
            } else {
                v.position
            };
            let z1 = (idx % trk.len) as f32 * SEG_LEN - cam_z;
            let z2 = z1 + SEG_LEN;

            let mut sc = SegScreen {
                clip: maxy,
                ..SegScreen::ZERO
            };
            if z1 > DEPTH {
                let s1 = DEPTH / z1;
                let s2 = DEPTH / z2;
                sc = SegScreen {
                    ok: true,
                    x1: XS + s1 * (x - cam_x) * XS,
                    y1: hz - s1 * (seg.y1 - cam_y) * YS,
                    s1,
                    x2: XS + s2 * (x + dx - cam_x) * XS,
                    y2: hz - s2 * (seg.y2 - cam_y) * YS,
                    s2,
                    clip: maxy,
                };
            }
            self.screens[n] = sc;
            x += dx;
            dx += seg.curve;

            if !sc.ok || sc.y2 >= sc.y1 || sc.y2 >= maxy {
                continue;
            }

            let fog = &self.look.road[self.fog[n] as usize];
            let light = (idx / 3).is_multiple_of(2);
            let chequer = matches!((idx % trk.len).wrapping_sub(start), 0..=1);
            let w1 = s1_w(sc.s1);
            let w2 = s1_w(sc.s2);
            let top = (sc.y2 + 0.5).max(0.0) as usize;
            let bot = ((sc.y1.min(maxy)) + 0.5).clamp(0.0, H as f32) as usize;
            let span = sc.y1 - sc.y2;
            for y in top..bot {
                let t = ((y as f32 + 0.5 - sc.y2) / span).clamp(0.0, 1.0);
                let cx = sc.x2 + (sc.x1 - sc.x2) * t;
                let w = w2 + (w1 - w2) * t;
                let parity = if chequer { Some((idx % 2) as u8) } else { None };
                road_row(buf, y, cx, w, fog, light, parity);
            }
            maxy = sc.y2;
        }
    }

    // ── Sprites ─────────────────────────────────────────────────────────────

    fn sprites(&self, buf: &mut [u16; PIXELS], trk: &Track, v: &View, cars: &[Car]) {
        let base = trk.index_at(v.position);
        let night = self.look.night;
        for n in (1..DRAW_DIST).rev() {
            let sc = self.screens[n];
            if !sc.ok {
                continue;
            }
            let idx = (base + n) % trk.len;
            let seg = trk.seg(idx);
            let fog = self.fog[n] as f32 / (FOG_LEVELS - 1) as f32;

            if trk.gates.contains(&idx) {
                let banner = if idx == trk.gates[0] { 0 } else { 1 };
                self.gantry(buf, &sc, banner, fog);
            }

            for (kind, off) in [(seg.left, seg.left_x), (seg.right, seg.right_x)] {
                if kind == track::NONE {
                    continue;
                }
                let x = sc.x1 + sc.s1 * off * ROAD_W * XS;
                let outward = if off < 0.0 { -1.0 } else { 0.0 };
                let (px, w, h, pal, mag, flip): (&[u8], usize, usize, &Palette, f32, bool) =
                    match kind {
                        track::PALM => {
                            let a = &art::PALM;
                            (a.px, a.w, a.h, &art::PALM_PAL, PALM_MAG, off > 0.0)
                        }
                        track::BOULDER => {
                            let a = &art::BOULDER;
                            (a.px, a.w, a.h, &art::BOULDER_PAL, BOULDER_MAG, false)
                        }
                        track::SIGN_L | track::SIGN_R => {
                            let a = &art::SIGN;
                            (a.px, a.w, a.h, &art::SIGN_PAL, SIGN_MAG, kind == track::SIGN_R)
                        }
                        _ => {
                            let b = ((kind - track::BOARD) % 4) as usize;
                            (
                                &self.boards[b][..],
                                art::BOARD_W,
                                art::BOARD_H,
                                &art::BOARD_PALS[b],
                                BOARD_MAG,
                                false,
                            )
                        }
                    };
                let dw = w as f32 * mag * sc.s1 * CAM_H;
                let dh = h as f32 * mag * sc.s1 * CAM_H;
                let pal = self.fogged(pal, fog, None);
                blit_scaled(
                    buf,
                    px,
                    w,
                    h,
                    &pal,
                    x + dw * outward,
                    sc.y1 - dh,
                    dw,
                    dh,
                    sc.clip,
                    flip,
                );
            }

            for car in cars {
                if trk.index_at(car.z) != idx {
                    continue;
                }
                let p = (car.z - idx as f32 * SEG_LEN) / SEG_LEN;
                let s = sc.s1 + (sc.s2 - sc.s1) * p;
                let x = sc.x1 + (sc.x2 - sc.x1) * p + s * car.offset * ROAD_W * XS;
                let y = sc.y1 + (sc.y2 - sc.y1) * p;
                let a = &art::CAR;
                let dw = a.w as f32 * CAR_MAG * s * CAM_H;
                let dh = a.h as f32 * CAR_MAG * s * CAM_H;
                // Tail lights cut through the fog at night.
                let keep = if night > 0.3 { Some(art::TAIL) } else { None };
                let pal = self.fogged(&art::CAR_PALS[car.paint as usize % 5], fog, keep);
                blit_scaled(
                    buf,
                    a.px,
                    a.w,
                    a.h,
                    &pal,
                    x - dw / 2.0,
                    y - dh,
                    dw,
                    dh,
                    sc.clip,
                    false,
                );
            }
        }
    }

    /// Two posts and a banner across the road.
    fn gantry(&self, buf: &mut [u16; PIXELS], sc: &SegScreen, banner: usize, fog: f32) {
        let k = sc.s1 * XS; // screen pixels per world unit
        let post_w = 90.0 * k;
        let post_h = 1500.0 * k;
        let span = 1.3 * ROAD_W * sc.s1 * XS;
        let pal = self.fogged(&art::BANNER_PAL, fog, None);
        let post = pal[4];
        for side in [-1.0, 1.0] {
            let cx = sc.x1 + side * span;
            fill_clipped(buf, cx - post_w / 2.0, sc.y1 - post_h, post_w, post_h, sc.clip, post);
        }
        let bw = 2.0 * span;
        let bh = bw * art::BANNER_H as f32 / art::BANNER_W as f32;
        blit_scaled(
            buf,
            &self.banners[banner],
            art::BANNER_W,
            art::BANNER_H,
            &pal,
            sc.x1 - span,
            sc.y1 - post_h,
            bw,
            bh,
            sc.clip,
            false,
        );
    }

    /// A palette faded toward the fog colour. `keep` names an index that stays
    /// at full brightness.
    fn fogged(&self, pal: &Palette, f: f32, keep: Option<usize>) -> [u16; 16] {
        let mut out = [0u16; 16];
        for i in 1..16 {
            out[i] = if f < 0.02 || Some(i) == keep {
                pal[i].0
            } else {
                pal[i].mix(self.look.fog, f).0
            };
        }
        out
    }

    fn player(&self, buf: &mut [u16; PIXELS], pose: &PlayerPose, shake: i32) {
        let a = &art::PLAYER;
        let mut pal = [0u16; 16];
        for (i, p) in pal.iter_mut().enumerate() {
            *p = art::PLAYER_PAL[i].0;
        }
        if pose.braking {
            pal[art::TAIL] = art::BRAKE_LIGHT.0;
        }
        let x0 = WIDTH as i32 / 2 - a.w as i32 / 2;
        let y0 = PLAYER_Y - a.h as i32 + pose.bounce + shake;
        for row in 0..a.h {
            let y = y0 + row as i32;
            if !(0..H as i32).contains(&y) {
                continue;
            }
            let lean = if row < art::CABIN_ROWS { pose.lean } else { 0 };
            for col in 0..a.w {
                let i = a.px[row * a.w + col];
                if i == 0 {
                    continue;
                }
                let x = x0 + col as i32 + lean;
                if (0..WIDTH as i32).contains(&x) {
                    buf[x as usize * H + y as usize] = pal[i as usize];
                }
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Road half-width in pixels at projection scale `s`.
#[inline]
fn s1_w(s: f32) -> f32 {
    s * ROAD_W * XS
}

fn hash32(n: u32) -> u32 {
    let mut x = n.wrapping_mul(0x9e37_79b9);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^ (x >> 13)
}

/// Map a virtual x on a wrapping strip `period` wide to a screen column.
fn wrap_px(x: f32, period: f32) -> usize {
    let w = x - math::floor(x / period) * period;
    w as usize
}

#[inline]
fn hspan(buf: &mut [u16; PIXELS], y: usize, x0: f32, x1: f32, c: u16) {
    let a = (x0 + 0.5).clamp(0.0, WIDTH as f32) as usize;
    let b = (x1 + 0.5).clamp(0.0, WIDTH as f32) as usize;
    let mut i = a * H + y;
    for _ in a..b {
        buf[i] = c;
        i += H;
    }
}

fn road_row(
    buf: &mut [u16; PIXELS],
    y: usize,
    cx: f32,
    w: f32,
    c: &[u16; 7],
    light: bool,
    chequer: Option<u8>,
) {
    let (grass, rumble, road) = if light {
        (c[0], c[2], c[4])
    } else {
        (c[1], c[3], c[5])
    };
    let rw = w / 6.0;
    hspan(buf, y, 0.0, WIDTH as f32, grass);
    hspan(buf, y, cx - w - rw, cx + w + rw, rumble);
    if let Some(phase) = chequer {
        let cell = w / 4.0;
        for k in 0..8u8 {
            let col = if (k + phase) % 2 == 0 { c[6] } else { 0 };
            let x0 = cx - w + k as f32 * cell;
            hspan(buf, y, x0, x0 + cell, col);
        }
        return;
    }
    hspan(buf, y, cx - w, cx + w, road);
    if light {
        let lw = (w / 32.0).max(0.6);
        for k in 1..3 {
            let lx = cx - w + k as f32 * (2.0 * w / 3.0);
            hspan(buf, y, lx - lw / 2.0, lx + lw / 2.0, c[6]);
        }
    }
}

fn fill_clipped(buf: &mut [u16; PIXELS], x: f32, y: f32, w: f32, h: f32, clip: f32, c: u16) {
    let x0 = (x + 0.5).clamp(0.0, WIDTH as f32) as usize;
    let x1 = (x + w + 0.5).clamp(0.0, WIDTH as f32) as usize;
    let y0 = (y + 0.5).clamp(0.0, H as f32) as usize;
    let y1 = (y + h + 0.5).clamp(0.0, clip.min(H as f32)) as usize;
    if y0 >= y1 {
        return;
    }
    for cx in x0..x1 {
        buf[cx * H + y0..cx * H + y1].fill(c);
    }
}

/// Nearest-neighbour scaled blit of indexed art, clipped to rows above `clip`.
#[allow(clippy::too_many_arguments)]
fn blit_scaled(
    buf: &mut [u16; PIXELS],
    px: &[u8],
    aw: usize,
    ah: usize,
    pal: &[u16; 16],
    dx: f32,
    dy: f32,
    dw: f32,
    dh: f32,
    clip: f32,
    flip: bool,
) {
    if dw < 1.0 || dh < 1.0 {
        return;
    }
    let x0 = (dx + 0.5).max(0.0) as i32;
    let x1 = ((dx + dw + 0.5).min(WIDTH as f32)) as i32;
    let y0 = (dy + 0.5).max(0.0) as i32;
    let y1 = ((dy + dh + 0.5).min(clip.min(H as f32))) as i32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // 16.16 fixed-point source steps.
    let ustep = ((aw as f32 / dw) * 65536.0) as u32;
    let vstep = ((ah as f32 / dh) * 65536.0) as u32;
    let v_start = (((y0 as f32 + 0.5 - dy) * ah as f32 / dh) * 65536.0).max(0.0) as u32;
    let mut u = (((x0 as f32 + 0.5 - dx) * aw as f32 / dw) * 65536.0).max(0.0) as u32;
    for x in x0..x1 {
        let mut sx = ((u >> 16) as usize).min(aw - 1);
        if flip {
            sx = aw - 1 - sx;
        }
        let col = x as usize * H;
        let mut v = v_start;
        for y in y0..y1 {
            let sy = ((v >> 16) as usize).min(ah - 1);
            let i = px[sy * aw + sx];
            if i != 0 {
                buf[col + y as usize] = pal[i as usize];
            }
            v += vstep;
        }
        u += ustep;
    }
}
