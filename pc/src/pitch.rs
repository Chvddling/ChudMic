//! Real-time voice pitch shifter (no external libraries).
//!
//! Method: WSOLA time-stretch by the pitch ratio, then resample back to the
//! original length. WSOLA picks each grain where it best lines up with the
//! previous one (cross-correlation), which keeps voices clean instead of
//! "phasey" or "warbly". Added delay is about 60 ms.

use std::collections::VecDeque;

const N: usize = 1920; // grain length: 40 ms @ 48 kHz
const HS: usize = N / 2; // output hop (50 % overlap)
const OVERLAP: usize = N - HS; // region compared when searching
const SEEK: usize = 384; // search ±8 ms for the best grain position

pub struct PitchShifter {
    window: Vec<f32>,
    input: Vec<f32>,
    in_base: i64,      // absolute index of input[0]
    ana_pos: f64,      // nominal absolute start of the next grain
    prev: Option<i64>, // absolute start of the previous grain
    ola: Vec<f32>,     // overlap-add accumulator, length N
    stretched: VecDeque<f32>,
    rs_pos: f64, // fractional read position in `stretched`
}

impl Default for PitchShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl PitchShifter {
    pub fn new() -> Self {
        // Periodic Hann: two windows at 50 % overlap sum to exactly 1.
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos())
            .collect();
        PitchShifter {
            window,
            input: Vec::with_capacity(N * 4),
            in_base: 0,
            ana_pos: SEEK as f64,
            prev: None,
            ola: vec![0.0; N],
            stretched: VecDeque::with_capacity(N * 4),
            rs_pos: 1.0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Shift `input` by `semitones` and append the result to `out`.
    /// Output keeps the same sample rate and duration (after the initial delay).
    pub fn process(&mut self, input: &[f32], semitones: f32, out: &mut Vec<f32>) {
        let ratio = 2f64.powf(semitones as f64 / 12.0);
        self.input.extend_from_slice(input);

        // ---- 1. WSOLA: stretch time by `ratio`, pitch unchanged ----
        let ha = HS as f64 / ratio; // input hop
        loop {
            let nominal = self.ana_pos.round() as i64;
            let need = nominal + SEEK as i64 + N as i64;
            if need > self.in_base + self.input.len() as i64 {
                break;
            }
            let start = match self.prev {
                None => nominal,
                Some(p) => self.best_offset(p + HS as i64, nominal),
            };
            let s = (start - self.in_base) as usize;
            for i in 0..N {
                self.ola[i] += self.input[s + i] * self.window[i];
            }
            self.stretched.extend(self.ola[..HS].iter().copied());
            self.ola.copy_within(HS.., 0);
            self.ola[HS..].fill(0.0);

            self.prev = Some(start);
            self.ana_pos += ha;

            // Drop input we can never look at again.
            let keep_from = (start + HS as i64).min(self.ana_pos.floor() as i64 - SEEK as i64);
            let drop = (keep_from - self.in_base).max(0) as usize;
            if drop > 0 {
                self.input.drain(..drop.min(self.input.len()));
                self.in_base += drop as i64;
            }
        }

        // ---- 2. Resample (read `ratio` times faster) to restore duration ----
        // Cubic (Catmull-Rom) interpolation; needs samples i-1 .. i+2.
        while self.rs_pos + 2.0 < self.stretched.len() as f64 {
            let i = self.rs_pos.floor() as usize;
            let t = (self.rs_pos - i as f64) as f32;
            let (y0, y1, y2, y3) = (
                self.stretched[i - 1],
                self.stretched[i],
                self.stretched[i + 1],
                self.stretched[i + 2],
            );
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            out.push(((a * t + b) * t + c) * t + y1);
            self.rs_pos += ratio;
        }
        // Keep one sample of history before the read position.
        let consumed = (self.rs_pos.floor() as usize).saturating_sub(1);
        self.stretched.drain(..consumed.min(self.stretched.len()));
        self.rs_pos -= consumed as f64;
    }

    /// Find the grain start within ±SEEK of `nominal` whose first OVERLAP samples
    /// best match the natural continuation of the previous grain at `target`.
    fn best_offset(&self, target: i64, nominal: i64) -> i64 {
        let base = self.in_base;
        let tgt = (target - base) as usize;
        let reference = &self.input[tgt..tgt + OVERLAP];
        let score = |cand: i64, stride: usize| -> f32 {
            let c = &self.input[(cand - base) as usize..][..OVERLAP];
            let (mut dot, mut energy) = (0.0f32, 1e-9f32);
            let mut i = 0;
            while i < OVERLAP {
                dot += reference[i] * c[i];
                energy += c[i] * c[i];
                i += stride;
            }
            dot / energy.sqrt()
        };
        let lo = (nominal - SEEK as i64).max(base);
        let hi = nominal + SEEK as i64;
        // Coarse search, then refine around the winner.
        let mut best = nominal.clamp(lo, hi);
        let mut best_score = f32::MIN;
        let mut cand = lo;
        while cand <= hi {
            let sc = score(cand, 4);
            if sc > best_score {
                best_score = sc;
                best = cand;
            }
            cand += 4;
        }
        let (flo, fhi) = ((best - 4).max(lo), (best + 4).min(hi));
        best_score = f32::MIN;
        for cand in flo..=fhi {
            let sc = score(cand, 1);
            if sc > best_score {
                best_score = sc;
                best = cand;
            }
        }
        best
    }
}
