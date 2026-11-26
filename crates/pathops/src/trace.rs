//! Image tracing: a raster image as filled shapes, one per colour, stacked bottom to top.
//!
//! The method is the textbook one, written from these sources:
//! - colours: k-means clustering (Lloyd, "Least squares quantization in PCM", 1982) seeded by
//!   farthest-point traversal (Gonzalez, "Clustering to minimize the maximum intercluster
//!   distance", 1985);
//! - speckles: connected-component labelling with 4-connectivity (Rosenfeld and Pfaltz,
//!   "Sequential operations in digital picture processing", 1966); a region smaller than the
//!   speckle size takes the colour it borders most;
//! - outlines: following the cracks between pixels (Rosenfeld and Kak, *Digital Picture
//!   Processing*, 1982), the inside always on the right, so holes run the other way round and a
//!   non-zero fill leaves them empty;
//! - smoothing: Ramer–Douglas–Peucker simplification (Ramer 1972; Douglas and Peucker 1973), then
//!   a cubic through each smooth point with its tangent along the neighbouring points (Catmull and
//!   Rom 1974), while a sharp turn stays a corner.
//!
//! Colours stack largest first, and each colour's shape covers its own pixels and those of every
//! colour above it: no seam between two neighbouring colours lets what's under the image show.

use figcraft_geom::kurbo::{BezPath, Point, Vec2};

/// The most pixels [`trace`] takes (2048 × 2048); a bigger image is refused (`None`).
pub const MAX_PIXELS: usize = 2048 * 2048;
/// The most colours [`trace`] keeps.
pub const MAX_COLORS: usize = 64;

/// How [`trace`] reads an image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceOptions {
    /// How many colours to keep (2 to [`MAX_COLORS`]); fewer when the image has fewer.
    pub colors: usize,
    /// Regions with fewer pixels than this take the colour around them.
    pub speckle: usize,
    /// How far (pixels) an outline may stray from the pixels' edges as it is smoothed.
    pub tolerance: f64,
}

/// One traced colour: the shape it fills (pixel units, y down) in that colour.
#[derive(Clone, Debug)]
pub struct TracedLayer {
    /// Red, green, blue (not premultiplied); every shape is opaque.
    pub color: [u8; 3],
    pub path: BezPath,
}

/// No colour: a pixel more than half transparent, left out of every shape.
const CLEAR: u16 = u16::MAX;
/// A turn sharper than this (degrees) stays a corner when outlines are smoothed.
const CORNER_DEGREES: f64 = 70.0;
/// How many colours k-means is seeded and refined from at most.
const SAMPLES: usize = 20_000;
/// Rounds of k-means refinement.
const ROUNDS: usize = 12;

/// Trace `rgba` (`width` × `height` pixels, 4 bytes each, not premultiplied) into filled shapes,
/// bottom to top. Pixels at least half transparent are left out. `None` when the buffer doesn't
/// match the size or the image is past [`MAX_PIXELS`]; an image with nothing opaque gives none.
pub fn trace(rgba: &[u8], width: usize, height: usize, opt: &TraceOptions) -> Option<Vec<TracedLayer>> {
    trace_with(rgba, width, height, opt, &mut |_| true).ok()
}

/// Why [`trace_with`] gave no shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceStop {
    /// The buffer doesn't match the size, or the image is past [`MAX_PIXELS`].
    BadInput,
    /// `progress` said to stop.
    Cancelled,
}

