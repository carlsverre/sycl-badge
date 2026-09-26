//! Sunset Run: an OutRun-style pseudo-3D racer.
//!
//! * Left / Right — steer. A (or Up) — accelerate. B (or Down) — brake.
//! * Start — start a race, pause, resume.
//! * Select — cycle the time of day: auto (light sensor), day, dusk, night.
//! * B on the title screen or while paused — sound on or off.
//!
//! Reach each checkpoint before the timer runs out. Traffic and roadside
//! scenery cost you speed; the grass costs you more.

#![no_std]
#![no_main]

mod art;
mod render;
mod sound;
mod track;

use render::{Car, PlayerPose, Renderer, View, PLAYER_Z};
use sound::Sound;
use sycl_cart::prelude::*;
use sycl_cart::{info, math};
use track::{Track, SEG_LEN};

const DT: f32 = 1.0 / 60.0;
/// One segment per frame.
const MAX_SPEED: f32 = SEG_LEN * 60.0;
const ACCEL: f32 = MAX_SPEED / 5.0;
const BRAKING: f32 = -MAX_SPEED;
const DECEL: f32 = -MAX_SPEED / 5.0;
const OFFROAD_DECEL: f32 = -MAX_SPEED / 2.0;
const OFFROAD_LIMIT: f32 = MAX_SPEED / 4.0;
const CENTRIFUGAL: f32 = 0.3;
const TOP_KMH: f32 = 293.0;

const N_CARS: usize = 20;
const START_TIME: f32 = 30.0;
/// Segments between the car's start position and the start line.
const GRID_SEGS: f32 = 5.0;

const TITLE: u8 = 0;
const COUNTDOWN: u8 = 1;
const RACE: u8 = 2;
const PAUSED: u8 = 3;
const GAME_OVER: u8 = 4;

const TOD_NAMES: [&str; 4] = ["AUTO", "DAY", "DUSK", "NIGHT"];

struct Game {
    trk: Track,
    r: Renderer,
    cars: [Car; N_CARS],
    snd: Sound,
    phase: u8,
    /// Frames since the phase began.
    t: u32,

    position: f32,
    speed: f32,
    player_x: f32,
    steer: i32,
    braking: bool,
    offroad: bool,
    sky_off: f32,
    far_off: f32,
    near_off: f32,

    /// Distance driven this race, including crash setbacks. Checkpoints are
    /// measured against it.
    odo: f32,
    next_cp: u32,
    time_left: f32,
    lap: u32,
    lap_frames: u32,
    best_lap: u32,
    score: u32,
    best: u32,

    crash: u16,
    celebrate: u16,
    toast: TextBuf<24>,
    toast_frames: u16,

    tod_mode: u8,
    tod: f32,

    /// Start of the current frame on the badge's microsecond timer.
    #[cfg_attr(not(target_arch = "arm"), allow(dead_code))]
    last_us: u32,
    render_us: u32,
}

impl Game {
    fn reset_race(&mut self, c: &mut Ctx) {
        let len = self.trk.length();
        self.position = len - GRID_SEGS * SEG_LEN - PLAYER_Z;
        self.speed = 0.0;
        self.player_x = -0.35;
        self.odo = 0.0;
        self.next_cp = 0;
        self.time_left = START_TIME;
        self.lap = 0;
        self.lap_frames = 0;
        self.score = 0;
        self.crash = 0;
        self.celebrate = 0;
        self.spawn_cars(c);
    }

    fn spawn_cars(&mut self, c: &mut Ctx) {
        let len = self.trk.length();
        for (i, car) in self.cars.iter_mut().enumerate() {
            // Keep the grid clear; spread the rest around the loop.
            let z = 20.0 * SEG_LEN + (len - 40.0 * SEG_LEN) * (i as f32 + c.rng.unit() * 0.8)
                / N_CARS as f32;
            let lane = c.rng.range(-1, 2) as f32 * 0.66;
            *car = Car {
                z,
                offset: lane + (c.rng.unit() - 0.5) * 0.2,
                speed: MAX_SPEED * (0.25 + c.rng.unit() * 0.4),
                paint: c.rng.below(5) as u8,
            };
        }
    }

