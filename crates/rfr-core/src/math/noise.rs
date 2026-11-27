// SPDX-License-Identifier: LGPL-3.0-or-later

//! Procedural noise of the formula language (`noise`, `perlin`, `voronoi`): functions of a
//! point in the plane on an integer lattice with hashed values. Computed in f32 like the GPU
//! (rfr-gpu `cstd.wgsl`, `n_*`), so both give the same patterns.

use crate::render::hash;

/// A random number in 0..1 for lattice point (`ix`, `iy`) and stream `k`.
fn lattice(ix: i32, iy: i32, k: u32) -> f32 {
    let h = hash((ix as u32).wrapping_mul(0x8DA6_B343) ^ hash((iy as u32).wrapping_mul(0xD816_3841) ^ k));
    (h >> 8) as f32 / 16_777_216.0
}

/// Value noise: smoothly interpolated random values at the lattice points (0..1).
pub fn value(x: f64, y: f64) -> f64 {
    let (x, y) = (x as f32, y as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = lattice(ix, iy, 0);
    let b = lattice(ix + 1, iy, 0);
    let c = lattice(ix, iy + 1, 0);
    let d = lattice(ix + 1, iy + 1, 0);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    (top + (bottom - top) * sy) as f64
}

/// Gradient (Perlin) noise: random directions at the lattice points, quintic fade (about
/// -1..1, 0 at the lattice points).
pub fn perlin(x: f64, y: f64) -> f64 {
    let (x, y) = (x as f32, y as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let grad = |i: i32, j: i32, dx: f32, dy: f32| {
        let a = lattice(i, j, 1) * std::f32::consts::TAU;
        a.cos() * dx + a.sin() * dy
    };
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (sx, sy) = (fade(fx), fade(fy));
    let a = grad(ix, iy, fx, fy);
    let b = grad(ix + 1, iy, fx - 1.0, fy);
    let c = grad(ix, iy + 1, fx, fy - 1.0);
    let d = grad(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    ((top + (bottom - top) * sy) * std::f32::consts::SQRT_2) as f64
}

/// Voronoi (cellular) noise: the distance to the nearest of the random points, one per
/// lattice cell (0 at a point, about 1 at most).
pub fn voronoi(x: f64, y: f64) -> f64 {
    let (x, y) = (x as f32, y as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (ix, iy) = (x0 as i32, y0 as i32);
    let mut best = f32::MAX;
    for j in -1..=1 {
        for i in -1..=1 {
            let (cx, cy) = (ix + i, iy + j);
            let px = cx as f32 + lattice(cx, cy, 2);
            let py = cy as f32 + lattice(cx, cy, 3);
            let d = (px - x) * (px - x) + (py - y) * (py - y);
            best = best.min(d);
        }
    }
    best.sqrt() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_ranges_and_continuity() {
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for k in 0..4000 {
            let (x, y) = (k as f64 * 0.0137 - 20.0, (k as f64 * 0.0291).sin() * 9.0);
            let v = [value(x, y), perlin(x, y), voronoi(x, y)];
            for i in 0..3 {
                lo[i] = lo[i].min(v[i]);
                hi[i] = hi[i].max(v[i]);
                // Continuous: a tiny step changes little.
                let w = [value(x + 1e-4, y), perlin(x + 1e-4, y), voronoi(x + 1e-4, y)][i];
                assert!((w - v[i]).abs() < 0.01, "{i} jumps at {x}, {y}");
            }
        }
        assert!(lo[0] >= 0.0 && hi[0] <= 1.0 && hi[0] - lo[0] > 0.5, "value {lo:?} {hi:?}");
        assert!(lo[1] >= -1.1 && hi[1] <= 1.1 && hi[1] - lo[1] > 0.8, "perlin {lo:?} {hi:?}");
        assert!(lo[2] >= 0.0 && hi[2] <= 1.5 && hi[2] > 0.4, "voronoi {lo:?} {hi:?}");
        assert_eq!(perlin(3.0, -2.0), 0.0);
    }
}
