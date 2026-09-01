//! The island's surface as a grid, and the sweep both bakes run over it.
//!
//! [`crate::sunlight`] and [`crate::shelter`] ask one question of one
//! surface: standing at a point and looking toward a source, how high does
//! the terrain between here and there reach, less what it loses per metre of
//! the way? For the sun the loss is the tangent of its altitude and the
//! answer is a shadow; for the wind it is a recovery slope and the answer is
//! a wake. Everything else — sampling the surface, walking the grid away from
//! the source, reading a point back — is the same, and lives here once. The
//! two bakes used to carry it twice, sign for sign, and a fix to the walk in
//! one would have moved a seed's lees without moving its shadows.
//!
//! [`sweep`] takes the direction the source lies *in*, whichever it is: the
//! sun's bearing, or the quarter the wind blows from. Passing the wind's own
//! velocity negated is exactly the same walk — the sign falls out of the
//! `slide`, the arm it picks and `(1 + slide²)` alike, bit for bit — which is
//! what lets one function serve both without a flag.
//!
//! [`Raster::sample`] is threaded; the sweeps are the callers' to thread,
//! since the two bakes band them differently. Every fill thread writes
//! disjoint rows and reads nothing another wrote, so the raster is the same
//! at any count — a promise the seed digests hold every island to.

use glam::Vec2;

/// How many threads a fill or a bake may split across — the machine's, less
/// one for whoever asked, and never none. Only the *speed* rides on this:
/// every answer is the same at any count, which is what lets it be the
/// host's.
pub fn hands() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

/// Where a grid's points stand in the world: the corner it starts from, the
/// metres between points, and how many along each axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// World position of point `(0, 0)`.
    pub min: Vec2,
    /// Metres between points.
    pub step: f32,
    /// Points along x and z.
    pub columns: usize,
    pub rows: usize,
}

impl Grid {
    /// The grid over `min..=max` at `step` metres — points at both ends, so a
    /// span that is a whole number of steps lands its last point exactly on
    /// `max`, which is what puts a chunk's far corner on a grid point rather
    /// than between two.
    pub fn over(min: Vec2, max: Vec2, step: f32) -> Self {
        Self {
            min,
            step,
            columns: ((max.x - min.x) / step).ceil() as usize + 1,
            rows: ((max.y - min.y) / step).ceil() as usize + 1,
        }
    }

    pub fn points(&self) -> usize {
        self.columns * self.rows
    }

    /// World position of point `(ix, iz)`.
    pub fn at(&self, ix: usize, iz: usize) -> Vec2 {
        self.min + Vec2::new(ix as f32, iz as f32) * self.step
    }

    /// The index of the grid point nearest a world point, or `None` past
    /// the grid's edge.
    ///
    /// The nearest point's own value and not a blend: the bakes are read at
    /// points that land on the grid exactly — a chunk corner on the ground's
    /// grid, a shelter lattice point on a multiple of it — so there is
    /// nothing to blend, and a baked pair is not a quantity to average
    /// anyway. Whoever needs an answer *between* points blends knowing what
    /// the points mean.
    pub fn nearest(&self, wx: f32, wz: f32) -> Option<usize> {
        let gx = (wx - self.min.x) / self.step;
        let gz = (wz - self.min.y) / self.step;
        if gx < 0.0 || gz < 0.0 || gx > (self.columns - 1) as f32 || gz > (self.rows - 1) as f32 {
            return None;
        }
        let x = (gx.round() as usize).min(self.columns - 1);
        let z = (gz.round() as usize).min(self.rows - 1);
        Some(z * self.columns + x)
    }

    /// How far point `(ix, iz)` stands inside the grid's edge, in metres —
    /// zero on the outermost ring.
    pub fn inset(&self, ix: usize, iz: usize) -> f32 {
        let along = ix.min(self.columns - 1 - ix);
        let across = iz.min(self.rows - 1 - iz);
        along.min(across) as f32 * self.step
    }
}

