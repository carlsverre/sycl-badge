//! The engine drone and the sound effects, sharing one monophonic voice.
//!
//! The engine is a sustained one-shot tone, which outranks the sequencer. So an
//! effect first stops the engine, then plays, and the engine comes back when
//! the effect's frame count runs out. The engine pitch is quantized and changed
//! at most every few frames, because every change is a FIFO message to Core 0.

use sycl_cart::audio::notes::*;
use sycl_cart::{Ctx, Shape, Step, ToneLen, Track};

const fn sq(freq: f32, frames: u16) -> Step {
    Step::shaped(freq, frames, 0.8, Shape::Square)
}

pub static CRASH: Track = Track::once(&[
    sq(320.0, 3),
    sq(230.0, 3),
    sq(170.0, 3),
    sq(125.0, 4),
    sq(90.0, 8),
]);
pub const CRASH_FRAMES: u16 = 21;

pub static BUMP: Track = Track::once(&[sq(200.0, 2), sq(150.0, 3)]);
pub const BUMP_FRAMES: u16 = 5;

pub static CHECKPOINT: Track = Track::once(&[sq(C6, 4), sq(E6, 4), sq(G6, 4), sq(C7, 12)]);
pub const CHECKPOINT_FRAMES: u16 = 24;

pub static LAP: Track = Track::once(&[
    sq(G5, 4),
    sq(C6, 4),
    sq(E6, 4),
    sq(G6, 4),
    sq(E6, 4),
    sq(G6, 4),
    sq(C7, 16),
]);
pub const LAP_FRAMES: u16 = 40;

pub static GAME_OVER: Track = Track::once(&[
    sq(G5, 10),
    sq(E5, 10),
    sq(C5, 10),
    sq(G4, 24),
]);

const fn tri(freq: f32, frames: u16) -> Step {
    Step::shaped(freq, frames, 0.35, Shape::Triangle)
}

/// Title theme: a synthwave arpeggio over Am, F, C, G.
pub static THEME: Track = Track::looping(&[
    tri(A4, 7), tri(C5, 7), tri(E5, 7), tri(A5, 7), tri(E5, 7), tri(C5, 7), tri(E5, 7), tri(C5, 7),
    tri(F4, 7), tri(A4, 7), tri(C5, 7), tri(F5, 7), tri(C5, 7), tri(A4, 7), tri(C5, 7), tri(A4, 7),
    tri(C5, 7), tri(E5, 7), tri(G5, 7), tri(C6, 7), tri(G5, 7), tri(E5, 7), tri(G5, 7), tri(E5, 7),
    tri(G4, 7), tri(B4, 7), tri(D5, 7), tri(G5, 7), tri(D5, 7), tri(B4, 7), tri(D5, 7), tri(B4, 7),
]);

pub struct Sound {
    pub on: bool,
    /// Frames until the engine may sound again.
    sfx_left: u16,
    /// The engine note last sent, quantized; -1 when nothing is sounding.
    last_q: i32,
    cool: u8,
}

impl Sound {
    pub const INIT: Sound = Sound {
        on: false,
        sfx_left: 0,
        last_q: 0,
        cool: 0,
    };

    pub fn init(&mut self) {
        self.on = true;
        self.last_q = -1;
    }

    pub fn sfx(&mut self, c: &mut Ctx, track: &'static Track, frames: u16) {
        if !self.on {
            return;
        }
        c.audio.stop_tone();
        c.audio.play_sfx(track);
        self.sfx_left = frames;
        self.last_q = -1;
    }

    pub fn beep(&mut self, c: &mut Ctx, freq: f32, frames: u16) {
        if !self.on {
            return;
        }
        c.audio.tone(freq, ToneLen::Frames(frames), 0.8);
        self.sfx_left = frames;
        self.last_q = -1;
    }

    /// Drive the engine note from the tachometer. `rough` adds an off-road
    /// judder.
    pub fn engine(&mut self, c: &mut Ctx, rpm: f32, rough: bool) {
        if !self.on {
            return;
        }
        if self.sfx_left > 0 {
            self.sfx_left -= 1;
            return;
        }
        if self.cool > 0 {
            self.cool -= 1;
            return;
        }
        let mut f = 55.0 + rpm * 190.0;
        if rough {
            f *= if c.frame() % 6 < 3 { 0.75 } else { 1.1 };
        }
        let q = (f / 4.0) as i32;
        if q != self.last_q {
            c.audio
                .tone_with(q as f32 * 4.0, ToneLen::Sustained, 0.4, Shape::Sawtooth);
            self.last_q = q;
            self.cool = 2;
        }
    }

    pub fn music(&mut self, c: &mut Ctx) {
        self.silence(c);
        if self.on {
            c.audio.play_music(&THEME);
        }
    }

    pub fn silence(&mut self, c: &mut Ctx) {
        c.audio.stop_all();
        self.last_q = -1;
        self.sfx_left = 0;
    }

    pub fn toggle(&mut self, c: &mut Ctx) -> bool {
        self.silence(c);
        self.on = !self.on;
        self.on
    }
}