    fn set_phase(&mut self, p: u8) {
        self.phase = p;
        self.t = 0;
    }

    fn say(&mut self, s: &str, frames: u16) {
        self.toast.clear();
        self.toast.push_str(s);
        self.toast_frames = frames;
    }

    // ── Physics ─────────────────────────────────────────────────────────────

    fn gear_rpm(&self) -> (u32, f32) {
        let s = self.speed / MAX_SPEED;
        const BANDS: [f32; 6] = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0001];
        for g in 0..5 {
            if s < BANDS[g + 1] {
                let f = (s - BANDS[g]) / (BANDS[g + 1] - BANDS[g]);
                return (g as u32 + 1, 0.3 + 0.7 * f);
            }
        }
        (5, 1.0)
    }

    fn drive(&mut self, c: &mut Ctx, steer: i32, throttle: bool, brake: bool) {
        let len = self.trk.length();
        let pidx = self.trk.index_at(self.position + PLAYER_Z);
        let seg = *self.trk.seg(pidx);
        let pw = render::world_w(art::PLAYER.w, 1.0);
        let pct = self.speed / MAX_SPEED;
        let dx = DT * 2.0 * pct;
        let start = self.position;

        self.update_cars(len);
        self.position = wrap(self.position + DT * self.speed, len);

        self.player_x += steer as f32 * dx;
        self.player_x -= dx * pct * seg.curve * CENTRIFUGAL;
        self.steer = steer;
        self.braking = brake;

        self.speed += DT
            * if throttle {
                ACCEL
            } else if brake {
                BRAKING
            } else {
                DECEL
            };

        self.offroad = self.player_x.abs() > 1.0;
        if self.offroad {
            if self.speed > OFFROAD_LIMIT {
                self.speed += OFFROAD_DECEL * DT;
            }
            for (kind, off) in [(seg.left, seg.left_x), (seg.right, seg.right_x)] {
                if kind == track::NONE {
                    continue;
                }
                let (centre, w) = render::scenery_hitbox(kind, off);
                if overlap(self.player_x, pw, centre, w, 1.0) {
                    self.speed = MAX_SPEED / 6.0;
                    self.position = wrap(pidx as f32 * SEG_LEN - PLAYER_Z, len);
                    self.crash = 40;
                    self.snd.sfx(c, &sound::CRASH, sound::CRASH_FRAMES);
                    break;
                }
            }
        }

        let cw = render::world_w(art::CAR.w, render::CAR_MAG);
        for car in &self.cars {
            if self.trk.index_at(car.z) != pidx || self.speed <= car.speed {
                continue;
            }
            if overlap(self.player_x, pw, car.offset, cw, 0.8) {
                self.speed = car.speed * (car.speed / self.speed);
                self.position = wrap(car.z - PLAYER_Z, len);
                self.crash = 20;
                self.snd.sfx(c, &sound::BUMP, sound::BUMP_FRAMES);
                break;
            }
        }

        self.player_x = self.player_x.clamp(-2.5, 2.5);
        self.speed = self.speed.clamp(0.0, MAX_SPEED);

        // Distance moved, allowing for the wrap and for crash setbacks.
        let mut moved = self.position - start;
        if moved < -len / 2.0 {
            moved += len;
        } else if moved > len / 2.0 {
            moved -= len;
        }
        self.odo += moved;
        let segs = moved / SEG_LEN;
        self.sky_off += 0.25 * seg.curve * segs;
        self.far_off += 0.5 * seg.curve * segs;
        self.near_off += 1.0 * seg.curve * segs;
    }

    fn update_cars(&mut self, len: f32) {
        let player_z = self.position + PLAYER_Z;
        let player_x = self.player_x;
        for i in 0..N_CARS {
            let mut car = self.cars[i];
            // Drift out of the way of a player closing from behind, as a
            // courteous driver would.
            let gap = wrap(car.z - player_z, len);
            if gap < 12.0 * SEG_LEN && self.speed > car.speed && (car.offset - player_x).abs() < 0.5
            {
                let dir = if car.offset > player_x { 1.0 } else { -1.0 };
                car.offset = (car.offset + dir * DT * 0.6).clamp(-0.85, 0.85);
            }
            car.z = wrap(car.z + DT * car.speed, len);
            self.cars[i] = car;
        }
    }

    /// Where the attract-mode driver steers and whether it lifts.
    fn autopilot(&self) -> (i32, bool, bool) {
        let len = self.trk.length();
        let pz = self.position + PLAYER_Z;
        let mut blocked = false;
        let lanes = [-0.66f32, 0.0, 0.66];
        let mut best = (f32::MAX, 0.0f32);
        for lane in lanes {
            let mut nearest = f32::MAX;
            for car in &self.cars {
                let gap = wrap(car.z - pz, len);
                if gap < 30.0 * SEG_LEN && (car.offset - lane).abs() < 0.45 {
                    nearest = nearest.min(gap);
                }
            }
            // Prefer the clearest lane, then the one nearest where we are.
            let score = -nearest + (lane - self.player_x).abs() * 800.0;
            if score < best.0 {
                best = (score, lane);
            }
        }
        let target = best.1;
        for car in &self.cars {
            let gap = wrap(car.z - pz, len);
            if gap < 6.0 * SEG_LEN && (car.offset - self.player_x).abs() < 0.4 {
                blocked = true;
            }
        }
        // Lean into bends against the centrifugal drift.
        let curve = self.trk.seg(self.trk.index_at(pz) + 4).curve;
        let want = target + curve * 0.02;
        let steer = if self.player_x < want - 0.06 {
            1
        } else if self.player_x > want + 0.06 {
            -1
        } else {
            0
        };
        (steer, !blocked, blocked && self.speed > MAX_SPEED * 0.5)
    }

    fn checkpoints(&mut self, c: &mut Ctx) {
        let len = self.trk.length();
        loop {
            let k = self.next_cp;
            let gate = self.trk.gates[(k % 2) as usize] as f32 * SEG_LEN;
            let at = GRID_SEGS * SEG_LEN + (k / 2) as f32 * len + gate;
            if self.odo < at {
                break;
            }
            self.next_cp += 1;
            if k == 0 {
                continue; // crossing the start line begins the race
            }
            let bonus = (17.0 - k as f32).max(8.0);
            self.time_left += bonus;
            self.celebrate = 60;
            let mut msg = TextBuf::<24>::new();
            if k.is_multiple_of(2) {
                self.lap += 1;
                if self.best_lap == 0 || self.lap_frames < self.best_lap {
                    self.best_lap = self.lap_frames;
                }
                msg.push_str("LAP ");
                push_time(&mut msg, self.lap_frames);
                self.lap_frames = 0;
                self.snd.sfx(c, &sound::LAP, sound::LAP_FRAMES);
            } else {
                msg.push_str("EXTEND +");
                msg.push_u32(bonus as u32);
                self.snd.sfx(c, &sound::CHECKPOINT, sound::CHECKPOINT_FRAMES);
            }
            self.toast = msg;
            self.toast_frames = 100;
        }
    }

    // ── Presentation ────────────────────────────────────────────────────────

    fn time_of_day(&mut self, c: &Ctx) {
        let target = match self.tod_mode {
            1 => 0.0,
            2 => 1.0,
            3 => 2.0,
            _ => {
                auto_tod(c.light_level())
            }
        };
        self.tod = math::approach(self.tod, target, 0.02);
    }

    fn leds(&mut self, c: &mut Ctx) {
        let f = c.frame();
        let l = &mut Dim(&mut c.leds);
        l.set_red(self.braking && self.phase == RACE);
        if self.crash > 0 {
            let on = (f / 4).is_multiple_of(2);
            l.set_all(if on { 90 } else { 0 }, 0, 0);
            return;
        }
        if self.celebrate > 0 {
            for i in 0..5 {
                let on = (f / 3 + i as u32).is_multiple_of(5);
                l.set(i, if on { 60 } else { 0 }, if on { 80 } else { 10 }, 90);
            }
            return;
        }
        match self.phase {
            TITLE => {
                for i in 0..5 {
                    let (r, g, b) = hue((f * 3 + i as u32 * 51) % 360);
                    l.set(i, r, g, b);
                }
            }
            COUNTDOWN => {
                let step = self.t / 50;
                for i in 0..5usize {
                    let ring = i.min(4 - i) as u32; // 0 outside, 2 centre
                    if step >= 3 {
                        l.set(i, 0, 90, 0);
                    } else if ring <= step {
                        l.set(i, 90, 0, 0);
                    } else {
                        l.set(i, 0, 0, 0);
                    }
                }
            }
            RACE => {
                let (_, rpm) = self.gear_rpm();
                if rpm > 0.96 && self.speed > MAX_SPEED * 0.9 && (f / 4).is_multiple_of(2) {
                    l.set_all(100, 0, 0);
                    return;
                }
                let lit = (rpm * 5.0 + 0.5) as usize;
                const TACH: [(u8, u8, u8); 5] =
                    [(0, 70, 0), (0, 70, 0), (60, 60, 0), (90, 35, 0), (100, 0, 0)];
                for (i, (r, g, b)) in TACH.iter().enumerate() {
                    if i < lit {
                        l.set(i, *r, *g, *b);
                    } else {
                        l.set(i, 0, 0, 0);
                    }
                }
            }
            PAUSED => l.set_all(0, 0, 20),
            _ => {
                let p = (math::sin(f as f32 * 0.05) + 1.0) * 0.5;
                l.set_all((10.0 + p * 50.0) as u8, 0, 0);
            }
        }
    }

    fn hud(&self, c: &mut Ctx) {
        let f = c.frame();
        let g = &mut c.gfx;
        let yellow = Color::hex(0xffe040);
        let white = Color::WHITE;
        match self.phase {
            TITLE => {
                let pulse = Color::hex(0xff5a90).mix(Color::hex(0xffb040), (f % 60) as f32 / 60.0);
                shadow(g, b"SUNSET", 8, 14, 3, pulse);
                shadow(g, b"RUN", 44, 40, 3, Color::hex(0xffe060));
                if (f / 30).is_multiple_of(2) {
                    centre(g, b"PRESS START", 72, 1, white);
                }
                if self.best > 0 {
                    let s = uformat!(20, "BEST {}", self.best);
                    centre(g, s.as_bytes(), 84, 1, yellow);
                }
                let mode = uformat!(24, "SELECT: {}", TOD_NAMES[self.tod_mode as usize]);
                centre(g, mode.as_bytes(), 2, 1, Color::hex(0xc0c0ff));
                return;
            }
            COUNTDOWN => {
                let step = self.t / 50;
                let label: &[u8] = match step {
                    0 => b"3",
                    1 => b"2",
                    2 => b"1",
                    _ => b"GO!",
                };
                centre(g, label, 56, 4, if step >= 3 { Color::GREEN } else { yellow });
            }
            _ => {}
        }

        // Timer, big and top-left; red and flashing near the end.
        shadow(g, b"TIME", 3, 2, 1, yellow);
        let secs = math::ceil(self.time_left.max(0.0)) as u32;
        let tcol = if secs <= 5 && (f / 8).is_multiple_of(2) {
            Color::RED
        } else {
            white
        };
        let tb = uformat!(6, "{}", secs);
        shadow(g, tb.as_bytes(), 3, 11, 2, tcol);

        let sc = uformat!(12, "{}", self.score);
        let sw = sc.len() as i32 * 8;
        shadow(g, b"SCORE", WIDTH as i32 - 43, 2, 1, yellow);
        shadow(g, sc.as_bytes(), WIDTH as i32 - 3 - sw, 11, 1, white);

        let mut lt = TextBuf::<16>::new();
        lt.push_str("LAP ");
        lt.push_u32(self.lap + 1);
        centre(g, lt.as_bytes(), 2, 1, yellow);
        let mut lt = TextBuf::<16>::new();
        push_time(&mut lt, self.lap_frames);
        centre(g, lt.as_bytes(), 11, 1, white);

        let kmh = (self.speed / MAX_SPEED * TOP_KMH) as u32;
        let sp = uformat!(6, "{}", kmh);
        shadow(g, sp.as_bytes(), 2, 109, 2, white);
        shadow(g, b"KM/H", 2, 100, 1, yellow);
        let (gear, _) = self.gear_rpm();
        let gs = uformat!(4, "{}", gear);
        shadow(g, b"GEAR", WIDTH as i32 - 34, 100, 1, yellow);
        shadow(g, gs.as_bytes(), WIDTH as i32 - 18, 109, 2, white);

        if self.toast_frames > 0 && (self.toast_frames > 30 || (f / 4).is_multiple_of(2)) {
            centre(g, self.toast.as_bytes(), 34, 1, Color::hex(0x80ffff));
        }
        match self.phase {
            PAUSED => {
                centre(g, b"PAUSED", 44, 2, white);
                let s: &[u8] = if self.snd.on { b"B: SOUND ON" } else { b"B: SOUND OFF" };
                centre(g, s, 64, 1, yellow);
            }
            GAME_OVER => {
                centre(g, b"GAME OVER", 40, 2, Color::hex(0xff5040));
                let s = uformat!(20, "SCORE {}", self.score);
                centre(g, s.as_bytes(), 60, 1, white);
                if self.best_lap > 0 {
                    let mut b = TextBuf::<20>::new();
                    b.push_str("BEST LAP ");
                    push_time(&mut b, self.best_lap);
                    centre(g, b.as_bytes(), 70, 1, yellow);
                }
                if self.t > 120 && (f / 30).is_multiple_of(2) {
                    centre(g, b"PRESS START", 84, 1, white);
                }
            }
            _ => {}
        }
    }

    /// Hold the badge to 60 Hz. The simulator paces itself.
    fn pace(&mut self) {
        #[cfg(target_arch = "arm")]
        {
            const FRAME_US: u32 = 16_667;
            loop {
                let now = sycl_cart::platform::micros_since_boot() as u32;
                let elapsed = now.wrapping_sub(self.last_us);
                if elapsed >= FRAME_US {
                    self.last_us = if elapsed > 3 * FRAME_US {
                        now
                    } else {
                        self.last_us.wrapping_add(FRAME_US)
                    };
                    break;
                }
            }
        }
    }
}