/// A surface sampled onto a [`Grid`]: one height per point, row-major.
pub struct Raster {
    pub grid: Grid,
    pub values: Vec<f32>,
}

impl Raster {
    /// Samples `surface` over `min..=max` at `step`, `hands` rows at a time.
    pub fn sample(
        min: Vec2,
        max: Vec2,
        step: f32,
        surface: impl Fn(f32, f32) -> f32 + Sync,
        hands: usize,
    ) -> Self {
        let grid = Grid::over(min, max, step);
        let mut values = vec![0.0f32; grid.points()];
        // By bands of whole rows — disjoint slices, so the threads never
        // look at each other's.
        let band = grid.rows.div_ceil(hands);
        std::thread::scope(|scope| {
            for (b, rows_of) in values.chunks_mut(band * grid.columns).enumerate() {
                let surface = &surface;
                scope.spawn(move || {
                    for (r, row) in rows_of.chunks_mut(grid.columns).enumerate() {
                        let wz = grid.min.y + (b * band + r) as f32 * step;
                        for (ix, point) in row.iter_mut().enumerate() {
                            *point = surface(grid.min.x + ix as f32 * step, wz);
                        }
                    }
                });
            }
        });
        Self { grid, values }
    }

    /// Every `by`-th point along both axes, from the same corner: the raster
    /// a coarser bake would have sampled, without sampling it again. Exact,
    /// since the points coincide — a copy, not a resample.
    pub fn decimated(&self, by: usize) -> Self {
        let grid = Grid {
            min: self.grid.min,
            step: self.grid.step * by as f32,
            columns: (self.grid.columns - 1) / by + 1,
            rows: (self.grid.rows - 1) / by + 1,
        };
        let mut values = Vec::with_capacity(grid.points());
        for iz in 0..grid.rows {
            for ix in 0..grid.columns {
                values.push(self.values[iz * by * self.grid.columns + ix * by]);
            }
        }
        Self { grid, values }
    }

    /// The height at `(ix, iz)`.
    pub fn get(&self, ix: usize, iz: usize) -> f32 {
        self.values[iz * self.grid.columns + ix]
    }
}