/// [`trace`], telling `progress` how far it got (0–1) after each step; when it returns false the
/// tracing stops ([`TraceStop::Cancelled`]).
pub fn trace_with(
    rgba: &[u8],
    width: usize,
    height: usize,
    opt: &TraceOptions,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<TracedLayer>, TraceStop> {
    let n = width.checked_mul(height).ok_or(TraceStop::BadInput)?;
    if n == 0 || n > MAX_PIXELS || rgba.len() != n * 4 {
        return Err(TraceStop::BadInput);
    }
    let pixel = |i: usize| [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]];
    let opaque = |i: usize| rgba[i * 4 + 3] >= 128;
    let centres = palette(n, &pixel, &opaque, opt.colors.clamp(2, MAX_COLORS));
    if centres.is_empty() {
        return Ok(Vec::new());
    }
    // The colours take about a fifth of the time, the outlines the rest.
    let go = |progress: &mut dyn FnMut(f32) -> bool, f: f32| if progress(f) { Ok(()) } else { Err(TraceStop::Cancelled) };
    go(progress, 0.1)?;
    let mut labels: Vec<u16> = (0..n).map(|i| if opaque(i) { nearest(&centres, pixel(i)) } else { CLEAR }).collect();
    merge_speckles(&mut labels, width, height, opt.speckle.max(1));
    go(progress, 0.2)?;
    // Each colour's own average (the pixels it ended up with), and how many it has.
    let mut sums = vec![([0u64; 3], 0usize); centres.len()];
    for (i, l) in labels.iter().enumerate() {
        if let Some((sum, count)) = sums.get_mut(usize::from(*l)) {
            let p = pixel(i);
            for c in 0..3 {
                sum[c] += u64::from(p[c]);
            }
            *count += 1;
        }
    }
    let mut order: Vec<usize> = (0..centres.len()).filter(|l| sums[*l].1 > 0).collect();
    order.sort_by_key(|l| std::cmp::Reverse(sums[*l].1));
    // A colour's place in the stack (0 at the bottom).
    let mut rank = vec![usize::MAX; centres.len()];
    for (r, l) in order.iter().enumerate() {
        rank[*l] = r;
    }
    let mut layers = Vec::with_capacity(order.len());
    for (r, l) in order.iter().enumerate() {
        go(progress, 0.2 + 0.8 * r as f32 / order.len() as f32)?;
        let (sum, count) = sums[*l];
        let avg = |c: usize| u8::try_from(sum[c] / count as u64).unwrap_or(u8::MAX);
        // This colour and every colour above it.
        let inside = |i: usize| labels.get(i).is_some_and(|l| *l != CLEAR && rank[usize::from(*l)] >= r);
        let mut path = BezPath::new();
        for ring in outlines(&inside, width, height) {
            smooth_into(&mut path, &simplify_ring(&ring, opt.tolerance.max(0.1)));
        }
        if !path.elements().is_empty() {
            layers.push(TracedLayer { color: [avg(0), avg(1), avg(2)], path });
        }
    }
    go(progress, 1.0)?;
    Ok(layers)
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn as_f32(p: [u8; 3]) -> [f32; 3] {
    [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
}

/// The label of the centre nearest `p`.
fn nearest(centres: &[[f32; 3]], p: [u8; 3]) -> u16 {
    nearest_f(centres, as_f32(p))
}

fn nearest_f(centres: &[[f32; 3]], p: [f32; 3]) -> u16 {
    let mut best = (0usize, f32::INFINITY);
    for (i, c) in centres.iter().enumerate() {
        let d = dist2(*c, p);
        if d < best.1 {
            best = (i, d);
        }
    }
    u16::try_from(best.0).unwrap_or(0)
}

/// Up to `k` colours that sum up the opaque pixels: seeded far apart, then refined by k-means
/// over an even sample of them. Empty when nothing is opaque.
fn palette(n: usize, pixel: &impl Fn(usize) -> [u8; 3], opaque: &impl Fn(usize) -> bool, k: usize) -> Vec<[f32; 3]> {
    let count = (0..n).filter(|i| opaque(*i)).count();
    let step = count.div_ceil(SAMPLES).max(1);
    let samples: Vec<[f32; 3]> = (0..n).filter(|i| opaque(*i)).step_by(step).map(|i| as_f32(pixel(i))).collect();
    let Some(first) = samples.first() else { return Vec::new() };
    // Farthest-point seeds: each the sample farthest from the seeds so far. Fewer when the image
    // has fewer distinct colours.
    let mut centres = vec![*first];
    let mut near: Vec<f32> = samples.iter().map(|s| dist2(*s, *first)).collect();
    while centres.len() < k {
        let Some((i, d)) = near.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)) else { break };
        if d <= 0.0 {
            break;
        }
        let c = samples[i];
        centres.push(c);
        for (s, m) in samples.iter().zip(near.iter_mut()) {
            *m = m.min(dist2(*s, c));
        }
    }
    for _ in 0..ROUNDS {
        let mut sums = vec![([0f64; 3], 0usize); centres.len()];
        for s in &samples {
            let (sum, count) = &mut sums[usize::from(nearest_f(&centres, *s))];
            for c in 0..3 {
                sum[c] += f64::from(s[c]);
            }
            *count += 1;
        }
        let mut moved = false;
        for (centre, (sum, count)) in centres.iter_mut().zip(sums) {
            if count > 0 {
                let next = [(sum[0] / count as f64) as f32, (sum[1] / count as f64) as f32, (sum[2] / count as f64) as f32];
                moved |= dist2(next, *centre) > 0.25;
                *centre = next;
            }
        }
        if !moved {
            break;
        }
    }
    centres
}