fn now_us() -> u32 {
    #[cfg(target_arch = "arm")]
    {
        sycl_cart::platform::micros_since_boot() as u32
    }
    #[cfg(not(target_arch = "arm"))]
    {
        0
    }
}

impl Cart for Game {
    const INIT: Self = Game {
        trk: Track::INIT,
        r: Renderer::INIT,
        cars: [Car::ZERO; N_CARS],
        snd: Sound::INIT,
        phase: TITLE,
        t: 0,
        position: 0.0,
        speed: 0.0,
        player_x: 0.0,
        steer: 0,
        braking: false,
        offroad: false,
        sky_off: 0.0,
        far_off: 0.0,
        near_off: 0.0,
        odo: 0.0,
        next_cp: 0,
        time_left: 0.0,
        lap: 0,
        lap_frames: 0,
        best_lap: 0,
        score: 0,
        best: 0,
        crash: 0,
        celebrate: 0,
        toast: TextBuf::new(),
        toast_frames: 0,
        tod_mode: 0,
        tod: 0.0,
        last_us: 0,
        render_us: 0,
    };

    fn start(&mut self, c: &mut Ctx) {
        self.trk.build();
        self.r.init();
        self.snd.init();
        self.best = c.save.read_u32(0).unwrap_or(0);
        self.reset_race(c);
        self.position = 0.0;
        self.speed = MAX_SPEED * 0.6;
        self.time_of_day(c);
        self.tod = match self.tod_mode {
            0 => auto_tod(c.light_level()),
            m => (m - 1) as f32,
        };
        self.snd.music(c);
        info!(
            "sunset run: {} segments, light {}",
            self.trk.len as u32,
            c.light_level() as u32
        );
    }

