//! The perspective effect: a layer, with everything it paints (its other effects, its children),
//! seen at an angle. The layer is drawn flat into a surface of its own as usual, then that surface
//! is mapped onto the canvas through the tilt ([`figcraft_doc::Perspective::matrix`] about the
//! layer's centre, carried into device pixels by the layer's transform): each pixel it covers reads
//! the flat surface where the tilt brings it from, between the four nearest pixels. The layer's
//! opacity and blend mode apply as it lands, as they would untilted.
//!
//! Only the part that lands in view is drawn: the flat surface covers what the tilt brings there
//! (and the whole layer when it fits). Background blur and glass on the tilted layer see what lies
//! under its untilted place, then tilt with it.

use figcraft_color::BlendMode;
use figcraft_doc::{EffectKind, Node, invert3, project3};
use figcraft_geom::{Affine, Rect};
use tiny_skia::{Pixmap, PremultipliedColorU8};

use crate::backdrop::Under;
use crate::draw::{Ctx, draw_layer, effect_margin};
use crate::paint::composite;

/// How far (document units) `n`'s perspective can move what it paints past its box: the tilt can
/// bring a corner nearer, up to four times its distance from the centre further out (the viewer
/// keeps at least 1.25 times that distance away).
pub fn reach(n: &Node) -> f64 {
    if n.perspective().is_none() {
        return 0.0;
    }
    let half = (n.width / 2.0).hypot(n.height / 2.0);
    if half.is_finite() { half * 4.0 } else { 0.0 }
}

/// Draw `n` (under `parent_xf`, its untilted paint area `local` in its own units) tilted by its
/// perspective onto `pix`.
pub(crate) fn draw(ctx: &Ctx, pix: &mut Pixmap, n: &Node, parent_xf: Affine, local: Rect, depth: usize) {
    // The layer as it would be without the tilt, landing as is: opacity and blend apply below.
    let mut flat = n.clone();
    flat.effects.retain(|e| !matches!(e.kind, EffectKind::Perspective(_)));
    flat.opacity = 1.0;
    flat.blend = BlendMode::Normal;
    let xf = parent_xf * n.transform;
    let m = (effect_margin(&flat) * ctx.scale).min(crate::draw::MAX_EFFECT_MARGIN_PX);
    let area = crate::draw::device_rect(xf, local).inflate(m, m);
    let Some(h) = n.page_perspective(xf, area) else { return };
    let Some(back) = invert3(&h) else { return };
    let view = Rect::new(0.0, 0.0, f64::from(pix.width()), f64::from(pix.height()));
    let dst = n.tilted(xf, area).intersect(view);
    if !(dst.width() > 0.0 && dst.height() > 0.0) {
        return;
    }
    // What of the flat layer lands in view: where the view's part of the tilted area comes from.
    let corners = [(dst.x0, dst.y0), (dst.x1, dst.y0), (dst.x1, dst.y1), (dst.x0, dst.y1)];
    let from: Vec<(f64, f64)> = corners.iter().filter_map(|c| project3(&back, *c)).collect();
    let src = if from.len() == 4 {
        let (x0, x1) = from.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (y0, y1) = from.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.1), b.max(p.1)));
        Rect::new(x0, y0, x1, y1).inflate(2.0, 2.0).intersect(area)
    } else {
        area
    };
    let (sx, sy) = (src.x0.floor(), src.y0.floor());
    let (sw, sh) = ((src.x1.ceil() - sx).min(16_384.0), (src.y1.ceil() - sy).min(16_384.0));
    let (dx, dy) = (dst.x0.floor(), dst.y0.floor());
    let (dw, dh) = ((dst.x1.ceil() - dx).min(16_384.0), (dst.y1.ceil() - dy).min(16_384.0));
    if !(sx.is_finite() && sy.is_finite() && sw >= 1.0 && sh >= 1.0 && dw >= 1.0 && dh >= 1.0) {
        return;
    }
    let (sw, sh, dw, dh) = (sw as u32, sh as u32, dw as u32, dh as u32);
    let Some(_held) = crate::layer_budget::Held::take(sw, sh, 1) else { return };
    let Some(_held_out) = crate::layer_budget::Held::take(dw, dh, 1) else { return };
    let (Some(mut flat_pix), Some(mut out)) = (Pixmap::new(sw, sh), Pixmap::new(dw, dh)) else { return };
    let (sxi, syi) = (sx as i32, sy as i32);
    // Backdrop effects of the flat layer read what lies under its untilted place.
    let under = Under::new(pix, (sxi, syi), ctx.under);
    let inner = Ctx { under: Some(&under), ..*ctx };
    draw_layer(&inner, &mut flat_pix, &flat, Affine::translate((-sx, -sy)) * parent_xf, depth);
    warp(&flat_pix, (sx, sy), &back, &mut out, (dx, dy));
    composite(pix, (dx as i32, dy as i32), &out, n.opacity.clamp(0.0, 1.0), n.blend);
}

/// Fill `out` (whose top-left pixel lies at device `at`) from `src` (at device `src_at`): each
/// pixel reads `src` where `back` (device to device) takes its centre, between the four nearest
/// pixels (outside `src` is clear, so the edges come out smooth).
fn warp(src: &Pixmap, src_at: (f64, f64), back: &[f64; 9], out: &mut Pixmap, at: (f64, f64)) {
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let pixels = src.pixels();
    let texel = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= sw || y >= sh {
            return [0.0; 4];
        }
        let p = pixels[(y * sw + x) as usize];
        [f32::from(p.red()), f32::from(p.green()), f32::from(p.blue()), f32::from(p.alpha())]
    };
    let w = out.width() as usize;
    for (i, px) in out.pixels_mut().iter_mut().enumerate() {
        let q = (at.0 + (i % w) as f64 + 0.5, at.1 + (i / w) as f64 + 0.5);
        let Some((u, v)) = project3(back, q) else { continue };
        let (fx, fy) = (u - src_at.0 - 0.5, v - src_at.1 - 0.5);
        if !(fx > -1.0 && fy > -1.0 && fx < sw as f64 && fy < sh as f64) {
            continue;
        }
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
        let mix = |k: usize| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            (top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8
        };
        let alpha = mix(3);
        if alpha == 0 {
            continue;
        }
        // Premultiplied: a colour never exceeds its alpha (rounding the same way keeps it so).
        let (r, g, bl) = (mix(0).min(alpha), mix(1).min(alpha), mix(2).min(alpha));
        if let Some(p) = PremultipliedColorU8::from_rgba(r, g, bl, alpha) {
            *px = p;
        }
    }
}