/// The 4-connected neighbours of pixel `i` in a `w` × `h` image.
fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % w, i / w);
    [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)].into_iter().flatten()
}

/// Regions (4-connected runs of one label, transparent ones too) smaller than `min` pixels take
/// the label they border most among the regions that are big enough.
fn merge_speckles(labels: &mut [u16], w: usize, h: usize, min: usize) {
    if min <= 1 {
        return;
    }
    let mut region = vec![u32::MAX; labels.len()];
    let mut sizes: Vec<usize> = Vec::new();
    let mut small: Vec<Vec<usize>> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..labels.len() {
        if region[start] != u32::MAX {
            continue;
        }
        let id = u32::try_from(sizes.len()).unwrap_or(u32::MAX - 1);
        let label = labels[start];
        let mut members = Vec::new();
        region[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            members.push(i);
            for j in neighbours(i, w, h) {
                if region[j] == u32::MAX && labels[j] == label {
                    region[j] = id;
                    stack.push(j);
                }
            }
        }
        sizes.push(members.len());
        if members.len() < min {
            small.push(members);
        }
    }
    for members in small {
        let mut votes: Vec<(u16, usize)> = Vec::new();
        for i in &members {
            for j in neighbours(*i, w, h) {
                let big = sizes.get(region[j] as usize).is_some_and(|s| *s >= min);
                if big {
                    match votes.iter_mut().find(|v| v.0 == labels[j]) {
                        Some(v) => v.1 += 1,
                        None => votes.push((labels[j], 1)),
                    }
                }
            }
        }
        if let Some((label, _)) = votes.into_iter().max_by_key(|v| v.1) {
            for i in members {
                labels[i] = label;
            }
        }
    }
}

/// Steps east, south, west and north (y down).
const STEPS: [(i64, i64); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

/// The outlines of the pixels `inside` says are in, along the pixels' edges: corner points of
/// each closed ring, the inside on the right of the way it runs (clockwise on screen; holes
/// anticlockwise). Where two pixels meet only at a corner the ring turns right, keeping them
/// apart, as 4-connected regions are.
fn outlines(inside: &impl Fn(usize) -> bool, w: usize, h: usize) -> Vec<Vec<(i64, i64)>> {
    let vw = w + 1;
    // For each pixel corner, the directions of the edges that leave it (bit d: STEPS[d]).
    let mut out = vec![0u8; vw * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !inside(i) {
                continue;
            }
            let v = y * vw + x;
            if y == 0 || !inside(i - w) {
                out[v] |= 1; // top edge, east from the top-left corner
            }
            if x + 1 == w || !inside(i + 1) {
                out[v + 1] |= 2; // right edge, south from the top-right corner
            }
            if y + 1 == h || !inside(i + w) {
                out[v + vw + 1] |= 4; // bottom edge, west from the bottom-right corner
            }
            if x == 0 || !inside(i - 1) {
                out[v + vw] |= 8; // left edge, north from the bottom-left corner
            }
        }
    }
    let mut rings = Vec::new();
    for start in 0..out.len() {
        while out[start] != 0 {
            let mut dir = out[start].trailing_zeros() as usize;
            out[start] &= !(1 << dir);
            let (sx, sy) = ((start % vw) as i64, (start / vw) as i64);
            let (mut x, mut y) = (sx, sy);
            let mut ring = vec![(sx, sy)];
            loop {
                (x, y) = (x + STEPS[dir].0, y + STEPS[dir].1);
                if (x, y) == (sx, sy) {
                    break;
                }
                let v = y as usize * vw + x as usize;
                let Some(next) = [(dir + 1) % 4, dir, (dir + 3) % 4].into_iter().find(|d| out[v] & (1 << d) != 0) else { break };
                out[v] &= !(1 << next);
                if next != dir {
                    ring.push((x, y));
                }
                dir = next;
            }
            if ring.len() >= 4 {
                rings.push(ring);
            }
        }
    }
    rings
}