    fn update(&mut self, c: &mut Ctx) {
        self.pace();
        let t0 = now_us();
        self.t += 1;
        let inp = c.input;

        if inp.just_pressed(Button::Select) {
            self.tod_mode = (self.tod_mode + 1) % 4;
            let mut m = TextBuf::<24>::new();
            m.push_str("TIME OF DAY: ");
            m.push_str(TOD_NAMES[self.tod_mode as usize]);
            self.toast = m;
            self.toast_frames = 90;
        }
        self.time_of_day(c);

        let steer_in = inp.dx();
        let throttle_in = inp.held(Button::A) || inp.held(Button::Up);
        let brake_in = inp.held(Button::B) || inp.held(Button::Down);

        match self.phase {
            TITLE => {
                let (s, th, br) = self.autopilot();
                self.drive(c, s, th, br);
                if inp.just_pressed(Button::B) {
                    let on = self.snd.toggle(c);
                    self.say(if on { "SOUND ON" } else { "SOUND OFF" }, 60);
                    if on {
                        self.snd.music(c);
                    }
                }
                if inp.just_pressed(Button::Start) || inp.just_pressed(Button::A) {
                    self.snd.silence(c);
                    self.reset_race(c);
                    self.set_phase(COUNTDOWN);
                }
            }
            COUNTDOWN => {
                // Rev the engine on the grid.
                let target = if throttle_in { MAX_SPEED * 0.19 } else { 0.0 };
                let (_, _) = (steer_in, brake_in);
                let rev = math::approach(self.speed, target, MAX_SPEED * 0.01);
                self.speed = 0.0;
                if self.t % 50 == 1 {
                    let freq = if self.t / 50 >= 3 { 1760.0 } else { 880.0 };
                    self.snd.beep(c, freq, if self.t / 50 >= 3 { 24 } else { 10 });
                }
                self.snd.engine(c, 0.3 + rev / MAX_SPEED * 3.5, false);
                self.speed = 0.0;
                if self.t >= 150 {
                    self.set_phase(RACE);
                }
            }
            RACE => {
                if inp.just_pressed(Button::Start) {
                    self.snd.silence(c);
                    self.set_phase(PAUSED);
                } else {
                    self.drive(c, steer_in, throttle_in, brake_in);
                    self.checkpoints(c);
                    if self.next_cp > 0 {
                        self.lap_frames += 1;
                    }
                    self.time_left -= DT;
                    self.score = (self.odo.max(0.0) / 10.0) as u32;
                    let (_, rpm) = self.gear_rpm();
                    self.snd.engine(c, rpm, self.offroad && self.speed > 100.0);
                    if self.time_left <= 0.0 {
                        self.time_left = 0.0;
                        self.snd.silence(c);
                        self.snd.sfx(c, &sound::GAME_OVER, 60);
                        if self.score > self.best {
                            self.best = self.score;
                            let mut page = [0u8; sycl_cart::save::PAGE_SIZE];
                            page[..4].copy_from_slice(&self.best.to_le_bytes());
                            c.save.write_page(0, &page);
                        }
                        self.set_phase(GAME_OVER);
                    }
                }
            }
            PAUSED => {
                if inp.just_pressed(Button::B) {
                    self.snd.toggle(c);
                }
                if inp.just_pressed(Button::Start) {
                    self.set_phase(RACE);
                }
            }
            _ => {
                // Game over: coast to a stop.
                self.drive(c, 0, false, false);
                if self.t > 120 && (inp.just_pressed(Button::Start) || inp.just_pressed(Button::A))
                {
                    self.reset_race(c);
                    self.position = 0.0;
                    self.speed = MAX_SPEED * 0.6;
                    self.snd.music(c);
                    self.set_phase(TITLE);
                }
            }
        }

        self.crash = self.crash.saturating_sub(1);
        self.celebrate = self.celebrate.saturating_sub(1);
        self.toast_frames = self.toast_frames.saturating_sub(1);
        self.leds(c);

        // Render.
        let pidx = self.trk.index_at(self.position + PLAYER_Z);
        let pseg = self.trk.seg(pidx);
        let pct = (self.position + PLAYER_Z - pidx as f32 * SEG_LEN) / SEG_LEN;
        let pct = pct.clamp(0.0, 1.0);
        let player_y = pseg.y1 + (pseg.y2 - pseg.y1) * pct;
        let moving = self.speed > 10.0 && self.phase != PAUSED;
        let jolt = self.offroad && moving || self.crash > 0;
        let shake = if jolt { (c.rng.below(3) as f32) - 1.0 } else { 0.0 };
        let bounce = if moving && c.rng.below(100) < (self.speed / MAX_SPEED * 25.0) as u32 {
            -1
        } else {
            0
        };
        let view = View {
            position: self.position,
            player_x: self.player_x,
            player_y,
            sky_off: self.sky_off,
            far_off: self.far_off,
            near_off: self.near_off,
            shake,
            time_of_day: self.tod,
            frame: c.frame(),
        };
        let pose = PlayerPose {
            lean: if self.speed > 50.0 { self.steer * 2 } else { 0 },
            bounce,
            braking: self.braking && self.phase == RACE,
        };
        self.r
            .draw(c.gfx.pixels_mut(), &self.trk, &view, &self.cars, &pose);
        c.gfx.mark_dirty(0, 0, WIDTH as u32, HEIGHT as u32);
        self.hud(c);

        self.render_us += now_us().wrapping_sub(t0);
        if c.frame().is_multiple_of(600) {
            info!(
                "frame {}: {} us/frame, light {}",
                c.frame(),
                self.render_us / 600,
                c.light_level() as u32
            );
            self.render_us = 0;
        }
    }
}