/// One pass over a raster away from a source: afterwards `field[p]` is the
/// height the source's shadow — or the sheltered air, which is the same
/// shape — stands at over point `p`, its own surface where nothing between
/// it and the source reaches higher.
///
/// `from` is the direction the source lies in on the ground plane and need
/// not be a unit vector; `drop` is how much height the carried line loses
/// per metre walked away from the source. The pass walks the raster away
/// from the source, so each point needs only the line before it: the height
/// there, read between the two nearest points, dropped by what one step
/// costs, against the point's own surface. Beyond the raster lies open sea,
/// whose height is its own surface at zero — which is why the first line
/// seeds from nothing.
pub fn sweep(field: &mut [f32], ground: &Raster, from: Vec2, drop: f32) {
    let Grid {
        columns,
        rows,
        step,
        ..
    } = ground.grid;
    let ground = &ground.values;
    let x_major = from.x.abs() >= from.y.abs();
    let (majors, minors, toward, cross) = if x_major {
        (columns, rows, from.x, from.y)
    } else {
        (rows, columns, from.y, from.x)
    };
    let at = |major: usize, minor: usize| {
        if x_major {
            minor * columns + major
        } else {
            major * columns + minor
        }
    };
    // The line toward the source is walked one major step at a time, sliding
    // this much along the minor axis — a fraction, since the major axis is
    // by construction the direction's larger component.
    let slide = cross / toward.abs();
    let fall = drop * (1.0 + slide * slide).sqrt() * step;

    for k in 0..majors {
        // From the raster's source side inward, so the line nearer the
        // source than this one is already swept.
        let (major, nearer) = if toward > 0.0 {
            (majors - 1 - k, majors - k)
        } else {
            (k, k.wrapping_sub(1))
        };
        for minor in 0..minors {
            let carried = if nearer >= majors {
                0.0
            } else {
                let m = (minor as f32 + slide).clamp(0.0, (minors - 1) as f32);
                let m0 = m.floor() as usize;
                let m1 = (m0 + 1).min(minors - 1);
                let t = m - m0 as f32;
                field[at(nearer, m0)] * (1.0 - t) + field[at(nearer, m1)] * t
            };
            let here = at(major, minor);
            field[here] = ground[here].max(carried - fall);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raster_is_the_same_however_many_hands_fill_it() {
        // The seed digests hold one answer per seed, so a fill that drifted
        // with the host's core count would make a seed mean somewhere else
        // on a different machine. A surface that varies both ways, so a band
        // boundary that read the wrong row would show.
        let relief = |wx: f32, wz: f32| (wx / 37.0).sin() * 3.0 + (wz / 23.0).cos() * 2.0;
        let one = Raster::sample(Vec2::ZERO, Vec2::splat(300.0), 4.0, relief, 1);
        for hands in [2, 3, 5, 16, 200] {
            let many = Raster::sample(Vec2::ZERO, Vec2::splat(300.0), 4.0, relief, hands);
            assert_eq!(one.grid, many.grid);
            assert_eq!(
                one.values, many.values,
                "{hands} hands filled a different raster"
            );
        }
    }

    #[test]
    fn a_decimated_raster_is_the_coarser_sample_it_stands_in_for() {
        // The whole point: the coarse bake reads the fine raster's own
        // points, so it must equal what sampling at the coarse step directly
        // would have given — origin, extent and every value.
        let relief = |wx: f32, wz: f32| wx * 0.5 - wz * 0.25;
        let fine = Raster::sample(
            Vec2::new(-128.0, 256.0),
            Vec2::new(384.0, 640.0),
            1.0,
            relief,
            3,
        );
        let coarse = Raster::sample(
            Vec2::new(-128.0, 256.0),
            Vec2::new(384.0, 640.0),
            4.0,
            relief,
            3,
        );
        let cut = fine.decimated(4);
        assert_eq!(cut.grid, coarse.grid);
        assert_eq!(cut.values, coarse.values);
    }

    #[test]
    fn the_edge_of_a_grid_is_inset_by_nothing() {
        let grid = Grid::over(Vec2::ZERO, Vec2::new(40.0, 20.0), 4.0);
        assert_eq!((grid.columns, grid.rows), (11, 6));
        assert_eq!(grid.inset(0, 3), 0.0);
        assert_eq!(grid.inset(10, 3), 0.0);
        assert_eq!(grid.inset(5, 0), 0.0);
        assert_eq!(grid.inset(5, 5), 0.0);
        assert_eq!(grid.inset(2, 2), 8.0);
        assert_eq!(
            grid.inset(5, 2),
            8.0,
            "the nearer edge is the one that counts"
        );
    }

    #[test]
    fn a_sweep_carries_a_wall_downwind_and_lets_it_fall() {
        // A wall across the middle, a source due north: south of the wall
        // the carried height starts at the wall's and loses `drop` a metre.
        let wall = |_wx: f32, wz: f32| if (wz - 100.0).abs() < 2.0 { 40.0 } else { 0.0 };
        let ground = Raster::sample(Vec2::ZERO, Vec2::splat(200.0), 4.0, wall, 2);
        let mut field = vec![0.0f32; ground.grid.points()];
        sweep(&mut field, &ground, Vec2::new(0.0, -1.0), 0.1);
        let read = |wz: f32| field[ground.grid.nearest(100.0, wz).expect("on the raster")];
        assert_eq!(read(80.0), 0.0, "nothing stands north of the wall");
        assert!(
            (read(120.0) - 38.0).abs() < 0.6,
            "20 m south read {}",
            read(120.0)
        );
        assert!(
            (read(160.0) - 34.0).abs() < 0.6,
            "60 m south read {}",
            read(160.0)
        );
    }
}
