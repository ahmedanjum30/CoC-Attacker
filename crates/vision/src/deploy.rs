//! Finds where troops can be deployed, using the red border the game draws
//! around a base's no-deploy zone at the start of a battle.

use image::RgbImage;

/// Screen areas covered by UI that must never be tapped as deploy points.
/// Each is (left, top, right, bottom).
const UI_AREAS: [(u32, u32, u32, u32); 6] = [
    (0, 0, 300, 165),       // base name and available loot
    (540, 0, 760, 80),      // battle timer
    (980, 0, 1280, 210),    // own resources
    (0, 500, 200, 580),     // End Battle / Surrender
    (1040, 450, 1280, 580), // Next / overall damage
    (0, 585, 1280, 720),    // troop bar
];

/// How far outside the border to drop troops, in pixels.
const MARGIN: f32 = 18.0;
/// Fewer border pixels than this means the border isn't showing.
const MIN_BORDER_PIXELS: usize = 300;
/// The border must span at least this much of the screen (width, height).
const MIN_BORDER_SPAN: (u32, u32) = (300, 200);
/// Slopes (dy/dx) of the map grid lines on screen. The camera's perspective
/// makes them vary across the screen, so a range is tried.
const LINE_SLOPES: [f32; 5] = [0.5, 0.6, 0.7, 0.8, 0.9];
/// Line test: samples taken each way along the line direction, and how
/// many of them (out of 2 * LINE_HALF_LENGTH + 1) must be red.
const LINE_HALF_LENGTH: i32 = 10;
const LINE_MIN_RUN: usize = 15;
/// Line test: distances either side of the line that must not be red.
const LINE_CLEARANCE: [f32; 3] = [5.0, 6.5, 8.0];
/// Fewer usable deploy points than this means the border wasn't really found.
const MIN_POINTS: usize = 6;
/// Rays whose border hit is closer than this share of the border's overall
/// extent in that direction probably hit a building, not the border.
const MIN_RAY_REACH: f32 = 0.45;

#[derive(Debug, Clone)]
pub struct DeployZone {
    /// Centre of the base (middle of the border's bounding box).
    pub center: (f32, f32),
    /// Deploy points just outside the border, ordered by angle (clockwise
    /// from the right). Rays without a reliable border hit are left out.
    pub points: Vec<DeployPoint>,
}

#[derive(Debug, Clone, Copy)]
pub struct DeployPoint {
    pub pos: (u32, u32),
    /// Direction from the centre, radians (0 = right, clockwise).
    pub angle: f32,
    /// Distance from the centre to the border along this ray.
    pub border_distance: f32,
}

fn is_border_color([r, g, b]: [u8; 3]) -> bool {
    r > 175 && (85..=135).contains(&g) && b < 75 && r as i32 - g as i32 > 60
}

fn in_ui(x: u32, y: u32) -> bool {
    UI_AREAS
        .iter()
        .any(|&(l, t, r, b)| x >= l && x < r && y >= t && y < b)
}

struct Mask {
    w: u32,
    h: u32,
    bits: Vec<bool>,
}

impl Mask {
    fn get(&self, x: f32, y: f32) -> bool {
        if x < 0.0 || y < 0.0 || x >= self.w as f32 || y >= self.h as f32 {
            return false;
        }
        self.bits[(y as u32 * self.w + x as u32) as usize]
    }

    /// Red at (x, y) or one pixel above or below (lines are anti-aliased).
    fn near(&self, x: f32, y: f32) -> bool {
        self.get(x, y) || self.get(x, y - 1.0) || self.get(x, y + 1.0)
    }

    /// True if (x, y) lies on a thin line running in a map-grid direction:
    /// red continues along the line, with no red to either side of it.
    fn on_line(&self, x: f32, y: f32) -> bool {
        LINE_SLOPES.iter().flat_map(|&s| [s, -s]).any(|s| {
            // Unit direction (1, s), sampled every 2 px.
            let len = (1.0 + s * s).sqrt();
            let (ux, uy) = (1.0 / len, s / len);
            let run = (-LINE_HALF_LENGTH..=LINE_HALF_LENGTH)
                .filter(|&k| self.near(x + 2.0 * ux * k as f32, y + 2.0 * uy * k as f32))
                .count();
            if run < LINE_MIN_RUN {
                return false;
            }
            let (nx, ny) = (-uy, ux);
            LINE_CLEARANCE.iter().all(|&t| {
                !self.get(x + nx * t, y + ny * t) && !self.get(x - nx * t, y - ny * t)
            })
        })
    }
}

/// Locates the deploy border and proposes up to `rays` evenly spaced deploy
/// points. Returns `None` if no border is visible.
pub fn find_deploy_zone(frame: &RgbImage, rays: usize) -> Option<DeployZone> {
    let (w, h) = frame.dimensions();
    let red = Mask {
        w,
        h,
        bits: frame
            .enumerate_pixels()
            .map(|(x, y, p)| !in_ui(x, y) && is_border_color(p.0))
            .collect(),
    };

    let mut border = Mask { w, h, bits: vec![false; red.bits.len()] };
    let (mut min, mut max, mut count) = ((w, h), (0, 0), 0);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if red.bits[i] && red.on_line(x as f32, y as f32) {
                border.bits[i] = true;
                count += 1;
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
            }
        }
    }
    if count < MIN_BORDER_PIXELS
        || max.0 - min.0 < MIN_BORDER_SPAN.0
        || max.1 - min.1 < MIN_BORDER_SPAN.1
    {
        return None;
    }
    let center = ((min.0 + max.0) as f32 / 2.0, (min.1 + max.1) as f32 / 2.0);
    let half = ((max.0 - min.0) as f32 / 2.0, (max.1 - min.1) as f32 / 2.0);

    let mut points = Vec::new();
    for i in 0..rays {
        let angle = i as f32 / rays as f32 * std::f32::consts::TAU;
        let (dx, dy) = (angle.cos(), angle.sin());

        // Walk outwards and remember the farthest border pixel.
        let mut farthest = None;
        let mut dist = 0.0;
        loop {
            let (x, y) = (center.0 + dx * dist, center.1 + dy * dist);
            if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                break;
            }
            if border.get(x, y) {
                farthest = Some(dist);
            }
            dist += 1.0;
        }
        let Some(border_distance) = farthest else { continue };

        // Distance from the centre to the bounding box along this ray.
        let extent = (half.0 / dx.abs().max(1e-6)).min(half.1 / dy.abs().max(1e-6));
        if border_distance < MIN_RAY_REACH * extent {
            continue;
        }

        let d = border_distance + MARGIN;
        let (x, y) = (center.0 + dx * d, center.1 + dy * d);
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 || in_ui(x as u32, y as u32) {
            continue;
        }
        points.push(DeployPoint { pos: (x as u32, y as u32), angle, border_distance });
    }
    (points.len() >= MIN_POINTS).then_some(DeployZone { center, points })
}