// ── Small helpers ───────────────────────────────────────────────────────────

/// Bright room: day. Dim: dusk. Dark: night. A reading of exactly zero means
/// the kernel does not report the sensor, so show dusk.
fn auto_tod(level: u16) -> f32 {
    if level == 0 {
        return 1.0;
    }
    2.0 - 2.0 * ((level as f32 - 150.0) / 1500.0).clamp(0.0, 1.0)
}

fn wrap(z: f32, len: f32) -> f32 {
    let mut z = z;
    while z >= len {
        z -= len;
    }
    while z < 0.0 {
        z += len;
    }
    z
}

fn overlap(x1: f32, w1: f32, x2: f32, w2: f32, pct: f32) -> bool {
    let h = pct / 2.0;
    !(x1 + w1 * h < x2 - w2 * h || x1 - w1 * h > x2 + w2 * h)
}

/// `m'ss.cc` from a frame count.
fn push_time<const N: usize>(b: &mut TextBuf<N>, frames: u32) {
    let cs = frames * 100 / 60;
    b.push_u32(cs / 6000);
    b.push_byte(b'\'');
    let s = cs / 100 % 60;
    if s < 10 {
        b.push_byte(b'0');
    }
    b.push_u32(s);
    b.push_byte(b'.');
    let c = cs % 100;
    if c < 10 {
        b.push_byte(b'0');
    }
    b.push_u32(c);
}