/// A closed ring with its points within `tol` of it kept out (Ramer–Douglas–Peucker), cut where
/// it runs farthest from its first point so both halves start and end on the ring.
fn simplify_ring(ring: &[(i64, i64)], tol: f64) -> Vec<Point> {
    let pts: Vec<Point> = ring.iter().map(|(x, y)| Point::new(*x as f64, *y as f64)).collect();
    let Some(first) = pts.first().copied() else { return pts };
    let far = pts.iter().enumerate().max_by(|a, b| (*a.1 - first).hypot2().total_cmp(&(*b.1 - first).hypot2())).map_or(0, |(i, _)| i);
    if far == 0 || pts.len() < 4 {
        return pts;
    }
    let mut keep = vec![false; pts.len() + 1];
    keep[0] = true;
    keep[far] = true;
    let looped: Vec<Point> = pts.iter().copied().chain(std::iter::once(first)).collect();
    keep_far(&looped, 0, far, tol, &mut keep);
    keep_far(&looped, far, pts.len(), tol, &mut keep);
    // A ring smaller than the tolerance keeps its corners rather than vanishing.
    if keep.iter().take(pts.len()).filter(|k| **k).count() < 3 {
        return pts;
    }
    pts.into_iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, p)| p).collect()
}

/// Ramer–Douglas–Peucker between `a` and `b` (both kept): marks the points needed to stay within
/// `tol` of `pts[a..=b]`. Iterative, so a long ring can't run out of stack.
fn keep_far(pts: &[Point], a: usize, b: usize, tol: f64, keep: &mut [bool]) {
    let mut todo = vec![(a, b)];
    while let Some((a, b)) = todo.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (pts[a], pts[b]);
        let along = pb - pa;
        let len = along.hypot();
        let off = |p: Point| if len < 1e-9 { (p - pa).hypot() } else { along.cross(p - pa).abs() / len };
        let Some((i, d)) = (a + 1..b).map(|i| (i, off(pts[i]))).max_by(|x, y| x.1.total_cmp(&y.1)) else { continue };
        if d > tol {
            keep[i] = true;
            todo.push((a, i));
            todo.push((i, b));
        }
    }
}

/// Append the closed ring `pts` to `path`: a cubic between each two points, its handles along the
/// tangent at smooth points (the direction from the point before to the point after) and along
/// the side at corners, each a third of the side long, so straight runs stay straight.
fn smooth_into(path: &mut BezPath, pts: &[Point]) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let at = |i: usize| pts[i % n];
    let unit = |v: Vec2| if v.hypot() > 1e-9 { v / v.hypot() } else { Vec2::ZERO };
    let corner_cos = CORNER_DEGREES.to_radians().cos();
    // (direction arriving, direction leaving) at each point.
    let tangents: Vec<(Vec2, Vec2)> = (0..n)
        .map(|i| {
            let (prev, p, next) = (at(i + n - 1), at(i), at(i + 1));
            let (inn, out) = (unit(p - prev), unit(next - p));
            if inn.dot(out) < corner_cos {
                (inn, out)
            } else {
                let t = unit(next - prev);
                (t, t)
            }
        })
        .collect();
    path.move_to(at(0));
    for i in 0..n {
        let (a, b) = (at(i), at(i + 1));
        let third = (b - a).hypot() / 3.0;
        let (out_a, in_b) = (tangents[i].1, tangents[(i + 1) % n].0);
        let straight = unit(b - a);
        if (out_a - straight).hypot() < 1e-6 && (in_b - straight).hypot() < 1e-6 {
            path.line_to(b);
        } else {
            path.curve_to(a + out_a * third, b - in_b * third, b);
        }
    }
    path.close_path();
}
