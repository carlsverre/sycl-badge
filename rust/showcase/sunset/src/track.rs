//! The course: a loop of short road segments, each with a curvature and a
//! height, plus the scenery that stands beside it.

use sycl_cart::math;

/// World length of one segment.
pub const SEG_LEN: f32 = 200.0;
pub const MAX_SEGS: usize = 1400;

/// Scenery kinds. Zero is nothing.
pub const NONE: u8 = 0;
pub const PALM: u8 = 1;
pub const BOULDER: u8 = 2;
pub const SIGN_L: u8 = 3;
pub const SIGN_R: u8 = 4;
/// Billboards are `BOARD + n`.
pub const BOARD: u8 = 8;

#[derive(Copy, Clone)]
pub struct Seg {
    /// World height at the near and far edges.
    pub y1: f32,
    pub y2: f32,
    pub curve: f32,
    /// Scenery on each side, and how far out it stands (road half-widths).
    pub left: u8,
    pub right: u8,
    pub left_x: f32,
    pub right_x: f32,
}

impl Seg {
    const ZERO: Seg = Seg {
        y1: 0.0,
        y2: 0.0,
        curve: 0.0,
        left: 0,
        right: 0,
        left_x: 0.0,
        right_x: 0.0,
    };
}

pub struct Track {
    pub segs: [Seg; MAX_SEGS],
    pub len: usize,
    /// Segment indices of the start line and the halfway checkpoint.
    pub gates: [usize; 2],
}

fn ease_in(a: f32, b: f32, p: f32) -> f32 {
    a + (b - a) * p * p
}

fn ease_in_out(a: f32, b: f32, p: f32) -> f32 {
    a + (b - a) * (0.5 - math::cos(p * core::f32::consts::PI) / 2.0)
}

/// Cheap deterministic hash, so the scenery is the same every run.
fn hash(n: usize) -> u32 {
    let mut x = n as u32 ^ 0x9e37_79b9;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    x = x.wrapping_mul(0xc2b2_ae35);
    x ^ (x >> 16)
}

impl Track {
    pub const INIT: Track = Track {
        segs: [Seg::ZERO; MAX_SEGS],
        len: 0,
        gates: [0; 2],
    };

    pub fn length(&self) -> f32 {
        self.len as f32 * SEG_LEN
    }

    #[inline]
    pub fn seg(&self, i: usize) -> &Seg {
        &self.segs[i % self.len]
    }

    pub fn index_at(&self, z: f32) -> usize {
        (math::floor(z / SEG_LEN) as usize) % self.len
    }

    fn last_y(&self) -> f32 {
        if self.len == 0 {
            0.0
        } else {
            self.segs[self.len - 1].y2
        }
    }

    fn push(&mut self, curve: f32, y: f32) {
        let y1 = self.last_y();
        self.segs[self.len] = Seg {
            y1,
            y2: y,
            curve,
            ..Seg::ZERO
        };
        self.len += 1;
    }

    /// One section: ease into `curve`, hold it, ease out, while the height
    /// rises by `hill` segment lengths over the whole section.
    fn road(&mut self, enter: usize, hold: usize, leave: usize, curve: f32, hill: f32) {
        let start = self.last_y();
        let end = start + hill * SEG_LEN;
        let total = (enter + hold + leave) as f32;
        for n in 0..enter {
            let p = n as f32 / total;
            self.push(ease_in(0.0, curve, n as f32 / enter as f32), ease_in_out(start, end, p));
        }
        for n in 0..hold {
            let p = (enter + n) as f32 / total;
            self.push(curve, ease_in_out(start, end, p));
        }
        for n in 0..leave {
            let p = (enter + hold + n) as f32 / total;
            let c = ease_in_out(curve, 0.0, n as f32 / leave as f32);
            self.push(c, ease_in_out(start, end, p));
        }
    }

    pub fn build(&mut self) {
        self.len = 0;
        // The start straight.
        self.road(20, 30, 20, 0.0, 0.0);
        // A gentle right that climbs.
        self.road(25, 40, 25, 2.0, 20.0);
        // S-bends.
        self.road(20, 20, 20, -4.0, 0.0);
        self.road(20, 20, 20, 4.0, -20.0);
        self.road(20, 20, 20, -2.0, 0.0);
        // The big hill: a blind crest, then a curving descent.
        self.road(40, 40, 40, 0.0, 60.0);
        self.road(30, 30, 30, 3.0, -40.0);
        // Bumps.
        for i in 0..6 {
            let h = if i % 2 == 0 { 6.0 } else { -6.0 };
            self.road(6, 6, 6, 0.0, h);
        }
        // A long hard left sweeper.
        self.road(40, 80, 40, -6.0, 0.0);
        // Rolling right-handers.
        self.road(30, 30, 30, 4.0, 30.0);
        self.road(30, 30, 30, 4.0, -30.0);
        // Chicane.
        self.road(15, 15, 15, 6.0, 0.0);
        self.road(15, 15, 15, -6.0, 0.0);
        // Back down to the start line's height.
        let down = -self.last_y() / SEG_LEN;
        self.road(40, 40, 40, 0.0, down);
        self.road(10, 20, 10, 0.0, 0.0);

        self.gates = [0, self.len / 2];
        self.decorate();
    }

    fn decorate(&mut self) {
        let n = self.len;
        let hill_start = 70 + 90 + 180;
        let hill_end = hill_start + 120 + 90 + 108;
        for i in 0..n {
            let h = hash(i);
            let jitter = (h & 0xff) as f32 / 255.0 * 0.5;
            let curve = self.segs[i].curve;
            let s = &mut self.segs[i];
            let rocky = (hill_start..hill_end).contains(&i);

            // Curve warnings on the outside of every sharp bend.
            if curve.abs() >= 3.5 && i % 12 == 0 {
                if curve > 0.0 {
                    s.left = SIGN_R;
                    s.left_x = -1.15;
                } else {
                    s.right = SIGN_L;
                    s.right_x = 1.15;
                }
                continue;
            }
            if i % 160 == 40 {
                let board = BOARD + ((i / 160) % 4) as u8;
                if (i / 160) % 2 == 0 {
                    s.left = board;
                    s.left_x = -1.6;
                } else {
                    s.right = board;
                    s.right_x = 1.6;
                }
                continue;
            }
            let (kind, every) = if rocky { (BOULDER, 7) } else { (PALM, 6) };
            if i % every == 0 && s.left == NONE {
                s.left = kind;
                s.left_x = -1.3 - jitter;
            }
            if (i + every / 2) % every == 0 && s.right == NONE {
                s.right = kind;
                s.right_x = 1.3 + jitter;
            }
            // A second, deeper row of palms on the straights.
            if !rocky && curve == 0.0 && i % 12 == 3 && s.left == NONE {
                s.left = PALM;
                s.left_x = -2.4 - jitter;
            }
        }
        // Keep the gantries clear.
        for g in self.gates {
            for k in 0..3 {
                let s = &mut self.segs[(g + k) % n];
                s.left = NONE;
                s.right = NONE;
            }
        }
    }
}