fn shadow(g: &mut sycl_cart::Gfx, s: &[u8], x: i32, y: i32, scale: u32, col: Color) {
    g.text_with(s, x + 1, y + 1, scale, Color::BLACK, None);
    g.text_with(s, x, y, scale, col, None);
}

fn centre(g: &mut sycl_cart::Gfx, s: &[u8], y: i32, scale: u32, col: Color) {
    let w = g.text_width(s, scale) as i32;
    shadow(g, s, (WIDTH as i32 - w) / 2, y, scale, col);
}

/// Neopixel brightness out of 256. The LEDs are very bright at full scale.
const LED_LEVEL: u16 = 2;

/// Scales every neopixel write by [`LED_LEVEL`].
struct Dim<'a>(&'a mut sycl_cart::Leds);

impl Dim<'_> {
    fn set(&mut self, i: usize, r: u8, g: u8, b: u8) {
        let s = |v: u8| ((v as u16 * LED_LEVEL + 255) >> 8) as u8;
        self.0.set(i, s(r), s(g), s(b));
    }
    fn set_all(&mut self, r: u8, g: u8, b: u8) {
        for i in 0..5 {
            self.set(i, r, g, b);
        }
    }
    fn set_red(&mut self, on: bool) {
        self.0.set_red(on);
    }
}

fn hue(h: u32) -> (u8, u8, u8) {
    let x = ((h % 60) * 255 / 60) as u8;
    match h / 60 {
        0 => (255, x, 0),
        1 => (255 - x, 255, 0),
        2 => (0, 255, x),
        3 => (0, 255 - x, 255),
        4 => (x, 0, 255),
        _ => (255, 0, 255 - x),
    }
}

sycl_cart::cart!(Game);
