//! Small self-contained gradient-noise implementation.
//!
//! Deliberately dependency-free so map generation is fully deterministic for a
//! given seed regardless of which crate versions happen to be in the lockfile.

/// A tiny PCG-flavoured random number generator, used only to shuffle the
/// permutation table.
struct Rng(u64);

impl Rng {
    fn new(seed: u32) -> Self {
        // Splitmix the seed so that adjacent seeds produce unrelated maps.
        let mut s = (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xDA3E_39CB_94B9_5BDB;
        s ^= s >> 31;
        Self(s | 1)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
}

/// 2D Perlin gradient noise.
pub struct Noise {
    perm: [u8; 512],
}

impl Noise {
    pub fn new(seed: u32) -> Self {
        let mut table: [u8; 256] = core::array::from_fn(|i| i as u8);
        let mut rng = Rng::new(seed);
        for i in (1..256).rev() {
            let j = (rng.next_u32() % (i as u32 + 1)) as usize;
            table.swap(i, j);
        }

        let mut perm = [0u8; 512];
        for (i, slot) in perm.iter_mut().enumerate() {
            *slot = table[i & 255];
        }
        Self { perm }
    }

    /// Raw noise in roughly `-1.0..1.0`.
    pub fn get(&self, x: f32, y: f32) -> f32 {
        let xi = x.floor();
        let yi = y.floor();
        let xf = x - xi;
        let yf = y - yi;

        let xi = xi as i32 as usize & 255;
        let yi = yi as i32 as usize & 255;

        let u = fade(xf);
        let v = fade(yf);

        let aa = self.perm[self.perm[xi] as usize + yi];
        let ab = self.perm[self.perm[xi] as usize + yi + 1];
        let ba = self.perm[self.perm[xi + 1] as usize + yi];
        let bb = self.perm[self.perm[xi + 1] as usize + yi + 1];

        let x1 = lerp(grad(aa, xf, yf), grad(ba, xf - 1.0, yf), u);
        let x2 = lerp(grad(ab, xf, yf - 1.0), grad(bb, xf - 1.0, yf - 1.0), u);
        lerp(x1, x2, v)
    }

    /// Fractal brownian motion: sum of `octaves` noise layers, each at double
    /// the frequency and half the amplitude of the last. Result is normalised
    /// back to roughly `-1.0..1.0`.
    pub fn fbm(&self, x: f32, y: f32, octaves: u32) -> f32 {
        let mut sum = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut total = 0.0;

        for _ in 0..octaves {
            sum += self.get(x * frequency, y * frequency) * amplitude;
            total += amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }

        sum / total
    }

    /// Ridged multifractal — inverted absolute noise, which produces sharp
    /// crests instead of rolling blobs. Returns `0.0..1.0`.
    pub fn ridged(&self, x: f32, y: f32, octaves: u32) -> f32 {
        let mut sum = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut total = 0.0;

        for _ in 0..octaves {
            let n = 1.0 - self.get(x * frequency, y * frequency).abs();
            sum += n * n * amplitude;
            total += amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }

        sum / total
    }
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Project `(x, y)` onto one of eight evenly spaced gradient directions chosen
/// by the hash.
fn grad(hash: u8, x: f32, y: f32) -> f32 {
    match hash & 7 {
        0 => x + y,
        1 => x - y,
        2 => -x + y,
        3 => -x - y,
        4 => x,
        5 => -x,
        6 => y,
        _ => -y,
    }
}

/// Hermite interpolation between `edge0` and `edge1`.
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
