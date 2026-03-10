/// Simple 2D Simplex noise implementation for procedural generation
/// Based on Stefan Gustavson's implementation

const F2: f64 = 0.3660254037844386; // (sqrt(3) - 1) / 2
const G2: f64 = 0.21132486540518713; // (3 - sqrt(3)) / 6

// Gradient vectors for 2D
const GRAD2: [[i32; 2]; 8] = [
    [1, 1],
    [-1, 1],
    [1, -1],
    [-1, -1],
    [1, 0],
    [-1, 0],
    [0, 1],
    [0, -1],
];

pub struct SimplexNoise {
    perm: [u8; 512],
}

impl SimplexNoise {
    pub fn new(seed: u64) -> Self {
        let mut perm = [0u8; 512];

        // Initialize with values 0-255
        let mut p: [u8; 256] = [0; 256];
        for i in 0..256 {
            p[i] = i as u8;
        }

        // Shuffle using seed
        let mut state = seed;
        for i in (1..256).rev() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = ((state >> 33) as usize) % (i + 1);
            p.swap(i, j);
        }

        // Duplicate for wrapping
        for i in 0..512 {
            perm[i] = p[i & 255];
        }

        Self { perm }
    }

    fn grad(&self, hash: usize, x: f64, y: f64) -> f64 {
        let g = &GRAD2[hash & 7];
        g[0] as f64 * x + g[1] as f64 * y
    }

    /// Generate 2D simplex noise value at (x, y)
    /// Returns value in range [-1, 1]
    pub fn noise2d(&self, x: f64, y: f64) -> f64 {
        // Skew input space to determine simplex cell
        let s = (x + y) * F2;
        let i = (x + s).floor() as i32;
        let j = (y + s).floor() as i32;

        // Unskew back to (x, y) space
        let t = (i + j) as f64 * G2;
        let x0 = x - (i as f64 - t);
        let y0 = y - (j as f64 - t);

        // Determine which simplex we're in
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        // Offsets for corners
        let x1 = x0 - i1 as f64 + G2;
        let y1 = y0 - j1 as f64 + G2;
        let x2 = x0 - 1.0 + 2.0 * G2;
        let y2 = y0 - 1.0 + 2.0 * G2;

        // Hash coordinates of corners
        let ii = (i & 255) as usize;
        let jj = (j & 255) as usize;
        let gi0 = self.perm[ii + self.perm[jj] as usize] as usize;
        let gi1 = self.perm[ii + i1 as usize + self.perm[jj + j1 as usize] as usize] as usize;
        let gi2 = self.perm[ii + 1 + self.perm[jj + 1] as usize] as usize;

        // Calculate contributions from corners
        let mut n0 = 0.0;
        let t0 = 0.5 - x0 * x0 - y0 * y0;
        if t0 >= 0.0 {
            let t0 = t0 * t0;
            n0 = t0 * t0 * self.grad(gi0, x0, y0);
        }

        let mut n1 = 0.0;
        let t1 = 0.5 - x1 * x1 - y1 * y1;
        if t1 >= 0.0 {
            let t1 = t1 * t1;
            n1 = t1 * t1 * self.grad(gi1, x1, y1);
        }

        let mut n2 = 0.0;
        let t2 = 0.5 - x2 * x2 - y2 * y2;
        if t2 >= 0.0 {
            let t2 = t2 * t2;
            n2 = t2 * t2 * self.grad(gi2, x2, y2);
        }

        // Scale to [-1, 1]
        70.0 * (n0 + n1 + n2)
    }

    /// Fractal Brownian Motion - layered noise for more natural terrain
    pub fn fbm(&self, x: f64, y: f64, octaves: u32, persistence: f64, lacunarity: f64) -> f64 {
        let mut total = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut max_value = 0.0;

        for _ in 0..octaves {
            total += self.noise2d(x * frequency, y * frequency) * amplitude;
            max_value += amplitude;
            amplitude *= persistence;
            frequency *= lacunarity;
        }

        total / max_value
    }
}
